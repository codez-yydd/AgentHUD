//! 额度快照历史：append-only JSONL 存储 + 周期解析查询。
//!
//! 设计要点：
//! - 快照来源：quota.rs::fetch_quota（当前账号，30s 轮询）与
//!   accounts.rs::account_quotas（全部账号快照，5 分钟一轮）成功后各追加一条，
//!   均带账号指纹（account 字段）——读路径按指纹过滤，互不污染。
//! - 存储：~/.zbar/quota_history.jsonl（每行一条 JSON），轻量、可 append、易调试。
//! - 周期划分：用 weekly_reset (nextResetTime) 的变化点切分"智谱重置周期"。
//! - 去重/限频：同账号同秒内只写一条（尾部窗口回读该账号最后一条防抖）。
//! - 滚动保留：定期（每 24h 至多一次）删除超过保留期的行，防止文件无限增长。
//! - 进程内缓存：load_all 以 (路径, mtime, len) 判断命中，未变时直接返回内存
//!   副本，免去 90 天约 38MB JSONL 的反复逐行解析；本模块写入/清理成功后
//!   同步联动缓存，磁盘文件被外部改动时自动失效重读（正确性优先）。

use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime};

use crate::pricing::config_dir;

/// 快照保留期：超过 90 天的行在滚动清理时删除（约 430KB/天，不清理会无限增长）
const RETENTION_MS: i64 = 90 * 86_400_000;
/// 清理检查最小间隔：避免每次写入都触发全量重写（读写整个文件）
const CLEANUP_INTERVAL: Duration = Duration::from_secs(24 * 3600);
/// 上次清理时间（None = 本进程尚未清理过）。
/// 模块级 Mutex 节流：快照写入来自多个线程（fetch_quota / 同步 worker）。
static LAST_CLEANUP: OnceLock<Mutex<Option<SystemTime>>> = OnceLock::new();

// ===== 进程内全量快照缓存 =====
// load_all 被对比页（60s）、今日增量（60s）、同步 worker、托盘刷新等高频调用，
// 90 天历史约 38MB JSONL 逐行解析代价高。以 (路径, mtime, len) 三元组判断
// 有效性——任何一方变化（本模块写入、滚动清理重写、外部改动）都自动失效重读，
// 正确性优先于命中率。锁只保护内存结构，文件读取/解析在锁外进行（读期间不持
// 锁，不阻塞写路径）；并发同时未命中至多多读一次，无害。

/// 缓存条目：文件身份（路径 + mtime + len）+ 全量快照（按 ts 升序）。
/// mtime/len 双 None 表示"文件不存在"的占位状态。
struct CachedSnaps {
    path: PathBuf,
    mtime: Option<SystemTime>,
    len: Option<u64>,
    snaps: Vec<QuotaSnapshot>,
}

static SNAPS_CACHE: OnceLock<Mutex<Option<CachedSnaps>>> = OnceLock::new();

fn snaps_cache() -> &'static Mutex<Option<CachedSnaps>> {
    SNAPS_CACHE.get_or_init(|| Mutex::new(None))
}

/// 命中判断 + 克隆返回（快照条目小，全量克隆远低于 38MB 逐行解析）。
/// 锁中毒等异常按未命中处理，退回全量重读，不影响正确性。
fn cache_hit(
    path: &std::path::Path,
    mtime: Option<SystemTime>,
    len: Option<u64>,
) -> Option<Vec<QuotaSnapshot>> {
    let guard = snaps_cache().lock().ok()?;
    let c = guard.as_ref()?;
    if c.path == path && c.mtime == mtime && c.len == len {
        return Some(c.snaps.clone());
    }
    None
}

/// 写回缓存；拿不到锁只是丢一次缓存机会，下次读取重建。
fn cache_store(path: PathBuf, mtime: Option<SystemTime>, len: Option<u64>, snaps: &[QuotaSnapshot]) {
    if let Ok(mut guard) = snaps_cache().lock() {
        *guard = Some(CachedSnaps {
            path,
            mtime,
            len,
            snaps: snaps.to_vec(),
        });
    }
}

/// 写路径联动：成功追加一条后增量更新缓存（免得下次 load_all 因 mtime/len
/// 变化白白全量重读）。缓存为空或路径不匹配时不主动加载（留给下次读重建）。
/// 同秒去重命中时 append_to 提前返回、不落盘，也就不会走到这里（缓存不动）。
fn cache_after_append(path: &std::path::Path, snap: &QuotaSnapshot) {
    let Ok(mut guard) = snaps_cache().lock() else {
        return;
    };
    let Some(c) = guard.as_mut() else {
        return;
    };
    if c.path != path {
        return;
    }
    // stat 是单次轻量系统调用，放在锁内：多线程并发追加时后拿锁者 stat 到的
    // 必然是最终（或更新的）文件状态，缓存 mtime/len 不会落后于已含的快照内容
    let meta = fs::metadata(path).ok();
    c.mtime = meta.as_ref().and_then(|m| m.modified().ok());
    c.len = meta.as_ref().map(|m| m.len());
    // 防重入（镜像磁盘防抖规则的防御检查）：封死「并发 load_all 全量重建已含
    // 新行 + 本线程延迟 push」的指令级窗口——cache_store 是把磁盘完整内容整体
    // 写入缓存，若 load_all 恰在本线程 write_all 之后、本函数拿锁之前完成了
    // 全量重建，新快照已随全量结果进入缓存，再 push 就会产生重复条目。
    // 故仅当缓存内该账号（account 指纹）最后一条快照严格早于新快照（不可能
    // 已含）时才 push，否则跳过；mtime/len 已同步为磁盘现状，跳过不影响下次
    // 命中判断。按账号而非只看全局尾条，与 append_to 的防抖口径一致：
    // 多账号并行落盘时其他账号的更新条目不应挡住本账号的追加。
    let already_cached = c
        .snaps
        .iter()
        .rev()
        .find(|s| s.account == snap.account)
        .is_some_and(|s| s.ts >= snap.ts);
    if already_cached {
        return;
    }
    // 常规场景新快照 ts 为当前时刻（大于等于全部历史），push 后仍保持升序；
    // 时钟回拨等极端场景 sort 一次兜底，当前条目量级下开销可忽略
    c.snaps.push(snap.clone());
    c.snaps.sort_by_key(|s| s.ts);
}

/// 缓存失效：磁盘文件被结构性改写（清理重写 / 删除）时调用，下次读取重建。
fn cache_invalidate() {
    if let Ok(mut guard) = snaps_cache().lock() {
        *guard = None;
    }
}

/// 单条额度快照（jsonl 一行）。
/// 字段尽量与 QuotaResult 对齐，只挑有价值的几项，控制单条体积。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuotaSnapshot {
    /// 采样毫秒时间戳（UTC，与 model_usage.started_at 口径一致）
    pub ts: i64,
    /// 归属账号指纹（accounts 的 user_id）。None = 旧版本写入的快照，
    /// 归属无法分辨：今日增量等账号敏感计算一律跳过（参与就会跨账号混算），
    /// 仅趋势图/周对比在"当前账号视角"下把 None 视作当前账号的历史遗留。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    /// 套餐等级："pro" / "max" ...
    #[serde(default)]
    pub level: String,
    /// weekly 已用百分比 0-100
    #[serde(default)]
    pub weekly_pct: u32,
    /// weekly 下次重置时间（毫秒）；周期边界的关键字段
    #[serde(default)]
    pub weekly_reset: Option<i64>,
    /// 5 小时窗口已用百分比（顺带记录，零成本）
    #[serde(default)]
    pub hour5_pct: u32,
    /// MCP 月度已用百分比（顺带）
    #[serde(default)]
    pub mcp_pct: u32,
    /// MCP 已用次数（如有）
    #[serde(default)]
    pub mcp_used: Option<i64>,
    /// MCP 总额度次数（如有）
    #[serde(default)]
    pub mcp_total: Option<i64>,
}

/// ~/.zbar/quota_history.jsonl
pub fn history_path() -> Result<PathBuf, String> {
    Ok(config_dir()?.join("quota_history.jsonl"))
}

/// 追加一条快照。同秒内只写一条（防抖）。
/// 写失败不影响调用方（额度查询本身），因此静默降级。
pub fn append_snapshot(snap: &QuotaSnapshot) {
    if let Err(e) = try_append(snap) {
        eprintln!("[zbar-history] 写快照失败: {e}");
    }
}

fn try_append(snap: &QuotaSnapshot) -> Result<(), String> {
    let dir = config_dir()?;
    fs::create_dir_all(&dir).map_err(|e| format!("创建配置目录失败: {e}"))?;
    let path = history_path()?;

    // 滚动清理：写入前检查（内部有 24h 节流，不会每次写都全量重写）
    maybe_cleanup(&path);

    append_to(&path, snap)
}

/// 追加一条快照到指定文件（防抖 + 残行修补），路径由调用方决定（便于单测）。
fn append_to(path: &std::path::Path, snap: &QuotaSnapshot) -> Result<(), String> {
    // 防抖：窗口内回读该账号的最后一条，同秒视为同一次刷新的重复采样，跳过。
    // 必须按账号查找而非只看文件最后一条——多账号并行查询几乎同时落盘时，
    // 本账号刚写的采样可能已排在其他账号的行后面，只比最后一条会漏判重复；
    // 反过来只看 ts 不比账号，又会把其他账号的采样误吞。
    if path.exists() {
        if let Ok(recent) = read_recent_entries(path) {
            let dup = recent
                .iter()
                .rev()
                .find(|s| s.account == snap.account)
                .is_some_and(|s| s.ts / 1000 == snap.ts / 1000);
            if dup {
                return Ok(());
            }
        }
        // 防残行拼接：若上次进程崩溃残留了无换行的半截行，直接 append 会
        // 与残行拼成一行导致 JSON 损坏（load_all 会跳过整行丢新快照）。
        // 文件尾字节非 \n 时先补一个换行，让新快照独占一行。
        ensure_trailing_newline(path)?;
    }

    let line = serde_json::to_string(snap)
        .map_err(|e| format!("序列化快照失败: {e}"))?;
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| format!("打开快照文件失败: {e}"))?;
    // 每行一条，末尾换行。拼成单个缓冲一次 write_all：fetch_quota 与
    // account_quotas 多线程并发追加时，writeln! 的分段 write 可能交错
    // 产生损坏行；单次写入让行交错窗口最小化（OS O_APPEND 单次写原子）。
    let mut buf = String::with_capacity(line.len() + 1);
    buf.push_str(&line);
    buf.push('\n');
    file.write_all(buf.as_bytes())
        .map_err(|e| format!("写入快照失败: {e}"))?;
    // 成功落盘后同步增量更新进程内缓存（缓存为空/路径不匹配时为无害 no-op，
    // 测试用临时路径不会污染真实缓存）
    cache_after_append(path, snap);
    Ok(())
}

/// 确保文件以换行符结尾：尾部是半截残行（进程崩溃残留）时补一个 \n，
/// 避免后续 append 的快照与残行拼成一行。空文件视为无需处理。
fn ensure_trailing_newline(path: &std::path::Path) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map_err(|e| format!("打开快照文件失败: {e}"))?;
    let len = file
        .seek(SeekFrom::End(0))
        .map_err(|e| format!("定位快照文件失败: {e}"))?;
    if len == 0 {
        return Ok(());
    }
    let mut last = [0u8; 1];
    file.seek(SeekFrom::Start(len - 1))
        .map_err(|e| format!("定位快照文件失败: {e}"))?;
    file.read_exact(&mut last)
        .map_err(|e| format!("读取快照文件末字节失败: {e}"))?;
    if last[0] != b'\n' {
        file.seek(SeekFrom::End(0))
            .map_err(|e| format!("定位快照文件失败: {e}"))?;
        file.write_all(b"\n")
            .map_err(|e| format!("补写换行失败: {e}"))?;
    }
    Ok(())
}

/// 滚动清理入口：删除超过保留期的快照行。
/// 用 LAST_CLEANUP 节流为每 24h 至多执行一次；失败时也推进节流时钟
/// （最坏情况文件多保留一天，避免持续失败时每 30s 全量重写）。
fn maybe_cleanup(path: &PathBuf) {
    let cell = LAST_CLEANUP.get_or_init(|| Mutex::new(None));
    let Ok(mut last) = cell.lock() else {
        return; // 锁中毒等异常：跳过清理，不影响写入主流程
    };
    if let Some(t) = *last {
        if t.elapsed().unwrap_or(Duration::ZERO) < CLEANUP_INTERVAL {
            return; // 距上次清理不足 24h，跳过
        }
    }
    // 无论本次是否删出内容都推进时钟，之后再释放锁（重写文件耗时不应阻塞写入方）
    *last = Some(SystemTime::now());
    drop(last);

    if let Err(e) = try_cleanup(path) {
        eprintln!("[zbar-history] 滚动清理失败（24h 后重试）: {e}");
    }
}

/// 清理实现：过滤掉 ts 早于保留边界的行，写临时文件后 rename 原子替换。
/// 损坏的行原样保留（不丢数据，与 load_all 的跳过策略对齐）。
///
/// 已知取舍：rename 与并发 try_append 之间存在毫秒级窗口——若恰在此刻有写入
/// 落到旧 inode，rename 后那一条快照会丢失（文件本身不会损坏）。考虑到清理
/// 每 24h 才执行一次、丢的至多 1 条非关键采样（30s 后就会补采），接受现状，
/// 不为此引入全局写锁。
fn try_cleanup(path: &std::path::Path) -> Result<(), String> {
    if !path.exists() {
        return Ok(());
    }
    let cutoff = chrono::Local::now().timestamp_millis() - RETENTION_MS;

    let file = OpenOptions::new()
        .read(true)
        .open(path)
        .map_err(|e| format!("打开快照文件失败: {e}"))?;
    let reader = BufReader::new(file);

    let mut kept: Vec<String> = Vec::new();
    let mut removed = 0usize;
    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => continue, // 读失败的行无法判断，直接丢弃（与 load_all 跳过策略一致）
        };
        if line.trim().is_empty() {
            continue;
        }
        // 只取 ts 字段判断是否过期，避免严格反序列化其他字段格式变化导致误删
        let ts = serde_json::from_str::<serde_json::Value>(&line)
            .ok()
            .and_then(|v| v.get("ts").and_then(|t| t.as_i64()));
        match ts {
            Some(t) if t < cutoff => removed += 1,
            _ => kept.push(line), // 无 ts 的损坏行按保留处理，不丢数据
        }
    }
    if removed == 0 {
        return Ok(()); // 无过期行，不必重写
    }

    // 临时文件与目标同目录，rename 在同一文件系统上保证原子性
    let tmp = path.with_extension("jsonl.tmp");
    let mut out = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&tmp)
        .map_err(|e| format!("打开临时文件失败: {e}"))?;
    for line in &kept {
        writeln!(out, "{line}").map_err(|e| format!("写入临时文件失败: {e}"))?;
    }
    out.flush().map_err(|e| format!("刷新临时文件失败: {e}"))?;
    drop(out);
    fs::rename(&tmp, path).map_err(|e| format!("替换快照文件失败: {e}"))?;
    // 文件被整体重写（mtime/len 变化）：作废进程内缓存，下次读取按新文件重建。
    // 必须显式作废而不是依赖后续 append 的 cache_after_append——那个函数只会
    // 在旧缓存基础上 push 增量，会把已被清理的过期行继续留在内存里。
    cache_invalidate();
    Ok(())
}

/// 回读文件尾部窗口内的全部完整快照（防抖判断用，通常几条到几十条）。
/// 从文件末尾往回读一个小窗口，解析窗口内的完整 JSON 行，避免全文件读取。
/// 单行快照 JSON 可能超过初始窗口，不够时逐倍扩大（上限 64KB）。
fn read_recent_entries(path: &std::path::Path) -> Result<Vec<QuotaSnapshot>, String> {
    let mut file = OpenOptions::new()
        .read(true)
        .open(path)
        .map_err(|e| format!("打开快照文件失败: {e}"))?;
    let len = file
        .metadata()
        .map_err(|e| format!("读取快照文件元数据失败: {e}"))?
        .len();
    if len == 0 {
        return Ok(Vec::new());
    }

    let mut window: u64 = 2048;
    const MAX_WINDOW: u64 = 64 * 1024;
    loop {
        let start = len.saturating_sub(window);
        file.seek(SeekFrom::Start(start))
            .map_err(|e| format!("定位快照文件失败: {e}"))?;
        let mut buf = Vec::with_capacity((len - start) as usize);
        file.read_to_end(&mut buf)
            .map_err(|e| format!("读取快照失败: {e}"))?;
        let text = String::from_utf8_lossy(&buf);

        // 窗口首行可能被左边界截断（start > 0 时），不可信，跳过；
        // 其余行前面必有换行边界，均为完整行。
        let mut entries = Vec::new();
        let mut saw_complete_line = false;
        for (i, line) in text.lines().enumerate() {
            if start > 0 && i == 0 {
                continue;
            }
            if line.trim().is_empty() {
                continue;
            }
            // 损坏行跳过（与 load_all 策略一致），不影响其余行
            if let Ok(snap) = serde_json::from_str::<QuotaSnapshot>(line.trim()) {
                entries.push(snap);
                saw_complete_line = true;
            }
        }

        if saw_complete_line || start == 0 {
            return Ok(entries);
        }
        // 窗口内没找到完整非空行：扩大窗口重试
        if window < MAX_WINDOW {
            window *= 2;
            continue;
        }
        return Ok(entries);
    }
}

/// 读取全部快照（按 ts 升序）。损坏的行跳过，不整体失败。
/// 进程内缓存：文件 (路径, mtime, len) 未变时直接返回内存副本，避免高频
/// 调用方（对比页 / 今日增量 / 同步 worker / 托盘刷新）反复全量解析。
/// append-only 文件任何写入都会变长，mtime 精度不足的场景由 len 双重校验兜底。
pub fn load_all() -> Result<Vec<QuotaSnapshot>, String> {
    let path = history_path()?;
    if !path.exists() {
        // 文件不存在：缓存"空"占位，文件随后出现（外部新建 / 本模块 append
        // 创建）时元数据由 None 变 Some，自动失效重读
        let empty: Vec<QuotaSnapshot> = Vec::new();
        cache_store(path, None, None, &empty);
        return Ok(empty);
    }
    // 元数据在锁外获取（IO 不持锁）
    let meta = fs::metadata(&path).ok();
    let mtime = meta.as_ref().and_then(|m| m.modified().ok());
    let len = meta.as_ref().map(|m| m.len());
    if let Some(snaps) = cache_hit(&path, mtime, len) {
        return Ok(snaps);
    }
    // 未命中：全量重读（锁外 IO + 解析），完成后写回缓存
    let snaps = read_all_from(&path)?;
    cache_store(path, mtime, len, &snaps);
    Ok(snaps)
}

/// 全量解析实现（原 load_all 主体）：逐行解析，损坏的行跳过，结果按 ts 升序。
fn read_all_from(path: &std::path::Path) -> Result<Vec<QuotaSnapshot>, String> {
    let file = OpenOptions::new()
        .read(true)
        .open(path)
        .map_err(|e| format!("打开快照文件失败: {e}"))?;
    let reader = BufReader::new(file);
    let mut out = Vec::new();
    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => continue,
        };
        if line.trim().is_empty() {
            continue;
        }
        if let Ok(snap) = serde_json::from_str::<QuotaSnapshot>(&line) {
            out.push(snap);
        }
    }
    // 保证升序
    out.sort_by_key(|s| s.ts);
    Ok(out)
}

/// 清空历史（设置页"清理历史"用）。
pub fn clear_history() -> Result<(), String> {
    let path = history_path()?;
    if path.exists() {
        fs::remove_file(&path).map_err(|e| format!("删除快照文件失败: {e}"))?;
    }
    // 文件已删除：作废缓存，避免下次读取命中"删除前"的旧数据
    cache_invalidate();
    Ok(())
}

/// 过滤为"当前账号视角"的快照序列（趋势图/周对比等当前账号读路径共用）：
/// - 无 account 字段的旧快照：归属无法分辨，视为当前账号的历史遗留保留
///   （单账号用户升级后 90 天历史不清空；代价是多账号用户切号后旧数据
///   仍在其视图内，随 90 天滚动清理自然消退）；
/// - 带指纹的快照：仅保留当前账号——多账号并行采样不得混入；
/// - 拿不到当前指纹（未登录等）：只剩旧版快照可看。
pub fn filter_for_current(
    snaps: Vec<QuotaSnapshot>,
    current: &Option<String>,
) -> Vec<QuotaSnapshot> {
    snaps
        .into_iter()
        .filter(|s| match (&s.account, current) {
            (None, _) => true,
            (Some(acc), Some(cur)) => acc == cur,
            (Some(_), None) => false,
        })
        .collect()
}

// ===== 周期解析 =====

/// 一个"智谱重置周期"的汇总（供对比页直接渲染）。
#[derive(Debug, Clone, Serialize)]
pub struct WeeklyPeriod {
    /// 周期开始（重置时刻）。首条历史周期可能拿不到，用首条快照 ts 兜底。
    pub reset_at: i64,
    /// 周期结束（下一次重置时刻）。当前未结束周期用 now 兜底。
    pub end_at: i64,
    /// 是否当前未结束的周期
    pub is_current: bool,
    /// 周期内 weekly 起始百分比（第一条快照）
    pub pct_start: u32,
    /// 周期内 weekly 峰值百分比
    pub pct_peak: u32,
    /// 周期内 weekly 结束百分比（最后一条快照）
    pub pct_end: u32,
    /// 周期内采样数（衡量可信度）
    pub sample_count: u32,
}

/// 把快照序列按 weekly_reset 的跳变点切分成多个周期。
///
/// 核心语义（关键，勿混淆）：
/// - 每条快照的 weekly_reset = "此刻的下次重置时间"（未来的）
/// - 当 weekly_reset 发生跳变（新值 > 旧值）时，说明刚发生了一次重置。
///   这个跳变点就是前后两周期的分界。
/// - 周期[i] 的 reset_at(开始) = min(跳变前最后一条快照预告的 weekly_reset,
///   跳变后首条快照 ts)：常规错过跳变场景取服务端预告的准确重置时刻；
///   服务端提前重置（旧预告时刻未到就跳变）时取快照 ts，两种场景都不漏用量
/// - 周期[i] 的 end_at(结束) = 与下一周期 reset_at 共用的统一分界
///   min(周期内最后一条快照预告的 weekly_reset, 下一周期首条快照 ts)，
///   保证相邻周期区间无缝不重叠（SQL 逐周期独立聚合，重叠会双计）
/// - 兼容补差价/重新订阅导致的提前重置：任何大幅跳变都视为新周期。
pub fn split_periods(snaps: &[QuotaSnapshot]) -> Vec<WeeklyPeriod> {
    if snaps.is_empty() {
        return Vec::new();
    }

    let now_ms = chrono::Local::now().timestamp_millis();
    let mut periods: Vec<WeeklyPeriod> = Vec::new();

    // 当前正在累积的周期的快照索引区间 [start_idx, end_idx)
    let mut start_idx = 0usize;
    // 当前周期的开始时间（重置时刻）
    // 第一条无法知道真正的重置时刻 → 用首条快照 ts 兜底（可能比真实开始晚）
    let mut cur_start = snaps[0].ts;

    for i in 1..snaps.len() {
        let prev_reset = snaps[i - 1].weekly_reset;
        let cur_reset = snaps[i].weekly_reset;
        // 跳变检测：前一条的 weekly_reset 是某未来时间 T，
        // 当前条的 weekly_reset 明显更大（>= 1天），说明 T 已经过去并发生了重置，
        // 新的 nextResetTime 变成了 T'（更远）。这个跳变 = 重置发生。
        let jumped = match (prev_reset, cur_reset) {
            (Some(p), Some(c)) => c > p + 86_400_000,
            _ => false,
        };
        if jumped {
            // 统一分界 = min(跳变前快照预告的 weekly_reset, 跳变后首条快照 ts)。
            // 上一周期的 end_at 与新周期的 reset_at 必须共用同一分界，区间才无缝
            // 不重叠——本地 SQL 与前端远端归属均按 [reset_at, end_at) 逐周期独立
            // 聚合，两个分界不一致会让重叠段的用量双计进两个周期：
            // - 常规场景（应用在重置后才恢复采样，snaps[i].ts > 预告时刻）：分界 =
            //   预告的准确重置时刻，闭合"重置~首条快照"的漏算缺口（否则对比页
            //   "实际 Token"偏低）；
            // - 服务端提前重置场景（跳变发生时旧预告时刻还没到，snaps[i].ts < 预告
            //   时刻）：分界 = snaps[i].ts，避免把 [snaps[i].ts, 预告时刻) 的用量
            //   漏出所有周期。
            // i 从 1 起循环，i-1 恒存在；jumped 要求 prev_reset 为 Some，unwrap_or 仅为兜底。
            let boundary = prev_reset.unwrap_or(snaps[i].ts).min(snaps[i].ts);
            // [start_idx, i) 这些快照属于上一周期（跳变前的）
            periods.push(build_period(
                &snaps[start_idx..i],
                cur_start,
                boundary,
                false,
                now_ms,
            ));
            start_idx = i;
            cur_start = boundary;
        }
    }
    // 收尾最后一个周期（当前未结束）
    let last = &snaps[snaps.len() - 1];
    periods.push(build_period(
        &snaps[start_idx..],
        cur_start,
        last.weekly_reset.unwrap_or(last.ts),
        true,
        now_ms,
    ));
    periods
}

/// 用一组快照构建一个周期汇总。
/// - reset_at: 周期开始（重置时刻，可能为首条 ts 兜底）
/// - next_reset: 周期结束（下次重置时刻；当前周期用 now）
fn build_period(
    snaps: &[QuotaSnapshot],
    reset_at: i64,
    next_reset: i64,
    is_current: bool,
    now_ms: i64,
) -> WeeklyPeriod {
    let pct_start = snaps.first().map(|s| s.weekly_pct).unwrap_or(0);
    let pct_end = snaps.last().map(|s| s.weekly_pct).unwrap_or(0);
    let pct_peak = snaps.iter().map(|s| s.weekly_pct).max().unwrap_or(0);
    // 当前周期的 end 用 now（实时延伸）；已结束周期用 next_reset
    let end_at = if is_current { now_ms } else { next_reset };
    WeeklyPeriod {
        reset_at,
        end_at,
        is_current,
        pct_start,
        pct_peak,
        pct_end,
        sample_count: snaps.len() as u32,
    }
}

/// 取"今日"快照（本地 0 点之后，含全部账号）。
fn today_snapshots() -> Result<Vec<QuotaSnapshot>, String> {
    let all = load_all()?;
    let today_start = chrono::Local::now()
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .unwrap()
        .and_local_timezone(chrono::Local)
        .single()
        .map(|d| d.timestamp_millis())
        .unwrap_or(0);
    Ok(all.into_iter().filter(|s| s.ts >= today_start).collect())
}

/// 一批账号的今日增量（多账号一次文件读取算完，避免逐账号重复解析全量历史）。
/// 单账号增量 = 按时间升序逐条遍历今日快照、遇 weekly_pct 回落即切段，
/// 逐段累计 (段末值 − 段首值) 后求和；返回 (增量, 采样数)。
///
/// 为什么不用"今日峰值 − 今日首条"：weekly_pct 在一天内并非单调不回落的量——
/// 周额度重置、使用重置券、补差价/重新订阅都会让已用百分比回落。旧公式在
/// 回落场景失真：用到 100% 后用重置券，峰值冻结在 100，增量从此恒定不动
/// （用户实际观察到"徽标卡死"）；当天发生周重置时首条与峰值还分属两个额度
/// 周期，口径错乱。分段累计把每次回落视为一次重置事件切开，只累加各段内
/// 真实消耗——注意多段累计允许超过 100（一天内多次重置后的各段消耗都算
/// 今日真实消耗），不做截断。
///
/// 账号严格匹配：快照 account 为 None（旧版本写入、归属无法分辨）的行
/// 不参与任何账号——切号当日的旧混合序列一旦被计入，峰值与首条分属不同
/// 账号，增量会跨账号失真（历史上"今日xx%像多账号总和"的根因）。
pub fn today_deltas(
    accounts: &[String],
) -> Result<std::collections::HashMap<String, (u32, u32)>, String> {
    let today = today_snapshots()?;
    Ok(today_deltas_from(&today, accounts))
}

/// today_deltas 的纯计算部分（已过滤为"今日"的快照，便于单测注入）。
/// 分段累计语义见 today_deltas 文档注释。
fn today_deltas_from(
    today: &[QuotaSnapshot],
    accounts: &[String],
) -> std::collections::HashMap<String, (u32, u32)> {
    let mut out = std::collections::HashMap::new();
    let wanted: std::collections::HashSet<&str> = accounts.iter().map(|s| s.as_str()).collect();
    // 按账号分组（保持传入顺序即时间升序）
    let mut grouped: std::collections::HashMap<&str, Vec<&QuotaSnapshot>> =
        std::collections::HashMap::new();
    for snap in today {
        if let Some(acc) = snap.account.as_deref() {
            if wanted.contains(acc) {
                grouped.entry(acc).or_default().push(snap);
            }
        }
    }
    for (acc, snaps) in grouped {
        // 分段累计：seg_start = 当前段首值，last = 上一条值。
        // 当前条低于上一条 = 检测到一次回落（周重置/重置券/补差价等重置事件），
        // 先结算上一段 (last − seg_start) 再从此条开新段；段内单调不减，
        // 差值恒非负，无需再取 max。遍历结束后单独结算最后一段。
        let mut seg_start = snaps.first().map(|s| s.weekly_pct).unwrap_or(0);
        let mut last = seg_start;
        // u64 累计：多段求和理论上可超出 u32 上限，先放宽再夹回 u32
        // （实际一天内的重置次数远不足以逼近），对现有返回类型无影响
        let mut total: u64 = 0;
        for snap in snaps.iter().skip(1) {
            let pct = snap.weekly_pct;
            if pct < last {
                total += u64::from(last - seg_start);
                seg_start = pct;
            }
            last = pct;
        }
        total += u64::from(last - seg_start);
        out.insert(
            acc.to_string(),
            (total.min(u64::from(u32::MAX)) as u32, snaps.len() as u32),
        );
    }
    // 请求了但今日无采样的账号给 (0, 0)，前端按采样数<2 自然不显示
    for acc in accounts {
        out.entry(acc.clone()).or_insert((0, 0));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 生成临时测试文件路径（用例各自创建/清理，不引入 tempfile 依赖）
    fn temp_jsonl(name: &str) -> PathBuf {
        let dir = std::env::temp_dir();
        std::fs::create_dir_all(&dir).ok();
        dir.join(format!(
            "zbar-history-test-{name}-{}.jsonl",
            std::process::id()
        ))
    }

    fn snap(ts: i64, level: &str) -> QuotaSnapshot {
        QuotaSnapshot {
            ts,
            account: None,
            level: level.to_string(),
            weekly_pct: 10,
            weekly_reset: Some(ts + 86_400_000),
            hour5_pct: 5,
            mcp_pct: 0,
            mcp_used: None,
            mcp_total: None,
        }
    }

    /// 生成指定账号与周额百分比的快照（今日增量隔离用例需要精确控制）
    fn snap_of(ts: i64, account: &str, weekly_pct: u32) -> QuotaSnapshot {
        QuotaSnapshot {
            account: Some(account.to_string()),
            weekly_pct,
            ..snap(ts, "pro")
        }
    }

    /// 生成指定 weekly_reset 的快照（周期切分用例需要精确控制预告重置时刻）
    fn snap_at(ts: i64, weekly_reset: Option<i64>) -> QuotaSnapshot {
        QuotaSnapshot {
            weekly_reset,
            ..snap(ts, "pro")
        }
    }

    /// 缓存测试互斥锁：SNAPS_CACHE 是进程级静态，cache 语义用例与成功重写
    /// 后会 cache_invalidate 的 cleanup 用例必须串行执行，否则并行断言会被
    /// 对方的 invalidate 打断（失效本身是正确性行为，测试只需互斥）。
    /// into_inner 容忍中毒：某用例持锁 panic 后不连坐其余缓存用例。
    static CACHE_TEST_LOCK: Mutex<()> = Mutex::new(());

    /// 进程内缓存一致性语义（全程持 CACHE_TEST_LOCK，临时路径与真实历史隔离）：
    /// - cache_store / cache_hit：三元组（路径, mtime, len）全一致才命中，
    ///   任一分量变化（append 变 len / 清理重写或外部改动变 mtime / 路径不同）
    ///   均未命中——这是 load_all 二次调用不读盘与磁盘改动自动失效的机制；
    /// - cache_invalidate：失效后 cache_hit 为 None（下次 load_all 全量重读）；
    /// - 写路径联动：缓存为空时 append 不主动加载；已有条目时增量追加且
    ///   元数据与磁盘一致；
    /// - 防重入（镜像磁盘防抖规则）：缓存内该账号最后一条 ts >= 新快照 ts 时
    ///   不 push（模拟并发 load_all 已把新行整体写入缓存的竞态窗口）；
    ///   同 ts 不同账号不受影响；
    /// - 同秒去重跳过（不落盘）不动缓存。
    #[test]
    fn cache_semantics_hit_store_append_invalidate() {
        let _serial = CACHE_TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        cache_invalidate(); // 从干净状态开始，不依赖其他用例遗留的缓存
        let path = temp_jsonl("cache");
        let mtime = SystemTime::UNIX_EPOCH;

        // ===== 命中/未命中：三元组各分量变化 =====
        let snaps = vec![snap_of(1_000, "A", 20), snap_of(2_000, "B", 50)];
        cache_store(path.clone(), Some(mtime), Some(10), &snaps);
        assert_eq!(
            cache_hit(&path, Some(mtime), Some(10)).unwrap().len(),
            2,
            "三元组一致应命中"
        );
        assert!(
            cache_hit(&path, Some(mtime), Some(11)).is_none(),
            "len 变化（append 变长）应未命中"
        );
        assert!(
            cache_hit(&path, Some(SystemTime::now()), Some(10)).is_none(),
            "mtime 变化（清理重写/外部改动）应未命中"
        );
        assert!(
            cache_hit(&temp_jsonl("cache-other"), Some(mtime), Some(10)).is_none(),
            "路径不同应未命中"
        );

        // ===== invalidate 后重读：失效即未命中 =====
        cache_invalidate();
        assert!(snaps_cache().lock().unwrap().is_none(), "invalidate 后缓存应为空");
        assert!(
            cache_hit(&path, Some(mtime), Some(10)).is_none(),
            "invalidate 后应未命中（下次 load_all 走全量重读）"
        );

        // ===== 写路径联动：缓存为空时 append 不主动加载（留给下次读重建）=====
        append_to(&path, &snap_of(3_000, "A", 30)).unwrap();
        assert!(
            snaps_cache().lock().unwrap().is_none(),
            "缓存为空时写入不主动加载"
        );

        // ===== 写路径联动：已有条目时 append 后缓存增量与磁盘一致 =====
        cache_store(path.clone(), None, None, &[]);
        append_to(&path, &snap_of(4_000, "A", 40)).unwrap();
        {
            let guard = snaps_cache().lock().unwrap();
            let c = guard.as_ref().unwrap();
            assert_eq!(c.snaps.len(), 1, "空占位不回填旧数据，仅追加本次新快照");
            assert_eq!(c.snaps[0].ts, 4_000);
            assert_eq!(c.snaps[0].weekly_pct, 40);
            // 元数据与磁盘一致：下次 load_all 的三元组校验直接命中
            let meta = std::fs::metadata(&path).unwrap();
            assert_eq!(c.len, Some(meta.len()));
            assert_eq!(c.mtime, meta.modified().ok());
        }

        // ===== 防重入：同账号 ts >= 新快照 ts 时不 push =====
        // 模拟竞态窗口：并发 load_all 已把含 A@5000 新行的磁盘内容整体写入
        // 缓存，随后同一快照的 cache_after_append 迟到——不得再 push 成重复条目
        let fresh = vec![snap_of(4_000, "A", 40), snap_of(5_000, "A", 50)];
        cache_store(path.clone(), None, None, &fresh);
        append_to(&path, &snap_of(5_000, "A", 50)).unwrap();
        {
            let guard = snaps_cache().lock().unwrap();
            assert_eq!(
                guard.as_ref().unwrap().snaps.len(),
                2,
                "防重入：缓存已含同账号同 ts 快照，不得重复 push"
            );
        }
        // 同 ts 不同账号不得被防重入误伤（与磁盘防抖按账号判断的口径一致）
        append_to(&path, &snap_of(5_000, "B", 60)).unwrap();
        {
            let guard = snaps_cache().lock().unwrap();
            let c = guard.as_ref().unwrap();
            assert_eq!(c.snaps.len(), 3, "同 ts 不同账号应正常 push");
            assert_eq!(c.snaps[2].account.as_deref(), Some("B"));
        }

        // ===== 去重跳过（不落盘）不动缓存 =====
        append_to(&path, &snap_of(5_000, "A", 55)).unwrap(); // 同秒同账号，append_to 去重跳过
        {
            let guard = snaps_cache().lock().unwrap();
            assert_eq!(
                guard.as_ref().unwrap().snaps.len(),
                3,
                "去重跳过不应改动缓存"
            );
        }

        std::fs::remove_file(&path).ok();
        cache_invalidate(); // 归还干净状态，不影响其他用例
    }

    /// 周期切分：跳变前快照预告的 weekly_reset 落在跳变后快照 ts 之前数小时
    /// （应用重置后很久才启动的场景），新周期 reset_at 应等于预告重置时刻，
    /// 否则真实重置~首条快照之间的用量会漏出所有周期
    #[test]
    fn split_periods_uses_announced_reset_as_new_start() {
        // 周五 22:00 采样，预告周六 02:00 重置；应用关机，周一 20:00 才恢复采样
        let announced = 1_800_000_000_000_i64;
        let before_ts = announced - 4 * 3600_000;
        let before = snap_at(before_ts, Some(announced));
        let after = snap_at(
            announced + 66 * 3600_000,
            Some(announced + 7 * 86_400_000),
        );
        let periods = split_periods(&[before, after]);
        assert_eq!(periods.len(), 2, "weekly_reset 大幅跳变应切出两个周期");
        // 上一周期：起点用首条快照 ts 兜底，终点 = 预告重置时刻
        assert_eq!(periods[0].reset_at, before_ts);
        assert_eq!(periods[0].end_at, announced, "上一周期终点应为预告重置时刻");
        // 新周期：起点 = 预告重置时刻（而非首条快照 ts），闭合 66h 的漏算缺口
        assert_eq!(periods[1].reset_at, announced);
        assert!(periods[0].end_at <= periods[1].reset_at, "相邻周期必须无缝不重叠");
        assert!(periods[1].is_current);
    }

    /// 周期切分：服务端提前重置（跳变发生时旧预告时刻还没到，snaps[i].ts <
    /// 预告时刻）新周期起点应取 snaps[i].ts，避免 [snaps[i].ts, 预告时刻)
    /// 的用量漏出所有周期
    #[test]
    fn split_periods_early_reset_uses_snapshot_ts() {
        let base = 1_800_000_000_000_i64;
        // 采样时预告 20 小时后重置；1 小时后再次采样，服务端已把重置大幅推远
        let before = snap_at(base, Some(base + 20 * 3600_000));
        let after_ts = base + 3600_000;
        let after = snap_at(after_ts, Some(after_ts + 8 * 86_400_000));
        let periods = split_periods(&[before, after]);
        assert_eq!(periods.len(), 2);
        // 新周期起点 = min(预告时刻, 首条快照 ts) = 快照 ts（无缝，不取未到的预告时刻）
        assert_eq!(periods[1].reset_at, after_ts);
        // 上一周期 end_at 与新周期 reset_at 共用同一分界：提前重置场景下
        // 上一周期 end_at 也必须收窄到快照 ts，否则重叠段用量会被双计
        assert_eq!(periods[0].end_at, after_ts);
        assert!(periods[0].end_at <= periods[1].reset_at, "相邻周期必须无缝不重叠");
        assert!(periods[1].is_current);
    }

    /// 周期切分：无跳变时只有单个当前周期，起点用首条快照 ts 兜底
    #[test]
    fn split_periods_single_when_no_jump() {
        let base = 1_800_000_000_000_i64;
        let snaps = vec![
            snap_at(base, Some(base + 7 * 86_400_000)),
            snap_at(base + 3600_000, Some(base + 7 * 86_400_000)),
        ];
        let periods = split_periods(&snaps);
        assert_eq!(periods.len(), 1, "weekly_reset 未跳变不应切分周期");
        assert_eq!(periods[0].reset_at, base);
        assert_eq!(periods[0].sample_count, 2);
        assert!(periods[0].is_current);
    }

    /// 尾部回读：常规多行文件能取到窗口内全部快照（末尾即最后一条）
    #[test]
    fn read_recent_entries_returns_tail_lines() {
        let path = temp_jsonl("last");
        let content = format!(
            "{}\n{}\n{}\n",
            serde_json::to_string(&snap(1000, "pro")).unwrap(),
            serde_json::to_string(&snap(2000, "pro")).unwrap(),
            serde_json::to_string(&snap(3000, "max")).unwrap()
        );
        std::fs::write(&path, &content).unwrap();
        let entries = read_recent_entries(&path).unwrap();
        assert_eq!(entries.len(), 3, "小文件窗口应覆盖全部行");
        assert_eq!(entries.last().unwrap().ts, 3000, "末尾应为最后一条快照");
        std::fs::remove_file(&path).ok();
    }

    /// 尾部回读：单行 JSON 超过初始 2KB 窗口时逐倍扩大窗口仍能取到
    #[test]
    fn read_recent_entries_expands_window_for_long_line() {
        let path = temp_jsonl("long");
        let long = snap(42, &"x".repeat(5000));
        let content = format!(
            "{}\n{}\n",
            serde_json::to_string(&snap(1000, "pro")).unwrap(),
            serde_json::to_string(&long).unwrap()
        );
        std::fs::write(&path, &content).unwrap();
        let entries = read_recent_entries(&path).unwrap();
        assert_eq!(entries.last().unwrap().ts, 42, "长行应通过扩大窗口解析到");
        std::fs::remove_file(&path).ok();
    }

    /// 尾部回读：空文件返回空（视作无历史，防抖直接放行）
    #[test]
    fn read_recent_entries_empty_file_returns_empty() {
        let path = temp_jsonl("empty");
        std::fs::write(&path, "").unwrap();
        assert!(read_recent_entries(&path).unwrap().is_empty(), "空文件应无快照");
        std::fs::remove_file(&path).ok();
    }

    /// 滚动清理：过期行被删、保留行原样保留，重写后仍可正常解析
    #[test]
    fn cleanup_removes_expired_lines() {
        let path = temp_jsonl("cleanup");
        let now = chrono::Local::now().timestamp_millis();
        let old_ts = now - RETENTION_MS - 86_400_000; // 超过保留期一天
        let content = format!(
            "{}\n{}\n{}\n",
            serde_json::to_string(&snap(old_ts, "old")).unwrap(),
            serde_json::to_string(&snap(now - 1000, "keep1")).unwrap(),
            serde_json::to_string(&snap(now, "keep2")).unwrap()
        );
        std::fs::write(&path, &content).unwrap();

        // 持缓存测试互斥锁：try_cleanup 成功重写后会 cache_invalidate 进程级
        // 静态缓存，与 cache 语义用例互斥执行，避免并行断言被打断
        let _serial = CACHE_TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        try_cleanup(&path).unwrap();

        let cleaned = std::fs::read_to_string(&path).unwrap();
        assert!(!cleaned.contains("old"), "过期行应被删除");
        assert_eq!(cleaned.lines().count(), 2, "保留行应原样保留");
        assert_eq!(read_recent_entries(&path).unwrap().last().unwrap().ts, now);
        std::fs::remove_file(&path).ok();
    }

    /// 残行补丁：文件尾是无换行的半截残行时补 \n，正常文件不动
    #[test]
    fn ensure_trailing_newline_patches_partial_line() {
        let path = temp_jsonl("newline");
        let ok_line = serde_json::to_string(&snap(1000, "pro")).unwrap();
        // 模拟进程崩溃残留：尾行是半截 JSON 且无换行
        std::fs::write(&path, format!("{ok_line}\n{{\"ts\":2000,\"level\":\"pr"))
            .unwrap();
        ensure_trailing_newline(&path).unwrap();
        let patched = std::fs::read_to_string(&path).unwrap();
        assert!(patched.ends_with('\n'), "残行后应补上换行");
        assert_eq!(patched.lines().count(), 2, "不应改变行数");

        // 已正常结尾的文件：幂等，不再追加换行
        ensure_trailing_newline(&path).unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            patched,
            "正常结尾时不应追加换行"
        );
        std::fs::remove_file(&path).ok();
    }

    /// 今日增量按账号严格隔离：账号 A 20→40、账号 B 50→85、
    /// 旧版无账号快照（weekly_pct=10）混在中间时，A/B 各得各的增量，
    /// None 快照不参与任何账号（否则峰值与首条跨账号取差，
    /// 增量会失真成"多账号总和"——本次修复的目标场景）
    #[test]
    fn today_deltas_isolates_accounts() {
        let snaps = vec![
            snap_of(1_000, "A", 20),
            snap(2_000, "pro"), // 旧版快照 weekly_pct=10
            snap_of(3_000, "B", 50),
            snap_of(4_000, "A", 40),
            snap(5_000, "pro"), // 旧版快照 weekly_pct=10
            snap_of(6_000, "B", 85),
        ];
        let accounts = vec!["A".to_string(), "B".to_string(), "C".to_string()];
        let deltas = today_deltas_from(&snaps, &accounts);
        assert_eq!(deltas["A"], (20, 2), "A：峰值40-起始20=20，2条采样");
        assert_eq!(deltas["B"], (35, 2), "B：峰值85-起始50=35，2条采样");
        assert_eq!(deltas["C"], (0, 0), "今日无采样的账号给(0,0)");
    }

    /// 今日增量分段累计：用到 100% 后用重置券（weekly_pct 回落到 0）再涨到 30，
    /// 增量应为 (100−10)+(30−0)=120，而非旧公式"峰值100−首条10=90"——
    /// 旧公式还把峰值冻结在 100、后续增量恒定不动（用户观察到的徽标卡死）。
    /// 多段累计允许超过 100：一天内多次重置后的各段消耗都是今日真实消耗，
    /// 不做截断（显示层直接展示 u32/number，无 100 上限）。
    #[test]
    fn today_deltas_counts_segments_across_reset_coupon() {
        let snaps = vec![
            snap_of(1_000, "A", 10),
            snap_of(2_000, "A", 40),
            snap_of(3_000, "A", 100),
            snap_of(4_000, "A", 0), // 重置券：回落切段
            snap_of(5_000, "A", 30),
        ];
        let deltas = today_deltas_from(&snaps, &["A".to_string()]);
        assert_eq!(deltas["A"], (120, 5), "分段累计 (100-10)+(30-0)=120，不截断");
    }

    /// 今日增量分段累计：当天发生周重置（3→50→0→40），
    /// 增量 = (50−3)+(40−0)=87，而不是把重置前后混成一段
    /// （首条与峰值分属两个额度周期会得到 50−3=47，漏掉重置后的消耗）
    #[test]
    fn today_deltas_splits_on_weekly_reset() {
        let snaps = vec![
            snap_of(1_000, "A", 3),
            snap_of(2_000, "A", 50),
            snap_of(3_000, "A", 0), // 周重置
            snap_of(4_000, "A", 40),
        ];
        let deltas = today_deltas_from(&snaps, &["A".to_string()]);
        assert_eq!(deltas["A"], (87, 4), "(50-3)+(40-0)=87");
    }

    /// 今日增量无回落回归：序列单调上涨时退化为"峰值 − 起始"，
    /// 与旧公式结果一致（升级后历史正常日期的展示不变）
    #[test]
    fn today_deltas_matches_old_formula_without_drop() {
        let snaps = vec![
            snap_of(1_000, "A", 5),
            snap_of(2_000, "A", 20),
            snap_of(3_000, "A", 35),
        ];
        let deltas = today_deltas_from(&snaps, &["A".to_string()]);
        assert_eq!(deltas["A"], (30, 3), "无回落时等价于峰值35-起始5=30");
    }

    /// 今日增量持续小幅回落（采样抖动）：每次回落都切段，段内差为 0，
    /// 累计为 0 而不是把负差当成消耗（分段口径天然把回落噪声吃掉）
    #[test]
    fn today_deltas_ignores_small_dips() {
        let snaps = vec![
            snap_of(1_000, "A", 50),
            snap_of(2_000, "A", 49),
            snap_of(3_000, "A", 48),
        ];
        let deltas = today_deltas_from(&snaps, &["A".to_string()]);
        assert_eq!(deltas["A"], (0, 3), "每段段末−段首均为 0，累计 0");
    }

    /// 防抖联合判断：同秒不同账号的采样不得互相吞掉（多账号并行查询
    /// 几乎同时落盘的场景）；同秒同账号仍视为重复采样跳过。
    /// 注：必须用 append_to（指定临时路径）——try_append 固定写真实
    /// ~/.zbar/quota_history.jsonl，测试不得触碰。
    #[test]
    fn try_append_same_second_different_account_not_dropped() {
        let path = temp_jsonl("dedup");
        let ts = 1_800_000_000_000_i64;
        // 账号 A 先写一条
        append_to(&path, &snap_of(ts, "A", 20)).unwrap();
        // 同秒账号 B：不得被 A 的防抖误伤
        append_to(&path, &snap_of(ts, "B", 50)).unwrap();
        // 同秒账号 A 再次写入：重复采样，跳过
        append_to(&path, &snap_of(ts, "A", 30)).unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = content
            .lines()
            .filter(|l| !l.trim().is_empty())
            .collect();
        assert_eq!(lines.len(), 2, "A、B 各一条，A 的重复采样被跳过");
        let first: QuotaSnapshot = serde_json::from_str(lines[0]).unwrap();
        let second: QuotaSnapshot = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(first.account.as_deref(), Some("A"));
        assert_eq!(second.account.as_deref(), Some("B"));
        std::fs::remove_file(&path).ok();
    }
}
