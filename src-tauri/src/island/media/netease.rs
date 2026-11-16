//! NetEase Cloud Music 3.x writes its player events to an encoded cloudmusic.elog.

use super::MediaState;
use rusqlite::{Connection, OpenFlags};
use serde::Deserialize;
use serde_json::Value;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::Mutex;
use windows::core::PWSTR;
use windows::Win32::Foundation::{CloseHandle, FILETIME, STILL_ACTIVE};
use windows::Win32::System::SystemInformation::{GetSystemTimeAsFileTime, GetTickCount64};
use windows::Win32::System::Threading::{
    GetExitCodeProcess, GetProcessTimes, OpenProcess, QueryFullProcessImageNameW,
    PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};

const MAX_TAIL: u64 = 4 * 1024 * 1024;
const MAX_LINE: usize = 1024 * 1024;
const CHECKPOINT: u64 = 64;
static READER: Mutex<Option<LogReader>> = Mutex::new(None);

#[derive(Default)]
struct LogReader {
    offset: u64,
    checkpoint: Vec<u8>,
    partial: Vec<u8>,
    skip_line: bool,
    player: Player,
}

#[derive(Default)]
struct Player {
    pid: u32,
    track: Option<Track>,
    playing: bool,
    position: f64,
    anchor: u64,
    track_tick: u64,
    next_lookup: u64,
    awaiting_load: bool,
}

#[derive(Default)]
struct Track {
    id: String,
    title: String,
    artist: String,
    album: String,
    duration: f64,
    cover: String,
}

// Byte substitution for cloudmusic.elog, documented by netease-cloudmusic-detector.
fn decode(byte: u8) -> u8 {
    let high = byte >> 4;
    let low = byte & 15;
    ((high ^ (low ^ 8)) << 4) | ((high & 12) | (!high & 3))
}

impl LogReader {
    fn poll(&mut self, file: &mut File, now: u64) -> io::Result<()> {
        let end = file.metadata()?.len();
        let mut previous = [0; CHECKPOINT as usize];
        let check = &mut previous[..self.checkpoint.len()];
        let unchanged = end >= self.offset && {
            file.seek(SeekFrom::Start(self.offset - check.len() as u64))?;
            file.read_exact(check)?;
            check == self.checkpoint
        };
        if unchanged && end == self.offset {
            return Ok(());
        }

        let restart = !unchanged || end - self.offset > MAX_TAIL;
        let start = if restart {
            end.saturating_sub(MAX_TAIL)
        } else {
            self.offset
        };
        if restart {
            let skip_line = if start > 0 {
                file.seek(SeekFrom::Start(start - 1))?;
                let mut before = [0];
                file.read_exact(&mut before)?;
                decode(before[0]) != b'\n'
            } else {
                false
            };
            *self = Self {
                skip_line,
                ..Self::default()
            };
        }

        // Read the exact snapshot length. On a concurrent truncate, retry on the next poll.
        file.seek(SeekFrom::Start(start))?;
        let mut encoded = vec![0; (end - start) as usize];
        file.read_exact(&mut encoded)?;
        let keep = end.min(CHECKPOINT) as usize;
        if encoded.len() < keep {
            self.checkpoint
                .drain(..self.checkpoint.len() - (keep - encoded.len()));
        } else {
            self.checkpoint.clear();
        }
        self.checkpoint
            .extend_from_slice(&encoded[encoded.len().saturating_sub(keep)..]);
        self.offset = end;

        for byte in encoded.into_iter().map(decode) {
            if byte == b'\n' {
                if !self.skip_line {
                    if let Ok(line) = std::str::from_utf8(&self.partial) {
                        self.player.event(line, now);
                    }
                }
                self.partial.clear();
                self.skip_line = false;
            } else if !self.skip_line {
                if self.partial.len() == MAX_LINE {
                    self.partial.clear();
                    self.skip_line = true;
                } else {
                    self.partial.push(byte);
                }
            }
        }
        Ok(())
    }
}

fn json_payload(text: &str) -> Option<Value> {
    let (_, value) = text.split_once('{')?;
    let start = text.len() - value.len() - 1;
    Value::deserialize(&mut serde_json::Deserializer::from_str(&text[start..])).ok()
}

fn song_id(value: &Value) -> Option<String> {
    if let Some(id) = value.as_u64().filter(|id| *id > 0) {
        return Some(id.to_string());
    }
    let id = value.as_str()?;
    (!id.is_empty() && id.bytes().all(|byte| byte.is_ascii_digit())).then(|| id.to_owned())
}

impl Track {
    fn from_event(args: &str) -> Option<Self> {
        let payload = json_payload(args)?;
        let data = payload
            .get("track")
            .filter(|value| value.is_object())
            .unwrap_or(&payload);
        let id = song_id(&data["id"]).or_else(|| song_id(&payload["id"]))?;
        let artists = data["artists"].as_array().or_else(|| data["ar"].as_array());
        let album = data
            .get("album")
            .or_else(|| data.get("al"))
            .unwrap_or(&Value::Null);
        let duration = data["duration"]
            .as_f64()
            .or_else(|| data["dt"].as_f64())
            .unwrap_or_default();
        Some(Self {
            id,
            title: data["name"].as_str().unwrap_or_default().trim().into(),
            artist: artists
                .into_iter()
                .flatten()
                .filter_map(|artist| artist["name"].as_str())
                .collect::<Vec<_>>()
                .join(" / "),
            album: album["name"].as_str().unwrap_or_default().into(),
            duration: duration.max(0.0) / 1000.0,
            cover: album["picUrl"]
                .as_str()
                .filter(|url| url.starts_with("https://"))
                .unwrap_or_default()
                .into(),
        })
    }
}

impl Player {
    fn event(&mut self, line: &str, now: u64) {
        let Some(header) = line.strip_prefix('[') else {
            return;
        };
        let mut fields = header.splitn(5, ':');
        let (Some(pid), Some(_), Some(date), Some(tick), Some(message)) = (
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
        ) else {
            return;
        };
        let (Ok(pid), Ok(tick)) = (pid.parse::<u32>(), tick.parse::<u64>()) else {
            return;
        };
        if pid == 0 || !date.contains('/') || tick > now {
            return;
        }
        if message.contains("exitApp") && pid == self.pid
            || message.split_once("Process ").is_some_and(|(_, tail)| {
                let mut words = tail.split_whitespace();
                words.next().and_then(|id| id.parse::<u32>().ok()) == Some(self.pid)
                    && words.next().is_some_and(|word| word.starts_with("exited"))
            })
        {
            *self = Self::default();
            return;
        }
        let Some((_, event)) = message.split_once("【playing】") else {
            return;
        };
        let Some((_, quoted)) = event.split_once('"') else {
            return;
        };
        let Some((kind, args)) = quoted.split_once('"') else {
            return;
        };
        match kind {
            "checkPlayPrivilege" | "playOneTrackInPlayingList" => {
                let Some(track) = Track::from_event(args) else {
                    return;
                };
                let same =
                    self.pid == pid && self.track.as_ref().is_some_and(|old| old.id == track.id);
                if !same {
                    self.pid = pid;
                    self.track_tick = tick;
                    self.next_lookup = 0;
                    self.set_position(0.0, tick, false);
                }
                if !same || !track.title.is_empty() {
                    self.track = Some(track);
                }
                if kind == "playOneTrackInPlayingList" {
                    self.set_position(0.0, tick, true);
                    self.awaiting_load = true;
                }
            }
            "setPlayingPosition" if pid == self.pid => {
                if let Some(value) = argument(args).as_ref().and_then(Value::as_f64) {
                    self.set_position(value, tick, self.playing);
                }
            }
            kind if pid == self.pid && kind.starts_with("native") && kind.ends_with("state") => {
                match argument(args).as_ref().and_then(Value::as_u64) {
                    Some(1) => {
                        self.position = self.position_at(tick);
                        self.anchor = tick;
                        self.playing = true;
                    }
                    Some(2) => self.set_position(self.position_at(tick), tick, false),
                    _ => {}
                }
            }
            kind if kind.starts_with("native")
                && kind.contains("load")
                && kind.contains("完成") =>
            {
                let id = json_payload(args)
                    .as_ref()
                    .and_then(|payload| song_id(&payload["songId"]));
                let Some(id) = id else { return };
                if self.pid != pid || self.track.as_ref().is_none_or(|track| track.id != id) {
                    self.pid = pid;
                    self.track_tick = tick;
                    self.track = Some(Track {
                        id,
                        ..Track::default()
                    });
                    self.next_lookup = 0;
                    self.set_position(0.0, tick, true);
                } else if self.awaiting_load || !self.playing {
                    self.set_position(0.0, tick, true);
                }
            }
            _ => {}
        }
    }

    fn position_at(&self, tick: u64) -> f64 {
        if self.playing {
            self.position + tick.saturating_sub(self.anchor) as f64 / 1000.0
        } else {
            self.position
        }
    }

    fn set_position(&mut self, position: f64, tick: u64, playing: bool) {
        self.position = position.max(0.0);
        self.anchor = tick;
        self.playing = playing;
        self.awaiting_load = false;
    }

    fn snapshot(&self, now: u64) -> Option<MediaState> {
        let track = self
            .track
            .as_ref()
            .filter(|track| !track.title.is_empty())?;
        let position = self.position_at(now);
        let finished = track.duration > 0.0 && position >= track.duration;
        Some(MediaState {
            has_session: true,
            source: "netease-log".into(),
            app_id: "cloudmusic.exe".into(),
            app_name: "网易云音乐".into(),
            title: track.title.clone(),
            artist: track.artist.clone(),
            album: track.album.clone(),
            cover: track.cover.clone(),
            duration: track.duration,
            position: if finished { track.duration } else { position },
            playing: self.playing && !finished,
            ..MediaState::default()
        })
    }
}

fn read_track(root: &Path, id: &str) -> Option<Track> {
    let id = id.parse::<i64>().ok()?;
    let database = Connection::open_with_flags(
        root.join("Library/webdb.dat"),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .ok()?;
    database.busy_timeout(std::time::Duration::ZERO).ok()?;
    let payload: String = database
        .query_row(
            "SELECT jsonStr FROM historyTracks WHERE id = ? ORDER BY playtime DESC LIMIT 1",
            [id],
            |row| row.get(0),
        )
        .ok()?;
    Track::from_event(&payload)
        .filter(|track| track.id == id.to_string() && !track.title.is_empty())
}

fn argument(args: &str) -> Option<Value> {
    let arg = args.trim_start().strip_prefix(',')?.trim_start();
    Value::deserialize(&mut serde_json::Deserializer::from_str(arg)).ok()
}

fn filetime_ticks(time: FILETIME) -> u64 {
    (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime)
}

fn cloudmusic_process(pid: u32, track_tick: u64, now: u64) -> bool {
    let Ok(handle) = (unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }) else {
        return false;
    };
    let mut name = [0u16; 1024];
    let mut len = name.len() as u32;
    let mut exit_code = 0;
    let mut created = FILETIME::default();
    let mut exited = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    let valid = unsafe {
        let valid = GetExitCodeProcess(handle, &mut exit_code).is_ok()
            && exit_code == STILL_ACTIVE.0 as u32
            && QueryFullProcessImageNameW(
                handle,
                PROCESS_NAME_WIN32,
                PWSTR(name.as_mut_ptr()),
                &mut len,
            )
            .is_ok()
            && GetProcessTimes(handle, &mut created, &mut exited, &mut kernel, &mut user).is_ok();
        let _ = CloseHandle(handle);
        valid
    };
    if !valid {
        return false;
    }
    let path = String::from_utf16_lossy(&name[..len as usize]);
    let age = filetime_ticks(unsafe { GetSystemTimeAsFileTime() })
        .saturating_sub(filetime_ticks(created))
        / 10_000;
    let start_tick = now.saturating_sub(age);
    track_tick.saturating_add(1000) >= start_tick
        && Path::new(&path)
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.eq_ignore_ascii_case("cloudmusic.exe"))
}

pub(super) fn snapshot() -> Option<MediaState> {
    let root = dirs::data_local_dir()?.join("NetEase/CloudMusic");
    let mut reader = READER.lock().ok()?;
    let Ok(mut file) = File::open(root.join("cloudmusic.elog")) else {
        *reader = None;
        return None;
    };
    let now = unsafe { GetTickCount64() };
    let log = reader.get_or_insert_with(LogReader::default);
    log.poll(&mut file, now).ok()?;
    let player = &mut log.player;
    if let Some(track) = &player.track {
        if !cloudmusic_process(player.pid, player.track_tick, now) {
            *player = Player::default();
        } else if track.title.is_empty() && now >= player.next_lookup {
            player.next_lookup = now.saturating_add(5_000);
            if let Some(track) = read_track(&root, &track.id) {
                player.track = Some(track);
            }
        }
    }
    player.snapshot(now)
}

pub(super) fn reset() {
    if let Ok(mut reader) = READER.lock() {
        *reader = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn line(tick: u64, event: &str) -> String {
        format!(
            "[42:3:1115/120000:{tick}:INFO:client(1)] [2026-11-15 12:00:00] 【playing】,{event}\n"
        )
    }

    #[test]
    fn playback_reconstructs_current_track_pause_seek_and_resume() {
        let mut player = Player::default();
        let track = r#"{"id":"123","track":{"id":"123","name":"歌曲","artists":[{"name":"歌手"}],"duration":120000}}"#;
        for (tick, event) in [
            (1_000, format!(r#""playOneTrackInPlayingList",{track}"#)),
            (
                2_000,
                r#""native播放资源load完成，开始播放",{"songId":"123"}"#.into(),
            ),
            (4_000, r#""setPlayingPosition",25.5"#.into()),
            (5_000, r#""native播放state",2,"other""#.into()),
        ] {
            player.event(&line(tick, &event), 20_000);
        }
        let paused = player.snapshot(8_000).unwrap();
        assert_eq!(paused.title, "歌曲");
        assert_eq!(paused.artist, "歌手");
        assert_eq!(paused.duration, 120.0);
        assert_eq!(paused.position, 26.5);
        assert!(!paused.playing);
        assert!(!paused.can_toggle);

        player.event(&line(9_000, r#""native播放state",1,"other""#), 20_000);
        assert_eq!(player.snapshot(11_000).unwrap().position, 28.5);
        assert!(player.snapshot(11_000).unwrap().playing);
        player.event(
            "[7:3:1115/120000:12000:INFO:client(1)] Process 42 exited",
            20_000,
        );
        assert!(player.snapshot(12_000).is_none());
    }

    #[test]
    fn reader_carries_split_records_and_replays_a_rewritten_file() {
        assert_eq!([0x2c, 0x91, 0x19].map(decode), *b"a\n\x02");
        let mut inverse = [0; 256];
        for byte in 0..=u8::MAX {
            inverse[usize::from(decode(byte))] = byte;
        }
        let encode = |line: String| {
            line.bytes()
                .map(|byte| inverse[usize::from(byte)])
                .collect::<Vec<_>>()
        };
        let path = std::env::temp_dir().join(format!("agenthud-elog-{}", uuid::Uuid::new_v4()));
        let mut file = File::options()
            .create_new(true)
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        let mut reader = LogReader::default();
        let old = encode(line(
            1_000,
            r#""playOneTrackInPlayingList",{"id":"123","name":"歌曲 A"}"#,
        ));
        file.write_all(&old[..old.len() - 3]).unwrap();
        reader.poll(&mut file, 20_000).unwrap();
        assert!(reader.player.snapshot(20_000).is_none());
        file.seek(SeekFrom::End(0)).unwrap();
        file.write_all(&old[old.len() - 3..]).unwrap();
        reader.poll(&mut file, 20_000).unwrap();
        assert_eq!(reader.player.snapshot(20_000).unwrap().title, "歌曲 A");

        let new = encode(line(
            2_000,
            r#""playOneTrackInPlayingList",{"id":"789","name":"歌曲 B"}"#,
        ));
        assert_eq!(new.len(), old.len());
        file.rewind().unwrap();
        file.write_all(&new).unwrap();
        reader.poll(&mut file, 20_000).unwrap();
        drop(file);
        std::fs::remove_file(&path).unwrap();
        assert_eq!(reader.player.snapshot(20_000).unwrap().title, "歌曲 B");
    }
}
