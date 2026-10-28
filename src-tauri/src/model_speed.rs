//! Per-model request speed statistics. Token volume and timing sample counts
//! are independent: only validated requests contribute to speed denominators.
//! Weighted mean = sum(output) * 1000 / sum(request/stream milliseconds).
//! Percentiles describe individual request rates, not whole-turn throughput.
//!
//! ZCode uses event timestamps and conservative request-average timing when
//! reasoning placement is uncertain. DSH already records stream spans; other
//! imported/remote sources record request spans. Never subtract TTFT from DSH.
//! Exact and approximate pools are kept separate, with exact samples preferred
//! within a source/provider/model group. Request-average values display ≈.
//! Tool execution gaps between model requests do not contribute to either pool.

use rusqlite::Connection;
use serde::Serialize;
use std::collections::BTreeMap;

use crate::token_speed::SpeedQuality;

/// 单模型速度统计（Tauri command 返回；serde camelCase 与前端契约一致）
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelSpeedStat {
    /// 模型 id（分组键之一）
    pub model_id: String,
    /// 来源名："zcode" | "codex" | "claude" | "kimi" | "dsh" | "opencode" |
    /// "minimaxcode"（分组键之一——同名模型跨来源绝不合并）
    pub source: String,
    /// provider 简名（分组键之一）：zcode 取 "account:" 后部分（完整值，
    /// **不截断**——截断由前端 CSS 承担，见 provider_short）；导入库原样
    /// ——opencode 的 providerID 是用户自配渠道名（如 opencode-go /
    /// deepseek），与 account: 前缀语义不同，不做剥离
    pub provider: String,
    /// 请求数（窗口内行数；全部设备口径下含远端行）
    pub requests: i64,
    /// 本机行数（纯本机计数，requests 减远端行）：前端 hero 成功率聚合
    /// 的权重口径——后端 successRate 分母只算本机行，权重用 requests 会
    /// 在全部设备口径下失真（本机模式两者相等，行为不变）
    pub local_requests: i64,
    /// 速度合格样本数（speed_sample 行级判定通过的请求数；与 requests
    /// 总数口径不同——无可信生成区间的请求不产生速度样本，null = 无可信样本）
    pub speed_sample_count: Option<i64>,
    /// 组级速度口径（serde 序列化为 "generation" | "request_average"，与
    /// 前端契约一致）：generation 池非空 → "generation"（zcode 精确样本
    /// 优先，远端 / 导入库近似样本不掺入）；generation 空 → "request_
    /// average"（请求平均近似，含首字延迟，前端 ≈ 前缀）；两池皆空 →
    /// None（codex 无耗时列，或窗口内全部行异常）
    pub speed_quality: Option<SpeedQuality>,
    /// 成功率（completed 行占比，0–1；status 列缺失的老库为 null）
    pub success_rate: Option<f64>,
    /// 平均速度 t/s（Σoutput ÷ Σ生成毫秒；无可信行为 null）
    pub avg_tps: Option<f64>,
    /// 慢速（每请求速度 P10）
    pub p10_tps: Option<f64>,
    /// 均速（每请求速度 P50）
    pub p50_tps: Option<f64>,
    /// 快速（每请求速度 P90）
    pub p90_tps: Option<f64>,
    /// 首 token 延迟最小值（毫秒）
    pub ttft_min_ms: Option<i64>,
    /// 首 token 延迟均值（毫秒，四舍五入）
    pub ttft_avg_ms: Option<i64>,
    /// 首 token 延迟 P90（毫秒）
    pub ttft_p90_ms: Option<i64>,
    /// 请求耗时均值（completed_at − started_at，毫秒）
    pub dur_avg_ms: Option<i64>,
    /// 请求耗时 P90（毫秒）
    pub dur_p90_ms: Option<i64>,
    /// 输入 token 均值（input_tokens 原值，含缓存读）
    pub in_avg: i64,
    /// 输入 token 最大值
    pub in_max: i64,
    /// 窗口内累计输入 token（= Σinput_tokens 原值，含缓存读；速度页大窗
    /// 展示总量、均值进数值格 title）
    pub in_total: i64,
    /// 输出 token 均值
    pub out_avg: i64,
    /// 输出 token 最大值
    pub out_max: i64,
    /// 窗口内累计输出 token（= Σoutput_tokens）
    pub out_total: i64,
}

/// get_model_speed 的返回（serde camelCase 与前端契约一致）：
/// 统计分组 + 远端拉取是否失败。remote_failed 仅 scope="all" 时可能为
/// true（同步未启用 / 未注册设备 / 网络失败 / 旧服务端 404 等一切「尝试
/// 拉远端但没拉到」）；本机模式恒 false。远端失败不阻塞本机数据，前端
/// 据此弱提示（对齐 request_logs failed_sources 的容错精神）。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelSpeedResponse {
    pub stats: Vec<ModelSpeedStat>,
    pub remote_failed: bool,
}

/// 远端单次拉取行数上限（服务端 /usage_records 同款夹紧 5000；今日 / 7 天
/// 窗口的速度分位样本量足够，不循环分页拉全量——见模块头「全部设备口径」）
const REMOTE_FETCH_LIMIT: i64 = 5000;

/// 查询时间范围起点（毫秒，纯函数供测试）：
/// - days > 0：now − N×24h（滚动窗口）；
/// - days = 0（今日）：本地时区自然日零点（与主面板 today / 菜单栏
///   today_tray_title 的零点口径一致——不能用 UTC，否则东八区午前会把
///   昨天计入）。
fn range_start_ms(days: i64, now_ms: i64) -> i64 {
    if days > 0 {
        return now_ms - days * 86_400_000;
    }
    let Some(dt) = chrono::DateTime::from_timestamp_millis(now_ms) else {
        return now_ms - 86_400_000;
    };
    let local = dt.with_timezone(&chrono::Local);
    local
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .and_then(|midnight| midnight.and_local_timezone(chrono::Local).single())
        .map(|d| d.timestamp_millis())
        .unwrap_or(now_ms - 86_400_000)
}

/// Tauri command：查询模型速度统计。窗口两模式（见模块头「查询窗口」）：
/// - days：缺省 7（近 7 天滚动）；0 = 今日（本地零点起）；无上界；
/// - from_ms + to_ms 同时给（自定义区间）：按半开区间 [from_ms, to_ms)
///   查询（含上界 started_at < to_ms，to 可在过去），忽略 days——窄窗
///   ModelSpeedPanel 只传 days 不受影响（区间参数可选）。
/// scope：None / "local" = 仅本机；"all" = 全部设备（并入远端其他设备的
/// 同步明细，见模块头「全部设备口径」）。遍历 7 个本地来源按
/// (source, provider, model_id) 分组聚合。多库 SQLite 查询 + scope=all 时
/// 的同步 HTTP 统一 async + spawn_blocking 卸载到阻塞线程池（与
/// request_logs 同款，不跑主线程）；单测直接构造同步实现。
#[tauri::command]
pub async fn get_model_speed(
    days: Option<i64>,
    scope: Option<String>,
    from_ms: Option<i64>,
    to_ms: Option<i64>,
) -> Result<ModelSpeedResponse, String> {
    tauri::async_runtime::spawn_blocking(move || {
        collect_model_speed(days, scope, from_ms, to_ms)
    })
    .await
    .map_err(|e| format!("模型速度统计任务失败: {e}"))?
}

/// scope 入参归一：`"all"` = 全部设备（并入远端其他设备明细）；None /
/// `"local"` / 未知值一律仅本机（缺省安全，与 request_logs::scope_all 同款）
fn scope_all(scope: Option<&str>) -> bool {
    scope == Some("all")
}

/// 远端一路的取数（scope = "all" 时调用）：同步未启用 / 未注册设备直接
/// Err（调用方记日志并置 remote_failed，本机数据照常，语义对齐
/// request_logs::fetch_remote_lane）。窗口与本机一致 [from_ms, to_ms]、
/// 无模型筛选、limit 夹 5000（服务端同款上限，见 REMOTE_FETCH_LIMIT）、
/// exclude_device = 本机（本机记录本地直读、两集合不相交无需去重）。
fn fetch_remote_speed_records(
    from_ms: i64,
    to_ms: i64,
) -> Result<Vec<crate::sync::RemoteUsageRecord>, String> {
    let cfg = crate::sync::load_sync_config()?;
    if !cfg.enabled || cfg.device_token.is_empty() || cfg.device_id.is_empty() {
        return Err("同步未启用或未注册设备".into());
    }
    let resp = crate::sync::fetch_remote_usage_records(crate::sync::RemoteUsageRecordsRequest {
        from_ms,
        to_ms,
        source: String::new(),
        model_id: String::new(),
        exclude_device: cfg.device_id.clone(),
        devices: String::new(),
        limit: REMOTE_FETCH_LIMIT,
        offset: 0,
    })?;
    Ok(resp.rows)
}

/// 遍历全部来源聚合并排序（可测接缝）：
/// - `remote`：scope = "all" 时为 Some（远端取数结果，含 Err 形态——错误
///   在此统一记日志并置 remote_failed，不阻塞本机数据）；本机模式 None；
/// - `local_fetch(source, remote_rows)`：单源「打开库 + 聚合」，remote_rows
///   是已按真实 source 分桶的远端行（未知来源已丢弃并记日志）。
/// 单源失败只记 eprintln! 并跳过该源（对齐 request_logs 容错风格；测试用
/// 假 fetch 构造失败源验证）。返回（统计分组, 远端是否失败）。
fn collect_all_speeds<F>(
    remote: Option<Result<Vec<crate::sync::RemoteUsageRecord>, String>>,
    local_fetch: F,
) -> (Vec<ModelSpeedStat>, bool)
where
    F: Fn(&str, &[crate::sync::RemoteUsageRecord]) -> Result<Vec<ModelSpeedStat>, String>,
{
    // 远端一路：失败不阻塞本机数据，只置 remote_failed 交前端弱提示
    let (remote_rows, remote_failed) = match remote {
        Some(Ok(rows)) => (rows, false),
        Some(Err(e)) => {
            eprintln!("[zbar-model-speed] 远端明细查询失败，已跳过: {e}");
            (Vec::new(), true)
        }
        None => (Vec::new(), false),
    };
    // 未知来源（不在 SOURCES 集合里，如 cursor）丢弃并记日志——本页无该
    // 来源路（与 request_logs 的远端分桶同款边界）
    let unknown = remote_rows
        .iter()
        .filter(|row| !crate::request_logs::SOURCES.contains(&row.source.as_str()))
        .count();
    if unknown > 0 {
        eprintln!("[zbar-model-speed] 丢弃 {unknown} 条未知来源的远端明细（不在速度页来源集合）");
    }
    let mut out: Vec<ModelSpeedStat> = Vec::new();
    for source in crate::request_logs::SOURCES {
        // 远端行按真实 source 分桶进本次来源集合
        let rows: Vec<crate::sync::RemoteUsageRecord> = remote_rows
            .iter()
            .filter(|row| row.source == source)
            .cloned()
            .collect();
        match local_fetch(source, &rows) {
            Ok(mut stats) => out.append(&mut stats),
            Err(e) => eprintln!("[zbar-model-speed] 来源 {source} 速度统计失败，已跳过: {e}"),
        }
    }
    // 全局排序：请求数降序（高频分组在前），同数按（来源, 模型, 渠道）稳定排序
    out.sort_by(|a, b| {
        b.requests
            .cmp(&a.requests)
            .then_with(|| a.source.cmp(&b.source))
            .then_with(|| a.model_id.cmp(&b.model_id))
            .then_with(|| a.provider.cmp(&b.provider))
    });
    (out, remote_failed)
}

/// 单源连接解析：
/// - `zcode`：主库**路径解析失败**（未安装 ZCode——`db::db_path` 只在文件
///   存在时返回 Ok，见 db.rs）降级 `Ok(None)`，与导入库分支「文件不存在
///   → Ok(None)」语义一致——全部设备模式下已分桶的远端 zcode 行照常经
///   空内存库并入，不因本机未装 ZCode 而丢弃（P2-1）；文件存在但打开
///   失败（损坏 / 锁）仍按 Err 上报，由调用方记日志跳过该源；
/// - 其余来源：复用 `request_logs::open_source`（文件不存在 → Ok(None)）。
fn open_speed_source(source: &str) -> Result<Option<Connection>, String> {
    if source == "zcode" {
        return match crate::db::db_path() {
            Ok(_) => crate::zcode_sessions::open_main_db_readonly_uri().map(Some),
            Err(_) => Ok(None),
        };
    }
    crate::request_logs::open_source(source)
}

/// 命令的同步实现：窗口两模式（见模块头「查询窗口」）——自定义区间
/// （from/to 同时给，含上界）优先，否则 days 滚动窗口 / 今日零点（无上界）。
/// 算出窗口后逐源「打开 + 聚合」。本地库不存在的来源在无远端行时按 0 条
/// 跳过；有远端行时用空内存库走同一条聚合管线（无本地表即 0 本地行，只吃
/// 远端行）。
fn collect_model_speed(
    days: Option<i64>,
    scope: Option<String>,
    from_ms: Option<i64>,
    to_ms: Option<i64>,
) -> Result<ModelSpeedResponse, String> {
    let now_ms = chrono::Utc::now().timestamp_millis();
    let (win_from, win_to) = match (from_ms, to_ms) {
        (Some(from), Some(to)) => (from, Some(to)),
        _ => (range_start_ms(days.unwrap_or(7).max(0), now_ms), None),
    };
    let all = scope_all(scope.as_deref());
    // 远端取数只在 scope="all" 时发生（惰性：本机模式不触网络）；自定义
    // 模式 to 传自定义上界（可在过去），days 模式维持 now
    let remote = if all {
        Some(fetch_remote_speed_records(win_from, win_to.unwrap_or(now_ms)))
    } else {
        None
    };
    let (stats, remote_failed) = collect_all_speeds(remote, |source, rows| {
        match open_speed_source(source)? {
            Some(conn) => {
                collect_source_speed_with_remote(&conn, source, win_from, win_to, now_ms, rows)
            }
            None if rows.is_empty() => Ok(Vec::new()),
            None => {
                // 本地库不存在（未安装 / 未用过）但其他设备有该来源的同步
                // 行：空内存库无 model_usage 表 → 0 本地行，只聚合远端行
                let mem = Connection::open_in_memory()
                    .map_err(|e| format!("打开内存库失败: {e}"))?;
                collect_source_speed_with_remote(&mem, source, win_from, win_to, now_ms, rows)
            }
        }
    });
    Ok(ModelSpeedResponse { stats, remote_failed })
}

/// 单组聚合中间态（分组键 = (model_id, provider_raw)，见模块头——同名模型
/// 跨渠道各自独立成组，键本身携带渠道，不再需要行内记 provider）；
/// 窗口内逐行累加，出口组装 DTO
#[derive(Default)]
struct ModelAgg {
    requests: i64,
    /// 本机行数（成功率的分母口径：远端行无 status 列不进成功率统计，
    /// 分子分母都只算本机行，避免远端行把成功率稀释失真）
    local_requests: i64,
    completed: i64,
    /// 速度样本两池（口径分层，见模块头）：精确生成池 = (output_tokens,
    /// 生成毫秒)，zcode 事件时刻行专用；组装时非空则速度统计只用本池
    generation_pairs: Vec<(f64, f64)>,
    /// 请求平均近似池 = (output_tokens, duration_ms)，导入库与远端行共用；
    /// 仅当 generation 池为空时才参与速度统计
    average_pairs: Vec<(f64, f64)>,
    /// 首 token 延迟样本（毫秒）
    ttfts: Vec<i64>,
    /// 请求耗时样本（毫秒）
    durs: Vec<i64>,
    in_sum: i64,
    in_max: i64,
    out_sum: i64,
    out_max: i64,
}

/// 分位数（最近邻取整：idx = round(p/100 × (n−1))，升序切片）。空集
/// 返回 None。
fn percentile(sorted: &[f64], p: f64) -> Option<f64> {
    let n = sorted.len();
    if n == 0 {
        return None;
    }
    if n == 1 {
        return Some(sorted[0]);
    }
    let idx = ((p / 100.0) * (n as f64 - 1.0)).round() as usize;
    Some(sorted[idx.min(n - 1)])
}

/// i64 均值（四舍五入；空集 None）
fn avg_i64(vals: &[i64]) -> Option<i64> {
    if vals.is_empty() {
        return None;
    }
    let sum: i64 = vals.iter().sum();
    let n = vals.len() as i64;
    Some((sum + n / 2) / n)
}

/// provider_id 简名：只取 "account:" 后部分（无前缀原样），**不做长度
/// 截断**——截断职责移交前端 CSS（窄窗渠道位 max-width + ellipsis、大窗
/// 渠道列同款，两处 title 都挂完整值，后端截断纯多余）。2026-10-24 教训：
/// 这里曾按「>20 字符取前 18 + "…"」截断，真实 slug
/// `bigmodel-individual-coding-plan`（30 字符）被截成 `bigmodel-individua…`，
/// 前端渠道友好名映射的 startsWith 在第 19 字符处撞上 '…' 永远失配——
/// 后端截断破坏了前缀可匹配性，是映射死锁的元凶，故整体移除。
fn provider_short(raw: &str) -> String {
    raw.split_once("account:")
        .map(|(_, rest)| rest)
        .unwrap_or(raw)
        .to_string()
}

/// 单源聚合（本机库行为不变的可测入口）：collect_source_speed_with_remote
/// 的 to_ms 传 None + remote_rows 传空包装——本机模式每源天然只有一池，
/// 行为与既有版本一致（生产路径统一走 with_remote，空 remote_rows 即本机
/// 形态；此包装仅供既有单测保持原签名）
#[cfg(test)]
pub(crate) fn collect_source_speed(
    conn: &Connection,
    source: &str,
    from_ms: i64,
    observed_at_ms: i64,
) -> Result<Vec<ModelSpeedStat>, String> {
    collect_source_speed_with_remote(conn, source, from_ms, None, observed_at_ms, &[])
}

/// 单源纯聚合逻辑（不依赖真实 ~/.zcode，供单元测试）：读该来源窗口内
/// model_usage 行 + 并入该来源的远端明细行（scope = "all"，见模块头「全部
/// 设备口径」），按 (model_id, provider_id) 分组组装统计（来源名写进 DTO
/// 的 source 字段，跨源合并在 collect_all_speeds）。窗口两模式（见模块头
/// 「查询窗口」）：to_ms = Some 时自定义区间上界（半开含上界），None 时
/// days 模式无上界。老版本库核心列缺失时本地行按 0 条降级（远端行照常
/// 并入——本地未安装的来源在全部设备模式下仍有数据）；非核心列缺失按
/// NULL 降级（对应统计为 null）。速度口径按池分层（见模块头）：zcode 走
/// token_speed::speed_sample 共享行级判定（B1，缺事件时刻的行不产
/// 速度样本、不降级 duration_ms）；导入库与远端行走 request_speed_at
/// 请求平均近似（duration_ms > 0 才有样本，codex 本地无该列恒无本地样本）。
pub(crate) fn collect_source_speed_with_remote(
    conn: &Connection,
    source: &str,
    from_ms: i64,
    to_ms: Option<i64>,
    observed_at_ms: i64,
    remote_rows: &[crate::sync::RemoteUsageRecord],
) -> Result<Vec<ModelSpeedStat>, String> {
    let mut groups: BTreeMap<(String, String), ModelAgg> = BTreeMap::new();
    let is_zcode = source == "zcode";
    // 本地行可读性：表/核心列缺失（老版本库 / 本地未安装的空内存库）时
    // 本地行按 0 条降级，不阻断远端行并入
    let local_readable = has_table(conn, "model_usage")
        && crate::db::has_column(conn, "model_usage", "model_id")
        && crate::db::has_column(conn, "model_usage", "started_at");
    // status 列存在性（本地行循环与出口组装共用；本地不可读 = 无 status =
    // 成功率 None）
    let has_status_col = local_readable && crate::db::has_column(conn, "model_usage", "status");
    if local_readable {
        // 列探测降级（与 usage_feed/session_hud 同款防御模式）
        let opt = |col: &str| {
            if crate::db::has_column(conn, "model_usage", col) {
                col.to_string()
            } else {
                "NULL".to_string()
            }
        };
        let num = |col: &str| {
            if crate::db::has_column(conn, "model_usage", col) {
                format!("COALESCE({col}, 0)")
            } else {
                "0".to_string()
            }
        };
        let (provider, status, first, completed, inp, out) = (
            opt("provider_id"),
            opt("status"),
            opt("first_token_at"),
            opt("completed_at"),
            num("input_tokens"),
            num("output_tokens"),
        );
        // TTFT 列（优先口径）与思考感知信号列：缺列（老版本库）按 NULL/0 降级
        let ttft = opt("time_to_first_token_ms");
        let reasoning = num("reasoning_tokens");
        // 导入库的近似速度窗口原料（zcode 主库也可能有该列但速度不使用——
        // 精确口径只认事件时刻；codex 无该列按 NULL → 恒无速度样本）
        let duration = opt("duration_ms");
        // 自定义区间上界（半开区间含上界 started_at < to；days 模式无上界
        // 保持既有行为——未来时刻由 zcode 的 observed 拒绝兜底）
        let upper_bound = if to_ms.is_some() {
            " AND started_at < ?2"
        } else {
            ""
        };
        let sql = format!(
            "SELECT COALESCE(model_id, ''), COALESCE({provider}, ''), {status}, \
                    started_at, {first}, {completed}, {inp}, {out}, {ttft}, {reasoning}, {duration} \
             FROM model_usage \
             WHERE started_at >= ?1 AND model_id IS NOT NULL AND model_id != ''{upper_bound}"
        );
        // 绑定参数随模式伸缩（days 模式 1 个、自定义模式 2 个），
        // params_from_iter 统一传递
        let mut binds: Vec<i64> = vec![from_ms];
        if let Some(to) = to_ms {
            binds.push(to);
        }
        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| format!("准备模型速度查询失败: {e}"))?;
        let rows = stmt
            .query_map(rusqlite::params_from_iter(binds), |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, Option<i64>>(4)?,
                    row.get::<_, Option<i64>>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, i64>(7)?,
                    // TTFT 与耗时按 f64 读入（各来源列类型 INTEGER/REAL 混杂：
                    // dsh/claude/opencode/minimaxcode 的 duration_ms 为 REAL，
                    // rusqlite 的 i64 读取对 REAL 列会报错，f64 两种都能收，
                    // 同 request_logs 的 row_to_log 口径），使用处再折整毫秒
                    row.get::<_, Option<f64>>(8)?,
                    row.get::<_, i64>(9)?,
                    row.get::<_, Option<f64>>(10)?,
                ))
            })
            .map_err(|e| format!("读取模型速度行失败: {e}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("读取模型速度行失败: {e}"))?;

        let has_status_col = status != "NULL";
        // zcode 维持既有的观测时刻拒绝未来样本；导入库是已落库的历史行，
        // observed 传 0 不做未来拒绝（与 request_logs 的 row_to_log 同款）
        let observed = if is_zcode { observed_at_ms } else { 0 };
        for (model, provider, status, started, first, completed, input, output, ttft, reasoning, duration) in
            rows
        {
            if model.is_empty() {
                continue;
            }
            // claude 系按名识别需要模型名，先于 entry 所有权转移捕获
            let model_id = Some(model.clone());
            // 分组键 = (model_id, provider_raw)：同名模型跨渠道各自独立成组
            // （跨来源合并在 collect_all_speeds，source 字段天然隔离）
            let g = groups.entry((model, provider)).or_default();
            g.requests += 1;
            // 本机行计数（成功率分母只算本机行，见 ModelAgg::local_requests）
            g.local_requests += 1;
            // 完成计数：与速度样本共用同一判定（token_speed::is_completed_status
            // 接受空串/success/completed/缺状态为完成）。原先只认 "completed"，
            // 导致 status="success"/"" 的行既参与速度样本（共享判定通过）又被
            // 成功率分子排除——参与速度的行反而拉低成功率的口径分裂不再存在。
            if has_status_col && crate::token_speed::is_completed_status(status.as_deref()) {
                g.completed += 1;
            } else if !has_status_col {
                // 老库无 status 列：model_usage 完成即落行，全部视作完成
                // （成功率无意义置 None，但 completed 计数仍用于一致性）
                g.completed += 1;
            }
            // 输入/输出 token：全部行参与（原值口径）
            g.in_sum += input.max(0);
            g.in_max = g.in_max.max(input);
            g.out_sum += output.max(0);
            g.out_max = g.out_max.max(output);
            // 首 token 延迟：优先 time_to_first_token_ms 列（正数才可信——
            // 零/负值是未上报或异常计时）；回退 first_token_at − started_at 仅
            // zcode（导入库无该列、回退天然不可能，显式限定来源防止未来导入库
            // 补列后口径悄悄漂移；时序倒置/NULL 跳过）
            let ttft_value = ttft
                .filter(|value| *value > 0.0)
                .map(|value| value as i64)
                .or_else(|| {
                    if is_zcode {
                        first
                            .filter(|value| started > 0 && *value >= started)
                            .map(|value| value - started)
                    } else {
                        None
                    }
                });
            if let Some(value) = ttft_value {
                g.ttfts.push(value);
            }
            // 真实耗时（导入库形态，> 0 才可信；NULL / 0 跳过不做估算回退）
            let duration_ms = duration
                .filter(|value| *value > 0.0)
                .map(|value| value as i64);
            // 请求耗时样本：zcode 用 completed − started（既有口径）；导入库
            // 用真实 duration_ms
            if is_zcode {
                if let (Some(c), true) = (completed, started > 0) {
                    if c > started {
                        g.durs.push(c - started);
                    }
                }
            } else if let Some(d) = duration_ms {
                g.durs.push(d);
            }
            // Each sample is one model request. Never use turn/tool wall time.
            if is_zcode {
                if !has_status_col || crate::token_speed::is_completed_status(status.as_deref()) {
                    let timing = crate::token_speed::RequestTiming {
                        output_tokens: output, started_at: Some(started),
                        first_token_at: first, completed_at: completed,
                        reasoning_tokens: (reasoning > 0).then_some(reasoning), model_id,
                        ..Default::default()
                    };
                    if let Some(sample) = crate::token_speed::speed_sample(&timing, observed) {
                        let pool = if sample.quality == SpeedQuality::Generation { &mut g.generation_pairs } else { &mut g.average_pairs };
                        pool.push((sample.output_tokens as f64, sample.generation_ms as f64));
                    }
                }
            } else if let Some(d) = duration_ms {
                let quality = crate::token_speed::duration_quality(source);
                if crate::token_speed::duration_speed(output, d as f64, quality, started).is_some() {
                    let pool = if quality == SpeedQuality::Generation { &mut g.generation_pairs } else { &mut g.average_pairs };
                    pool.push((output as f64, d as f64));
                }
            }
        }
    }
    // 远端行并入（scope = "all" 的第 8 路，见 merge_remote_speed_rows）
    merge_remote_speed_rows(source, remote_rows, from_ms, to_ms, &mut groups);

    let mut out: Vec<ModelSpeedStat> = Vec::with_capacity(groups.len());
    for ((model, provider_raw), g) in groups {
        // 口径分层（见模块头）：generation 池非空 → 速度统计只用精确池
        // （远端 / 导入库近似样本绝不掺入——同一分组两种口径混算 P50 会
        // 失真）；generation 空 → 退回 average 池；两池皆空 → 全部速度
        // 字段 null、口径 null
        let (pairs, speed_quality): (&[(f64, f64)], Option<SpeedQuality>) =
            if !g.generation_pairs.is_empty() {
                (&g.generation_pairs, Some(SpeedQuality::Generation))
            } else if !g.average_pairs.is_empty() {
                (&g.average_pairs, Some(SpeedQuality::RequestAverage))
            } else {
                (&[], None)
            };
        // 速度：均值 = Σout ÷ Σ窗口毫秒 × 1000；分位 = 每请求速度
        let (avg_tps, p10, p50, p90) = if pairs.is_empty() {
            (None, None, None, None)
        } else {
            let sum_out: f64 = pairs.iter().map(|(o, _)| *o).sum();
            let sum_ms: f64 = pairs.iter().map(|(_, m)| *m).sum();
            let avg = if sum_ms > 0.0 {
                Some(sum_out * 1000.0 / sum_ms)
            } else {
                None
            };
            let mut speeds: Vec<f64> = pairs.iter().map(|(o, m)| o * 1000.0 / *m).collect();
            speeds.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            (
                avg,
                percentile(&speeds, 10.0),
                percentile(&speeds, 50.0),
                percentile(&speeds, 90.0),
            )
        };
        // 首 token / 耗时分位（毫秒，升序后取分位再截断）
        let mut ttfts = g.ttfts.clone();
        ttfts.sort_unstable();
        let mut durs = g.durs.clone();
        durs.sort_unstable();
        let ttft_f64: Vec<f64> = ttfts.iter().map(|&v| v as f64).collect();
        let dur_f64: Vec<f64> = durs.iter().map(|&v| v as f64).collect();
        // 成功率：分子分母都只算本机行（远端行无 status 列，进了分母会把
        // 本机成功率稀释失真——全部设备模式下仍代表本机口径）
        let success_rate = if has_status_col && g.local_requests > 0 {
            Some(g.completed as f64 / g.local_requests as f64)
        } else {
            None
        };
        let req_n = g.requests.max(1);
        out.push(ModelSpeedStat {
            model_id: model,
            source: source.to_string(),
            // zcode 渠道剥 "account:" 前缀（完整值不截断，前端 CSS 负责收
            // 尾）；导入库渠道原样（opencode 等渠道名本身是用户可读的
            // 语义，截断丢信息）
            provider: if is_zcode {
                provider_short(&provider_raw)
            } else {
                provider_raw
            },
            requests: g.requests,
            local_requests: g.local_requests,
            speed_sample_count: (!pairs.is_empty()).then_some(pairs.len() as i64),
            speed_quality,
            success_rate,
            avg_tps,
            p10_tps: p10,
            p50_tps: p50,
            p90_tps: p90,
            ttft_min_ms: ttfts.first().copied(),
            ttft_avg_ms: avg_i64(&ttfts),
            ttft_p90_ms: percentile(&ttft_f64, 90.0).map(|v| v as i64),
            dur_avg_ms: avg_i64(&durs),
            dur_p90_ms: percentile(&dur_f64, 90.0).map(|v| v as i64),
            in_avg: (g.in_sum + req_n / 2) / req_n,
            in_max: g.in_max,
            in_total: g.in_sum,
            out_avg: (g.out_sum + req_n / 2) / req_n,
            out_max: g.out_max,
            out_total: g.out_sum,
        });
    }
    // 组内排序：请求数降序（高频分组在前），同数按（模型名, 渠道）稳定排序
    // （跨源合并在 collect_all_speeds 再做带 source 的全局排序）
    out.sort_by(|a, b| {
        b.requests
            .cmp(&a.requests)
            .then_with(|| a.model_id.cmp(&b.model_id))
            .then_with(|| a.provider.cmp(&b.provider))
    });
    Ok(out)
}

/// 远端明细行并入单源分组（scope = "all" 的第 8 路，见模块头「全部设备
/// 口径」）。调用方已按真实 source 分桶（未知来源在 collect_all_speeds
/// 丢弃），这里再防御性卡一遍 source 与窗口（自定义模式含上界，与本地
/// 行同一窗口口径）：
/// - 行进 requests / 输入输出 token / TTFT / 耗时 / average 速度池；
/// - **不进 completed / 成功率统计**（远端行无 status 列，分子分母都只算
///   本机行，避免远端行把本机成功率稀释失真）；
/// - 速度原料只有 duration_ms（first_token_at / completed_at 永不上传）→
///   只可能点亮 average 池，与导入库同款构造 RequestTiming 经
///   request_speed_at(timing, 0) 判定（窗口边界 / 零输出校验在判定内）；
///   旧服务端（proto 8 前）duration_ms / time_to_first_token_ms 恒 None
///   → 不产速度样本、不计耗时，requests / token 正常；
/// - started_at 在窗口外（服务端已过滤，防御性再卡）或模型名为空丢弃。
fn merge_remote_speed_rows(
    source: &str,
    rows: &[crate::sync::RemoteUsageRecord],
    from_ms: i64,
    to_ms: Option<i64>,
    groups: &mut BTreeMap<(String, String), ModelAgg>,
) {
    for row in rows {
        if row.source != source || row.model_id.is_empty() || row.started_at < from_ms {
            continue;
        }
        // 自定义区间上界（半开含上界 started_at < to，与本地行同口径）
        if to_ms.is_some_and(|to| row.started_at >= to) {
            continue;
        }
        let input = row.input_tokens.max(0);
        let output = row.output_tokens.max(0);
        let g = groups
            .entry((row.model_id.clone(), row.provider_id.clone()))
            .or_default();
        g.requests += 1;
        // 输入/输出 token：与本地行同口径（原值，out_max 参与展示峰值）
        g.in_sum += input;
        g.in_max = g.in_max.max(input);
        g.out_sum += output;
        g.out_max = g.out_max.max(output);
        // TTFT：服务端回传的真实列值（正数才可信；旧服务端 None）
        if let Some(ttft) = row.time_to_first_token_ms.filter(|value| *value > 0.0) {
            g.ttfts.push(ttft as i64);
        }
        // 耗时 + average 速度池：与导入库同款（> 0 才可信；[200ms, 1h]
        // 窗口边界与零输出校验在 request_speed_at 内部，通过后分子分母
        // 与判定严格同源）
        if let Some(d) = row.duration_ms.filter(|value| *value > 0.0) {
            let d = d as i64;
            g.durs.push(d);
            let quality = crate::token_speed::duration_quality(source);
            if crate::token_speed::duration_speed(row.output_tokens, d as f64, quality, row.started_at).is_some() {
                let pool = if quality == SpeedQuality::Generation { &mut g.generation_pairs } else { &mut g.average_pairs };
                pool.push((row.output_tokens as f64, d as f64));
            }
        }
    }
}

/// 表存在性探测（模块自持一份，与 usage_feed/session_hud 同款实现）
fn has_table(conn: &Connection, table: &str) -> bool {
    conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
        [table],
        |row| row.get::<_, i64>(0),
    )
    .map(|c| c > 0)
    .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 速度统计测试库（model_usage 按实测 schema 最小化）
    fn speed_db(name: &str) -> (Connection, std::path::PathBuf) {
        let path = std::env::temp_dir().join(format!(
            "zbar-model-speed-db-{}-{name}.sqlite",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE model_usage (
                session_id TEXT, turn_id TEXT, started_at INTEGER,
                first_token_at INTEGER, completed_at INTEGER, model_id TEXT,
                provider_id TEXT, status TEXT,
                input_tokens INTEGER, output_tokens INTEGER);",
        )
        .unwrap();
        (conn, path)
    }

    /// 导入库形态测试库（duration_ms REAL 有值、无 first_token_at /
    /// completed_at / status——claude / kimi / dsh / opencode / minimaxcode
    /// 的列形态；with_duration = false 时连 duration_ms 也不建 = codex 形态，
    /// 无任何耗时原料）
    fn imported_speed_db(name: &str, with_duration: bool) -> (Connection, std::path::PathBuf) {
        let path = std::env::temp_dir().join(format!(
            "zbar-model-speed-import-{}-{name}.sqlite",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        let conn = Connection::open(&path).unwrap();
        let duration_col = if with_duration {
            ", duration_ms REAL"
        } else {
            ""
        };
        conn.execute_batch(&format!(
            "CREATE TABLE model_usage (
                started_at INTEGER, model_id TEXT, provider_id TEXT,
                input_tokens INTEGER, output_tokens INTEGER,
                reasoning_tokens INTEGER{duration_col});",
        ))
        .unwrap();
        (conn, path)
    }

    /// 远端明细行测试构造（RemoteUsageRecord 字段多，只填速度页关心的；
    /// input 固定 100，ttft 走独立参数）
    fn remote_record(
        source: &str,
        model: &str,
        provider: &str,
        started_at: i64,
        output: i64,
        duration_ms: Option<f64>,
        ttft_ms: Option<f64>,
    ) -> crate::sync::RemoteUsageRecord {
        crate::sync::RemoteUsageRecord {
            device_id: "dev-x".into(),
            source: source.into(),
            local_rowid: 0,
            started_at,
            model_id: model.into(),
            provider_id: provider.into(),
            input_tokens: 100,
            output_tokens: output,
            cache_read_input_tokens: 0,
            cache_creation_input_tokens: 0,
            reasoning_tokens: 0,
            computed_total_tokens: 0,
            session_id: None,
            project_key: None,
            project_display: None,
            duration_ms,
            time_to_first_token_ms: ttft_ms,
        }
    }

    #[test]
    fn 分位数_最近邻与边界() {
        assert_eq!(percentile(&[], 50.0), None);
        assert_eq!(percentile(&[7.0], 10.0), Some(7.0));
        // 4 样本 [10,20,30,40]：P10 → round(0.1×3)=0 → 10；P50 →
        // round(1.5)=2 → 30；P90 → round(2.7)=3 → 40
        let v = [10.0, 20.0, 30.0, 40.0];
        assert_eq!(percentile(&v, 10.0), Some(10.0));
        assert_eq!(percentile(&v, 50.0), Some(30.0));
        assert_eq!(percentile(&v, 90.0), Some(40.0));
        assert_eq!(percentile(&v, 0.0), Some(10.0));
        assert_eq!(percentile(&v, 100.0), Some(40.0));
        // 五分位经典值：[1..=5] 的 P50 = 3
        assert_eq!(percentile(&[1.0, 2.0, 3.0, 4.0, 5.0], 50.0), Some(3.0));
    }

    #[test]
    fn 均值_四舍五入() {
        assert_eq!(avg_i64(&[]), None);
        assert_eq!(avg_i64(&[10]), Some(10));
        assert_eq!(avg_i64(&[10, 15]), Some(13), "12.5 四舍五入 13");
        assert_eq!(avg_i64(&[-10, -15]), Some(-12), "-12.5 向上取整 -12");
    }

    #[test]
    fn provider简名_account前缀剥离不截断() {
        assert_eq!(provider_short("account:bigmodel-individual"), "bigmodel-individual");
        assert_eq!(provider_short("doubao"), "doubao");
        // 长 slug 不截断（完整值返回）：截断职责在前端 CSS——后端截断曾把
        // bigmodel-individual-coding-plan 截成 bigmodel-individua…，破坏前端
        // 友好名映射的前缀可匹配性（2026-10-24 映射死锁教训，见
        // provider_short 注释）
        assert_eq!(
            provider_short("account:bigmodel-individual-coding-plan"),
            "bigmodel-individual-coding-plan"
        );
        assert_eq!(provider_short("opencode-go"), "opencode-go");
        // 无前缀超长值原样返回
        assert_eq!(provider_short(&"a".repeat(64)), "a".repeat(64));
    }

    #[test]
    fn 聚合_速度分位与token量级与成功率() {
        let (conn, path) = speed_db("agg");
        let t0 = 1_000_000_000_i64;
        conn.execute_batch(&format!(
            "INSERT INTO model_usage (session_id, turn_id, started_at, first_token_at,
                completed_at, model_id, provider_id, status, input_tokens, output_tokens) VALUES
              -- A 模型：两笔可信速度样本（1000ms 生成 100tok → 100 t/s；
              -- 500ms 生成 50tok → 100 t/s）+ 一笔 error 行（计入请求与
              -- token，不计速度）
              ('s1', 't1', {t0}, {f1}, {c1}, 'GLM-5.3', 'account:bigmodel-individual', 'completed', 1000, 100),
              ('s1', 't2', {t2}, {f2}, {c2}, 'GLM-5.3', 'account:bigmodel-individual', 'completed', 800, 50),
              ('s1', 't3', {t3}, {f3}, {c3}, 'GLM-5.3', 'account:bigmodel-individual', 'error', 600, 0),
              -- B 模型：窗口外行（不参与）
              ('s1', 't9', {old}, {of}, {oc}, 'GLM-5.3', 'account:x', 'completed', 999, 999),
              -- C 模型：turn_id NULL 的后台请求（不过滤：计入全部统计）
              ('s1', NULL, {t4}, {f4}, {c4}, 'GLM-4.7', 'doubao', 'completed', 200, 30);",
            t0 = t0,
            f1 = t0 + 500,
            c1 = t0 + 1500,
            t2 = t0 + 10_000,
            f2 = t0 + 10_200,
            c2 = t0 + 10_700,
            t3 = t0 + 20_000,
            f3 = t0 + 20_100,
            c3 = t0 + 20_200,
            old = t0 - 100_000,
            of = t0 - 99_000,
            oc = t0 - 98_000,
            t4 = t0 + 30_000,
            f4 = t0 + 30_100,
            c4 = t0 + 30_500,
        ))
        .unwrap();
        let stats = collect_source_speed(&conn, "zcode", t0, t0 + 1_000_000).unwrap();
        assert_eq!(stats.len(), 2, "{stats:?}");
        // 排序：A（3 请求）在前，C（1 请求）在后
        let a = &stats[0];
        assert_eq!(a.model_id, "GLM-5.3");
        assert_eq!(a.source, "zcode");
        assert_eq!(a.provider, "bigmodel-individual", "provider 应取 account: 后简名");
        assert_eq!(a.requests, 3);
        // zcode 组级口径 = generation（精确生成窗口）；serde 值锁前端契约
        assert_eq!(a.speed_quality, Some(SpeedQuality::Generation));
        let json = serde_json::to_value(a).unwrap();
        assert_eq!(json["source"], "zcode");
        assert_eq!(json["speedQuality"], "generation");
        // 速度合格样本数：2（error 行不计），与总请求数 3 口径不同
        assert_eq!(a.speed_sample_count, Some(2));
        // 成功率 2/3
        assert!((a.success_rate.unwrap() - 2.0 / 3.0).abs() < 1e-9);
        // 平均速度 = (100+50)×1000 ÷ (1000+500) = 100.0；P10/P50/P90 均 100
        assert!((a.avg_tps.unwrap() - 100.0).abs() < 1e-6);
        assert!((a.p10_tps.unwrap() - 100.0).abs() < 1e-6);
        assert!((a.p50_tps.unwrap() - 100.0).abs() < 1e-6);
        assert!((a.p90_tps.unwrap() - 100.0).abs() < 1e-6);
        // TTFT：500/200/100 → min 100、avg 267、P90 500
        assert_eq!(a.ttft_min_ms, Some(100));
        assert_eq!(a.ttft_avg_ms, Some(267));
        assert_eq!(a.ttft_p90_ms, Some(500));
        // 耗时（completed−started，不分状态——error 行的耗时同样有效）：
        // 1500/700/200 → avg 800、P90 1500
        assert_eq!(a.dur_avg_ms, Some(800));
        assert_eq!(a.dur_p90_ms, Some(1500));
        // token：in avg = (1000+800+600+0)/3 → 800（error 行 input 600 计入）
        assert_eq!(a.in_avg, 800);
        assert_eq!(a.in_max, 1000);
        assert_eq!(a.out_avg, 50);
        assert_eq!(a.out_max, 100);
        // C 模型（NULL turn_id 后台请求不过滤）：TTFT 100、生成 400ms
        // 30 tok → 75 t/s
        let c = &stats[1];
        assert_eq!(c.model_id, "GLM-4.7");
        assert_eq!(c.provider, "doubao");
        assert_eq!(c.requests, 1);
        assert_eq!(c.speed_sample_count, Some(1));
        assert!((c.avg_tps.unwrap() - 75.0).abs() < 1e-6);
        assert_eq!(c.ttft_min_ms, Some(100));
        drop(conn);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn 聚合_异常行防御与降级() {
        let (conn, path) = speed_db("dirty");
        let t0 = 2_000_000_000_i64;
        conn.execute_batch(&format!(
            "INSERT INTO model_usage (session_id, turn_id, started_at, first_token_at,
                completed_at, model_id, provider_id, status, input_tokens, output_tokens) VALUES
              -- 时刻 NULL 行：跳过速度/TTFT/耗时，计入请求数与 token
              ('s1', 't1', {t0}, NULL, {c1}, 'M1', 'account:p', 'completed', 100, 10),
              -- 时序倒置（completed < first）：跳过速度；耗时（completed>started）仍有效
              ('s1', 't2', {t0}, {f2}, {c2}, 'M1', 'account:p', 'completed', 100, 10),
              -- output=0 的 completed 行：不产生速度样本（0 速度无意义），
              -- TTFT/耗时正常计入
              ('s1', 't3', {t0}, {f3}, {c3}, 'M1', 'account:p', 'completed', 100, 0);",
            t0 = t0,
            c1 = t0 + 900,
            f2 = t0 + 800,
            c2 = t0 + 500,
            f3 = t0 + 100,
            c3 = t0 + 300,
        ))
        .unwrap();
        let stats = collect_source_speed(&conn, "zcode", t0, t0 + 1_000_000).unwrap();
        assert_eq!(stats.len(), 1);
        let m = &stats[0];
        assert_eq!(m.requests, 3);
        assert_eq!(m.success_rate, Some(1.0));
        assert!((m.avg_tps.unwrap() - 10_000.0 / 900.0).abs() < 1e-9);
        assert_eq!(m.speed_sample_count, Some(1));
        assert_eq!(m.speed_quality, Some(SpeedQuality::RequestAverage));
        // TTFT：t2 行 first(800) ≥ started 但 completed<first 不影响 TTFT；
        // 样本 = [800, 100] → min 100、P90 800
        assert_eq!(m.ttft_min_ms, Some(100));
        assert_eq!(m.ttft_p90_ms, Some(800));
        // 耗时：t1 行 900（first NULL 不影响耗时）、t2 行 completed(t0+500)
        // > started 有效 500、t3 行 300 → 样本 [900, 500, 300] → avg 567、
        // P90 900（P90 最近邻取 idx=round(0.9×2)=2）
        assert_eq!(m.dur_avg_ms, Some(567));
        assert_eq!(m.dur_p90_ms, Some(900));
        drop(conn);
        let _ = std::fs::remove_file(&path);

        // 老版本库缺 status 列：成功率 null（无状态可判），速度按全完成
        // 降级参与
        let path2 = std::env::temp_dir().join(format!(
            "zbar-model-speed-db-{}-nostatus.sqlite",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path2);
        let conn = Connection::open(&path2).unwrap();
        conn.execute_batch(
            "CREATE TABLE model_usage (
                started_at INTEGER, first_token_at INTEGER, completed_at INTEGER,
                model_id TEXT, provider_id TEXT,
                input_tokens INTEGER, output_tokens INTEGER);",
        )
        .unwrap();
        conn.execute_batch(&format!(
            "INSERT INTO model_usage VALUES
               ({t0}, {f1}, {c1}, 'M1', 'account:p', 100, 100);",
            t0 = t0,
            f1 = t0 + 500,
            c1 = t0 + 1500,
        ))
        .unwrap();
        let stats = collect_source_speed(&conn, "zcode", t0, t0 + 1_000_000).unwrap();
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].success_rate, None, "缺 status 列成功率应为 null");
        assert_eq!(stats[0].speed_sample_count, Some(1));
        assert!((stats[0].avg_tps.unwrap() - 100.0).abs() < 1e-6);
        drop(conn);
        let _ = std::fs::remove_file(&path2);

        // 更老版本库缺 provider_id 列：整体不报错，其余统计正常，
        // 仅 provider 展示为空
        let path4 = std::env::temp_dir().join(format!(
            "zbar-model-speed-db-{}-noprovider.sqlite",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path4);
        let conn = Connection::open(&path4).unwrap();
        conn.execute_batch(
            "CREATE TABLE model_usage (
                started_at INTEGER, first_token_at INTEGER, completed_at INTEGER,
                model_id TEXT, status TEXT,
                input_tokens INTEGER, output_tokens INTEGER);",
        )
        .unwrap();
        conn.execute_batch(&format!(
            "INSERT INTO model_usage VALUES
               ({t0}, {f1}, {c1}, 'M1', 'completed', 100, 100);",
            t0 = t0,
            f1 = t0 + 500,
            c1 = t0 + 1500,
        ))
        .unwrap();
        let stats = collect_source_speed(&conn, "zcode", t0, t0 + 1_000_000).unwrap();
        assert_eq!(stats.len(), 1, "缺 provider_id 列不应整体失败");
        assert_eq!(stats[0].provider, "", "缺 provider_id 列 provider 应为空");
        assert_eq!(stats[0].requests, 1);
        assert_eq!(stats[0].success_rate, Some(1.0));
        assert!((stats[0].avg_tps.unwrap() - 100.0).abs() < 1e-6);
        drop(conn);
        let _ = std::fs::remove_file(&path4);

        // 无 model_usage 表（极端老库）→ 空数组不报错
        let path3 = std::env::temp_dir().join(format!(
            "zbar-model-speed-db-{}-empty.sqlite",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path3);
        let conn = Connection::open(&path3).unwrap();
        assert!(collect_source_speed(&conn, "zcode", 0, 1).unwrap().is_empty());
        drop(conn);
        let _ = std::fs::remove_file(&path3);
    }

    #[test]
    fn 聚合_共享行级判定补齐first起点与未来时刻拒绝() {
        // B1：排行原先只查 completed > first > 0；切到共享判定后，
        // first < started 与未来时刻的行不再产生速度样本。
        let (conn, path) = speed_db("shared-validity");
        let t0 = 3_000_000_000_i64;
        conn.execute_batch(&format!(
            "INSERT INTO model_usage (session_id, turn_id, started_at, first_token_at,
                completed_at, model_id, provider_id, status, input_tokens, output_tokens) VALUES
              -- first 早于 started：无速度样本（TTFT 为负同样不计）
              ('s1', 't1', {t0}, {f1}, {c1}, 'M1', 'account:p', 'completed', 100, 100),
              -- 未来时刻（晚于观测时刻 t0+10_000）：无速度样本
              ('s1', 't2', {t2}, {f2}, {c2}, 'M1', 'account:p', 'completed', 100, 300),
              -- 合法样本：100 tok / 1s → 100 t/s
              ('s1', 't3', {t3}, {f3}, {c3}, 'M1', 'account:p', 'completed', 100, 100);",
            t0 = t0,
            f1 = t0 - 100,
            c1 = t0 + 1_000,
            t2 = t0 + 20_000,
            f2 = t0 + 20_100,
            c2 = t0 + 21_100,
            t3 = t0 + 2_000,
            f3 = t0 + 2_100,
            c3 = t0 + 3_100,
        ))
        .unwrap();
        // 观测时刻 t0+10_000：未来行被拒，first<started 行被拒 → 仅 t3
        let stats = collect_source_speed(&conn, "zcode", t0, t0 + 10_000).unwrap();
        assert_eq!(stats.len(), 1);
        let m = &stats[0];
        assert_eq!(m.requests, 3);
        assert_eq!(m.success_rate, Some(1.0));
        assert_eq!(m.speed_sample_count, Some(1));
        assert!((m.avg_tps.unwrap() - 100.0).abs() < 1e-6);
        drop(conn);

        // 观测时刻移到未来之后：未来行恢复参与 → (100+300)×1000÷(1000+1000)
        // = 200 t/s；first < started 的行仍被拒
        let conn = Connection::open(&path).unwrap();
        let stats = collect_source_speed(&conn, "zcode", t0, t0 + 1_000_000).unwrap();
        let m = &stats[0];
        assert_eq!(m.speed_sample_count, Some(2));
        assert!((m.avg_tps.unwrap() - 200.0).abs() < 1e-6);
        drop(conn);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn 聚合_success与空状态按完成计入成功率() {
        // 完成判定统一回归：success/空状态行与 completed 行共用
        // token_speed::is_completed_status。此前成功率分子只认 "completed"，
        // 而速度样本用共享判定（接受 success/空状态）——同一行既参与速度
        // 又拉低成功率的口径分裂；修复后两口径一致。
        let (conn, path) = speed_db("success-status");
        let t0 = 4_000_000_000_i64;
        conn.execute_batch(&format!(
            "INSERT INTO model_usage (session_id, turn_id, started_at, first_token_at,
                completed_at, model_id, provider_id, status, input_tokens, output_tokens) VALUES
              -- success 状态：合格速度样本（100tok/1s），计入完成
              ('s1', 't1', {t0}, {f1}, {c1}, 'M1', 'account:p', 'success', 100, 100),
              -- 空状态：合格速度样本（100tok/500ms），计入完成
              ('s1', 't2', {t2}, {f2}, {c2}, 'M1', 'account:p', '', 100, 100),
              -- error 状态：不计入完成、output=0 也无速度样本
              ('s1', 't3', {t3}, {f3}, {c3}, 'M1', 'account:p', 'error', 100, 0);",
            t0 = t0,
            f1 = t0 + 500,
            c1 = t0 + 1500,
            t2 = t0 + 10_000,
            f2 = t0 + 10_200,
            c2 = t0 + 10_700,
            t3 = t0 + 20_000,
            f3 = t0 + 20_100,
            c3 = t0 + 20_200,
        ))
        .unwrap();
        let stats = collect_source_speed(&conn, "zcode", t0, t0 + 1_000_000).unwrap();
        assert_eq!(stats.len(), 1, "{stats:?}");
        let m = &stats[0];
        assert_eq!(m.requests, 3);
        // 成功率 = 2/3（success + 空状态计入完成，error 排除）
        assert!(
            (m.success_rate.unwrap() - 2.0 / 3.0).abs() < 1e-9,
            "{:?}",
            m.success_rate
        );
        // 速度样本同为 2 行（success + 空状态）：参与速度的行集与完成
        // 分子完全一致
        assert_eq!(m.speed_sample_count, Some(2));
        // 均速 = (100+100)×1000 ÷ (1000+500) ≈ 133.33
        assert!((m.avg_tps.unwrap() - 133.333).abs() < 0.01);
        drop(conn);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn 时间范围_今日零点与滚动窗口() {
        use chrono::Timelike as _;
        // 滚动窗口：now − N 天
        let now = 5_000_000_000_000_i64;
        assert_eq!(range_start_ms(7, now), now - 7 * 86_400_000);
        assert_eq!(range_start_ms(1, now), now - 86_400_000);
        // 今日：本地零点（结果落在 [now−24h, now] 内，且为本地零点整点）
        let start = range_start_ms(0, now);
        assert!(start <= now && start > now - 86_400_000, "{start}");
        let dt = chrono::DateTime::from_timestamp_millis(start).unwrap();
        let local = dt.with_timezone(&chrono::Local);
        assert_eq!(local.time().second(), 0);
        assert_eq!(local.time().minute(), 0);
        assert_eq!(local.time().hour(), 0);
        // 负数防御按 0 处理
        assert_eq!(range_start_ms(-3, now), range_start_ms(0, now));
    }

    #[test]
    fn 聚合_导入库请求平均口径与多渠道分组() {
        // opencode 形态（有 duration_ms、无事件时刻 / status）：同名模型在
        // 不同 provider_id（opencode-go 订阅 vs deepseek 官方 key）必须分成
        // 两组各自独立统计；速度口径 request_average（output ÷ duration，
        // 含首字延迟），serde 值 "request_average"（前端 ≈ 前缀契约）
        let (conn, path) = imported_speed_db("opencode-multi-provider", true);
        let t0 = 6_000_000_000_i64;
        conn.execute_batch(&format!(
            "INSERT INTO model_usage (started_at, model_id, provider_id,
                input_tokens, output_tokens, reasoning_tokens, duration_ms) VALUES
              -- opencode-go 渠道：两笔样本（1000ms/100tok → 100 t/s；
              -- 500ms/50tok → 100 t/s）
              ({t0}, 'deepseek-v4.1-flash', 'opencode-go', 500, 100, 0, 1000.0),
              ({t2}, 'deepseek-v4.1-flash', 'opencode-go', 400, 50, 0, 500.0),
              -- deepseek 官方 key 渠道：一笔样本（1000ms/300tok → 300 t/s）
              ({t3}, 'deepseek-v4.1-flash', 'deepseek', 600, 300, 0, 1000.0);",
            t0 = t0,
            t2 = t0 + 10_000,
            t3 = t0 + 20_000,
        ))
        .unwrap();
        let stats = collect_source_speed(&conn, "opencode", t0, t0 + 1_000_000).unwrap();
        assert_eq!(stats.len(), 2, "{stats:?}，同名模型不同 provider 必须分成两组");
        // 组内排序：请求数降序 → opencode-go（2 请求）在前
        let go = &stats[0];
        assert_eq!(go.source, "opencode");
        assert_eq!(go.model_id, "deepseek-v4.1-flash");
        assert_eq!(go.provider, "opencode-go", "导入库 provider 原样展示不剥前缀");
        assert_eq!(go.requests, 2);
        assert_eq!(go.speed_sample_count, Some(2));
        assert_eq!(go.speed_quality, Some(SpeedQuality::RequestAverage));
        assert!((go.avg_tps.unwrap() - 100.0).abs() < 1e-6);
        assert!((go.p50_tps.unwrap() - 100.0).abs() < 1e-6);
        assert_eq!(go.success_rate, None, "导入库无 status 列 → 成功率 null");
        // 耗时样本 = 真实 duration_ms：1000/500 → avg 750
        assert_eq!(go.dur_avg_ms, Some(750));
        // TTFT：opencode 无该列 → null
        assert_eq!(go.ttft_avg_ms, None);
        let official = &stats[1];
        assert_eq!(official.provider, "deepseek");
        assert_eq!(official.requests, 1);
        assert_eq!(official.speed_quality, Some(SpeedQuality::RequestAverage));
        assert!((official.avg_tps.unwrap() - 300.0).abs() < 1e-6);
        // 序列化契约：speedQuality = "request_average"（前端 ≈ 前缀依赖该值）
        let json = serde_json::to_value(go).unwrap();
        assert_eq!(json["speedQuality"], "request_average");
        assert_eq!(json["source"], "opencode");
        drop(conn);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn 聚合_导入库耗时异常行不产速度样本() {
        // duration_ms NULL / 0 / 窗口越界（> 1h）的行不产速度样本、不进耗时
        // 统计（不做估算回退），但请求数与 token 量级照常计入
        let (conn, path) = imported_speed_db("kimi-dirty-duration", true);
        let t0 = 7_000_000_000_i64;
        conn.execute_batch(&format!(
            "INSERT INTO model_usage (started_at, model_id, provider_id,
                input_tokens, output_tokens, reasoning_tokens, duration_ms) VALUES
              ({t0}, 'kimi-k2', 'kimi', 100, 100, 0, 2000.0),
              ({t2}, 'kimi-k2', 'kimi', 100, 100, 0, NULL),
              ({t3}, 'kimi-k2', 'kimi', 100, 50, 0, 0.0),
              -- 窗口越界（> 1h）：整体拒收不产速度
              ({t4}, 'kimi-k2', 'kimi', 100, 100, 0, 3600001.0);",
            t0 = t0,
            t2 = t0 + 10_000,
            t3 = t0 + 20_000,
            t4 = t0 + 30_000,
        ))
        .unwrap();
        let stats = collect_source_speed(&conn, "kimi", t0, t0 + 1_000_000).unwrap();
        assert_eq!(stats.len(), 1);
        let m = &stats[0];
        assert_eq!(m.requests, 4);
        assert_eq!(m.speed_sample_count, Some(1), "只有 2000ms 行产速度样本");
        assert_eq!(m.speed_quality, Some(SpeedQuality::RequestAverage));
        assert!((m.avg_tps.unwrap() - 50.0).abs() < 1e-6, "100 tok / 2s = 50 t/s");
        // 耗时样本不做窗口边界过滤（与 zcode 的 completed − started 同语义：
        // 挂死请求的真实时长仍计入耗时统计，只是不产速度样本）：
        // 样本 [2000, 3600001] → avg (3602001+1)/2 = 1801001、P90 3600001
        assert_eq!(m.dur_avg_ms, Some(1801001));
        assert_eq!(m.dur_p90_ms, Some(3600001));
        // token：全部行计入
        assert_eq!(m.in_avg, 100);
        assert_eq!(m.out_max, 100);
        drop(conn);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn 聚合_codex形态无耗时列() {
        // codex 导入库没有任何耗时列：无速度样本（speedQuality = null）、无
        // 耗时统计，只有请求数 / token 量级 / TTFT(null)，前端速度值显示 —
        let (conn, path) = imported_speed_db("codex-no-duration", false);
        let t0 = 8_000_000_000_i64;
        conn.execute_batch(&format!(
            "INSERT INTO model_usage (started_at, model_id, provider_id,
                input_tokens, output_tokens, reasoning_tokens) VALUES
              ({t0}, 'gpt-5.2', 'codex', 1000, 2000, 0),
              ({t2}, 'gpt-5.2', 'codex', 800, 1200, 0);",
            t0 = t0,
            t2 = t0 + 10_000,
        ))
        .unwrap();
        let stats = collect_source_speed(&conn, "codex", t0, t0 + 1_000_000).unwrap();
        assert_eq!(stats.len(), 1);
        let m = &stats[0];
        assert_eq!(m.source, "codex");
        assert_eq!(m.provider, "codex", "codex 默认单渠道原样展示");
        assert_eq!(m.requests, 2);
        assert_eq!(m.speed_sample_count, None);
        assert_eq!(m.speed_quality, None, "codex 无速度原料 → 口径 null");
        assert_eq!(m.avg_tps, None);
        assert_eq!(m.p50_tps, None);
        assert_eq!(m.dur_avg_ms, None);
        assert_eq!(m.ttft_avg_ms, None);
        assert_eq!(m.in_avg, 900);
        assert_eq!(m.out_max, 2000);
        let json = serde_json::to_value(m).unwrap();
        assert!(json["speedQuality"].is_null());
        drop(conn);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn 聚合_自定义区间上下界过滤() {
        // 自定义区间模式（from/to 同时给，见模块头「查询窗口」）：按半开
        // 区间 [from, to) 过滤——早于 from 与晚于 to（含恰好等于 to）的本
        // 地行都被排除；远端行同样卡上下界。速度样本 / requests / token
        // 只含区间内行
        let (conn, path) = speed_db("custom-range");
        let t0 = 12_000_000_000_i64;
        conn.execute_batch(&format!(
            "INSERT INTO model_usage (session_id, turn_id, started_at, first_token_at,
                completed_at, model_id, provider_id, status, input_tokens, output_tokens) VALUES
              -- 区间内（from ≤ t < to）：参与全部统计
              ('s1', 't1', {t0}, {f1}, {c1}, 'M1', 'account:p', 'completed', 100, 100),
              -- 晚于 to（含恰好等于 to，半开区间含上界）：排除
              ('s1', 't2', {to_exact}, {f2}, {c2}, 'M1', 'account:p', 'completed', 100, 100),
              ('s1', 't3', {t3}, {f3}, {c3}, 'M1', 'account:p', 'completed', 100, 100),
              -- 早于 from：排除
              ('s1', 't4', {before}, {f4}, {c4}, 'M1', 'account:p', 'completed', 100, 100);",
            t0 = t0,
            f1 = t0 + 500,
            c1 = t0 + 1500,
            to_exact = t0 + 20_000,
            f2 = t0 + 20_500,
            c2 = t0 + 21_500,
            t3 = t0 + 30_000,
            f3 = t0 + 30_500,
            c3 = t0 + 31_500,
            before = t0 - 10_000,
            f4 = t0 - 9_500,
            c4 = t0 - 8_500,
        ))
        .unwrap();
        // 区间 [t0, t0+20_000)：本地只有 t1 在内
        let remote = vec![
            // 区间内：并入
            remote_record("zcode", "M1", "account:p", t0 + 5_000, 100, Some(2000.0), None),
            // 晚于 to：被上界排除（不进 requests / token）
            remote_record("zcode", "M1", "account:p", t0 + 25_000, 999, Some(1000.0), None),
        ];
        let stats = collect_source_speed_with_remote(
            &conn,
            "zcode",
            t0,
            Some(t0 + 20_000),
            t0 + 1_000_000,
            &remote,
        )
        .unwrap();
        assert_eq!(stats.len(), 1, "{stats:?}");
        let m = &stats[0];
        // 本地 t1 + 远端区间内一行；恰等于 to 的 t2、晚于 to 的 t3 与远端
        // 晚于 to 的行全部排除
        assert_eq!(m.requests, 2);
        // 速度分层：本地 t1 是 generation 样本（100 tok / 1s = 100 t/s），
        // 远端行只进 average 池不掺入 → 精确口径仍 generation
        assert_eq!(m.speed_sample_count, Some(1));
        assert_eq!(m.speed_quality, Some(SpeedQuality::Generation));
        assert!((m.avg_tps.unwrap() - 100.0).abs() < 1e-6);
        // token：只有区间内两行（100 + 100）
        assert_eq!(m.in_total, 200);
        drop(conn);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn 聚合_同名模型跨来源独立分组() {
        // 同一模型在 zcode 主库与 opencode 导入库各自独立成组：口径分别为
        // generation / request_average，绝不合并统计；collect_all_speeds 的
        // 全局排序（请求数降序 + 来源稳定序）对合并结果生效
        let (zconn, zpath) = speed_db("cross-zcode");
        let (oconn, opath) = imported_speed_db("cross-opencode", true);
        let t0 = 9_000_000_000_i64;
        zconn
            .execute_batch(&format!(
                "INSERT INTO model_usage (session_id, turn_id, started_at, first_token_at,
                    completed_at, model_id, provider_id, status, input_tokens, output_tokens) VALUES
                  ('s1', 't1', {t0}, {f}, {c}, 'GLM-5.3', 'account:bigmodel-individual',
                   'completed', 100, 100);",
                t0 = t0,
                f = t0 + 500,
                c = t0 + 1500,
            ))
            .unwrap();
        oconn
            .execute_batch(&format!(
                "INSERT INTO model_usage (started_at, model_id, provider_id,
                    input_tokens, output_tokens, reasoning_tokens, duration_ms) VALUES
                  ({t0}, 'GLM-5.3', 'opencode-go', 100, 200, 0, 1000.0);",
                t0 = t0,
            ))
            .unwrap();
        let z_stats = collect_source_speed(&zconn, "zcode", t0, t0 + 1_000_000).unwrap();
        let o_stats = collect_source_speed(&oconn, "opencode", t0, t0 + 1_000_000).unwrap();
        let (merged, remote_failed) = collect_all_speeds(
            None,
            |source, _rows| match source {
                "zcode" => Ok(z_stats.clone()),
                "opencode" => Ok(o_stats.clone()),
                _ => Ok(Vec::new()),
            },
        );
        assert!(!remote_failed, "本机模式远端失败标记恒 false");
        assert_eq!(merged.len(), 2, "跨来源同名模型不合并");
        let z = merged.iter().find(|s| s.source == "zcode").unwrap();
        let o = merged.iter().find(|s| s.source == "opencode").unwrap();
        assert_eq!(z.model_id, "GLM-5.3");
        assert_eq!(o.model_id, "GLM-5.3");
        assert_eq!(z.speed_quality, Some(SpeedQuality::Generation));
        assert_eq!(o.speed_quality, Some(SpeedQuality::RequestAverage));
        assert_eq!(z.provider, "bigmodel-individual", "zcode 渠道剥 account: 前缀");
        assert_eq!(o.provider, "opencode-go", "opencode 渠道原样");
        // 两侧速度口径独立计算：zcode 100tok/1s = 100 t/s（精确）；
        // opencode 200tok/1s = 200 t/s（近似，含首字延迟）
        assert!((z.avg_tps.unwrap() - 100.0).abs() < 1e-6);
        assert!((o.avg_tps.unwrap() - 200.0).abs() < 1e-6);
        drop(zconn);
        drop(oconn);
        let _ = std::fs::remove_file(&zpath);
        let _ = std::fs::remove_file(&opath);
    }

    #[test]
    fn 聚合_单源失败跳过其余来源照常() {
        // 单源打开 / 查询失败（如库文件损坏）只记日志并跳过该源，不拖垮
        // 整页；库文件不存在走 Ok(空) 同样不算失败（request_logs::open_source
        // 的语义，编排层用 fetch 注入验证容错路径）；本机模式 remote_failed
        // 恒 false
        let make = |source: &str| ModelSpeedStat {
            model_id: "m".into(),
            source: source.to_string(),
            provider: "p".into(),
            requests: 1,
            local_requests: 1,
            speed_sample_count: Some(1),
            speed_quality: Some(SpeedQuality::Generation),
            success_rate: Some(1.0),
            avg_tps: Some(10.0),
            p10_tps: Some(10.0),
            p50_tps: Some(10.0),
            p90_tps: Some(10.0),
            ttft_min_ms: Some(1),
            ttft_avg_ms: Some(1),
            ttft_p90_ms: Some(1),
            dur_avg_ms: Some(1),
            dur_p90_ms: Some(1),
            in_avg: 1,
            in_max: 1,
            in_total: 1,
            out_avg: 1,
            out_max: 1,
            out_total: 1,
        };
        let (out, remote_failed) = collect_all_speeds(None, |source, _rows| match source {
            "zcode" => Ok(vec![make("zcode")]),
            "kimi" => Err("database disk image is malformed".to_string()),
            _ => Ok(Vec::new()),
        });
        assert_eq!(out.len(), 1, "失败源被跳过，成功源照常返回");
        assert_eq!(out[0].source, "zcode");
        assert!(!remote_failed, "本机模式远端失败标记恒 false");
    }

    #[test]
    fn 聚合_远端行合并与口径分层() {
        // zcode 组本机有 generation 精确样本 + 远端 duration 行：远端行并入
        // requests / token / 耗时 / TTFT 统计，但**不掺速度池**——speedQuality
        // 仍 generation、速度值只由精确样本决定（混算会把 100 t/s 拉成
        // 549.5）；成功率分母只算本机行（1/1 而非被远端行稀释成 1/2）
        let (conn, path) = speed_db("remote-layering");
        let t0 = 10_000_000_000_i64;
        conn.execute_batch(&format!(
            "INSERT INTO model_usage (session_id, turn_id, started_at, first_token_at,
                completed_at, model_id, provider_id, status, input_tokens, output_tokens) VALUES
              ('s1', 't1', {t0}, {f}, {c}, 'GLM-5.3', 'account:bigmodel-individual',
               'completed', 100, 100);",
            t0 = t0,
            f = t0 + 500,
            c = t0 + 1500,
        ))
        .unwrap();
        let remote = vec![remote_record(
            "zcode",
            "GLM-5.3",
            // 与本机 raw provider 相同（account: 前缀同款形态）→ 并入同组
            "account:bigmodel-individual",
            t0 + 5_000,
            999,
            Some(1000.0),
            Some(200.0),
        )];
        let stats =
            collect_source_speed_with_remote(&conn, "zcode", t0, None, t0 + 1_000_000, &remote).unwrap();
        assert_eq!(stats.len(), 1, "{stats:?}");
        let m = &stats[0];
        assert_eq!(m.requests, 2, "远端行计入请求数");
        assert_eq!(m.local_requests, 1, "本机行计数不含远端行（hero 成功率权重）");
        // 速度分层：只用本机精确样本（100 tok / 1s = 100 t/s）；远端
        // 999 tok / 1s 若混入会是 549.5
        assert_eq!(m.speed_sample_count, Some(1));
        assert_eq!(m.speed_quality, Some(SpeedQuality::Generation));
        assert!((m.avg_tps.unwrap() - 100.0).abs() < 1e-6, "远端近似样本不得掺入精确池");
        assert!((m.p50_tps.unwrap() - 100.0).abs() < 1e-6);
        // 耗时：本机 1500 + 远端 1000 → avg 1250
        assert_eq!(m.dur_avg_ms, Some(1250));
        // TTFT：本机 500 + 远端 200 → min 200、avg 350
        assert_eq!(m.ttft_min_ms, Some(200));
        assert_eq!(m.ttft_avg_ms, Some(350));
        // token：全部行计入（本机 100 + 远端 100 → avg 100；总量 = 200）
        assert_eq!(m.in_avg, 100);
        assert_eq!(m.in_total, 200);
        assert_eq!(m.out_total, 1099, "输出总量含远端行（100 + 999）");
        // 成功率：本机 1/1 = 100%（不被远端行稀释成 1/2）
        assert_eq!(m.success_rate, Some(1.0));
        drop(conn);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn 聚合_本机无样本时远端点亮近似口径() {
        // 本机 zcode 行只有 error（无精确样本）+ 远端行有 duration → 平均
        // 池点亮 request_average；旧服务端（proto 8 前）duration_ms /
        // ttft 恒 None 的远端行不产速度样本、不计耗时，requests / token 正常
        let (conn, path) = speed_db("remote-average");
        let t0 = 11_000_000_000_i64;
        conn.execute_batch(&format!(
            "INSERT INTO model_usage (session_id, turn_id, started_at, first_token_at,
                completed_at, model_id, provider_id, status, input_tokens, output_tokens) VALUES
              ('s1', 't1', {t0}, NULL, NULL, 'GLM-5.3', 'account:p', 'error', 100, 0);",
            t0 = t0,
        ))
        .unwrap();
        let remote = vec![
            // 有耗时：100 tok / 2s = 50 t/s（average 池）
            remote_record("zcode", "GLM-5.3", "account:p", t0 + 5_000, 100, Some(2000.0), None),
            // 旧服务端形态：无耗时无首字 → 只计 requests / token
            remote_record("zcode", "GLM-5.3", "account:p", t0 + 6_000, 50, None, None),
        ];
        let stats =
            collect_source_speed_with_remote(&conn, "zcode", t0, None, t0 + 1_000_000, &remote).unwrap();
        assert_eq!(stats.len(), 1, "{stats:?}");
        let m = &stats[0];
        assert_eq!(m.requests, 3);
        assert_eq!(m.speed_sample_count, Some(1), "只有带耗时的远端行产速度样本");
        assert_eq!(m.speed_quality, Some(SpeedQuality::RequestAverage));
        assert!((m.avg_tps.unwrap() - 50.0).abs() < 1e-6);
        // 耗时样本只有远端带耗时的一笔：avg 2000
        assert_eq!(m.dur_avg_ms, Some(2000));
        // TTFT：本机 first NULL、两笔远端均无 → null
        assert_eq!(m.ttft_avg_ms, None);
        // token：三行 input 各 100 → avg 100；总量 = 300
        assert_eq!(m.in_avg, 100);
        assert_eq!(m.in_total, 300);
        assert_eq!(m.out_max, 100);
        // 成功率：本机 1 行 error → 0/1 = 0.0（远端行不进分子分母）
        assert_eq!(m.success_rate, Some(0.0));
        drop(conn);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn 聚合_远端分桶未知来源丢弃与失败标记() {
        // 远端行按真实 source 分桶进来源集合：未知来源（cursor，本页无该
        // 来源路）丢弃；远端拉取失败 → remote_failed = true 且本机数据照常；
        // 响应序列化契约 remoteFailed（camelCase）
        let make = |source: &str, model: &str| ModelSpeedStat {
            model_id: model.into(),
            source: source.to_string(),
            provider: "p".into(),
            requests: 1,
            local_requests: 1,
            speed_sample_count: None,
            speed_quality: None,
            success_rate: None,
            avg_tps: None,
            p10_tps: None,
            p50_tps: None,
            p90_tps: None,
            ttft_min_ms: None,
            ttft_avg_ms: None,
            ttft_p90_ms: None,
            dur_avg_ms: None,
            dur_p90_ms: None,
            in_avg: 1,
            in_max: 1,
            in_total: 1,
            out_avg: 1,
            out_max: 1,
            out_total: 1,
        };
        // Ok 路径：opencode 一条 + cursor 一条 → cursor 被丢弃，只有
        // opencode 一路收到行并产出分组
        let (out, failed) = collect_all_speeds(
            Some(Ok(vec![
                remote_record("opencode", "m", "opencode-go", 1000, 10, Some(1000.0), None),
                remote_record("cursor", "m", "p", 1000, 10, Some(1000.0), None),
            ])),
            |source, rows| {
                Ok(rows.iter().map(|row| make(source, &row.model_id)).collect())
            },
        );
        assert!(!failed);
        assert_eq!(out.len(), 1, "未知来源 cursor 被丢弃，只有 opencode 产出");
        assert_eq!(out[0].source, "opencode");
        let json =
            serde_json::to_value(&ModelSpeedResponse { stats: out, remote_failed: failed })
                .unwrap();
        assert_eq!(json["remoteFailed"], false);
        assert!(json["stats"].is_array());

        // Err 路径：远端失败 → remote_failed = true，本机 7 源照常返回
        let (out, failed) = collect_all_speeds(
            Some(Err("同步未启用或未注册设备".to_string())),
            |source, _rows| Ok(vec![make(source, "m")]),
        );
        assert!(failed, "远端拉取失败应置 remote_failed");
        assert_eq!(out.len(), 7, "本机数据照常返回，不被远端失败拖垮");
    }

    #[test]
    fn 聚合_zcode本地库不可用时远端行照常并入() {
        // P2-1 回归：本机未装 ZCode（db::db_path 解析失败 → open_speed_source
        // 降级 Ok(None)）时走「空内存库 + 远端行」路径——无 model_usage 表
        // 即 0 本地行，已分桶的远端 zcode 行照常成组（request_average 近似
        // 口径），不再被整个 zcode 路丢弃
        let mem = Connection::open_in_memory().unwrap();
        let remote = vec![remote_record(
            "zcode",
            "GLM-5.3",
            "account:bigmodel-individual",
            1000,
            100,
            Some(2000.0),
            None,
        )];
        let stats = collect_source_speed_with_remote(&mem, "zcode", 0, None, 1_000_000, &remote).unwrap();
        assert_eq!(stats.len(), 1, "{stats:?}");
        let m = &stats[0];
        assert_eq!(m.source, "zcode");
        assert_eq!(m.model_id, "GLM-5.3");
        assert_eq!(m.provider, "bigmodel-individual", "zcode 渠道剥 account: 前缀");
        assert_eq!(m.requests, 1);
        assert_eq!(m.local_requests, 0, "本地不可读 = 0 本机行");
        assert_eq!(m.speed_quality, Some(SpeedQuality::RequestAverage));
        assert!((m.avg_tps.unwrap() - 50.0).abs() < 1e-6);
        assert_eq!(m.success_rate, None, "本地不可读 = 无 status 列 → 成功率 null");
    }
}
