//! 实时字幕历史（后端唯一权威副本，≤80 条）。
//!
//! 前端（灵动岛收起态字幕层 + 展开面板翻译页）只负责显示；历史顺序、
//! id 分配、草稿 → 定稿的替换、译文增量都以后端为准：
//!
//! - **草稿占位「下一句 id」**：一句话还没定稿时，草稿用 `next_id` 更新同一条
//!   字幕；定稿时复用同一个 id 改成 `draft = false, final = true`，然后
//!   `next_id += 1`。这样前端按 id upsert 即可，不需要处理「草稿删除」。
//! - **译文增量**：`zh` 每次推送的是**累计文本**（不是 delta），同一 id 反复
//!   覆盖；定稿译文只允许写 `final = true` 的条目，草稿译文只允许写
//!   `final = false` 的条目，避免两者互相覆盖（定稿到达时草稿流式翻译会被
//!   中止，见 `pipeline`）。
//! - **上限 80 条**：`LIVE_CAPTION_LIMIT`（前端 types.ts）同口径，超出后从
//!   最旧的一条开始丢。
//!
//! 事件：每次变更推一条 [`super::LIVE_CAPTION_EVENT`]，payload 是完整
//! [`LiveCaption`]（前端按 id upsert）。

use std::sync::{Mutex, OnceLock};

use tauri::Emitter;

use super::{LiveCaption, LIVE_CAPTION_EVENT};

/// 与前端 `LIVE_CAPTION_LIMIT` 一致：最多保留 80 条。
const CAPTION_LIMIT: usize = 80;

struct CaptionStore {
    captions: Vec<LiveCaption>,
    /// 下一条定稿字幕的 id（草稿先占用它）。
    next_id: u64,
}

fn store() -> &'static Mutex<CaptionStore> {
    static STORE: OnceLock<Mutex<CaptionStore>> = OnceLock::new();
    STORE.get_or_init(|| {
        Mutex::new(CaptionStore {
            captions: Vec::new(),
            next_id: 1,
        })
    })
}

fn lock_store() -> std::sync::MutexGuard<'static, CaptionStore> {
    store()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 丢弃超出上限的最旧字幕。
fn trim(captions: &mut Vec<LiveCaption>) {
    if captions.len() > CAPTION_LIMIT {
        let overflow = captions.len() - CAPTION_LIMIT;
        captions.drain(..overflow);
    }
}

/// 推送单条字幕（`zh` 为累计文本，前端整条 upsert）。
fn emit(caption: &LiveCaption) {
    if let Some(app) = super::app_handle() {
        let _ = app.emit(LIVE_CAPTION_EVENT, caption);
    }
}

/// 更新（或新建）当前句的草稿。
///
/// 返回被推送的字幕；`jp` 为空时不创建空条目（返回 `None`）。
pub(crate) fn update_draft(jp: &str) -> Option<LiveCaption> {
    let jp = jp.trim();
    if jp.is_empty() {
        return None;
    }
    let caption = {
        let mut guard = lock_store();
        let id = guard.next_id;
        let ts = super::now_millis();
        let caption = match guard.captions.last_mut() {
            Some(last) if last.id == id && !last.final_ => {
                last.jp = jp.to_string();
                last.draft = true;
                last.ts = ts;
                last.clone()
            }
            _ => {
                let caption = LiveCaption {
                    id,
                    jp: jp.to_string(),
                    zh: String::new(),
                    draft: true,
                    final_: false,
                    ts,
                };
                guard.captions.push(caption.clone());
                trim(&mut guard.captions);
                caption
            }
        };
        caption
    };
    emit(&caption);
    Some(caption)
}

/// 定稿当前句：复用草稿的 id，`draft = false, final = true`，然后 id 自增。
pub(crate) fn finalize(jp: &str) -> LiveCaption {
    let jp = jp.trim().to_string();
    let caption = {
        let mut guard = lock_store();
        let id = guard.next_id;
        let ts = super::now_millis();
        let caption = match guard.captions.iter_mut().rev().find(|item| item.id == id) {
            Some(existing) => {
                existing.jp = jp;
                existing.draft = false;
                existing.final_ = true;
                existing.ts = ts;
                existing.clone()
            }
            None => {
                let caption = LiveCaption {
                    id,
                    jp,
                    zh: String::new(),
                    draft: false,
                    final_: true,
                    ts,
                };
                guard.captions.push(caption.clone());
                trim(&mut guard.captions);
                caption
            }
        };
        guard.next_id = guard.next_id.saturating_add(1);
        caption
    };
    emit(&caption);
    caption
}

/// 定稿结果为空（没听清）时，把上一句草稿冻结为定稿，避免历史里留下
/// 永远处于 `draft = true` 的幽灵条目；没有草稿时什么也不做。
pub(crate) fn freeze_draft_as_final() -> Option<LiveCaption> {
    let caption = {
        let mut guard = lock_store();
        let id = guard.next_id;
        let ts = super::now_millis();
        let caption = guard
            .captions
            .iter_mut()
            .rev()
            .find(|item| item.id == id && !item.final_)
            .map(|existing| {
                existing.draft = false;
                existing.final_ = true;
                existing.ts = ts;
                existing.clone()
            });
        if caption.is_some() {
            guard.next_id = guard.next_id.saturating_add(1);
        }
        caption
    };
    if let Some(caption) = &caption {
        emit(caption);
    }
    caption
}

/// 写草稿译文（只写还没定稿的同 id 条目）。
///
/// 返回 `false` 表示该句已经不是草稿（定稿 / 被清空），流式翻译应立即中止。
pub(crate) fn set_draft_zh(id: u64, text: &str) -> bool {
    let caption = {
        let mut guard = lock_store();
        let Some(existing) = guard
            .captions
            .iter_mut()
            .rev()
            .find(|item| item.id == id && !item.final_)
        else {
            return false;
        };
        if existing.zh == text {
            return true;
        }
        existing.zh = text.to_string();
        existing.clone()
    };
    emit(&caption);
    true
}

/// 写定稿译文（只写已经定稿的同 id 条目；清空历史后返回 false 中止流）。
pub(crate) fn set_final_zh(id: u64, text: &str) -> bool {
    let caption = {
        let mut guard = lock_store();
        let Some(existing) = guard
            .captions
            .iter_mut()
            .rev()
            .find(|item| item.id == id && item.final_)
        else {
            return false;
        };
        if existing.zh == text {
            return true;
        }
        existing.zh = text.to_string();
        existing.clone()
    };
    emit(&caption);
    true
}

/// 当前活跃草稿（`id`, `jp`）：草稿识别 / 草稿翻译线程读取。
pub(crate) fn active_draft() -> Option<(u64, String)> {
    let guard = lock_store();
    guard
        .captions
        .last()
        .filter(|item| item.draft && !item.final_ && !item.jp.trim().is_empty())
        .map(|item| (item.id, item.jp.clone()))
}

/// 当前句之前最近 `limit` 句**定稿**日文（旧 → 新），给翻译当上下文。
///
/// `id` 是当前句 id（草稿也是这个 id），必须排除；纯符号 / 空文本跳过。
pub(crate) fn context_before(id: u64, limit: usize) -> Vec<String> {
    if limit == 0 {
        return Vec::new();
    }
    let guard = lock_store();
    let mut context: Vec<String> = guard
        .captions
        .iter()
        .rev()
        .filter(|item| item.final_ && item.id != id)
        .map(|item| item.jp.trim().to_string())
        .filter(|jp| !jp.is_empty())
        .take(limit)
        .collect();
    context.reverse();
    context
}

/// 历史快照（旧 → 新）。
pub(crate) fn snapshot() -> Vec<LiveCaption> {
    lock_store().captions.clone()
}

/// 清空历史（id 继续自增，避免前端把新字幕合并到旧 id 上）。
pub(crate) fn clear() {
    lock_store().captions.clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 字幕存储是进程级全局状态：这些测试共用一把锁，避免并行互踩。
    fn test_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// 每个测试开始前清空并重置 id，结束也清空（不污染其它测试）。
    fn reset() {
        let mut guard = lock_store();
        guard.captions.clear();
        guard.next_id = 1;
    }

    #[test]
    fn draft_reuses_next_id_and_finalize_consumes_it() {
        let _guard = test_lock();
        reset();

        let draft = update_draft("こんにちは").expect("草稿应创建");
        assert_eq!(draft.id, 1);
        assert!(draft.draft && !draft.final_);

        let updated = update_draft("こんにちは、世界").expect("草稿应更新");
        assert_eq!(updated.id, 1, "同一句草稿必须复用 id");
        assert_eq!(snapshot().len(), 1, "草稿只占一条");

        let fin = finalize("こんにちは、世界。");
        assert_eq!(fin.id, 1, "定稿复用草稿 id");
        assert!(!fin.draft && fin.final_);
        assert_eq!(snapshot().len(), 1);

        let next = update_draft("次の文");
        assert_eq!(next.expect("下一句草稿").id, 2, "定稿后 id 自增");

        reset();
    }

    #[test]
    fn finalize_without_draft_creates_final_caption() {
        let _guard = test_lock();
        reset();
        let caption = finalize("直接定稿");
        assert!(caption.final_ && !caption.draft);
        assert_eq!(snapshot().len(), 1);
        reset();
    }

    #[test]
    fn freeze_keeps_draft_text_and_bumps_id() {
        let _guard = test_lock();
        reset();
        update_draft("途中まで");
        let frozen = freeze_draft_as_final().expect("应冻结草稿");
        assert!(frozen.final_);
        assert_eq!(frozen.jp, "途中まで", "冻结保留草稿文本");
        // 没有草稿时是空操作
        assert!(freeze_draft_as_final().is_none());
        reset();
    }

    #[test]
    fn draft_and_final_translations_do_not_cross_write() {
        let _guard = test_lock();
        reset();
        update_draft("テスト");
        assert!(set_draft_zh(1, "测试"));
        assert!(!set_final_zh(1, "覆盖"), "定稿译文不能写草稿");
        assert!(!set_final_zh(999, "不存在"), "找不到条目返回 false");
        let fin = finalize("テスト");
        assert!(!set_draft_zh(fin.id, "草稿覆盖"), "草稿译文不能写定稿");
        assert!(set_final_zh(fin.id, "测试定稿"));
        let stored = snapshot();
        assert_eq!(stored[0].zh, "测试定稿");
        reset();
    }

    #[test]
    fn history_is_capped_and_oldest_dropped() {
        let _guard = test_lock();
        reset();
        for index in 0..(CAPTION_LIMIT + 5) {
            finalize(&format!("句子{index}"));
        }
        let stored = snapshot();
        assert_eq!(stored.len(), CAPTION_LIMIT);
        assert_eq!(stored.last().expect("最后一条").jp, "句子84");
        assert_eq!(stored.first().expect("第一条").jp, "句子5");
        reset();
    }

    #[test]
    fn context_before_skips_current_and_non_final_lines() {
        let _guard = test_lock();
        reset();
        finalize("一句目");
        finalize("二句目");
        update_draft("三句目の草稿");
        // 当前句 id = 3，上文只取已经定稿的最近两条
        assert_eq!(context_before(3, 3), vec!["一句目", "二句目"]);
        assert_eq!(context_before(3, 1), vec!["二句目"]);
        assert!(context_before(3, 0).is_empty());
        finalize("三句目");
        // 定稿后排除自己
        assert_eq!(context_before(3, 3), vec!["一句目", "二句目"]);
        reset();
    }

    #[test]
    fn clear_keeps_id_counter_moving() {
        let _guard = test_lock();
        reset();
        update_draft("一句目");
        finalize("一句目");
        clear();
        assert!(snapshot().is_empty());
        let next = update_draft("二句目");
        assert!(next.expect("新草稿").id >= 2, "清空后 id 仍要前进");
        reset();
    }
}
