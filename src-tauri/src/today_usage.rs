//! 今日各 Agent 的 token 用量快照 + 来源命名归一表。
//!
//! **为什么有这个模块**：HUD 停靠条要按「既没有订阅额度、今日也没有用量
//! 就隐藏；一旦有订阅或有用量就动态加回来」决定条目集合，而「今日用量」
//! 这笔数据托盘标题线程（`spawn_title_updater` → `today_tray_title`）本来
//! 就每 30s 算一遍：它需要把本机各来源今日用量 + 多设备同步合并成菜单栏
//! 的合计。停靠条直接读同一份结果，就不必在 30s 拍的数据线程里再跑一轮
//! 8 个本地库聚合（部分来源还会触发会话日志增量导入）＋一次同步服务请求
//! ——那会让停靠条数据明显变钝，还会和托盘线程抢 SQLite 写锁。
//!
//! **口径**：本地自然日 0 点起；含同步服务上其他设备（托盘标题的远端
//! 请求已带 `exclude_device=本机`，本机直读 + 远端两个集合不相交），与
//! 详情卡「全部设备」口径一致。
//!
//! **快照失效的兜底**：托盘线程还没跑完（冷启动 Cursor events 分页可能
//! 要几秒）或已停摆时，`fresh()` 为 false —— 消费方（hud_bar）把「不知道」
//! 当作「保守放行」（退回旧行为：设置里开启就显示占位条目），绝不能把
//! 「不知道」误判成「没用量」而把用户的图标藏掉。

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

/// 快照多久没更新就视为不可信（生产线程 30s 一写，留 10 倍余量）
const STALE_MS: i64 = 5 * 60 * 1000;

/// 冷启动种子文件名（落在 ~/.agenthud）：托盘线程第一次跑完前（Cursor
/// events 冷缓存/远端请求可能要几秒到十几秒）先按上次退出前的同一天结果
/// 渲染，避免停靠条先冒出满屏图标、几秒后又缩回去的闪动。
const SEED_FILE: &str = "today-usage.json";

/// 一张快照：统计日 + 各来源今日 token（本地 + 其他设备）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TodayUsage {
    /// 统计区间起点（本地自然日 0 点，毫秒）
    pub(crate) day_start: i64,
    /// 写入时刻（毫秒）
    pub(crate) at: i64,
    /// HUD source → 今日 token（缺键视为 0）
    pub(crate) tokens: BTreeMap<String, i64>,
}

impl TodayUsage {
    /// 快照是否还能代表「今日」：同一自然日且未过期。
    ///
    /// 跨日（0 点后托盘线程还没刷新）与过期（线程停摆）都返回 false，
    /// 消费方据此退回「保守放行」。
    pub(crate) fn fresh(&self) -> bool {
        let now = now_ms();
        self.day_start == local_day_start(now) && now - self.at < STALE_MS
    }

    /// 该来源今日是否用过量（仅在 `fresh()` 为 true 时有意义）。
    pub(crate) fn used(&self, source: &str) -> bool {
        self.tokens.get(source).copied().unwrap_or(0) > 0
    }
}

fn slot() -> &'static Mutex<Option<TodayUsage>> {
    static SLOT: OnceLock<Mutex<Option<TodayUsage>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

/// 是否已经尝试过读种子文件（只在快照为空时试一次，避免每拍 stat 磁盘）
fn seed_tried() -> &'static AtomicBool {
    static TRIED: AtomicBool = AtomicBool::new(false);
    &TRIED
}

/// 落盘形态：只存"哪天 + 各来源今日 token"。不存写入时刻——文件里的
/// 结果是上一个进程算的，可能已经过了一段时间；只要还是同一个自然日，
/// 「今日是否用过量」的结论就仍然成立（只会少算，不会多算，且托盘线程
/// 第一拍跑完就会覆盖）。
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SeedFile {
    day_start: i64,
    #[serde(default)]
    tokens: BTreeMap<String, i64>,
}

fn seed_path() -> Option<PathBuf> {
    crate::pricing::config_dir()
        .ok()
        .map(|dir| dir.join(SEED_FILE))
}

/// 读种子文件：只接受同一个自然日的文件（跨日直接丢弃）。
fn read_seed(path: &std::path::Path) -> Option<TodayUsage> {
    let text = std::fs::read_to_string(path).ok()?;
    let seed: SeedFile = serde_json::from_str(&text).ok()?;
    let now = now_ms();
    if seed.day_start != local_day_start(now) {
        return None;
    }
    Some(TodayUsage {
        day_start: seed.day_start,
        // 种子的"写入时刻"按读到的此刻算：它只用于快照新鲜度（5 分钟窗口），
        // 而种子本身来自上一进程，真实时刻没有意义——用旧时刻会让这条
        // 快照在启动瞬间就被判过期，冷启动闪动照样出现。
        at: now,
        tokens: seed.tokens,
    })
}

/// 落盘（原子写，与其它配置文件同口径：先写临时文件再改名）。写失败
/// 静默降级——只影响下一次冷启动的首帧，不影响运行期判定。
fn write_seed(
    path: &std::path::Path,
    day_start: i64,
    tokens: &BTreeMap<String, i64>,
) -> std::io::Result<()> {
    let seed = SeedFile {
        day_start,
        tokens: tokens.clone(),
    };
    let json = serde_json::to_string_pretty(&seed).map_err(std::io::Error::other)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, path)
}

fn load_seed() -> Option<TodayUsage> {
    read_seed(&seed_path()?)
}

fn persist(day_start: i64, tokens: &BTreeMap<String, i64>) {
    if let Some(path) = seed_path() {
        let _ = write_seed(&path, day_start, tokens);
    }
}

/// 当前时刻（毫秒）。抽成函数便于单测替换语义（单测直接构造结构体，
/// 不依赖真实时钟）。
pub(crate) fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// 本地自然日 0 点（毫秒）。夏令时跳过 0 点的极端时区回退到 `now_ms`
/// 本身（此时"今日"退化成"最近 24h"，不会 panic）。
pub(crate) fn local_day_start(now_ms: i64) -> i64 {
    use chrono::TimeZone as _;
    chrono::Local
        .timestamp_millis_opt(now_ms)
        .single()
        .and_then(|dt| dt.date_naive().and_hms_opt(0, 0, 0))
        .and_then(|naive| chrono::Local.from_local_datetime(&naive).single())
        .map(|dt| dt.timestamp_millis())
        .unwrap_or(now_ms)
}

/// 生产端：托盘标题线程每次算完覆盖写一份（`now` 由本函数取，保证
/// 新鲜度判定与写入时刻一致）。
pub(crate) fn store(day_start: i64, tokens: BTreeMap<String, i64>) {
    // 内容没变就不落盘：空闲时每 30s 一拍不该产生磁盘写入
    let unchanged = slot()
        .lock()
        .map(|guard| {
            guard
                .as_ref()
                .is_some_and(|prev| prev.day_start == day_start && prev.tokens == tokens)
        })
        .unwrap_or(false);
    let mut guard = match slot().lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    guard.replace(TodayUsage {
        day_start,
        at: now_ms(),
        tokens: tokens.clone(),
    });
    drop(guard);
    // 单测不落盘：否则会写真实 ~/.agenthud 的用户数据（种子的读写逻辑
    // 由下面的临时文件用例单独覆盖）
    if !cfg!(test) && !unchanged {
        persist(day_start, &tokens);
    }
}

/// 消费端：取一份快照（从未写入时 None）。拷贝量只有几个条目，开销可忽略。
pub(crate) fn snapshot() -> Option<TodayUsage> {
    {
        let guard = match slot().lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Some(snapshot) = guard.as_ref() {
            return Some(snapshot.clone());
        }
    }
    // 快照还没被生产过（进程刚起）：试一次冷启动种子（同日才认）。
    if !seed_tried().swap(true, Ordering::Relaxed) {
        if let Some(seeded) = load_seed() {
            let mut guard = match slot().lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            if guard.is_none() {
                *guard = Some(seeded.clone());
            }
            return guard.clone();
        }
    }
    None
}

// ============================================================
// 来源命名归一
// ============================================================

/// 同一个 Agent 在三个命名域里的名字：
/// - HUD source：停靠条条目 / 前端 `agent.source` / 设置页可见性键
///   （zai、opencodego、minimax …）；
/// - 同步服务 source：`/usage`、`/usage_records` 的 `source` 参数与服务端
///   `by_model[].source`（zcode、opencode、minimaxcode …；Cursor 不进
///   明细同步，故为 None）；
/// - 本地/后端 source：各模块 `query_*` 与 projects 的来源字符串
///   （zai、opencode、minimaxcode …）。
///
/// 三张域名两两不同名（zai↔zcode、opencodego↔opencode、
/// minimax↔minimaxcode），历史上每加一处跨域映射都是一次手写字面量，
/// 写错就静默查不到数据；这里收成一张表，两个方向都从它派生。
const SOURCE_NAMES: &[(&str, Option<&str>, &str)] = &[
    ("zai", Some("zcode"), "zai"),
    ("codex", Some("codex"), "codex"),
    ("claude", Some("claude"), "claude"),
    ("cursor", None, "cursor"),
    ("kimi", Some("kimi"), "kimi"),
    ("dsh", Some("dsh"), "dsh"),
    ("opencodego", Some("opencode"), "opencode"),
    ("minimax", Some("minimaxcode"), "minimaxcode"),
];

fn row_of(name: &str) -> Option<&'static (&'static str, Option<&'static str>, &'static str)> {
    SOURCE_NAMES.iter().find(|(hud, sync, local)| {
        *hud == name || *local == name || sync.is_some_and(|value| value == name)
    })
}

/// 任一命名域的名字 → HUD source（未知名称返回 None，调用方忽略即可）。
pub(crate) fn hud_source(name: &str) -> Option<&'static str> {
    row_of(name).map(|(hud, _, _)| *hud)
}

/// 任一命名域的名字 → 同步服务 source。该来源不进明细同步时返回 None
/// （Cursor 的用量在 Cursor 自己的 events 接口里）。
pub(crate) fn sync_source(name: &str) -> Option<&'static str> {
    row_of(name).and_then(|(_, sync, _)| *sync)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 三域名字两两互查：HUD id / 服务端 source / 本地 source 都能落到同一行。
    #[test]
    fn source_names_resolve_across_domains() {
        assert_eq!(hud_source("zai"), Some("zai"));
        assert_eq!(hud_source("zcode"), Some("zai"));
        assert_eq!(hud_source("opencode"), Some("opencodego"));
        assert_eq!(hud_source("opencodego"), Some("opencodego"));
        assert_eq!(hud_source("minimaxcode"), Some("minimax"));
        assert_eq!(hud_source("cursor"), Some("cursor"));
        assert_eq!(hud_source("unknown"), None);

        assert_eq!(sync_source("zai"), Some("zcode"));
        assert_eq!(sync_source("zcode"), Some("zcode"));
        assert_eq!(sync_source("opencodego"), Some("opencode"));
        assert_eq!(sync_source("minimax"), Some("minimaxcode"));
        // Cursor 不走明细同步：本地有数据也不该让 HUD 去同步服务查
        assert_eq!(sync_source("cursor"), None);
        assert_eq!(sync_source("unknown"), None);
    }

    /// 今日 0 点必须落在当天：区间 [day_start, day_start + 1s) 的日期等于 now
    /// 的日期（不用固定时间戳，避免依赖测试机器的时区/夏令时）。
    #[test]
    fn local_day_start_is_midnight_of_same_day() {
        use chrono::{Datelike as _, TimeZone as _};
        let now = now_ms();
        let start = local_day_start(now);
        let now_date = chrono::Local.timestamp_millis_opt(now).single().unwrap();
        let start_date = chrono::Local
            .timestamp_millis_opt(start)
            .single()
            .unwrap();
        assert_eq!(
            (now_date.year(), now_date.month(), now_date.day()),
            (start_date.year(), start_date.month(), start_date.day())
        );
        assert!(start <= now && now - start < 86_400_000);
    }

    /// 快照新鲜度：同一自然日 + 未过期才算 fresh；跨日/过期都不算。
    #[test]
    fn snapshot_freshness_covers_day_and_staleness() {
        let now = now_ms();
        let today = TodayUsage {
            day_start: local_day_start(now),
            at: now,
            tokens: BTreeMap::from([("zai".to_string(), 42)]),
        };
        assert!(today.fresh());
        assert!(today.used("zai"));
        assert!(!today.used("codex"));

        let yesterday = TodayUsage {
            day_start: local_day_start(now - 86_400_000),
            ..today.clone()
        };
        assert!(!yesterday.fresh());

        let stale = TodayUsage {
            at: now - STALE_MS - 1,
            ..today
        };
        assert!(!stale.fresh());
    }

    /// 种子的读写：同日文件能读回，跨日文件被丢弃（否则昨天"用过"的
    /// agent 今天会凭空多出来）。
    #[test]
    fn seed_file_round_trip_and_rejects_other_day() {
        let path = std::env::temp_dir().join(format!(
            "zbar-today-usage-seed-{}.json",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        let now = now_ms();
        let today = local_day_start(now);
        let tokens = BTreeMap::from([("dsh".to_string(), 99_i64)]);
        write_seed(&path, today, &tokens).expect("种子应写入成功");
        let loaded = read_seed(&path).expect("同日种子应可读");
        assert_eq!(loaded.tokens.get("dsh"), Some(&99));
        assert!(loaded.fresh());

        // 跨日：即使文件存在也必须丢弃
        write_seed(&path, today - 86_400_000, &tokens).expect("种子应写入成功");
        assert!(read_seed(&path).is_none());
        let _ = std::fs::remove_file(&path);
    }

    /// store/snapshot 往返：写入即读到，覆盖写生效。
    #[test]
    fn store_and_snapshot_round_trip() {
        let now = now_ms();
        let day_start = local_day_start(now);
        store(
            day_start,
            BTreeMap::from([("codex".to_string(), 7), ("kimi".to_string(), 3)]),
        );
        let got = snapshot().expect("刚写入的快照必须读得到");
        assert_eq!(got.day_start, day_start);
        assert_eq!(got.tokens.get("codex"), Some(&7));
        assert!(got.used("kimi"));
        assert!(got.fresh());

        store(day_start, BTreeMap::new());
        let got = snapshot().unwrap();
        assert!(got.tokens.is_empty());
        assert!(!got.used("codex"));
    }
}
