//! 本地读取型开源 agent 的**声明式注册表**。
//!
//! ## 这是什么
//!
//! 「本地读取型 agent」= 各自读本机数据源（jsonl / sqlite）→ 增量导入自有派生库
//! `~/.zbar/<agent>.sqlite` → 主面板 / 悬浮窗 / 项目页 / 对比页 / 价格页从这里取数。
//! 每条这样的链路过去要在这十来个集成面里各改一处：
//!
//! - `projects.rs`：来源白名单、默认来源列表、4 个项目维度查询的分发、回填；
//! - `session_hud.rs`：agent 组标识常量、固定序、分组列表、今日 stats、速度口径分支；
//! - `lib.rs`：`list_models`（价格页清单）、`collect_relevant_models`（价格检查主体）、
//!   `get_compare_tokens_for_agent`（对比页分发）、`pending_upload_count`（待上传计数）、
//!   `today_tray_title`（托盘今日合计）。
//!
//! 本模块把这些集成面收敛成 **`LocalAgentSource` trait + 一份静态注册表**：
//!
//! ## 新增一个开源 agent 的步骤
//!
//! 1. 新建 `src-tauri/src/<agent>.rs`，照 `dsh.rs` / `opencode.rs` 的形态写导入器
//!    （派生库、`import_incremental`、那族 `query_*` 函数在模块内部自己实现）；
//! 2. 在本文件底部加一个零成本单元结构体 + `impl LocalAgentSource`（方法体就是
//!    一行转发，不重写 SQL；项目维度等方法若确实没有数据源，直接用默认实现返回空）；
//! 3. 在 `SOURCES` 数组里加一行；
//! 4. `lib.rs` 加 `mod <agent>;`。
//!
//! 之后**上面列出的所有集成面自动生效**（项目页可见并可按来源过滤、悬浮窗成组显示、
//! 价格页出现其模型、对比页可按来源查询、托盘今日合计计入）；不需要再动
//! `projects.rs` / `session_hud.rs` / `lib.rs` 里的任何分发代码。唯一还需要单独接线的是
//! **同步上传通道**（`sync.rs` 的上传实现与 `SyncConfig` 游标字段）——见 `sync_cursor`。
//!
//! ## 为什么不收 codex / claude / cursor / zcode
//!
//! - `zcode`：主库 `~/.zbar/zbar.sqlite` 走 `db::*`，是「功能最全」的标杆（主面板、
//!   轮次/rollout 速度、生成口径与 TTFT 都只有它有），另有 `zcode_sessions` 派生库
//!   `ATTACH` 的特殊读法，不适合塞进同族转发；
//! - `codex` / `claude`：形态自成一派（有 `message` 级上下文回填、模型名修订补传、
//!   自己的 `backfill_*` 迁移），且用户明确不要求本次纳入；
//! - `cursor`：纯远端 API（无本地派生库），只有「今日总量」一种汇总形态。
//!
//! 这四者仍按各自既有路径接在集成面里（显式分支，不再逐个枚举本族成员），
//! 与本注册表互不干扰。
//!
//! ## 口径约束
//!
//! 各实现**只转发**到对应模块的既有公开函数，绝不在本模块重写 SQL 或改口径
//! （尤其：`opencode` 的 TTFT 列恒 NULL → 速度是 `RequestAverage`，不能像
//! `kimi` / `dsh` 那样按 `Generation` 展示，见其模块头说明）。

use crate::db;
use crate::projects::{ProjectModelRow, ProjectSessionModelRow};
use crate::session_hud::{AgentWindowSessions, HudModelSpeed};
use crate::sync::SyncConfig;
use crate::token_speed::SpeedQuality;

/// 注册来源名常量（**单一口径**：`session_hud` 的 `AGENT_*` 组标识与
/// `projects` / `lib.rs` 的来源字符串都从这里取，防止两处字符串各自漂移）。
pub const KIMI: &str = "kimi";
pub const DSH: &str = "dsh";
pub const OPENCODE: &str = "opencode";
/// MiniMax Code（`~/.minimax/v2/sqlite/runtime-state.sqlite` → `~/.zbar/minimaxcode.sqlite`）。
/// 注意这是**后端来源名**：前端页签仍是既有的 `minimax`（本地用量与 Coding Plan
/// 额度卡同页），故本常量只用于来源过滤 / 项目页 / 同步与待上传口径。
pub const MINIMAXCODE: &str = "minimaxcode";

/// 本地读取型 agent 的查询契约。
///
/// 方法一律**转发**到各 agent 模块的既有公开函数——本 trait 只负责「找到实现」，
/// 不负责改口径，故各模块的函数签名与行为完全不变（不牵动同步与前端）。
///
/// 默认实现只给「对个别来源确实无意义」的方法：项目维度四查询（没有项目维度的
/// 来源不必写样板）、`sync_cursor` / `pending_rows`（没有上传通道的来源）、
/// `speed_quality`（默认请求平均口径）、`backfill_dimensions`（无需存量回填）。
// 四个方法（has_local_data / query_trend / query_since / query_revised_since）
// 当前主线尚未调用：presence 由 provider_credentials 按**凭证 provider id**
// 判断（那套 id 与这里的来源名不同域）、趋势图只画 zcode、同步上传在 sync.rs
// 内按来源名直接调各模块函数（本任务明确不改 sync.rs 的上传实现）。它们留在
// trait 里是为了**契约完整性**——新增 agent 有统一的对标面，将来把上面任一处
// 改成注册表遍历时不必再扩 trait。故这里是「已声明但暂无调用方」，不是死代码。
#[allow(dead_code)]
pub trait LocalAgentSource: Sync {
    // ===== 身份 =====

    /// 注册来源名（`projects` / `session_hud` / `lib.rs` 的来源字符串，
    /// 也是前端按来源过滤 / 对比页传参用的值）。
    fn source(&self) -> &'static str;

    // ===== presence =====

    /// 本机是否装过该 agent 并产生过数据（各模块自定的 presence 口径）。
    fn has_local_data(&self) -> bool;

    // ===== 模型清单（价格页 / 价格检查）=====

    /// 该来源派生库出现过的 (provider_id, model_id) 组合。
    fn list_models(&self) -> Result<Vec<db::ModelInfo>, String>;

    // ===== 统计 =====

    /// [from_ms, to_ms) 的整体 + 按模型统计（主面板 / 悬浮窗今日合计）。
    fn query_stats(&self, from_ms: i64, to_ms: i64) -> Result<db::Stats, String>;

    /// [from_ms, to_ms) 的分桶统计（bucket = "hour" / "day"）。
    fn query_trend(
        &self,
        from_ms: i64,
        to_ms: i64,
        bucket: &str,
    ) -> Result<Vec<db::TrendBucketRaw>, String> {
        let _ = (from_ms, to_ms, bucket);
        Ok(Vec::new())
    }

    /// 按指定计费周期聚合 token（对比页用真实 [reset_at, end_at) 边界）。
    fn query_period_buckets(
        &self,
        periods: &[(i64, i64)],
    ) -> Result<Vec<db::PeriodBucket>, String> {
        let _ = periods;
        Ok(Vec::new())
    }

    // ===== 同步上传面 =====

    /// 派生库内最大自增 id（配合 `sync_cursor` 算待上传条数）。
    fn max_rowid(&self) -> Result<i64, String> {
        Ok(0)
    }

    /// id > since 的明细行（同步增量上传）。
    fn query_since(&self, since: i64, limit: usize) -> Result<Vec<db::UsageRow>, String> {
        let _ = (since, limit);
        Ok(Vec::new())
    }

    /// 修订行（updated_at > since_ts，同步补传）。
    fn query_revised_since(
        &self,
        since_ts: i64,
        after_id: i64,
        limit: usize,
    ) -> Result<Vec<db::UsageRow>, String> {
        let _ = (since_ts, after_id, limit);
        Ok(Vec::new())
    }

    /// 已上传到本来源的 rowid 游标（`SyncConfig` 里对应字段）。
    ///
    /// **默认 `None` = 该来源还没有上传通道**。这一步刻意做成显式声明而不是
    /// 「有没有游标字段都能算」：没有上传通道的来源如果也按 `max_rowid - 0`
    /// 计入待上传，就会永远报「有待上传」却永远传不上去（假信号）。
    fn sync_cursor(&self, cfg: &SyncConfig) -> Option<i64> {
        let _ = cfg;
        None
    }

    /// 本来源的待上传条数（`max_rowid − 游标`，负值归零；查询失败按 0）。
    /// 无游标（`sync_cursor` 为 None）恒 0，理由见 `sync_cursor`。
    fn pending_rows(&self, cfg: &SyncConfig) -> i64 {
        match self.sync_cursor(cfg) {
            Some(cursor) => (self.max_rowid().unwrap_or(0) - cursor).max(0),
            None => 0,
        }
    }

    // ===== 会话悬浮窗 =====

    /// 活跃窗口内的会话行（会话悬浮窗分组用）。
    fn query_window_sessions(
        &self,
        window_start_ms: i64,
        limit: usize,
    ) -> Result<AgentWindowSessions, String>;

    /// 该来源的窗口速度口径：`Generation` = 有真生成窗口原料（逐 step 流式时长），
    /// `RequestAverage` = 只有请求总耗时（界面带 ≈ 前缀，默认值）。
    ///
    /// 这是**能力声明**而不是「本次数据一定有」：`kimi` 旧会话没有 `step_meta`
    /// 时 `gen_*` 原料为空，`session_hud::brief_from_agent_row` 会自然回退请求平均；
    /// 反过来，声明 `RequestAverage` 的来源即使拿到 `gen_*` 也不按生成口径展示。
    fn speed_quality(&self) -> SpeedQuality {
        SpeedQuality::RequestAverage
    }

    /// 该来源能否给出**实时状态**（生成中 / 空闲）。
    ///
    /// `true` = 有独立的运行中信号（kimi = `step_meta` 的 running step + 新鲜度
    /// 保护；dsh = 会话投影的 `openTurnStartSeq`（当前轮未结束）+ 新鲜度保护），
    /// 悬浮窗的状态点 tooltip 按「生成中 / 空闲」读。`false`（默认）= 只有导入库
    /// 聚合，行上的灰点是**中性占位**，前端按"无实时状态"文案展示——绝不能把
    /// 这种灰点读成"空闲"，那是在替来源编造它给不出的信号。
    fn has_live_status(&self) -> bool {
        false
    }

    // ===== 会话悬浮窗：模型速度区 =====

    /// 活跃窗口内按「本来源 × 模型」分组的输出速度摘要（悬浮窗底部速度区
    /// 一行一个模型；ZCode 走自有路径，不在此列）。
    ///
    /// 默认空实现 = 该来源不贡献速度区行（数据源给不出按模型的速度）。各来源
    /// 的实现只声明**口径**，聚合本身复用
    /// `session_hud::collect_derived_model_speeds`（各来源派生库 schema 同形）。
    fn query_window_model_speeds(
        &self,
        window_start_ms: i64,
        limit: usize,
    ) -> Result<Vec<HudModelSpeed>, String> {
        let _ = (window_start_ms, limit);
        Ok(Vec::new())
    }

    // ===== 项目维度（没有项目维度的来源用默认空实现，不必写样板）=====

    /// 按项目 × 模型聚合 [from_ms, to_ms) 的用量。
    fn query_project_model_rows(
        &self,
        from_ms: i64,
        to_ms: i64,
    ) -> Result<Vec<ProjectModelRow>, String> {
        let _ = (from_ms, to_ms);
        Ok(Vec::new())
    }

    /// 按项目统计 [from_ms, to_ms) 内的会话数。
    fn query_project_session_counts(
        &self,
        from_ms: i64,
        to_ms: i64,
    ) -> Result<Vec<(String, i64)>, String> {
        let _ = (from_ms, to_ms);
        Ok(Vec::new())
    }

    /// 项目键 → 原始形态 cwd（保留大小写）。
    fn query_project_display_paths(&self) -> Result<Vec<(String, String)>, String> {
        Ok(Vec::new())
    }

    /// 指定项目的会话明细分页。
    fn query_project_sessions(
        &self,
        project_key: &str,
        from_ms: i64,
        to_ms: i64,
        offset: u32,
        limit: u32,
    ) -> Result<(u32, Vec<ProjectSessionModelRow>), String> {
        let _ = (project_key, from_ms, to_ms, offset, limit);
        Ok((0, Vec::new()))
    }

    // ===== 存量回填 =====

    /// 项目/会话维度的存量回填（各自带节流，失败自行静默）。默认无事可做：
    /// 结构规整的来源在 `query_project_*` 内部就完成了映射，只有需要兼容
    /// 历史数据的来源（如 `kimi` 的 `session_meta` 项目键）才实现它。
    fn backfill_dimensions(&self) {}
}

// ============================================================
// 各来源实现（零成本单元结构体，方法直接转发）
// ============================================================

/// Kimi Code CLI（`~/.kimi-code/sessions/**/wire.jsonl` → `~/.zbar/kimi.sqlite`）。
struct KimiSource;

impl LocalAgentSource for KimiSource {
    fn source(&self) -> &'static str {
        KIMI
    }

    fn has_local_data(&self) -> bool {
        // kimi 没有独立的 presence 探测：会话目录存在即可探测（与
        // `kimi::sessions_dir()` 同口径，但那边返回 Err 文案，这里只判存在）。
        crate::kimi::sessions_dir().is_ok()
    }

    fn list_models(&self) -> Result<Vec<db::ModelInfo>, String> {
        crate::kimi::list_models()
    }

    fn query_stats(&self, from_ms: i64, to_ms: i64) -> Result<db::Stats, String> {
        crate::kimi::query_stats(from_ms, to_ms)
    }

    fn query_trend(
        &self,
        from_ms: i64,
        to_ms: i64,
        bucket: &str,
    ) -> Result<Vec<db::TrendBucketRaw>, String> {
        crate::kimi::query_trend(from_ms, to_ms, bucket)
    }

    fn query_period_buckets(
        &self,
        periods: &[(i64, i64)],
    ) -> Result<Vec<db::PeriodBucket>, String> {
        crate::kimi::query_period_buckets(periods)
    }

    fn max_rowid(&self) -> Result<i64, String> {
        crate::kimi::max_rowid()
    }

    fn query_since(&self, since: i64, limit: usize) -> Result<Vec<db::UsageRow>, String> {
        crate::kimi::query_since(since, limit)
    }

    fn query_revised_since(
        &self,
        since_ts: i64,
        after_id: i64,
        limit: usize,
    ) -> Result<Vec<db::UsageRow>, String> {
        crate::kimi::query_revised_since(since_ts, after_id, limit)
    }

    /// 上游游标字段口径：`SyncConfig::last_uploaded_<source>_rowid`。
    /// kimi 已有上传通道（sync.rs 的 kimi 阶段 + `last_uploaded_kimi_rowid` /
    /// `last_uploaded_kimi_rev_ts` 游标），故这里如实返回游标、参与待上传计数。
    fn sync_cursor(&self, cfg: &SyncConfig) -> Option<i64> {
        Some(cfg.last_uploaded_kimi_rowid)
    }

    fn query_window_sessions(
        &self,
        window_start_ms: i64,
        limit: usize,
    ) -> Result<AgentWindowSessions, String> {
        crate::kimi::query_window_sessions(window_start_ms, limit)
    }

    /// kimi 的 `step_meta`（wire.jsonl 的 transcript.ops）给出逐 step 流式时长
    /// → 有真生成窗口原料；旧会话无该数据时由 session_hud 自然回退请求平均。
    fn speed_quality(&self) -> SpeedQuality {
        SpeedQuality::Generation
    }

    /// kimi 的 `step_meta` 有 `state = running` 的 step（10 分钟新鲜度保护，
    /// 见 kimi.rs GENERATING_FRESH_MS）→ 能给出实时生成中/空闲。
    fn has_live_status(&self) -> bool {
        true
    }

    /// 模型速度区：kimi 的 `step_meta` 没有 model 归属列（模型只在
    /// `model_usage.model_id` 上），无法把真流式时长拆到模型上，故模型维度
    /// 只能按 `duration_ms`（llm.request→usage.record 事件差 = 整请求耗时）
    /// 走**请求平均**口径（前端带 ≈）；会话行仍走 step_meta 的生成口径。
    fn query_window_model_speeds(
        &self,
        window_start_ms: i64,
        limit: usize,
    ) -> Result<Vec<HudModelSpeed>, String> {
        crate::kimi::query_window_model_speeds(window_start_ms, limit)
    }

    fn query_project_model_rows(
        &self,
        from_ms: i64,
        to_ms: i64,
    ) -> Result<Vec<ProjectModelRow>, String> {
        crate::kimi::query_project_model_rows(from_ms, to_ms)
    }

    fn query_project_session_counts(
        &self,
        from_ms: i64,
        to_ms: i64,
    ) -> Result<Vec<(String, i64)>, String> {
        crate::kimi::query_project_session_counts(from_ms, to_ms)
    }

    fn query_project_display_paths(&self) -> Result<Vec<(String, String)>, String> {
        crate::kimi::query_project_display_paths()
    }

    fn query_project_sessions(
        &self,
        project_key: &str,
        from_ms: i64,
        to_ms: i64,
        offset: u32,
        limit: u32,
    ) -> Result<(u32, Vec<ProjectSessionModelRow>), String> {
        crate::kimi::query_project_sessions(project_key, from_ms, to_ms, offset, limit)
    }

    fn backfill_dimensions(&self) {
        // session_meta 的项目键存量回填（自带 30 秒节流；未安装时静默失败）
        let _ = crate::kimi::backfill_session_meta();
    }
}

/// DeepSeek Harness（`~/.dsh/sessions/**/*.jsonl.zstd` → `~/.zbar/dsh.sqlite`）。
struct DshSource;

impl LocalAgentSource for DshSource {
    fn source(&self) -> &'static str {
        DSH
    }

    fn has_local_data(&self) -> bool {
        crate::dsh::has_local_data()
    }

    fn list_models(&self) -> Result<Vec<db::ModelInfo>, String> {
        crate::dsh::list_models()
    }

    fn query_stats(&self, from_ms: i64, to_ms: i64) -> Result<db::Stats, String> {
        crate::dsh::query_stats(from_ms, to_ms)
    }

    fn query_trend(
        &self,
        from_ms: i64,
        to_ms: i64,
        bucket: &str,
    ) -> Result<Vec<db::TrendBucketRaw>, String> {
        crate::dsh::query_trend(from_ms, to_ms, bucket)
    }

    fn query_period_buckets(
        &self,
        periods: &[(i64, i64)],
    ) -> Result<Vec<db::PeriodBucket>, String> {
        crate::dsh::query_period_buckets(periods)
    }

    fn max_rowid(&self) -> Result<i64, String> {
        crate::dsh::max_rowid()
    }

    fn query_since(&self, since: i64, limit: usize) -> Result<Vec<db::UsageRow>, String> {
        crate::dsh::query_since(since, limit)
    }

    fn query_revised_since(
        &self,
        since_ts: i64,
        after_id: i64,
        limit: usize,
    ) -> Result<Vec<db::UsageRow>, String> {
        crate::dsh::query_revised_since(since_ts, after_id, limit)
    }

    /// dsh 的上传通道：`sync.rs` 的 dsh 阶段 +
    /// `SyncConfig::last_uploaded_dsh_rowid` / `last_uploaded_dsh_rev_ts` 游标。
    fn sync_cursor(&self, cfg: &SyncConfig) -> Option<i64> {
        Some(cfg.last_uploaded_dsh_rowid)
    }

    fn query_window_sessions(
        &self,
        window_start_ms: i64,
        limit: usize,
    ) -> Result<AgentWindowSessions, String> {
        crate::dsh::query_window_sessions(window_start_ms, limit)
    }

    /// dsh 的生成窗口取自 stream 首末 chunk，并有真实 TTFT（见 dsh.rs 模块头）。
    fn speed_quality(&self) -> SpeedQuality {
        SpeedQuality::Generation
    }

    /// dsh 的会话投影（`session_projcache/sessions/<sid>.json`）有
    /// `turnBoundary.val.openTurnStartSeq`：非空 = 当前轮未结束（工具执行期间
    /// 也非空，见 dsh.rs generating_from_projection）→ 能给出实时生成中/空闲。
    fn has_live_status(&self) -> bool {
        true
    }

    /// 模型速度区：dsh 的 `duration_ms` 就是 stream 首末 chunk 之差（真生成
    /// 窗口）→ 生成口径，与会话行同源。
    fn query_window_model_speeds(
        &self,
        window_start_ms: i64,
        limit: usize,
    ) -> Result<Vec<HudModelSpeed>, String> {
        crate::dsh::query_window_model_speeds(window_start_ms, limit)
    }

    fn query_project_model_rows(
        &self,
        from_ms: i64,
        to_ms: i64,
    ) -> Result<Vec<ProjectModelRow>, String> {
        crate::dsh::query_project_model_rows(from_ms, to_ms)
    }

    fn query_project_session_counts(
        &self,
        from_ms: i64,
        to_ms: i64,
    ) -> Result<Vec<(String, i64)>, String> {
        crate::dsh::query_project_session_counts(from_ms, to_ms)
    }

    fn query_project_display_paths(&self) -> Result<Vec<(String, String)>, String> {
        crate::dsh::query_project_display_paths()
    }

    fn query_project_sessions(
        &self,
        project_key: &str,
        from_ms: i64,
        to_ms: i64,
        offset: u32,
        limit: u32,
    ) -> Result<(u32, Vec<ProjectSessionModelRow>), String> {
        crate::dsh::query_project_sessions(project_key, from_ms, to_ms, offset, limit)
    }
}

/// OpenCode（`~/.local/share/opencode/opencode.db` → `~/.zbar/opencode.sqlite`）。
struct OpencodeSource;

impl LocalAgentSource for OpencodeSource {
    fn source(&self) -> &'static str {
        OPENCODE
    }

    fn has_local_data(&self) -> bool {
        crate::opencode::has_local_data()
    }

    fn list_models(&self) -> Result<Vec<db::ModelInfo>, String> {
        crate::opencode::list_models()
    }

    fn query_stats(&self, from_ms: i64, to_ms: i64) -> Result<db::Stats, String> {
        crate::opencode::query_stats(from_ms, to_ms)
    }

    fn query_trend(
        &self,
        from_ms: i64,
        to_ms: i64,
        bucket: &str,
    ) -> Result<Vec<db::TrendBucketRaw>, String> {
        crate::opencode::query_trend(from_ms, to_ms, bucket)
    }

    fn query_period_buckets(
        &self,
        periods: &[(i64, i64)],
    ) -> Result<Vec<db::PeriodBucket>, String> {
        crate::opencode::query_period_buckets(periods)
    }

    fn max_rowid(&self) -> Result<i64, String> {
        crate::opencode::max_rowid()
    }

    fn query_since(&self, since: i64, limit: usize) -> Result<Vec<db::UsageRow>, String> {
        crate::opencode::query_since(since, limit)
    }

    fn query_revised_since(
        &self,
        since_ts: i64,
        after_id: i64,
        limit: usize,
    ) -> Result<Vec<db::UsageRow>, String> {
        crate::opencode::query_revised_since(since_ts, after_id, limit)
    }

    /// 上游游标字段口径：`SyncConfig::last_uploaded_<source>_rowid`。
    /// opencode 已有上传通道（sync.rs 的 opencode 阶段 +
    /// `last_uploaded_opencode_rowid` / `last_uploaded_opencode_rev_ts` 游标），
    /// 故如实返回游标、参与待上传计数。
    fn sync_cursor(&self, cfg: &SyncConfig) -> Option<i64> {
        Some(cfg.last_uploaded_opencode_rowid)
    }

    fn query_window_sessions(
        &self,
        window_start_ms: i64,
        limit: usize,
    ) -> Result<AgentWindowSessions, String> {
        crate::opencode::query_window_sessions(window_start_ms, limit)
    }

    /// **恒请求平均**：opencode 的 `time_to_first_token_ms` 列存在但恒 NULL
    /// （`$.time.streamed` 语义未确认，不冒充 TTFT），`duration_ms` 是请求总耗时
    /// → 没有真生成窗口，不能标 Generation（见 opencode.rs 模块头）。
    fn speed_quality(&self) -> SpeedQuality {
        SpeedQuality::RequestAverage
    }

    /// 模型速度区：`duration_ms` 是请求总耗时 → 请求平均口径（前端带 ≈）。
    fn query_window_model_speeds(
        &self,
        window_start_ms: i64,
        limit: usize,
    ) -> Result<Vec<HudModelSpeed>, String> {
        crate::opencode::query_window_model_speeds(window_start_ms, limit)
    }

    fn query_project_model_rows(
        &self,
        from_ms: i64,
        to_ms: i64,
    ) -> Result<Vec<ProjectModelRow>, String> {
        crate::opencode::query_project_model_rows(from_ms, to_ms)
    }

    fn query_project_session_counts(
        &self,
        from_ms: i64,
        to_ms: i64,
    ) -> Result<Vec<(String, i64)>, String> {
        crate::opencode::query_project_session_counts(from_ms, to_ms)
    }

    fn query_project_display_paths(&self) -> Result<Vec<(String, String)>, String> {
        crate::opencode::query_project_display_paths()
    }

    fn query_project_sessions(
        &self,
        project_key: &str,
        from_ms: i64,
        to_ms: i64,
        offset: u32,
        limit: u32,
    ) -> Result<(u32, Vec<ProjectSessionModelRow>), String> {
        crate::opencode::query_project_sessions(project_key, from_ms, to_ms, offset, limit)
    }
}

/// MiniMax Code（`~/.minimax/v2/sqlite/runtime-state.sqlite` →
/// `~/.zbar/minimaxcode.sqlite`）。前端页签复用既有的 `minimax` 入口：本来源只
/// 提供本地用量统计，Coding Plan 额度仍走 provider_quota 的 minimax 通道。
struct MinimaxcodeSource;

impl LocalAgentSource for MinimaxcodeSource {
    fn source(&self) -> &'static str {
        MINIMAXCODE
    }

    fn has_local_data(&self) -> bool {
        crate::minimaxcode::has_local_data()
    }

    fn list_models(&self) -> Result<Vec<db::ModelInfo>, String> {
        crate::minimaxcode::list_models()
    }

    fn query_stats(&self, from_ms: i64, to_ms: i64) -> Result<db::Stats, String> {
        crate::minimaxcode::query_stats(from_ms, to_ms)
    }

    fn query_trend(
        &self,
        from_ms: i64,
        to_ms: i64,
        bucket: &str,
    ) -> Result<Vec<db::TrendBucketRaw>, String> {
        crate::minimaxcode::query_trend(from_ms, to_ms, bucket)
    }

    fn query_period_buckets(
        &self,
        periods: &[(i64, i64)],
    ) -> Result<Vec<db::PeriodBucket>, String> {
        crate::minimaxcode::query_period_buckets(periods)
    }

    fn max_rowid(&self) -> Result<i64, String> {
        crate::minimaxcode::max_rowid()
    }

    fn query_since(&self, since: i64, limit: usize) -> Result<Vec<db::UsageRow>, String> {
        crate::minimaxcode::query_since(since, limit)
    }

    fn query_revised_since(
        &self,
        since_ts: i64,
        after_id: i64,
        limit: usize,
    ) -> Result<Vec<db::UsageRow>, String> {
        crate::minimaxcode::query_revised_since(since_ts, after_id, limit)
    }

    /// MiniMax Code 已有上传通道（sync.rs 的 minimaxcode 阶段 +
    /// `last_uploaded_minimaxcode_rowid` / `last_uploaded_minimaxcode_rev_ts` 游标），
    /// 故如实返回游标、参与待上传计数。
    fn sync_cursor(&self, cfg: &SyncConfig) -> Option<i64> {
        Some(cfg.last_uploaded_minimaxcode_rowid)
    }

    fn query_window_sessions(
        &self,
        window_start_ms: i64,
        limit: usize,
    ) -> Result<AgentWindowSessions, String> {
        crate::minimaxcode::query_window_sessions(window_start_ms, limit)
    }

    /// **恒请求平均**：MiniMax Code 源库没有首字时刻（只有整请求耗时
    /// `request_duration_ms` 与整段解码时长），`time_to_first_token_ms` 列恒 NULL
    /// → 没有真生成窗口，不能标 Generation（见 minimaxcode.rs 模块头）。
    fn speed_quality(&self) -> SpeedQuality {
        SpeedQuality::RequestAverage
    }

    /// 模型速度区：`duration_ms` 是整请求耗时 → 请求平均口径（前端带 ≈）。
    fn query_window_model_speeds(
        &self,
        window_start_ms: i64,
        limit: usize,
    ) -> Result<Vec<HudModelSpeed>, String> {
        crate::minimaxcode::query_window_model_speeds(window_start_ms, limit)
    }

    fn query_project_model_rows(
        &self,
        from_ms: i64,
        to_ms: i64,
    ) -> Result<Vec<ProjectModelRow>, String> {
        crate::minimaxcode::query_project_model_rows(from_ms, to_ms)
    }

    fn query_project_session_counts(
        &self,
        from_ms: i64,
        to_ms: i64,
    ) -> Result<Vec<(String, i64)>, String> {
        crate::minimaxcode::query_project_session_counts(from_ms, to_ms)
    }

    fn query_project_display_paths(&self) -> Result<Vec<(String, String)>, String> {
        crate::minimaxcode::query_project_display_paths()
    }

    fn query_project_sessions(
        &self,
        project_key: &str,
        from_ms: i64,
        to_ms: i64,
        offset: u32,
        limit: u32,
    ) -> Result<(u32, Vec<ProjectSessionModelRow>), String> {
        crate::minimaxcode::query_project_sessions(project_key, from_ms, to_ms, offset, limit)
    }
}

// ============================================================
// 注册表
// ============================================================
/// 四个实现各一份静态实例（零成本单元结构体，无内部状态）。
static KIMI_SOURCE: KimiSource = KimiSource;
static DSH_SOURCE: DshSource = DshSource;
static OPENCODE_SOURCE: OpencodeSource = OpencodeSource;
static MINIMAXCODE_SOURCE: MinimaxcodeSource = MinimaxcodeSource;

/// **注册表本体**：新增开源 agent 只需在此加一行（见模块头「新增步骤」）。
/// 数组顺序 = 各集成面里该族的稳定顺序（悬浮窗固定序、项目页默认来源顺序）；
/// 用切片而不是定长数组，是为了让「加一行」真的只加一行（不必再改长度）。
static SOURCES: &[&'static dyn LocalAgentSource] =
    &[&KIMI_SOURCE, &DSH_SOURCE, &OPENCODE_SOURCE, &MINIMAXCODE_SOURCE];

/// 全部本地读取型来源（顺序即注册顺序）。
pub fn all() -> &'static [&'static dyn LocalAgentSource] {
    SOURCES
}

/// 按来源名查实现（生产路径）。
pub fn find(source: &str) -> Option<&'static dyn LocalAgentSource> {
    find_in(all(), source)
}

/// 按来源名在**给定**来源集合里查实现。
///
/// 为什么要带「给定集合」参数：下面的遍历辅助函数全部以 `sources` 为入参，
/// 生产调用方传 `all()`，单测传「注册表 + 假来源」的切片，从而直接验证
/// 「注册表加一行 → 各集成面自动路由到它」这条承诺（不必让单测去碰真实
/// `~/.zbar/*.sqlite`）。
pub(crate) fn find_in(
    sources: &[&'static dyn LocalAgentSource],
    source: &str,
) -> Option<&'static dyn LocalAgentSource> {
    sources.iter().copied().find(|s| s.source() == source)
}

/// 遍历：各来源 [from, to) 的 stats（**查询失败静默跳过**——未安装该 agent 时
/// 查询失败是常态，不该让某个来源拖垮合计）。调用方按各自口径消费
/// （托盘标题取 `overall.total_tokens` + `by_model` 计费；悬浮窗取 `overall`）。
pub(crate) fn stats(
    sources: &[&'static dyn LocalAgentSource],
    from_ms: i64,
    to_ms: i64,
) -> Vec<(&'static str, db::Stats)> {
    let mut out = Vec::with_capacity(sources.len());
    for source in sources {
        if let Ok(stats) = source.query_stats(from_ms, to_ms) {
            out.push((source.source(), stats));
        }
    }
    out
}

/// 遍历：各来源的模型清单（失败静默为空，与既有 `unwrap_or_default()` 同口径）。
pub(crate) fn model_lists(
    sources: &[&'static dyn LocalAgentSource],
) -> Vec<Vec<db::ModelInfo>> {
    sources
        .iter()
        .map(|s| s.list_models().unwrap_or_default())
        .collect()
}

/// 遍历：各来源的窗口模型速度行（悬浮窗底部速度区用；失败静默为空——未安装
/// 该 agent 时查询失败是常态，不该让某个来源拖垮整区）。
pub(crate) fn window_model_speeds(
    sources: &[&'static dyn LocalAgentSource],
    window_start_ms: i64,
    limit: usize,
) -> Vec<Vec<HudModelSpeed>> {
    sources
        .iter()
        .map(|s| {
            s.query_window_model_speeds(window_start_ms, limit)
                .unwrap_or_default()
        })
        .collect()
}

/// 遍历：各来源的项目 × 模型聚合行。失败项**保留 Err 返回**，由调用方按各自
/// 既有口径打印日志后跳过（日志前缀在 projects.rs，不搬到这里）。
pub(crate) fn project_model_rows(
    sources: &[&'static dyn LocalAgentSource],
    from_ms: i64,
    to_ms: i64,
) -> Vec<(&'static str, Result<Vec<ProjectModelRow>, String>)> {
    sources
        .iter()
        .map(|s| (s.source(), s.query_project_model_rows(from_ms, to_ms)))
        .collect()
}

/// 遍历：各来源的项目会话数（失败项保留 Err，同上）。
pub(crate) fn project_session_counts(
    sources: &[&'static dyn LocalAgentSource],
    from_ms: i64,
    to_ms: i64,
) -> Vec<(&'static str, Result<Vec<(String, i64)>, String>)> {
    sources
        .iter()
        .map(|s| (s.source(), s.query_project_session_counts(from_ms, to_ms)))
        .collect()
}

/// 遍历：各来源的项目展示路径（失败项保留 Err，同上）。
pub(crate) fn project_display_paths(
    sources: &[&'static dyn LocalAgentSource],
) -> Vec<(&'static str, Result<Vec<(String, String)>, String>)> {
    sources
        .iter()
        .map(|s| (s.source(), s.query_project_display_paths()))
        .collect()
}

/// 单来源分发：指定项目的会话明细。`None` = 该来源名不在给定集合里
/// （调用方自行决定是报错还是返回空页）。
pub(crate) fn project_sessions(
    sources: &[&'static dyn LocalAgentSource],
    source: &str,
    project_key: &str,
    from_ms: i64,
    to_ms: i64,
    offset: u32,
    limit: u32,
) -> Option<Result<(u32, Vec<ProjectSessionModelRow>), String>> {
    find_in(sources, source).map(|s| {
        s.query_project_sessions(project_key, from_ms, to_ms, offset, limit)
    })
}

/// 单来源分发：对比页的周期 token 聚合。`None` = 该来源名不在给定集合里
/// （lib.rs 据此给出「未知的对比 Agent」错误，保持既有错误口径）。
pub(crate) fn compare_period_buckets(
    sources: &[&'static dyn LocalAgentSource],
    source: &str,
    periods: &[(i64, i64)],
) -> Option<Result<Vec<db::PeriodBucket>, String>> {
    find_in(sources, source).map(|s| s.query_period_buckets(periods))
}

/// 遍历：注册表各来源的待上传条数合计（无游标的来源计 0，见 `sync_cursor`）。
pub(crate) fn pending_rows(
    sources: &[&'static dyn LocalAgentSource],
    cfg: &SyncConfig,
) -> i64 {
    sources.iter().map(|s| s.pending_rows(cfg)).sum()
}

/// 遍历：注册表各来源的存量维度回填（各自节流、失败自行静默）。
pub(crate) fn backfill_dimensions(sources: &[&'static dyn LocalAgentSource]) {
    for source in sources {
        source.backfill_dimensions();
    }
}

// ============================================================
// 单测
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试用「假来源」：所有查询返回可识别的哨兵值，用于证明
    /// 「注册表加一行 → 各集成面自动路由到它」。它不是注册表成员，只在单测里
    /// 作为「新增 agent」的替身，故不会碰任何真实 `~/.zbar/*.sqlite`。
    struct FakeAgent;

    static FAKE: FakeAgent = FakeAgent;

    /// 哨兵：项目键与模型名，断言里靠它确认数据确实来自假来源。
    const FAKE_PROJECT: &str = "fake-项目";
    const FAKE_MODEL: &str = "fake-model";
    const FAKE_ROWID: i64 = 1_000;
    /// 哨兵：悬浮窗模型速度区的模型名
    const FAKE_SPEED_MODEL: &str = "fake-speed-model";

    impl LocalAgentSource for FakeAgent {
        fn source(&self) -> &'static str {
            "fake-agent"
        }

        fn has_local_data(&self) -> bool {
            true
        }

        fn list_models(&self) -> Result<Vec<db::ModelInfo>, String> {
            Ok(vec![db::ModelInfo {
                provider_id: "fake".into(),
                model_id: FAKE_MODEL.into(),
            }])
        }

        fn query_stats(&self, from_ms: i64, to_ms: i64) -> Result<db::Stats, String> {
            Ok(db::Stats {
                from_ms,
                to_ms,
                overall: db::OverallStat {
                    requests: 3,
                    input_tokens: 30,
                    output_tokens: 7,
                    cache_read_tokens: 0,
                    cache_write_tokens: 0,
                    reasoning_tokens: 0,
                    total_tokens: 37,
                    speed: Default::default(),
                },
                by_model: vec![db::ModelStat {
                    model_id: FAKE_MODEL.into(),
                    provider_id: "fake".into(),
                    requests: 3,
                    input_tokens: 30,
                    output_tokens: 7,
                    cache_read_tokens: 0,
                    cache_write_tokens: 0,
                    reasoning_tokens: 0,
                    total_tokens: 37,
                    speed: Default::default(),
                }],
                earliest_ms: Some(from_ms),
                latest_ms: Some(to_ms),
                current_model: None,
            })
        }

        fn query_period_buckets(
            &self,
            periods: &[(i64, i64)],
        ) -> Result<Vec<db::PeriodBucket>, String> {
            Ok(periods
                .iter()
                .map(|&(reset_at, end_at)| db::PeriodBucket {
                    reset_at,
                    end_at,
                    total_tokens: 42,
                    requests: 1,
                })
                .collect())
        }

        fn max_rowid(&self) -> Result<i64, String> {
            Ok(FAKE_ROWID)
        }

        fn query_window_sessions(
            &self,
            _window_start_ms: i64,
            _limit: usize,
        ) -> Result<AgentWindowSessions, String> {
            Ok(AgentWindowSessions {
                total_active: 1,
                sessions: Vec::new(),
            })
        }

        /// 悬浮窗模型速度区：哨兵行（来源名与口径按生产形态自己带，
        /// session_hud 侧只做合并与截断）
        fn query_window_model_speeds(
            &self,
            _window_start_ms: i64,
            _limit: usize,
        ) -> Result<Vec<HudModelSpeed>, String> {
            Ok(vec![HudModelSpeed {
                agent: "fake-agent".into(),
                model: FAKE_SPEED_MODEL.into(),
                quality: SpeedQuality::RequestAverage,
                last_at: 777,
                tps: 12.5,
                avg_tps: 10.0,
                max_tps: 20.0,
                min_tps: 5.0,
                samples: 3,
            }])
        }

        fn query_project_model_rows(
            &self,
            _from_ms: i64,
            _to_ms: i64,
        ) -> Result<Vec<ProjectModelRow>, String> {
            Ok(vec![ProjectModelRow {
                project_key: FAKE_PROJECT.into(),
                model_id: FAKE_MODEL.into(),
                requests: 3,
                input_tokens: 30,
                output_tokens: 7,
                cache_read_tokens: 0,
                total_tokens: 37,
            }])
        }

        fn query_project_session_counts(
            &self,
            _from_ms: i64,
            _to_ms: i64,
        ) -> Result<Vec<(String, i64)>, String> {
            Ok(vec![(FAKE_PROJECT.into(), 7)])
        }

        fn query_project_display_paths(&self) -> Result<Vec<(String, String)>, String> {
            Ok(vec![(FAKE_PROJECT.into(), "C:/fake".into())])
        }

        fn query_project_sessions(
            &self,
            _project_key: &str,
            from_ms: i64,
            to_ms: i64,
            _offset: u32,
            _limit: u32,
        ) -> Result<(u32, Vec<ProjectSessionModelRow>), String> {
            Ok((
                9,
                vec![ProjectSessionModelRow {
                    session_id: "fake-session".into(),
                    model_id: FAKE_MODEL.into(),
                    first_at: from_ms,
                    last_at: to_ms,
                    requests: 3,
                    input_tokens: 30,
                    output_tokens: 7,
                    cache_read_tokens: 0,
                    cache_write_tokens: 0,
                    tps_sum: Some(9.0),
                    tps_count: 1,
                    ttft_sum: None,
                    ttft_count: 0,
                    gen_out: None,
                    gen_ms: None,
                    gen_count: 0,
                }],
            ))
        }

    }

    /// 无游标假来源：`max_rowid` 很大，但因为声明「没有上传通道」，待上传计 0。
    struct NoCursorAgent;

    static NO_CURSOR: NoCursorAgent = NoCursorAgent;

    impl LocalAgentSource for NoCursorAgent {
        fn source(&self) -> &'static str {
            "no-cursor-agent"
        }
        fn has_local_data(&self) -> bool {
            true
        }
        fn list_models(&self) -> Result<Vec<db::ModelInfo>, String> {
            Ok(Vec::new())
        }
        fn query_stats(&self, _from_ms: i64, _to_ms: i64) -> Result<db::Stats, String> {
            Err("不该被调用".into())
        }
        fn max_rowid(&self) -> Result<i64, String> {
            Ok(500)
        }
        fn query_window_sessions(
            &self,
            _window_start_ms: i64,
            _limit: usize,
        ) -> Result<AgentWindowSessions, String> {
            Err("不该被调用".into())
        }

        /// 查询失败的模型速度区来源：遍历必须静默吞掉该错误（未安装 agent 的
        /// 常态），绝不让它拖垮整块速度区
        fn query_window_model_speeds(
            &self,
            _window_start_ms: i64,
            _limit: usize,
        ) -> Result<Vec<HudModelSpeed>, String> {
            Err("查询失败".into())
        }
    }

    /// 有游标假来源：待上传 = max_rowid − 游标（负值归零）。
    struct CursorAgent;

    static CURSOR: CursorAgent = CursorAgent;

    impl LocalAgentSource for CursorAgent {
        fn source(&self) -> &'static str {
            "cursor-agent"
        }
        fn has_local_data(&self) -> bool {
            true
        }
        fn list_models(&self) -> Result<Vec<db::ModelInfo>, String> {
            Ok(Vec::new())
        }
        fn query_stats(&self, _from_ms: i64, _to_ms: i64) -> Result<db::Stats, String> {
            Err("不该被调用".into())
        }
        fn max_rowid(&self) -> Result<i64, String> {
            Ok(500)
        }
        fn sync_cursor(&self, cfg: &SyncConfig) -> Option<i64> {
            Some(cfg.last_uploaded_dsh_rowid)
        }
        fn query_window_sessions(
            &self,
            _window_start_ms: i64,
            _limit: usize,
        ) -> Result<AgentWindowSessions, String> {
            Err("不该被调用".into())
        }
    }

    /// 注册表覆盖到的来源集合：名字唯一、可按下标定位、`find` 命中且实现自洽。
    #[test]
    fn 注册表_覆盖集合与查找() {
        let names: Vec<&str> = all().iter().map(|s| s.source()).collect();
        assert_eq!(
            names,
            [KIMI, DSH, OPENCODE, MINIMAXCODE],
            "注册表覆盖 kimi/dsh/opencode/minimaxcode（顺序即各组固定序）"
        );
        // 名字唯一（否则 find 会静默取第一个，两个实现互相遮蔽）
        let mut uniq = names.clone();
        uniq.sort_unstable();
        uniq.dedup();
        assert_eq!(uniq.len(), names.len(), "来源名必须唯一");
        // 每个注册来源都能被 find 命中，且命中的就是它自己
        for source in all() {
            let found = find(source.source()).expect("注册来源必须能被 find 命中");
            assert_eq!(found.source(), source.source());
            assert!(!source.source().is_empty());
        }
        assert!(find("nonexistent-agent").is_none(), "未注册来源返回 None");
    }

    /// 速度口径声明：kimi/dsh 有真生成窗口（Generation），opencode/minimaxcode
    /// 只有请求总耗时（RequestAverage，见各自模块的 has_ttft = false）。
    #[test]
    fn 注册表_速度口径声明() {
        assert_eq!(
            find(KIMI).unwrap().speed_quality(),
            SpeedQuality::Generation
        );
        assert_eq!(find(DSH).unwrap().speed_quality(), SpeedQuality::Generation);
        assert_eq!(
            find(OPENCODE).unwrap().speed_quality(),
            SpeedQuality::RequestAverage,
            "opencode 无 TTFT / 无生成窗口，不得标 Generation"
        );
        assert_eq!(
            find(MINIMAXCODE).unwrap().speed_quality(),
            SpeedQuality::RequestAverage,
            "minimaxcode 只有整请求耗时、无 TTFT，不得标 Generation"
        );
    }

    /// 实时状态声明：只有拿得到"运行中"信号的来源才算有实时状态。
    /// kimi = step_meta running step、dsh = 会话投影的当前轮未结束；opencode /
    /// minimaxcode 没有可信的运行中标记（语义未确认），必须如实声明 false
    /// ——否则界面会把灰点读成"空闲"，等于替来源编造状态。
    #[test]
    fn 注册表_实时状态声明() {
        assert!(find(KIMI).unwrap().has_live_status());
        assert!(find(DSH).unwrap().has_live_status());
        assert!(!find(OPENCODE).unwrap().has_live_status());
        assert!(!find(MINIMAXCODE).unwrap().has_live_status());
        // 默认实现 = 无实时状态（新增来源不写就如实是"无状态信号"）
        assert!(!NO_CURSOR.has_live_status());
    }

    /// 待上传口径：只有声明了上传游标的来源才计入，其余恒 0。
    #[test]
    fn 注册表_待上传只计有游标的来源() {
        let cfg = SyncConfig::default();
        // 生产注册表：四个来源都已接入同步上传（各自独立的 rowid 游标字段，
        // 命名口径 SyncConfig::last_uploaded_<source>_rowid），故都参与计数
        //（本断言只读游标字段，不触发任何派生库查询）
        assert_eq!(
            find(KIMI).unwrap().sync_cursor(&cfg),
            Some(cfg.last_uploaded_kimi_rowid)
        );
        assert_eq!(
            find(DSH).unwrap().sync_cursor(&cfg),
            Some(cfg.last_uploaded_dsh_rowid)
        );
        assert_eq!(
            find(OPENCODE).unwrap().sync_cursor(&cfg),
            Some(cfg.last_uploaded_opencode_rowid)
        );
        assert_eq!(
            find(MINIMAXCODE).unwrap().sync_cursor(&cfg),
            Some(cfg.last_uploaded_minimaxcode_rowid)
        );

        // 无游标来源（未来新增但尚未接入同步的 agent）：max_rowid 再大也计 0，
        // 不产生「永远有待上传」的假信号
        assert_eq!(NO_CURSOR.pending_rows(&cfg), 0);
        // 有游标来源：max_rowid − 游标（未上传过 = 全量）
        assert_eq!(CURSOR.pending_rows(&cfg), 500);
        let mut advanced = SyncConfig::default();
        advanced.last_uploaded_dsh_rowid = 480;
        assert_eq!(CURSOR.pending_rows(&advanced), 20);
        // 游标超过 max_rowid（乱序/回退）不产生负数
        advanced.last_uploaded_dsh_rowid = 999;
        assert_eq!(CURSOR.pending_rows(&advanced), 0);
        // 合计只累加有游标的来源
        assert_eq!(pending_rows(&[&NO_CURSOR, &CURSOR], &cfg), 500);
    }

    /// 「注册表加一行」的等价验证：把假来源放进同一个遍历辅助函数（生产调用
    /// 方传 `all()`），四个项目维度查询与对比页分发都必须路由到它。
    #[test]
    fn 注册表_四个项目查询与对比分发路由到新来源() {
        let sources: &[&'static dyn LocalAgentSource] = &[&FAKE];

        // 1) 项目 × 模型聚合
        let rows = project_model_rows(sources, 0, 100);
        assert_eq!(rows.len(), 1, "每个注册来源产出一项");
        assert_eq!(rows[0].0, "fake-agent", "结果带来源名（日志/归属用）");
        let rows = rows[0].1.as_ref().expect("假来源不该失败");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].project_key, FAKE_PROJECT);
        assert_eq!(rows[0].model_id, FAKE_MODEL);

        // 2) 项目会话数
        let counts = project_session_counts(sources, 0, 100);
        assert_eq!(counts[0].0, "fake-agent");
        assert_eq!(
            counts[0].1.as_ref().unwrap(),
            &vec![(FAKE_PROJECT.to_string(), 7)]
        );

        // 3) 项目展示路径
        let paths = project_display_paths(sources);
        assert_eq!(paths[0].0, "fake-agent");
        assert_eq!(
            paths[0].1.as_ref().unwrap(),
            &vec![(FAKE_PROJECT.to_string(), "C:/fake".to_string())]
        );

        // 4) 项目会话明细（单来源分发，入参 project_key 原样透传）
        let page = project_sessions(sources, "fake-agent", FAKE_PROJECT, 0, 100, 0, 20)
            .expect("注册来源必须能分发");
        let (total, items) = page.expect("假来源不该失败");
        assert_eq!(total, 9);
        assert_eq!(items[0].session_id, "fake-session");
        assert_eq!(items[0].last_at, 100);
        // 未注册来源名：None（调用方按各自既有口径处理，不在这里编造错误文案）
        assert!(project_sessions(sources, "kimi", FAKE_PROJECT, 0, 100, 0, 20).is_none());

        // 5) 对比页周期聚合分发
        let buckets = compare_period_buckets(sources, "fake-agent", &[(0, 1_000), (1_000, 2_000)])
            .expect("注册来源必须能分发")
            .expect("假来源不该失败");
        assert_eq!(buckets.len(), 2);
        assert!(buckets.iter().all(|b| b.total_tokens == 42 && b.requests == 1));
        assert_eq!(buckets[1].reset_at, 1_000);
        assert!(compare_period_buckets(sources, "opencode", &[(0, 1)]).is_none());
    }

    /// 窗口模型速度区的遍历：同样路由到新来源；查询失败的来源静默为空
    /// （未安装 agent 的常态，不能让某个来源拖垮整块速度区）。
    #[test]
    fn 注册表_窗口模型速度区遍历() {
        let sources: &[&'static dyn LocalAgentSource] = &[&FAKE, &NO_CURSOR];
        let lists = window_model_speeds(sources, 1_000, 3);
        assert_eq!(lists.len(), 2, "逐来源返回一项（与 sources 等长）");
        assert_eq!(lists[0].len(), 1);
        assert_eq!(lists[0][0].model, FAKE_SPEED_MODEL);
        assert_eq!(lists[0][0].agent, "fake-agent");
        assert_eq!(lists[0][0].last_at, 777);
        assert!(lists[1].is_empty(), "查询失败的来源静默为空");
        // 生产注册表：逐来源一项（真实查询只读派生库；各来源行内容由模块
        // 自己的测试与真机冒烟覆盖）
        assert_eq!(window_model_speeds(all(), 1_000, 3).len(), all().len());
    }

    /// 今日 stats 与模型清单的遍历：同样路由到新来源；查询失败的来源静默跳过
    /// （未安装 agent 的常态，不能让某个来源拖垮合计）。
    #[test]
    fn 注册表_今日stats与模型清单遍历() {
        let sources: &[&'static dyn LocalAgentSource] = &[&FAKE, &NO_CURSOR];

        let stats = stats(sources, 10, 20);
        assert_eq!(stats.len(), 1, "NO_CURSOR 查询失败被静默跳过");
        assert_eq!(stats[0].0, "fake-agent");
        assert_eq!(stats[0].1.overall.total_tokens, 37);
        assert_eq!(stats[0].1.by_model.len(), 1, "托盘标题按 by_model 计费");

        let lists = model_lists(sources);
        assert_eq!(lists.len(), 2, "模型清单逐来源返回（失败项为空表）");
        assert_eq!(lists[0][0].model_id, FAKE_MODEL);
        assert!(lists[1].is_empty());
    }

    /// 遍历辅助函数只认传入的集合，不偷看全局注册表——这正是单测能注入假来源
    /// 的前提，也保证生产路径（传 `all()`）与测试路径走的是同一份代码。
    #[test]
    fn 注册表_遍历只认传入集合() {
        let fake_only: &[&'static dyn LocalAgentSource] = &[&FAKE];
        assert!(find_in(fake_only, DSH).is_none());
        assert!(find_in(fake_only, "fake-agent").is_some());
        assert_eq!(project_model_rows(fake_only, 0, 1).len(), 1);
        assert_eq!(project_model_rows(all(), 0, 1).len(), all().len());
    }
}
