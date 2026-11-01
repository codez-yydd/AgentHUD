//! Read-only evidence for ZCode's first model output (including tool arguments).
//!
//! `model_usage.first_token_at` only covers text/reasoning in some CLI versions.
//! Join the database's (trace_id, span_id) to model.request.completed, then join
//! its (trace, session, requestId, attempt) to ttft.model.first_content. Never
//! associate concurrent requests by time proximity. Use the log's wall-clock
//! timestamp: context.at uses performance.timeOrigin and can have clock skew.
//!
//! The shared index tails JSONL incrementally. Each SQL connection captures an
//! immutable snapshot, so every row in a query uses the same evidence. Neither
//! ZCode's database nor its logs are modified; no prompts are retained.

use rusqlite::{functions::FunctionFlags, Connection};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs::{self, File},
    io::{BufRead, BufReader, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
    time::SystemTime,
};

const SQL_FUNCTION: &str = "agenthud_zcode_first_content";

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct AttemptKey {
    trace: String,
    session: String,
    request: String,
    attempt: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Bridge {
    request: AttemptKey,
    completed_at: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Evidence {
    session: String,
    first_at: i64,
    completed_at: i64,
}

type Snapshot = HashMap<String, HashMap<String, Evidence>>;

#[derive(Default)]
struct LogIndex {
    first: HashMap<AttemptKey, Option<i64>>,
    bridges: HashMap<(String, String), Option<Bridge>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LogLine {
    event: String,
    timestamp: String,
    trace_id: String,
    span_id: String,
    session_id: String,
    context: LogContext,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LogContext {
    request_id: String,
    attempt: u32,
    #[serde(default)]
    output_kind: Option<String>,
}

/// Duplicate copies from rotation are harmless; conflicting evidence is not.
fn insert_unique<K: Eq + std::hash::Hash, V: PartialEq>(
    map: &mut HashMap<K, Option<V>>,
    key: K,
    value: V,
) {
    use std::collections::hash_map::Entry;
    match map.entry(key) {
        Entry::Vacant(entry) => {
            entry.insert(Some(value));
        }
        Entry::Occupied(mut entry) => {
            if entry.get().as_ref() != Some(&value) {
                entry.insert(None);
            }
        }
    }
}

impl LogIndex {
    fn ingest(&mut self, bytes: &[u8]) {
        // Skip unrelated (and potentially large) prompt/tool log messages before
        // JSON deserialization. We only retain identifiers and timing evidence.
        let Ok(text) = std::str::from_utf8(bytes) else {
            return;
        };
        if !text.contains("\"ttft.model.first_content\"")
            && !text.contains("\"model.request.completed\"")
        {
            return;
        }
        let Ok(line) = serde_json::from_slice::<LogLine>(bytes) else {
            return;
        };
        if line.trace_id.is_empty()
            || line.session_id.is_empty()
            || line.context.request_id.is_empty()
            || line.context.attempt == 0
        {
            return;
        }
        let Ok(timestamp) = chrono::DateTime::parse_from_rfc3339(&line.timestamp) else {
            return;
        };
        let at = timestamp.timestamp_millis();
        if at <= 0 {
            return;
        }
        let key = AttemptKey {
            trace: line.trace_id.clone(),
            session: line.session_id,
            request: line.context.request_id,
            attempt: line.context.attempt,
        };
        match line.event.as_str() {
            "ttft.model.first_content"
                if matches!(
                    line.context.output_kind.as_deref(),
                    Some("text" | "reasoning" | "tool")
                ) =>
            {
                insert_unique(&mut self.first, key, at);
            }
            "model.request.completed" if !line.span_id.is_empty() => {
                insert_unique(
                    &mut self.bridges,
                    (line.trace_id, line.span_id),
                    Bridge {
                        request: key,
                        completed_at: at,
                    },
                );
            }
            _ => {}
        }
    }

    fn snapshot(&self) -> Snapshot {
        let mut result = Snapshot::new();
        for ((trace, span), bridge) in &self.bridges {
            let Some(bridge) = bridge else {
                continue;
            };
            let Some(Some(first_at)) = self.first.get(&bridge.request) else {
                continue;
            };
            if *first_at > bridge.completed_at {
                continue;
            }
            result.entry(trace.clone()).or_default().insert(
                span.clone(),
                Evidence {
                    session: bridge.request.session.clone(),
                    first_at: *first_at,
                    completed_at: bridge.completed_at,
                },
            );
        }
        result
    }
}

#[derive(Default)]
struct Cursor {
    offset: u64,
    modified: Option<SystemTime>,
    created: Option<SystemTime>,
}

#[derive(Default)]
struct CachedLogs {
    files: HashMap<PathBuf, Cursor>,
    index: LogIndex,
    snapshot: Arc<Snapshot>,
    cache_file: Option<PathBuf>,
}

#[derive(Serialize, Deserialize)]
struct SavedSnapshot {
    version: u32,
    snapshot: Snapshot,
}

impl CachedLogs {
    fn load(cache_file: Option<PathBuf>) -> Self {
        let snapshot = cache_file
            .as_ref()
            .and_then(|path| fs::read(path).ok())
            .and_then(|bytes| serde_json::from_slice::<SavedSnapshot>(&bytes).ok())
            .filter(|saved| saved.version == 1)
            .map(|saved| saved.snapshot)
            .unwrap_or_default();
        Self {
            snapshot: Arc::new(snapshot),
            cache_file,
            ..Self::default()
        }
    }

    fn save(&self) {
        let Some(path) = &self.cache_file else {
            return;
        };
        let Ok(bytes) = serde_json::to_vec(&SavedSnapshot {
            version: 1,
            snapshot: self.snapshot.as_ref().clone(),
        }) else {
            return;
        };
        let temporary = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
        if fs::write(&temporary, bytes).is_ok() {
            // Only our own derived cache is replaced, never a ZCode source file.
            if fs::rename(&temporary, path).is_err() {
                let _ = fs::remove_file(&temporary);
            }
        }
    }

    fn refresh(&mut self, directory: &Path) -> Arc<Snapshot> {
        let Ok(entries) = fs::read_dir(directory) else {
            return self.snapshot.clone();
        };
        let mut files: Vec<_> = entries
            .flatten()
            .filter_map(|entry| {
                let name = entry.file_name();
                let name = name.to_str()?;
                if !name.starts_with("zcode-") || !name.ends_with(".jsonl") {
                    return None;
                }
                // Windows directory entries may cache a stale size while ZCode's
                // writer keeps the file open. Stat the path for the live length.
                let metadata = fs::metadata(entry.path()).ok()?;
                metadata.is_file().then(|| (entry.path(), metadata))
            })
            .collect();
        files.sort_by(|a, b| a.0.cmp(&b.0));
        // Replacement/truncation invalidates old associations. Removed rotated
        // files may retain already observed evidence until this process exits.
        if files.iter().any(|(path, meta)| {
            self.files.get(path).is_some_and(|old| {
                meta.len() < old.offset
                    || old.created != meta.created().ok()
                    || (meta.len() == old.offset && old.modified != meta.modified().ok())
            })
        }) {
            self.files.clear();
            self.index = LogIndex::default();
            self.snapshot = Arc::new(Snapshot::new());
            self.save();
        }
        let mut changed = false;
        for (path, meta) in files {
            let cursor = self.files.entry(path.clone()).or_default();
            if meta.len() == cursor.offset {
                continue;
            }
            let Ok(mut file) = File::open(&path) else {
                continue;
            };
            if file.seek(SeekFrom::Start(cursor.offset)).is_err() {
                continue;
            }
            let mut reader = BufReader::new(file);
            let mut line = Vec::new();
            while let Ok(bytes) = reader.read_until(b'\n', &mut line) {
                if bytes == 0 || line.last() != Some(&b'\n') {
                    break;
                }
                cursor.offset += bytes as u64;
                self.index.ingest(&line);
                changed = true;
                line.clear();
            }
            cursor.modified = meta.modified().ok();
            cursor.created = meta.created().ok();
        }
        if changed {
            // Retain verified historical evidence after normal log rotation and
            // app restart. Newly conflicting identities explicitly invalidate it.
            let mut next = self.snapshot.as_ref().clone();
            for ((trace, span), bridge) in &self.index.bridges {
                let invalid = match bridge {
                    None => true,
                    Some(bridge) => match self.index.first.get(&bridge.request) {
                        Some(Some(first)) => *first > bridge.completed_at,
                        Some(None) => true,
                        None => false, // Rotated first-content log: retain a saved proof.
                    },
                };
                if invalid {
                    if let Some(spans) = next.get_mut(trace) {
                        spans.remove(span);
                    }
                }
            }
            for (trace, spans) in self.index.snapshot() {
                next.entry(trace).or_default().extend(spans);
            }
            next.retain(|_, spans| !spans.is_empty());
            if next != *self.snapshot {
                self.snapshot = Arc::new(next);
                self.save();
            }
        }
        self.snapshot.clone()
    }
}

fn register(conn: &Connection, snapshot: Arc<Snapshot>) -> rusqlite::Result<()> {
    conn.create_scalar_function(
        SQL_FUNCTION,
        7,
        FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
        move |ctx| {
            let trace = ctx.get_raw(0).as_str().ok();
            let span = ctx.get_raw(1).as_str().ok();
            let session = ctx.get_raw(2).as_str().ok();
            let started: Option<i64> = ctx.get(3)?;
            let completed: Option<i64> = ctx.get(4)?;
            let database_first: Option<i64> = ctx.get(5)?;
            let tools: bool = ctx.get(6)?;
            let evidence = trace
                .zip(span)
                .and_then(|(trace, span)| snapshot.get(trace)?.get(span));
            if let (Some(evidence), Some(started), Some(completed)) = (evidence, started, completed)
            {
                if session == Some(evidence.session.as_str())
                    && started <= evidence.first_at
                    && evidence.first_at <= evidence.completed_at
                    && evidence.completed_at <= completed
                {
                    // A database delta may include whitespace that the log's
                    // first-content classifier skips. Keep the earliest actual
                    // output event; never shorten a covered stream window.
                    let first = database_first
                        .filter(|first| *first >= started && *first <= evidence.completed_at)
                        .map(|first| first.min(evidence.first_at))
                        .unwrap_or(evidence.first_at);
                    return Ok(Some(first));
                }
            }
            // A text/reasoning first event still covers a request without tools.
            // An unverified tool request keeps an explicitly labelled E2E rate.
            Ok(if tools { None } else { database_first })
        },
    )
}

/// Install an immutable, incrementally loaded log snapshot for this database.
/// In-memory/test and unrelated source databases never read the user's logs.
pub(crate) fn install(conn: &Connection) -> rusqlite::Result<bool> {
    let Some(path) = conn
        .path()
        .filter(|path| !path.is_empty() && *path != ":memory:")
    else {
        return Ok(false);
    };
    let Some(root) = Path::new(path).parent().and_then(Path::parent) else {
        return Ok(false);
    };
    let log_dir = root.join("log");
    let cache_file = persistence_path(&log_dir);
    if !log_dir.is_dir() && !cache_file.as_ref().is_some_and(|path| path.is_file()) {
        return Ok(false);
    }
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, CachedLogs>>> = OnceLock::new();
    let snapshot = match CACHE.get_or_init(Default::default).lock() {
        Ok(mut cache) => cache
            .entry(log_dir.clone())
            .or_insert_with(|| CachedLogs::load(cache_file))
            .refresh(&log_dir),
        Err(_) => return Ok(false),
    };
    register(conn, snapshot)?;
    Ok(true)
}

#[cfg(not(test))]
fn persistence_path(log_dir: &Path) -> Option<PathBuf> {
    use sha2::{Digest, Sha256};
    let root = fs::canonicalize(log_dir.parent()?).ok()?;
    let fingerprint = format!("{:x}", Sha256::digest(root.to_string_lossy().as_bytes()));
    Some(
        crate::pricing::config_dir()
            .ok()?
            .join(format!("zcode-speed-evidence-{fingerprint}.json")),
    )
}

// Regular tests must never write the user's application data. Persistence is
// exercised below with an explicit temporary path instead.
#[cfg(test)]
fn persistence_path(_: &Path) -> Option<PathBuf> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "reads the installed ZCode database and local diagnostic logs"]
    fn local_zcode_speed_evidence_audit() {
        let conn = crate::db::open_db().unwrap();
        conn.execute_batch("BEGIN").unwrap();
        let started = std::time::Instant::now();
        let first = crate::db::zcode_first_token_expr(&conn);
        let sql = format!("SELECT rowid,started_at,completed_at,output_tokens,{first},status,tool_call_count FROM model_usage WHERE started_at>=1793376000000 ORDER BY rowid");
        let values = conn
            .prepare(&sql)
            .unwrap()
            .query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, Option<i64>>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, Option<i64>>(4)?,
                    r.get::<_, Option<String>>(5)?,
                    r.get::<_, i64>(6)?,
                ))
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(!values.is_empty());
        let uploads = crate::db::query_since_at(&conn, values[0].0 - 1, 100000).unwrap();
        let uploaded: HashMap<_, _> = uploads.iter().map(|row| (row.local_rowid, row)).collect();
        let now = chrono::Utc::now().timestamp_millis();
        let mut verified_tools = 0;
        let mut stream = 0;
        let mut average = 0;
        let mut unavailable = 0;
        for (id, start, end, output, first, status, tools) in &values {
            let timing = crate::token_speed::RequestTiming {
                output_tokens: *output,
                started_at: Some(*start),
                completed_at: *end,
                first_token_at: *first,
                ..Default::default()
            };
            let local = crate::token_speed::is_completed_status(status.as_deref())
                .then(|| crate::token_speed::request_speed_at(&timing, now))
                .flatten();
            let wire: crate::sync::RemoteUsageRecord =
                serde_json::from_value(serde_json::to_value(uploaded[id]).unwrap()).unwrap();
            let remote = wire.speed_evidence.unwrap().speed(*output, *start);
            assert_eq!(
                local.as_ref().map(|s| (s.value, s.quality)),
                remote.as_ref().map(|s| (s.value, s.quality))
            );
            if *tools > 0 && first.is_some() {
                verified_tools += 1;
            }
            match local.map(|s| s.quality) {
                Some(crate::token_speed::SpeedQuality::Generation) => stream += 1,
                Some(crate::token_speed::SpeedQuality::RequestAverage) => average += 1,
                None => unavailable += 1,
            }
            if *start == 1793520715804 {
                assert_eq!(*first, Some(1793520719718));
                let speed = remote.unwrap();
                assert!((speed.value - 334000.0 / 868.0).abs() < 1e-9);
                eprintln!(
                    "[first-content audit] screenshot: {:.3} t/s, {:?}, source-to-sync exact match",
                    speed.value, speed.quality
                );
            }
        }
        assert!(verified_tools > 0);
        eprintln!("[first-content audit] {} rows: {verified_tools} tool requests with exact evidence, {stream} stream / {average} request-average / {unavailable} unavailable; {:?}",values.len(),started.elapsed());
    }

    fn event(
        kind: &str,
        at: i64,
        span: &str,
        request: &str,
        attempt: u32,
        output: &str,
    ) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "event":kind,"timestamp":chrono::DateTime::from_timestamp_millis(at).unwrap().to_rfc3339(),
            "traceId":"trace","sessionId":"session","spanId":span,
            "context":{"requestId":request,"attempt":attempt,"outputKind":output,
                // Deliberately wrong: never use performance.timeOrigin as UTC.
                "at":at+3000}
        })).unwrap()
    }

    #[test]
    fn exact_span_and_attempt_cover_tool_output_and_ignore_clock_skew() {
        let mut index = LogIndex::default();
        // Out of order, as when files from several processes are interleaved.
        index.ingest(&event(
            "model.request.completed",
            4800,
            "span",
            "request",
            2,
            "",
        ));
        index.ingest(&event(
            "ttft.model.first_content",
            3900,
            "parent",
            "request",
            2,
            "tool",
        ));
        index.ingest(&event(
            "ttft.model.first_content",
            1100,
            "parent",
            "request",
            1,
            "text",
        ));
        let conn = Connection::open_in_memory().unwrap();
        register(&conn, Arc::new(index.snapshot())).unwrap();
        let query = |span, session, tools| {
            conn.query_row(
                "SELECT agenthud_zcode_first_content('trace',?1,?2,1000,4850,4700,?3)",
                rusqlite::params![span, session, tools],
                |r| r.get::<_, Option<i64>>(0),
            )
            .unwrap()
        };
        assert_eq!(query("span", "session", true), Some(3900));
        assert_eq!(query("other-span", "session", true), None);
        assert_eq!(query("span", "other-session", true), None);
        assert_eq!(query("other-span", "session", false), Some(4700));
    }

    #[test]
    fn conflicting_ids_or_milestones_fail_closed() {
        let first = event("ttft.model.first_content", 2000, "parent", "a", 1, "text");
        let done = event("model.request.completed", 3000, "span", "a", 1, "");
        let mut index = LogIndex::default();
        for line in [&first, &done, &first, &done] {
            index.ingest(line);
        }
        assert_eq!(index.snapshot()["trace"]["span"].first_at, 2000);
        index.ingest(&event("model.request.completed", 3000, "span", "b", 1, ""));
        assert!(index.snapshot().is_empty());
        let mut index = LogIndex::default();
        for line in [&first, &done] {
            index.ingest(line);
        }
        index.ingest(&event(
            "ttft.model.first_content",
            2100,
            "parent",
            "a",
            1,
            "reasoning",
        ));
        assert!(index.snapshot().is_empty());
    }

    #[test]
    fn incremental_reader_retries_partial_lines_and_rebuilds_on_truncation() {
        use std::io::Write;
        let dir =
            std::env::temp_dir().join(format!("agenthud-speed-evidence-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("zcode-test.jsonl");
        let first = event("ttft.model.first_content", 2000, "parent", "a", 1, "tool");
        let done = event("model.request.completed", 3000, "span", "a", 1, "");
        let mut file = File::create(&path).unwrap();
        file.write_all(&first).unwrap();
        file.write_all(b"\n").unwrap();
        file.write_all(&done[..10]).unwrap();
        file.flush().unwrap();
        let mut cache = CachedLogs::default();
        assert!(cache.refresh(&dir).is_empty());
        let offset = cache.files[&path].offset;
        file.write_all(&done[10..]).unwrap();
        file.write_all(b"\n").unwrap();
        file.flush().unwrap();
        let snapshot = cache.refresh(&dir);
        assert!(
            !snapshot.is_empty(),
            "first={:?} bridges={:?} offset={} len={}",
            cache.index.first,
            cache.index.bridges,
            cache.files[&path].offset,
            fs::metadata(&path).unwrap().len()
        );
        assert_eq!(snapshot["trace"]["span"].first_at, 2000);
        assert!(cache.files[&path].offset > offset);
        drop(file);
        fs::write(&path, b"{}\n").unwrap();
        assert!(cache.refresh(&dir).is_empty());
        assert!(fs::canonicalize(&dir)
            .unwrap()
            .starts_with(fs::canonicalize(std::env::temp_dir()).unwrap()));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn persisted_evidence_survives_rotation_but_not_conflicting_identity() {
        let dir =
            std::env::temp_dir().join(format!("agenthud-speed-evidence-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("zcode-test.jsonl");
        let saved = dir.join("evidence.json");
        let mut lines = event("ttft.model.first_content", 2000, "parent", "a", 1, "tool");
        lines.push(b'\n');
        lines.extend(event("model.request.completed", 3000, "span", "a", 1, ""));
        lines.push(b'\n');
        fs::write(&path, lines).unwrap();
        let mut cache = CachedLogs::load(Some(saved.clone()));
        assert_eq!(cache.refresh(&dir)["trace"]["span"].first_at, 2000);
        fs::remove_file(&path).unwrap();
        let mut restarted = CachedLogs::load(Some(saved.clone()));
        assert_eq!(restarted.refresh(&dir)["trace"]["span"].first_at, 2000);
        let mut conflict = event("model.request.completed", 3000, "span", "a", 1, "");
        conflict.push(b'\n');
        conflict.extend(event("model.request.completed", 3000, "span", "b", 1, ""));
        conflict.push(b'\n');
        fs::write(&path, conflict).unwrap();
        assert!(restarted.refresh(&dir).is_empty());
        assert!(CachedLogs::load(Some(saved)).snapshot.is_empty());
        assert!(fs::canonicalize(&dir)
            .unwrap()
            .starts_with(fs::canonicalize(std::env::temp_dir()).unwrap()));
        fs::remove_dir_all(&dir).unwrap();
    }
}
