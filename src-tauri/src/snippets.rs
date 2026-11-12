//! 提示词片段库：常用 prompt / 提交信息模板 / 审查清单，全部存在本机。
//!
//! - 文件：`~/.agenthud/snippets.json`（全量原子写，复用 credentials 的
//!   `atomic_write`；目录不存在时由同一函数报错，不静默丢数据）
//! - 顺序：`updated_at` 倒序（最近改过的在前），前端直接用这个顺序
//! - 限额：条目 ≤ 500；名称 ≤ 80 字、正文 ≤ 20000 字（超限直接报错，
//!   不静默截断——片段被悄悄截掉一半比报错更难排查）
//! - 名称留空时取正文第一行前 40 字，避免列表里一片「未命名」
//!
//! 不联网、不加密：片段就是用户自己写的文本，和 `pricing.json` 同级。

use crate::provider_credentials::atomic_write;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// 条目上限（超出后淘汰最久未更新的）
const MAX_ITEMS: usize = 500;
const MAX_NAME_CHARS: usize = 80;
const MAX_BODY_CHARS: usize = 20_000;
/// 名称留空时从正文摘取的字符数
const DERIVED_NAME_CHARS: usize = 40;
/// 文件版本（后续改结构时用它做迁移判断）
const STORE_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Snippet {
    pub id: String,
    pub name: String,
    pub body: String,
    /// 毫秒时间戳
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Store {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    items: Vec<Snippet>,
}

impl Store {
    fn empty() -> Self {
        Store {
            version: STORE_VERSION,
            items: Vec::new(),
        }
    }
}

pub fn config_path() -> Result<PathBuf, String> {
    Ok(crate::pricing::config_dir()?.join("snippets.json"))
}

/// 读文件：不存在 / 损坏都按空库处理（损坏时打印一行，方便定位）。
fn load_from(path: &Path) -> Store {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return Store::empty();
    };
    match serde_json::from_str::<Store>(&raw) {
        Ok(mut store) => {
            store.version = STORE_VERSION;
            store
        }
        Err(error) => {
            eprintln!("[zbar-snippets] 片段文件无法解析，按空库处理: {error}");
            Store::empty()
        }
    }
}

fn save_to(path: &Path, store: &Store) -> Result<(), String> {
    let json =
        serde_json::to_string_pretty(store).map_err(|e| format!("序列化片段失败: {e}"))?;
    atomic_write(path, &json)
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// 名称留空的兜底：正文第一行前 40 字
fn derive_name(body: &str) -> String {
    body.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("")
        .chars()
        .take(DERIVED_NAME_CHARS)
        .collect()
}

fn sort_items(items: &mut [Snippet]) {
    items.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
}

/// 新增或更新：`id` 命中就更新（保留 created_at、刷新 updated_at），
/// 否则新建。名称/正文都会 trim，正文为空直接报错。
fn upsert(
    store: &mut Store,
    id: Option<&str>,
    name: &str,
    body: &str,
    now: i64,
) -> Result<Snippet, String> {
    let body = body.trim().to_string();
    if body.is_empty() {
        return Err("片段内容不能为空".into());
    }
    if body.chars().count() > MAX_BODY_CHARS {
        return Err(format!("片段内容太长（上限 {MAX_BODY_CHARS} 字符）"));
    }
    let mut name = name.trim().to_string();
    if name.chars().count() > MAX_NAME_CHARS {
        return Err(format!("片段名称太长（上限 {MAX_NAME_CHARS} 字）"));
    }
    if name.is_empty() {
        name = derive_name(&body);
    }
    if name.is_empty() {
        name = "未命名片段".into();
    }

    if let Some(id) = id {
        if let Some(item) = store.items.iter_mut().find(|item| item.id == id) {
            item.name = name;
            item.body = body;
            item.updated_at = now;
            let updated = item.clone();
            sort_items(&mut store.items);
            return Ok(updated);
        }
    }

    let snippet = Snippet {
        id: uuid::Uuid::new_v4().to_string(),
        name,
        body,
        created_at: now,
        updated_at: now,
    };
    store.items.push(snippet.clone());
    sort_items(&mut store.items);
    // 超上限淘汰最久未更新的（倒序后末尾最旧）
    store.items.truncate(MAX_ITEMS);
    Ok(snippet)
}

fn remove(store: &mut Store, id: &str) -> Result<(), String> {
    let before = store.items.len();
    store.items.retain(|item| item.id != id);
    if store.items.len() == before {
        return Err("片段不存在".into());
    }
    Ok(())
}

// ===== 命令 =====

/// 全部片段（updated_at 倒序）
#[tauri::command]
pub async fn list_snippets() -> Result<Vec<Snippet>, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let path = config_path()?;
        let mut store = load_from(&path);
        sort_items(&mut store.items);
        Ok(store.items)
    })
    .await
    .map_err(|e| format!("读取片段任务失败: {e}"))?
}

/// 保存片段：`id` 为 None 时新建，命中已有 id 时更新
#[tauri::command]
pub async fn save_snippet(
    id: Option<String>,
    name: String,
    body: String,
) -> Result<Snippet, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let path = config_path()?;
        let mut store = load_from(&path);
        let snippet = upsert(&mut store, id.as_deref(), &name, &body, now_ms())?;
        save_to(&path, &store)?;
        Ok(snippet)
    })
    .await
    .map_err(|e| format!("保存片段任务失败: {e}"))?
}

/// 删除片段（id 不存在按失败返回，前端提示即可）
#[tauri::command]
pub async fn delete_snippet(id: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let path = config_path()?;
        let mut store = load_from(&path);
        remove(&mut store, &id)?;
        save_to(&path, &store)
    })
    .await
    .map_err(|e| format!("删除片段任务失败: {e}"))?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("agenthud-snippets-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("建临时目录");
        dir.join(name)
    }

    #[test]
    fn upsert_derives_name_from_first_line() {
        let mut store = Store::empty();
        let snippet = upsert(&mut store, None, "", "  审一下这段代码\n第二行", 1_000).unwrap();
        assert_eq!(snippet.name, "审一下这段代码");
        assert_eq!(snippet.body, "审一下这段代码\n第二行");
        assert_eq!(store.items.len(), 1);
    }

    #[test]
    fn upsert_rejects_empty_and_oversized_body() {
        let mut store = Store::empty();
        assert!(upsert(&mut store, None, "x", "   ", 1).is_err());
        let oversized = "a".repeat(MAX_BODY_CHARS + 1);
        assert!(upsert(&mut store, None, "x", &oversized, 1).is_err());
        // 名称过长同样报错，不静默截断
        let long_name = "名".repeat(MAX_NAME_CHARS + 1);
        assert!(upsert(&mut store, None, &long_name, "body", 1).is_err());
    }

    #[test]
    fn upsert_updates_existing_and_keeps_created_at() {
        let mut store = Store::empty();
        let created = upsert(&mut store, None, "旧名", "旧正文", 1_000).unwrap();
        let updated = upsert(&mut store, Some(&created.id), "新名", "新正文", 2_000).unwrap();
        assert_eq!(updated.id, created.id);
        assert_eq!(updated.name, "新名");
        assert_eq!(updated.created_at, 1_000);
        assert_eq!(updated.updated_at, 2_000);
        assert_eq!(store.items.len(), 1);
    }

    #[test]
    fn list_is_sorted_by_updated_at_desc() {
        let mut store = Store::empty();
        let first = upsert(&mut store, None, "a", "正文 a", 1_000).unwrap();
        upsert(&mut store, None, "b", "正文 b", 3_000).unwrap();
        upsert(&mut store, Some(&first.id), "a2", "正文 a2", 5_000).unwrap();
        sort_items(&mut store.items);
        let names: Vec<_> = store.items.iter().map(|item| item.name.as_str()).collect();
        assert_eq!(names, vec!["a2", "b"]);
    }

    #[test]
    fn remove_reports_missing_id() {
        let mut store = Store::empty();
        let snippet = upsert(&mut store, None, "x", "正文", 1).unwrap();
        remove(&mut store, &snippet.id).unwrap();
        assert!(store.items.is_empty());
        assert!(remove(&mut store, "nope").is_err());
    }

    #[test]
    fn store_round_trips_through_disk() {
        let path = temp_path("snippets.json");
        let mut store = Store::empty();
        upsert(&mut store, None, "提交模板", "fix: 修复导出路径", 1_000).unwrap();
        save_to(&path, &store).expect("写盘");
        let loaded = load_from(&path);
        assert_eq!(loaded.items, store.items);
        assert_eq!(loaded.version, STORE_VERSION);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn corrupt_file_falls_back_to_empty_store() {
        let path = temp_path("snippets.json");
        std::fs::write(&path, "{ not json").expect("写坏文件");
        assert!(load_from(&path).items.is_empty());
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
