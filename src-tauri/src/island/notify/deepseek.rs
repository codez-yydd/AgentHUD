//! Read completion events from the local DeepSeek Harness session log.

use crate::dsh::{sessions_root_path, ZSTD_MAGIC};
use crate::island::settings::DshDoc;
use ruzstd::decoding::StreamingDecoder;
use serde_json::Value;
use std::collections::{HashMap, VecDeque};
use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub(super) const EXE: &str = "deepseek harness.exe";
const MAX_FILES: usize = 40;
const RECENT_MS: i64 = 30 * 60 * 1000;
const MAX_READ: u64 = 8 * 1024 * 1024;
const INITIAL_TAIL: u64 = 1024 * 1024;
const HEADER_READ: u64 = 256 * 1024;
const MAX_BODY_CHARS: usize = 240;

pub(super) struct Completion {
    pub title: String,
    pub body: String,
    pub time: i64,
}

struct EndEvent {
    sequence: i64,
    completion: Completion,
}

#[derive(Default)]
pub(super) struct Watcher {
    root: Option<PathBuf>,
    since: i64,
    cursors: HashMap<PathBuf, FileCursor>,
    delivered: HashMap<(String, i64), i64>,
}

impl Watcher {
    pub(super) fn poll(&mut self, config: &DshDoc, module_enabled: bool) -> Vec<Completion> {
        self.poll_at(
            config,
            module_enabled,
            chrono::Utc::now().timestamp_millis(),
        )
    }

    fn poll_at(&mut self, config: &DshDoc, module_enabled: bool, now: i64) -> Vec<Completion> {
        let root = (config.enabled && module_enabled).then(|| sessions_dir(&config.sessions_dir));
        if self.root != root {
            self.root = root;
            self.since = now;
            self.cursors.clear();
            self.delivered.clear();
        }
        let Some(root) = &self.root else {
            return Vec::new();
        };

        let since = self.since.max(now - RECENT_MS);
        self.delivered.retain(|_, time| *time > since);
        let mut files = Vec::new();
        collect_recent_files(root, 3, since, &mut files);
        files.sort_unstable_by_key(|file| std::cmp::Reverse(file.modified));
        let mut fresh = Vec::new();
        for file in files.into_iter().take(MAX_FILES) {
            let cursor = self.cursors.entry(file.path.clone()).or_default();
            cursor.last_seen = now;
            let events = cursor.scan(&file, root, config, since).unwrap_or_default();
            if events.is_empty() {
                continue;
            }
            let id = cursor
                .id
                .as_deref()
                .unwrap_or_else(|| directory_id(&file.path));
            for event in events {
                let key = (id.to_string(), event.sequence);
                if let std::collections::hash_map::Entry::Vacant(entry) = self.delivered.entry(key)
                {
                    entry.insert(event.completion.time);
                    fresh.push(event.completion);
                }
            }
        }
        self.cursors.retain(|_, cursor| cursor.last_seen >= since);
        fresh.sort_by_key(|item| item.time);
        fresh
    }
}

fn sessions_dir(raw: &str) -> PathBuf {
    let raw = raw.trim().trim_matches('"').trim();
    if raw.is_empty() {
        return sessions_root_path();
    }
    #[cfg(any(target_os = "windows", test))]
    let expanded = super::expand_env_vars(raw, |name| std::env::var(name).ok());
    #[cfg(any(target_os = "windows", test))]
    let raw = expanded.as_str();
    if let Some(relative) = raw.strip_prefix("~/").or_else(|| raw.strip_prefix("~\\")) {
        if let Some(home) = dirs::home_dir() {
            return home.join(relative);
        }
    }
    PathBuf::from(raw)
}

struct SessionFile {
    path: PathBuf,
    size: u64,
    modified: SystemTime,
}

fn collect_recent_files(dir: &Path, depth: u8, since: i64, files: &mut Vec<SessionFile>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if kind.is_dir() && depth > 0 {
            collect_recent_files(&path, depth - 1, since, files);
        } else if kind.is_file()
            && entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with("session.v") && name.ends_with(".jsonl.zstd"))
        {
            // Query the open file because Windows directory metadata can lag while
            // the DeepSeek process is appending a frame through an open handle.
            let Ok(metadata) = File::open(&path).and_then(|file| file.metadata()) else {
                continue;
            };
            let Ok(modified) = metadata.modified() else {
                continue;
            };
            if epoch_ms(modified) >= since - 2000 {
                files.push(SessionFile {
                    path,
                    size: metadata.len(),
                    modified,
                });
            }
        }
    }
}

fn epoch_ms(time: SystemTime) -> i64 {
    time.duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as i64)
}

fn directory_id(path: &Path) -> &str {
    path.parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .unwrap_or("")
}

fn read_at(path: &Path, offset: u64, limit: u64) -> io::Result<Vec<u8>> {
    let mut file = File::open(path)?;
    file.seek(SeekFrom::Start(offset))?;
    let mut data = Vec::new();
    file.take(limit).read_to_end(&mut data)?;
    Ok(data)
}

/// Consume one frame only. An unfinished frame keeps its original offset for the next poll.
fn decode_frame<'a>(data: &'a [u8], decoded: &mut Vec<u8>) -> Option<&'a [u8]> {
    decoded.clear();
    let mut decoder = StreamingDecoder::new(data).ok()?;
    decoder
        .by_ref()
        .take(MAX_READ + 1)
        .read_to_end(decoded)
        .ok()?;
    (decoded.len() as u64 <= MAX_READ).then(|| decoder.into_inner())
}

fn next_frame(data: &[u8]) -> Option<usize> {
    data.windows(ZSTD_MAGIC.len())
        .position(|magic| magic == ZSTD_MAGIC)
}

#[derive(Default)]
struct FileCursor {
    offset: u64,
    observed: Option<(u64, SystemTime)>,
    needs_more: bool,
    last_seen: i64,
    id: Option<String>,
    delegation_depth: i64,
    title: String,
    prompt: String,
    answers: VecDeque<(i64, String)>,
    line_tail: Vec<u8>,
}

impl FileCursor {
    fn scan(
        &mut self,
        file: &SessionFile,
        root: &Path,
        config: &DshDoc,
        since: i64,
    ) -> io::Result<Vec<EndEvent>> {
        let observed = (file.size, file.modified);
        if self.observed == Some(observed) && !self.needs_more {
            return Ok(Vec::new());
        }
        let rewritten = self.observed.is_some_and(|(size, modified)| {
            file.size < size || (file.size == size && file.modified != modified)
        });
        if self.observed.is_none() || rewritten {
            *self = FileCursor {
                last_seen: self.last_seen,
                ..Default::default()
            };
            self.title = cached_title(root, directory_id(&file.path));
            // Older sessions may already be large. The first frame carries the delegation
            // metadata; after that, only the recent tail is needed for completion previews.
            if file.size > INITIAL_TAIL {
                let header = read_at(&file.path, 0, HEADER_READ)?;
                let mut decoded = Vec::new();
                if decode_frame(&header, &mut decoded).is_some() {
                    self.accept(&decoded, config, i64::MAX, &mut Vec::new());
                }
                self.offset = file.size - INITIAL_TAIL;
            }
        }

        let available = file.size.saturating_sub(self.offset);
        let data = read_at(&file.path, self.offset, available.min(MAX_READ))?;
        let mut rest = data.as_slice();
        let mut decoded = Vec::new();
        let mut fresh = Vec::new();
        while !rest.is_empty() {
            let Some(start) = next_frame(rest) else {
                rest = &rest[rest.len().saturating_sub(ZSTD_MAGIC.len() - 1)..];
                break;
            };
            rest = &rest[start..];
            if let Some(next) = decode_frame(rest, &mut decoded) {
                self.accept(&decoded, config, since, &mut fresh);
                rest = next;
            } else {
                // Try any later frame when a rewritten file or tail read started inside a
                // frame. With only a partial final frame, leave it in place for the next append.
                let Some(next) = next_frame(&rest[1..]) else {
                    if rest.len() as u64 == MAX_READ {
                        rest = &rest[rest.len() - (ZSTD_MAGIC.len() - 1)..];
                        self.line_tail.clear();
                    }
                    break;
                };
                rest = &rest[next + 1..];
                self.line_tail.clear();
            }
        }
        self.offset += (data.len() - rest.len()) as u64;
        self.observed = Some(observed);
        self.needs_more = available > MAX_READ && data.len() as u64 == MAX_READ;
        Ok(fresh)
    }

    fn accept(&mut self, decoded: &[u8], config: &DshDoc, since: i64, fresh: &mut Vec<EndEvent>) {
        let mut bytes = std::mem::take(&mut self.line_tail);
        bytes.extend_from_slice(decoded);
        for line in bytes.split_inclusive(|byte| *byte == b'\n') {
            match serde_json::from_slice::<Value>(line) {
                Ok(event) => {
                    if let Some(end) = self.event(&event, config, since) {
                        fresh.push(end);
                    }
                }
                Err(_) if !line.ends_with(b"\n") && line.len() < MAX_READ as usize => {
                    self.line_tail.extend_from_slice(line);
                }
                Err(_) => {}
            }
        }
    }

    fn event(&mut self, event: &Value, config: &DshDoc, since: i64) -> Option<EndEvent> {
        let data = &event["data"];
        match event["type"].as_str()? {
            "session" => {
                self.id = event["id"].as_str().map(str::to_owned);
                self.delegation_depth = event["delegationDepth"].as_i64().unwrap_or(0);
            }
            "session/title" => {
                if let Some(title) = data["title"]
                    .as_str()
                    .filter(|title| !title.trim().is_empty())
                {
                    self.title = preview(title, 30);
                }
            }
            "user/message" if self.prompt.is_empty() => {
                self.prompt = blocks_preview(&data["content"], 30);
            }
            "assistant/message" => {
                let turn = data["turn"].as_i64()?;
                let answer = blocks_preview(&data["message"]["content"], MAX_BODY_CHARS);
                if !answer.is_empty() {
                    self.answers.retain(|(known, _)| *known != turn);
                    self.answers.push_back((turn, answer));
                    if self.answers.len() > 12 {
                        self.answers.pop_front();
                    }
                }
            }
            "turn/end" => return self.end(event, config, since),
            _ => {}
        }
        None
    }

    fn end(&mut self, event: &Value, config: &DshDoc, since: i64) -> Option<EndEvent> {
        let data = &event["data"];
        let answer = data["turn"].as_i64().and_then(|turn| {
            let index = self.answers.iter().position(|(known, _)| *known == turn)?;
            self.answers.remove(index).map(|(_, answer)| answer)
        });
        let time = event["time"].as_i64()?;
        if time <= since || (self.delegation_depth > 0 && !config.include_subagents) {
            return None;
        }
        let sequence = event["seq"].as_i64()?;
        let limit = config.body_chars.clamp(40, MAX_BODY_CHARS as i64) as usize;
        let title = [self.title.as_str(), self.prompt.as_str(), "DeepSeek"]
            .into_iter()
            .find(|title| !title.is_empty())?;
        let reason = &data["reason"];
        let (title, body) = match reason["kind"].as_str()? {
            "completed" => {
                let text = data["text"].as_str().filter(|text| !text.trim().is_empty());
                (
                    title.to_string(),
                    preview(text.or(answer.as_deref()).unwrap_or(""), limit),
                )
            }
            "error" => {
                let message = reason["error"]["message"].as_str().unwrap_or("生成失败");
                (format!("{title} · 生成失败"), preview(message, limit))
            }
            "max-tokens" => (
                format!("{title} · 输出被截断"),
                "已达到输出 token 上限".into(),
            ),
            _ => return None,
        };
        Some(EndEvent {
            sequence,
            completion: Completion { title, body, time },
        })
    }
}

fn cached_title(root: &Path, id: &str) -> String {
    let cached = root
        .parent()
        .and_then(|dir| {
            let path = dir
                .join("storages/session_projcache/sessions")
                .join(format!("{id}.json"));
            let data = read_at(&path, 0, INITIAL_TAIL).ok()?;
            serde_json::from_slice::<Value>(&data).ok()
        })
        .and_then(|document| {
            document
                .pointer("/record/rows/title/val")?
                .as_str()
                .map(str::to_owned)
        });
    preview(cached.as_deref().unwrap_or(""), 30)
}

fn blocks_preview(content: &Value, limit: usize) -> String {
    let text = content
        .as_array()
        .into_iter()
        .flatten()
        .filter(|block| block["type"].as_str() == Some("text"))
        .filter_map(|block| block["text"].as_str())
        .collect::<Vec<_>>()
        .join(" ");
    preview(&text, limit)
}

fn preview(raw: &str, limit: usize) -> String {
    let text = raw.replace("```", " ").replace("**", "").replace('`', "");
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let text = text.trim_start_matches(['#', '>', '-', '*', ' ']);
    match text.char_indices().nth(limit) {
        Some((end, _)) => format!("{}…", text[..end].trim_end()),
        None => text.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ruzstd::encoding::{compress_to_vec, CompressionLevel};
    use serde_json::json;
    use std::io::Write;

    fn end(seq: i64, time: i64, kind: &str) -> Value {
        json!({"type":"turn/end", "seq":seq, "time":time,
            "data":{"turn":1, "reason":{"kind":kind, "error":{"message":"测试错误"}}}})
    }

    #[test]
    fn previews_only_terminal_turns_with_visible_assistant_text() {
        let config = DshDoc::default();
        let mut cursor = FileCursor::default();
        let message = json!({"type":"assistant/message", "data":{"turn":1, "message":{"content":[
            {"type":"thinking","text":"private reasoning"},
            {"type":"text","text":"## 你好\n**世界**"}]}}});
        cursor.event(
            &json!({"type":"session/title","data":{"title":"测试会话"}}),
            &config,
            100,
        );
        cursor.event(&message, &config, 100);
        let completion = cursor
            .event(&end(4, 101, "completed"), &config, 100)
            .unwrap();
        assert_eq!(completion.sequence, 4);
        assert_eq!(completion.completion.title, "测试会话");
        assert_eq!(completion.completion.body, "你好 世界");
        assert!(cursor
            .event(&end(5, 100, "completed"), &config, 100)
            .is_none());
        assert!(cursor
            .event(&end(6, 102, "aborted"), &config, 100)
            .is_none());
        let error = cursor
            .event(&end(7, 102, "error"), &config, 100)
            .unwrap()
            .completion;
        assert_eq!(error.title, "测试会话 · 生成失败");
        assert_eq!(error.body, "测试错误");
        let truncated = cursor
            .event(&end(8, 102, "max-tokens"), &config, 100)
            .unwrap()
            .completion;
        assert_eq!(truncated.title, "测试会话 · 输出被截断");
    }

    #[test]
    fn subagent_detection_applies_even_with_a_separate_header_and_json_line_tail() {
        let mut cursor = FileCursor::default();
        let mut config = DshDoc::default();
        let header = br#"{"type":"session","id":"session-child","delegationDepth":1}"#;
        let mut emitted = Vec::new();
        cursor.accept(header, &config, i64::MAX, &mut emitted);
        assert!(cursor
            .event(&end(1, 101, "completed"), &config, 100)
            .is_none());
        config.include_subagents = true;
        let line = format!("{}\n", end(2, 102, "completed"));
        let (first, second) = line.as_bytes().split_at(12);
        cursor.accept(first, &config, 100, &mut emitted);
        assert!(emitted.is_empty());
        cursor.accept(second, &config, 100, &mut emitted);
        assert_eq!(emitted.len(), 1);
        assert_eq!(emitted[0].sequence, 2);
    }

    #[test]
    fn compressed_frames_leave_a_partial_final_frame_unconsumed() {
        let first = compress_to_vec(&b"first\n"[..], CompressionLevel::Uncompressed);
        let second = compress_to_vec(&b"second\n"[..], CompressionLevel::Uncompressed);
        let mut data = first.clone();
        data.extend_from_slice(&second[..second.len() - 2]);
        let mut decoded = Vec::new();
        let partial = decode_frame(&data, &mut decoded).unwrap();
        assert_eq!(decoded, b"first\n");
        assert_eq!(partial, &data[first.len()..]);
        assert!(decode_frame(partial, &mut decoded).is_none());
        data.extend_from_slice(&second[second.len() - 2..]);
        let rest = decode_frame(&data[first.len()..], &mut decoded).unwrap();
        assert!(rest.is_empty());
        assert_eq!(decoded, b"second\n");
    }

    #[test]
    fn reads_appended_completions_once_and_skips_history_on_reenable() {
        struct Fixture {
            root: PathBuf,
            log: PathBuf,
        }
        impl Drop for Fixture {
            fn drop(&mut self) {
                let _ = fs::remove_file(&self.log);
                let _ = fs::remove_dir(self.log.parent().unwrap());
                let _ = fs::remove_dir(self.root.join("test-project"));
                let _ = fs::remove_dir(&self.root);
            }
        }
        fn frame(event: &Value) -> Vec<u8> {
            let line = format!("{event}\n");
            compress_to_vec(line.as_bytes(), CompressionLevel::Uncompressed)
        }

        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("agenthud-dsh-{}-{unique}", std::process::id()));
        let dir = root.join("test-project/session-test");
        let fixture = Fixture {
            root,
            log: dir.join("session.v4.jsonl.zstd"),
        };
        fs::create_dir_all(&dir).unwrap();
        let mut file = File::create(&fixture.log).unwrap();
        let push = |file: &mut File, event: &Value| file.write_all(&frame(event)).unwrap();
        let now = chrono::Utc::now().timestamp_millis();
        push(
            &mut file,
            &json!({"type":"session","id":"session-test","delegationDepth":0}),
        );
        push(&mut file, &end(1, now - 1, "completed"));
        file.flush().unwrap();

        let config = DshDoc {
            sessions_dir: fixture.root.to_string_lossy().into_owned(),
            ..Default::default()
        };
        let mut watcher = Watcher::default();
        assert!(watcher.poll_at(&config, true, now).is_empty());
        let complete = end(2, now + 1, "completed");
        let compressed = frame(&complete);
        let (first, last) = compressed.split_at(compressed.len() - 5);
        file.write_all(first).unwrap();
        file.flush().unwrap();
        assert!(watcher.poll_at(&config, true, now + 2).is_empty());
        file.write_all(last).unwrap();
        file.flush().unwrap();
        assert_eq!(watcher.poll_at(&config, true, now + 3).len(), 1);
        push(&mut file, &complete);
        file.flush().unwrap();
        assert!(watcher.poll_at(&config, true, now + 4).is_empty());

        assert!(watcher.poll_at(&config, false, now + 5).is_empty());
        push(&mut file, &end(3, now + 6, "completed"));
        file.flush().unwrap();
        assert!(watcher.poll_at(&config, true, now + 7).is_empty());
        push(&mut file, &end(4, now + 8, "max-tokens"));
        file.flush().unwrap();
        let items = watcher.poll_at(&config, true, now + 9);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].title, "DeepSeek · 输出被截断");
        drop(file);
    }
}
