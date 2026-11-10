//! 灵动岛配置持久化。
//!
//! 以前配置单独落在 `~/.agenthud/island.json`，右键菜单还能直接打开编辑；
//! 现在所有选项都在设置页里，配置文件不再作为用户入口。这里把配置收进
//! 一个只由应用读写的 SQLite 小库（`~/.agenthud/island.db`），首次启动
//! 自动把旧版 `island.json` 迁移进来并删除旧文件。
//!
//! 读路径走进程内缓存：全屏守护线程会频繁 `load_config()`，不能每次都开库。
//! 写路径先更新缓存，再以单行覆盖的方式落库；写失败会回滚缓存，避免界面
//! 显示“已保存”但重启后丢失。

use rusqlite::{params, Connection};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use super::screenshot::ShotSettings;
use super::IslandConfig;

const DB_NAME: &str = "island.db";
const LEGACY_JSON: &str = "island.json";

static CACHE: OnceLock<Mutex<Option<IslandConfig>>> = OnceLock::new();

fn cache() -> &'static Mutex<Option<IslandConfig>> {
    CACHE.get_or_init(|| Mutex::new(None))
}

fn db_path() -> Result<PathBuf, String> {
    Ok(crate::pricing::config_dir()?.join(DB_NAME))
}

fn legacy_path() -> Result<PathBuf, String> {
    Ok(crate::pricing::config_dir()?.join(LEGACY_JSON))
}

fn open() -> Result<Connection, String> {
    let path = db_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建配置目录失败: {e}"))?;
    }
    let conn = Connection::open(&path).map_err(|e| format!("打开灵动岛配置失败: {e}"))?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS island_config (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            payload TEXT NOT NULL,
            updated_at INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS shot_settings (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            payload TEXT NOT NULL,
            updated_at INTEGER NOT NULL
        );",
    )
    .map_err(|e| format!("初始化灵动岛配置失败: {e}"))?;
    Ok(conn)
}

fn read_db() -> Result<Option<IslandConfig>, String> {
    let conn = open()?;
    let mut stmt = conn
        .prepare("SELECT payload FROM island_config WHERE id = 1")
        .map_err(|e| format!("读取灵动岛配置失败: {e}"))?;
    let mut rows = stmt
        .query([])
        .map_err(|e| format!("查询灵动岛配置失败: {e}"))?;
    let Some(row) = rows.next().map_err(|e| format!("读取灵动岛配置失败: {e}"))? else {
        return Ok(None);
    };
    let payload: String = row.get(0).map_err(|e| format!("读取灵动岛配置失败: {e}"))?;
    let cfg = deserialize_config(&payload)?;
    Ok(Some(cfg))
}

/// 解析配置，并迁移上一版内部字段 `fontScale`（倍率）到新的 `fontSize`
///（基准逻辑 px）。上一版已让用户选过 1.25 等档位，不能升级后静默回到 13px。
fn deserialize_config(payload: &str) -> Result<IslandConfig, String> {
    let mut value: serde_json::Value =
        serde_json::from_str(payload).map_err(|e| format!("解析灵动岛配置失败: {e}"))?;
    if value.get("fontSize").is_none() {
        if let Some(scale) = value.get("fontScale").and_then(|v| v.as_f64()) {
            if scale.is_finite() {
                let px = (13.0 * scale).clamp(10.0, 18.0);
                if let Some(obj) = value.as_object_mut() {
                    obj.insert("fontSize".into(), serde_json::json!(px));
                }
            }
        }
    }
    serde_json::from_value(value).map_err(|e| format!("解析灵动岛配置失败: {e}"))
}

fn write_db(config: &IslandConfig) -> Result<(), String> {
    let payload = serde_json::to_string(&config.clone().clamped())
        .map_err(|e| format!("序列化灵动岛配置失败: {e}"))?;
    let conn = open()?;
    conn.execute(
        "INSERT INTO island_config (id, payload, updated_at) VALUES (1, ?1, ?2)
         ON CONFLICT(id) DO UPDATE SET payload = excluded.payload, updated_at = excluded.updated_at",
        params![payload, chrono::Utc::now().timestamp_millis()],
    )
    .map_err(|e| format!("保存灵动岛配置失败: {e}"))?;
    Ok(())
}

/// 旧版 JSON → 内部库的一次性迁移。迁移成功后删除 JSON；任何一步失败都
/// 保留旧文件并回退到旧配置，绝不让用户设置凭空重置。
fn migrate_legacy() -> Option<IslandConfig> {
    let path = legacy_path().ok()?;
    let text = std::fs::read_to_string(&path).ok()?;
    let cfg = deserialize_config(&text).ok()?;
    if write_db(&cfg).is_ok() {
        let _ = std::fs::remove_file(&path);
    }
    Some(cfg)
}

pub fn load() -> IslandConfig {
    if let Ok(guard) = cache().lock() {
        if let Some(cfg) = guard.as_ref() {
            return cfg.clone();
        }
    }
    let loaded = read_db()
        .ok()
        .flatten()
        .or_else(migrate_legacy)
        .unwrap_or_default()
        .clamped();
    if let Ok(mut guard) = cache().lock() {
        *guard = Some(loaded.clone());
    }
    loaded
}

pub fn save(config: &IslandConfig) -> Result<(), String> {
    let next = config.clone().clamped();
    write_db(&next)?;
    if let Ok(mut guard) = cache().lock() {
        *guard = Some(next);
    }
    Ok(())
}

/// 设置页里的“恢复默认值”如果后续接入，可从缓存和库同时清空。
#[allow(dead_code)]
pub fn clear() -> Result<(), String> {
    let conn = open()?;
    conn.execute("DELETE FROM island_config WHERE id = 1", [])
        .map_err(|e| format!("清空灵动岛配置失败: {e}"))?;
    if let Ok(mut guard) = cache().lock() {
        *guard = None;
    }
    Ok(())
}

// ============================================================
// 截图设置（同为内部状态库；不向用户暴露 JSON）
// ============================================================

static SHOT_CACHE: OnceLock<Mutex<Option<ShotSettings>>> = OnceLock::new();

fn shot_cache() -> &'static Mutex<Option<ShotSettings>> {
    SHOT_CACHE.get_or_init(|| Mutex::new(None))
}

pub fn load_shot_settings() -> ShotSettings {
    if let Ok(guard) = shot_cache().lock() {
        if let Some(settings) = guard.as_ref() {
            return settings.clone();
        }
    }
    let loaded = (|| -> Result<ShotSettings, String> {
        let conn = open()?;
        let mut stmt = conn
            .prepare("SELECT payload FROM shot_settings WHERE id = 1")
            .map_err(|e| format!("读取截图设置失败: {e}"))?;
        let mut rows = stmt
            .query([])
            .map_err(|e| format!("查询截图设置失败: {e}"))?;
        let Some(row) = rows.next().map_err(|e| format!("读取截图设置失败: {e}"))? else {
            return Ok(ShotSettings::default());
        };
        let payload: String = row.get(0).map_err(|e| format!("读取截图设置失败: {e}"))?;
        deserialize_shot_settings(&payload)
            .map(|s| s.clamped())
            .map_err(|e| format!("解析截图设置失败: {e}"))
    })()
    .unwrap_or_default();
    if let Ok(mut guard) = shot_cache().lock() {
        *guard = Some(loaded.clone());
    }
    loaded
}

/// 解析截图设置，并把旧版三个分模式快捷键（`hotkeyRegion` 优先）合并成
/// 唯一的 `hotkey`。旧字段读出来就丢掉，下次保存时自然清理。
fn deserialize_shot_settings(payload: &str) -> Result<ShotSettings, serde_json::Error> {
    let mut value: serde_json::Value = serde_json::from_str(payload)?;
    if let Some(obj) = value.as_object_mut() {
        let current = obj
            .get("hotkey")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if current.is_empty() {
            let region = ["hotkeyRegion", "hotkeyWindow", "hotkeyFull"]
                .iter()
                .filter_map(|key| obj.get(*key).and_then(|v| v.as_str()))
                .map(|s| s.trim())
                .find(|s| !s.is_empty())
                .map(|s| s.to_string())
                .unwrap_or_else(|| super::screenshot::DEFAULT_SHOT_HOTKEY.to_string());
            obj.insert("hotkey".into(), serde_json::Value::String(region));
        }
        for key in ["hotkeyRegion", "hotkeyWindow", "hotkeyFull"] {
            obj.remove(key);
        }
    }
    serde_json::from_value(value)
}

pub fn save_shot_settings(settings: &ShotSettings) -> Result<(), String> {
    let next = settings.clone().clamped();
    let payload = serde_json::to_string(&next).map_err(|e| format!("序列化截图设置失败: {e}"))?;
    let conn = open()?;
    conn.execute(
        "INSERT INTO shot_settings (id, payload, updated_at) VALUES (1, ?1, ?2)
         ON CONFLICT(id) DO UPDATE SET payload = excluded.payload, updated_at = excluded.updated_at",
        params![payload, chrono::Utc::now().timestamp_millis()],
    )
    .map_err(|e| format!("保存截图设置失败: {e}"))?;
    if let Ok(mut guard) = shot_cache().lock() {
        *guard = Some(next);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrates_legacy_font_scale_to_px() {
        let cfg = deserialize_config(r#"{"enabled":true,"fontScale":1.25}"#).unwrap();
        assert!((cfg.font_size - 16.25).abs() < 1e-9);
    }

    #[test]
    fn font_size_wins_over_legacy_font_scale() {
        let cfg =
            deserialize_config(r#"{"enabled":true,"fontSize":14.5,"fontScale":1.25}"#).unwrap();
        assert!((cfg.font_size - 14.5).abs() < 1e-9);
    }

    #[test]
    fn merges_legacy_shot_hotkeys_into_one() {
        let s = deserialize_shot_settings(
            r#"{"autoCopy":true,"hotkeyRegion":"Ctrl+Alt+S","hotkeyWindow":"Ctrl+Alt+Shift+W","hotkeyFull":"Ctrl+Alt+Shift+F"}"#,
        )
        .unwrap();
        assert_eq!(s.hotkey, "Ctrl+Alt+S");
    }

    #[test]
    fn single_shot_hotkey_wins_over_legacy_fields() {
        let s =
            deserialize_shot_settings(r#"{"hotkey":"Ctrl+Alt+Q","hotkeyRegion":"Ctrl+Alt+S"}"#)
                .unwrap();
        assert_eq!(s.hotkey, "Ctrl+Alt+Q");
    }

    #[test]
    fn shot_hotkey_falls_back_to_default() {
        let s = deserialize_shot_settings(r#"{"autoCopy":false}"#).unwrap();
        assert_eq!(s.hotkey, crate::island::screenshot::DEFAULT_SHOT_HOTKEY);
    }
}
