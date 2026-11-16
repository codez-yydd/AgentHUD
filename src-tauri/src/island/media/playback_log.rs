//! User-configured UTF-8 playback state files and append-only playback logs.

use super::MediaState;
use serde::Deserialize;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

const MAX_TAIL: u64 = 64 * 1024;
const MAX_AGE_MS: i64 = 60_000;
const MAX_CLOCK_SKEW_MS: i64 = 5_000;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PlaybackRecord {
    app_id: String,
    #[serde(default)]
    app_name: String,
    title: String,
    #[serde(default)]
    artist: String,
    #[serde(default)]
    album: String,
    playing: bool,
    #[serde(default)]
    position: Option<f64>,
    #[serde(default)]
    duration: f64,
    updated_at: i64,
}

impl PlaybackRecord {
    fn state(self, now: i64) -> Option<MediaState> {
        let title = self.title.trim();
        let app_id = self.app_id.trim();
        let age = now.saturating_sub(self.updated_at);
        if title.is_empty()
            || app_id.is_empty()
            || !(-MAX_CLOCK_SKEW_MS..=MAX_AGE_MS).contains(&age)
        {
            return None;
        }

        let duration = self.duration.max(0.0);
        let mut position = self.position.unwrap_or_default().max(0.0);
        if self.playing && self.position.is_some() {
            position += age.max(0) as f64 / 1000.0;
        }
        let finished = duration > 0.0 && position >= duration;
        Some(MediaState {
            has_session: true,
            app_id: app_id.into(),
            app_name: if self.app_name.trim().is_empty() {
                super::friendly_name(app_id)
            } else {
                self.app_name.trim().into()
            },
            title: title.into(),
            artist: self.artist,
            album: self.album,
            playing: self.playing && !finished,
            position: if finished { duration } else { position },
            duration,
            source: "playback-log".into(),
            ..MediaState::default()
        })
    }
}

fn parse(file: &[u8], jsonl: bool, skip_first: bool) -> Option<PlaybackRecord> {
    let file = file.strip_prefix(b"\xef\xbb\xbf").unwrap_or(file);
    if !jsonl {
        return serde_json::from_slice(file).ok();
    }
    let file = if skip_first {
        let newline = file.iter().position(|byte| *byte == b'\n')?;
        &file[newline + 1..]
    } else {
        file
    };
    // A writer may still be appending the last record. Fall back to the last complete record.
    file.rsplit(|byte| *byte == b'\n')
        .find_map(|line| serde_json::from_slice(line).ok())
}

pub(super) fn snapshot(path: &Path, now: i64) -> Option<MediaState> {
    let extension = path.extension()?.to_str()?;
    let jsonl = extension.eq_ignore_ascii_case("jsonl");
    if !path.is_absolute() || (!jsonl && !extension.eq_ignore_ascii_case("json")) {
        return None;
    }
    let metadata = path.metadata().ok()?;
    if !metadata.is_file() || (!jsonl && metadata.len() > MAX_TAIL) {
        return None;
    }
    let start = metadata.len().saturating_sub(MAX_TAIL);
    let mut file = File::open(path).ok()?;
    let mut bytes = Vec::new();
    file.seek(SeekFrom::Start(start.saturating_sub(1))).ok()?;
    file.take(MAX_TAIL + u64::from(start > 0))
        .read_to_end(&mut bytes)
        .ok()?;
    let (bytes, skip_first) = if start == 0 {
        (bytes.as_slice(), false)
    } else {
        let (preceding, bytes) = bytes.split_first()?;
        (bytes, *preceding != b'\n')
    };
    parse(bytes, jsonl, skip_first)?.state(now)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_snapshot_advances_playing_progress_and_expires() {
        let bytes = br#"{"appId":"QQMusic.exe","title":"Song","playing":true,"position":4,"duration":20,"updatedAt":100000}"#;
        let state = parse(bytes, false, false).unwrap().state(101500).unwrap();
        assert_eq!(state.app_name, "QQ音乐");
        assert_eq!(state.position, 5.5);
        assert!(state.playing);
        assert!(!state.can_toggle);
        assert!(parse(bytes, false, false).unwrap().state(160001).is_none());
        let finished = parse(bytes, false, false).unwrap().state(120000).unwrap();
        assert_eq!(finished.position, 20.0);
        assert!(!finished.playing);
    }

    #[test]
    fn jsonl_uses_newest_complete_record_and_accepts_empty_title_to_clear_it() {
        let bytes = br#"partial
{"appId":"test","title":"First","playing":true,"updatedAt":100000}
{"appId":"test","title":"","playing":false,"updatedAt":100100}
{"appId":"test","title":"unfinished"#;
        let record = parse(bytes, true, true).unwrap();
        assert!(record.state(100200).is_none());
    }
}
