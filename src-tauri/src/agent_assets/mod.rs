//! 「Agent 与 Skill 管理」（agent_assets）：桌面端骨架与首期适配器（A + C/D）。
//!
//! 上游方案：`doc/agent-skill-management-plan.md`；三端（服务端 B、适配器 C/D/E、
//! 前端 F）共同遵守的契约见 `doc/agent-assets-contract.md`，本目录的 Rust 结构体
//! 与契约 JSON 字段一一对应，改动任意一侧必须同步另一侧。
//!
//! 模块划分（与契约章节对应）：
//!
//! - [`safety`]：§9 路径安全（穿越/绝对路径/Windows 设备名/8.3 短名/大小写碰撞/
//!   符号链接与 junction），客户端侧执行，服务端按同一规则复验；
//! - [`bundle`]：§3 资源包打包（磁盘收集→安全检查→SHA-256→清单+内容）、
//!   解析与指纹复验；
//! - [`adapter`]：§6 适配器 trait 与 DTO；[`adapters`] 内为各工具实现
//!   （zcode 参考实现 + codex/claude/kimi/opencode 首期适配器）；
//! - [`capabilities`]：§5 能力位四档静态矩阵（DSH/MiniMax 占位待发行版验证）；
//! - [`registry`]：工具 ID → 适配器实例的分发；
//! - [`install`]：共用安装 / 原生导出层（指纹复验、路径安全、冲突备份、
//!   同卷暂存 + 原子发布）；
//! - [`transport`]：§8 服务端 HTTP 客户端（复用 `load_sync_config` 的地址凭证；
//!   旧服务端无能力接口时返回明确错误）。
//!
//! 本模块不写 `local_agents.rs` 的用量来源接口（方案明确两者隔离）。

pub mod adapter;
pub mod adapters;
pub mod bundle;
pub mod capabilities;
pub mod install;
pub mod registry;
pub mod safety;
pub mod transport;

use serde::{Deserialize, Serialize};

// ===== 核心枚举（契约 §2）=====

/// 资源作用域：用户级（`user`）/ 项目级（`project`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    User,
    Project,
}

impl Scope {
    /// 契约 wire 字符串（错误信息与日志用）。
    pub fn as_str(&self) -> &'static str {
        match self {
            Scope::User => "user",
            Scope::Project => "project",
        }
    }
}

/// 资源类型：子代理（`agent`）/ 技能（`skill`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AssetKind {
    Agent,
    Skill,
}

impl AssetKind {
    /// 契约 wire 字符串。
    pub fn as_str(&self) -> &'static str {
        match self {
            AssetKind::Agent => "agent",
            AssetKind::Skill => "skill",
        }
    }
}

// ===== 统一错误（契约 §4）=====

/// agent_assets 全链路统一错误：`code` 为契约 §4.1 稳定字符串，前端据此分支；
/// `message` 为可读中文；`details` 携带结构化补充（如 409 冲突的当前版本）。
///
/// Tauri 命令以本类型作为 `Result::Err`，序列化后前端 catch 到的对象即
/// `{ code, message, details? }`。服务端返回的错误码原样透传。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssetError {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub details: Option<serde_json::Value>,
}

impl AssetError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            message: message.into(),
            details: None,
        }
    }

    /// 附结构化 details（链式）。
    pub fn with_details(mut self, details: serde_json::Value) -> Self {
        self.details = Some(details);
        self
    }

    /// 错误码读取（前端/后续工作包分支用）。
    #[allow(dead_code)]
    pub fn code(&self) -> &str {
        &self.code
    }

    // ---- 常用构造器（码表见契约 §4.1）----

    pub fn config_missing(message: impl Into<String>) -> Self {
        Self::new("CONFIG_MISSING", message)
    }
    // 以下构造器当前主线尚未调用：old_server/unauthorized 在 transport 里以
    // 状态对象（server_status）形式表达，not_implemented 留给 C/D/E 适配器。
    // 它们是契约 §4.1 的完整码表实现，保留给后续工作包与测试直接使用。
    #[allow(dead_code)]
    pub fn old_server(message: impl Into<String>) -> Self {
        Self::new("OLD_SERVER_UNSUPPORTED", message)
    }
    pub fn network(message: impl Into<String>) -> Self {
        Self::new("NETWORK_ERROR", message)
    }
    #[allow(dead_code)]
    pub fn unauthorized(message: impl Into<String>) -> Self {
        Self::new("UNAUTHORIZED", message)
    }
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new("NOT_FOUND", message)
    }
    pub fn validation(message: impl Into<String>) -> Self {
        Self::new("VALIDATION_FAILED", message)
    }
    pub fn path_unsafe(message: impl Into<String>) -> Self {
        Self::new("PATH_UNSAFE", message)
    }
    pub fn too_many_entries(message: impl Into<String>) -> Self {
        Self::new("TOO_MANY_ENTRIES", message)
    }
    pub fn payload_too_large(message: impl Into<String>) -> Self {
        Self::new("PAYLOAD_TOO_LARGE", message)
    }
    pub fn capability_unsupported(message: impl Into<String>) -> Self {
        Self::new("CAPABILITY_UNSUPPORTED", message)
    }
    pub fn adapter_not_found(message: impl Into<String>) -> Self {
        Self::new("ADAPTER_NOT_FOUND", message)
    }
    #[allow(dead_code)]
    pub fn not_implemented(message: impl Into<String>) -> Self {
        Self::new("NOT_IMPLEMENTED", message)
    }
    pub fn io(message: impl Into<String>) -> Self {
        Self::new("IO_ERROR", message)
    }
    pub fn bundle_invalid(message: impl Into<String>) -> Self {
        Self::new("BUNDLE_INVALID", message)
    }
    pub fn internal(message: impl Into<String>) -> Self {
        Self::new("INTERNAL", message)
    }
}

impl std::fmt::Display for AssetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for AssetError {}

/// 本地 IO/系统错误统一折算（沿用项目 `Result<T, String>` 风格的中文报错）。
impl From<std::io::Error> for AssetError {
    fn from(e: std::io::Error) -> Self {
        AssetError::io(format!("本地文件读写失败: {e}"))
    }
}

// ===== Tauri 命令（契约 §7；lib.rs 已注册）=====
//
// 全部 async + spawn_blocking（同 lib.rs 现有远端命令风格）：文件扫描、哈希与
// HTTP 都不占主线程。命令名即前端 invoke 名。

use crate::agent_assets::adapter::{
    DiscoveredAsset, ExportPlan, ImportCandidate, InspectionReport,
};
use crate::agent_assets::bundle::AssetBundle;
use crate::agent_assets::capabilities::CapabilityEntry;
use crate::agent_assets::install::{DiskExportResult, InstallResult};
use crate::agent_assets::transport::{
    ListQuery, RemoteAssetPage, RemoteAssetResult, RemoteRevision, ServerAssetStatus,
};

/// spawn_blocking 的 JoinError 统一折 INTERNAL（与 lib.rs 的远端任务失败口径一致）。
fn join_error(e: tauri::Error) -> AssetError {
    AssetError::internal(format!("后台任务失败: {e}"))
}

/// 本地能力矩阵（契约 §5；含 DSH/MiniMax 的 pending 占位）。
#[tauri::command]
pub async fn agent_assets_capabilities() -> Result<Vec<CapabilityEntry>, AssetError> {
    Ok(
        tauri::async_runtime::spawn_blocking(registry::all_capabilities)
            .await
            .map_err(join_error)?,
    )
}

/// 探测服务端 agent-assets 支持状态与容量限制（契约 §8.1）。
/// 旧服务端无该路由 → `supported=false, reason="OLD_SERVER_UNSUPPORTED"`，
/// 前端提示升级后仍可使用本地发现/打包/原生导出。
#[tauri::command]
pub async fn agent_assets_server_status() -> Result<ServerAssetStatus, AssetError> {
    tauri::async_runtime::spawn_blocking(transport::server_status)
        .await
        .map_err(join_error)?
}

/// 只读扫描某工具某作用域下的资源（契约 §7 discover；先做能力预检）。
#[tauri::command]
pub async fn agent_assets_discover(
    tool: String,
    kind: AssetKind,
    scope: Scope,
    project_root: Option<String>,
) -> Result<Vec<DiscoveredAsset>, AssetError> {
    tauri::async_runtime::spawn_blocking(move || {
        registry::ensure_scannable(&tool, kind, scope)?;
        let adapter = registry::adapter_for(&tool)?;
        adapter.discover(
            kind,
            scope,
            project_root.as_deref().map(std::path::Path::new),
        )
    })
    .await
    .map_err(join_error)?
}

/// 解析单个资源元数据（frontmatter 失败只降级为警告，不阻断归档）。
#[tauri::command]
pub async fn agent_assets_inspect(
    tool: String,
    kind: AssetKind,
    scope: Scope,
    native_id: String,
    project_root: Option<String>,
) -> Result<InspectionReport, AssetError> {
    tauri::async_runtime::spawn_blocking(move || {
        let adapter = registry::adapter_for(&tool)?;
        adapter.inspect(
            kind,
            scope,
            &native_id,
            project_root.as_deref().map(std::path::Path::new),
        )
    })
    .await
    .map_err(join_error)?
}

/// 生成导出计划：文件清单 + 敏感字段提示（上传预览的数据源，契约 §7；
/// 先做可归档能力预检）。
#[tauri::command]
pub async fn agent_assets_plan_export(
    tool: String,
    kind: AssetKind,
    scope: Scope,
    native_id: String,
    project_root: Option<String>,
    source_override: Option<String>,
) -> Result<ExportPlan, AssetError> {
    tauri::async_runtime::spawn_blocking(move || {
        build_export_plan(
            &tool,
            kind,
            scope,
            &native_id,
            project_root.as_deref().map(std::path::Path::new),
            source_override.as_deref().map(std::path::Path::new),
        )
    })
    .await
    .map_err(join_error)?
}

/// 打包：plan_export → 磁盘收集 → 路径安全检查 → SHA-256 → 清单+内容
/// （契约 §3；返回信封可直接 POST /agent-assets）。
#[tauri::command]
pub async fn agent_assets_pack(
    tool: String,
    kind: AssetKind,
    scope: Scope,
    native_id: String,
    project_root: Option<String>,
    source_override: Option<String>,
) -> Result<AssetBundle, AssetError> {
    tauri::async_runtime::spawn_blocking(move || {
        let plan = build_export_plan(
            &tool,
            kind,
            scope,
            &native_id,
            project_root.as_deref().map(std::path::Path::new),
            source_override.as_deref().map(std::path::Path::new),
        )?;
        // 普通资源从磁盘收集；jsonc-inline 使用适配器提取的内存片段（契约 §3.4）。
        bundle::pack_plan(&plan)
    })
    .await
    .map_err(join_error)?
}

/// 计划生成共用入口：普通流程走 discover；`source_override` 走手选导入路径。
fn build_export_plan(
    tool: &str,
    kind: AssetKind,
    scope: Scope,
    native_id: &str,
    project_root: Option<&std::path::Path>,
    source_override: Option<&std::path::Path>,
) -> Result<ExportPlan, AssetError> {
    registry::ensure_archivable(tool, kind, scope)?;
    let adapter = registry::adapter_for(tool)?;
    match source_override {
        Some(source) => {
            let plan = adapter.plan_export_from_source(kind, scope, source)?;
            if plan.tool != tool || plan.kind != kind || plan.native_id != native_id {
                return Err(AssetError::validation(format!(
                    "source_override 解析结果与请求不一致：tool={}、kind={}、native_id={}；期望 tool={tool}、kind={}、native_id={native_id}",
                    plan.tool,
                    plan.kind.as_str(),
                    plan.native_id,
                    kind.as_str()
                )));
            }
            Ok(plan)
        }
        None => adapter.plan_export(kind, scope, native_id, project_root),
    }
}

/// 手选原生文件/目录导入预览：逐项解析，单项失败不影响整批。
///
/// 返回 `ImportCandidate`：成功项复用 `DiscoveredAsset`/`InspectionReport`
/// 字段；不识别项 `importable=false` + `metadata.parse_error` + `error`。
#[tauri::command]
pub async fn agent_assets_import_files(
    tool: String,
    kind: String,
    paths: Vec<String>,
) -> Vec<ImportCandidate> {
    let fallback_tool = tool.clone();
    let fallback_kind = kind.clone();
    let fallback_paths = paths.clone();
    match tauri::async_runtime::spawn_blocking(move || import_files_impl(&tool, &kind, &paths))
        .await
    {
        Ok(items) => items,
        Err(e) => fallback_paths
            .into_iter()
            .map(|p| {
                ImportCandidate::error(
                    &fallback_tool,
                    &fallback_kind,
                    &p,
                    format!("后台任务失败: {e}"),
                )
            })
            .collect(),
    }
}

fn import_files_impl(tool: &str, kind_raw: &str, paths: &[String]) -> Vec<ImportCandidate> {
    let kind = match kind_raw {
        "agent" => Ok(AssetKind::Agent),
        "skill" => Ok(AssetKind::Skill),
        other => Err(format!("kind 必须是 agent 或 skill: {other}")),
    };
    let adapter = registry::adapter_for(tool);
    paths
        .iter()
        .map(|path| {
            let p = std::path::Path::new(path);
            match (&kind, &adapter) {
                (Err(msg), _) => ImportCandidate::error(tool, kind_raw, path, msg.clone()),
                (_, Err(e)) => ImportCandidate::error(tool, kind_raw, path, e.message.clone()),
                (Ok(k), Ok(a)) => match a.import_path(*k, p) {
                    Ok(report) => ImportCandidate::from_report(report, path.clone()),
                    Err(e) => ImportCandidate::error(tool, kind_raw, path, e.message),
                },
            }
        })
        .collect()
}

/// 远端资源列表（GET /agent-assets，仅元数据 + 分页筛选）。
#[tauri::command]
pub async fn agent_assets_list_remote(query: ListQuery) -> Result<RemoteAssetPage, AssetError> {
    tauri::async_runtime::spawn_blocking(move || transport::list_remote_assets(query))
        .await
        .map_err(join_error)?
}

/// 上传（POST /agent-assets；client_request_id 幂等，契约 §8.6）。
#[tauri::command]
pub async fn agent_assets_upload(
    bundle: AssetBundle,
    client_request_id: String,
) -> Result<RemoteAssetResult, AssetError> {
    tauri::async_runtime::spawn_blocking(move || {
        transport::upload_asset(&bundle, &client_request_id)
    })
    .await
    .map_err(join_error)?
}

/// 更新（PUT /agent-assets/<id>；expected_version 冲突返回 409，契约 §8.4/8.6）。
#[tauri::command]
pub async fn agent_assets_update_remote(
    asset_id: String,
    expected_version: u64,
    bundle: AssetBundle,
) -> Result<RemoteAssetResult, AssetError> {
    tauri::async_runtime::spawn_blocking(move || {
        transport::update_asset(&asset_id, expected_version, &bundle)
    })
    .await
    .map_err(join_error)?
}

/// 某资源的版本列表（GET /agent-assets/<id>/revisions，按版本降序）。
#[tauri::command]
pub async fn agent_assets_list_revisions(
    asset_id: String,
) -> Result<Vec<RemoteRevision>, AssetError> {
    tauri::async_runtime::spawn_blocking(move || transport::list_revisions(&asset_id))
        .await
        .map_err(join_error)?
}

/// 下载某版本资源包（GET /agent-assets/<id>/revisions/<rev>/bundle）。
/// 返回信封未做指纹复验；安装/预览前由调用方走 bundle::parse_bundle 复验。
#[tauri::command]
pub async fn agent_assets_fetch_bundle(
    asset_id: String,
    version: u64,
) -> Result<AssetBundle, AssetError> {
    tauri::async_runtime::spawn_blocking(move || transport::fetch_bundle(&asset_id, version))
        .await
        .map_err(join_error)?
}

/// 原生导出：把资源包还原为 `dest_dir/<tool>-<kind>-<native_id>/` 下的文件。
/// 先复验指纹；目标已存在默认跳过，`overwrite=true` 先备份再写（契约 §9）。
#[tauri::command]
pub async fn agent_assets_export_to_disk(
    bundle: AssetBundle,
    dest_dir: String,
    overwrite: bool,
) -> Result<DiskExportResult, AssetError> {
    tauri::async_runtime::spawn_blocking(move || {
        install::export_bundle(&bundle, std::path::Path::new(&dest_dir), overwrite)
    })
    .await
    .map_err(join_error)?
}

/// 安装到本机默认目录（仅能力矩阵声明 installable 的工具×kind×作用域；
/// 先复验指纹、路径安全，目标已存在默认跳过，覆盖才备份）。
#[tauri::command]
pub async fn agent_assets_install(
    bundle: AssetBundle,
    target_scope: Scope,
    project_root: Option<String>,
    overwrite: bool,
) -> Result<InstallResult, AssetError> {
    tauri::async_runtime::spawn_blocking(move || {
        install::install_bundle(
            &bundle,
            target_scope,
            project_root.as_deref().map(std::path::Path::new),
            overwrite,
        )
    })
    .await
    .map_err(join_error)?
}

// ============================================================
// 测试支撑（仅测试编译）：临时目录助手。绝不触碰用户真实 ~/.zcode。
// ============================================================

#[cfg(test)]
pub(crate) mod tests_support {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static SEQ: AtomicU64 = AtomicU64::new(0);

    /// 系统临时目录下按「标签-进程-纳秒-序号」建唯一目录（含创建）。
    /// 调用方测试结束自行 remove_dir_all（失败忽略）。
    pub fn temp_root(tag: &str) -> PathBuf {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "agenthud-assets-{tag}-{}-{now}-{seq}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("创建测试临时目录失败");
        dir
    }
}

// ============================================================
// 单测：能力矩阵与错误形状的静态约束（打包/路径/适配器用例在各自模块）
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_kind_wire_strings() {
        assert_eq!(Scope::User.as_str(), "user");
        assert_eq!(Scope::Project.as_str(), "project");
        assert_eq!(AssetKind::Agent.as_str(), "agent");
        assert_eq!(AssetKind::Skill.as_str(), "skill");
        // serde wire 形状与契约一致（小写字符串）
        assert_eq!(
            serde_json::to_string(&Scope::Project).unwrap(),
            "\"project\""
        );
        assert_eq!(
            serde_json::to_string(&AssetKind::Skill).unwrap(),
            "\"skill\""
        );
    }

    #[test]
    fn asset_error_serializes_contract_shape() {
        let err = AssetError::new("VERSION_CONFLICT", "预期版本 3，服务端当前为 5")
            .with_details(serde_json::json!({ "current_version": 5 }));
        let v = serde_json::to_value(&err).unwrap();
        assert_eq!(v["code"], "VERSION_CONFLICT");
        assert_eq!(v["details"]["current_version"], 5);
        // details 为 None 时省略（契约 §4.3）
        let plain = serde_json::to_value(AssetError::not_found("不存在")).unwrap();
        assert!(plain.get("details").is_none());
    }

    #[test]
    fn capability_matrix_basic_invariants() {
        let entries = registry::all_capabilities();
        // 七款工具全部在矩阵中
        let tools: std::collections::BTreeSet<_> =
            entries.iter().map(|e| e.tool.as_str()).collect();
        for t in [
            "zcode", "codex", "claude", "kimi", "opencode", "dsh", "minimax",
        ] {
            assert!(tools.contains(t), "矩阵缺少工具 {t}");
        }
        // 依赖关系：installable ⊆ native_exportable ⊆ archivable ⊆ scannable
        for e in &entries {
            if e.installable {
                assert!(
                    e.native_exportable && e.archivable && e.scannable,
                    "{} 违反依赖",
                    e.tool
                );
            }
            if e.native_exportable {
                assert!(e.archivable && e.scannable, "{} 违反依赖", e.tool);
            }
            if e.archivable {
                assert!(e.scannable, "{} 违反依赖", e.tool);
            }
            // pending 条目一律全 false（前端不可操作）
            if e.status == "pending" {
                assert!(!e.scannable && !e.archivable && !e.native_exportable && !e.installable);
            }
        }
        // 关键条目抽查：zcode 用户级 agent 四档全开；dsh 占位
        let zc = entries
            .iter()
            .find(|e| e.tool == "zcode" && e.kind == AssetKind::Agent && e.scope == Scope::User)
            .expect("zcode 用户级 agent 条目");
        assert!(zc.scannable && zc.archivable && zc.native_exportable && zc.installable);
        assert!(entries
            .iter()
            .any(|e| e.tool == "dsh" && e.status == "pending"));
        assert!(entries
            .iter()
            .any(|e| e.tool == "minimax" && e.status == "pending"));
        // 未知工具查不到适配器
        assert!(registry::adapter_for("nope").is_err());
    }

    #[test]
    fn import_files_three_states_and_source_override_round_trip() {
        use crate::agent_assets::bundle;

        let root = crate::agent_assets::tests_support::temp_root("import-files");
        let agent = root.join("manual-agent.md");
        let agent_body = "---\nname: manual-agent\ndescription: 手选导入样例\n---\n正文".as_bytes();
        std::fs::write(&agent, agent_body).unwrap();

        let skill = root.join("manual-skill");
        std::fs::create_dir_all(skill.join("scripts")).unwrap();
        std::fs::create_dir_all(skill.join("assets")).unwrap();
        std::fs::write(
            skill.join("SKILL.md"),
            "---\nname: manual-skill\ndescription: 手选 skill\n---\n正文",
        )
        .unwrap();
        std::fs::write(skill.join("scripts/run.sh"), b"#!/bin/sh\necho hi\n").unwrap();
        std::fs::write(skill.join("assets/logo.png"), [0x89u8, 0x50, 0x4E, 0x00]).unwrap();

        let notes = root.join("notes.txt");
        std::fs::write(&notes, b"not an agent").unwrap();

        // agent 批次：agent 文件可导入，txt 不识别
        let items = import_files_impl(
            "claude",
            "agent",
            &[
                agent.to_string_lossy().into_owned(),
                notes.to_string_lossy().into_owned(),
            ],
        );
        assert_eq!(items.len(), 2);
        assert!(items[0].importable);
        let asset = items[0].asset.as_ref().expect("可导入项应有 asset");
        assert_eq!(asset.native_id, "manual-agent");
        assert_eq!(asset.native_format, "markdown-frontmatter");
        assert!(items[0].metadata.as_ref().expect("元数据").parsed_ok);
        assert_eq!(
            std::path::Path::new(items[0].source_root.as_deref().unwrap()),
            root.as_path()
        );
        assert!(!items[1].importable);
        assert!(items[1]
            .metadata
            .as_ref()
            .expect("错误项应有 metadata.parse_error")
            .parse_error
            .is_some());
        assert!(items[1].error.is_some());
        assert!(items[1].warnings.iter().any(|w| w.contains("扩展名")));

        // skill 批次：skill 目录可导入，agent 文件与 txt 不识别
        let items = import_files_impl(
            "claude",
            "skill",
            &[
                skill.to_string_lossy().into_owned(),
                agent.to_string_lossy().into_owned(),
                notes.to_string_lossy().into_owned(),
            ],
        );
        assert_eq!(items.len(), 3);
        assert!(items[0].importable);
        assert_eq!(items[0].asset.as_ref().unwrap().native_id, "manual-skill");
        assert_eq!(items[0].files.len(), 3, "SKILL.md + 脚本 + 二进制附件");
        assert!(!items[1].importable);
        assert!(!items[2].importable);

        // 非法 kind：逐项错误，不整批失败
        let items = import_files_impl("claude", "nope", &[agent.to_string_lossy().into_owned()]);
        assert_eq!(items.len(), 1);
        assert!(!items[0].importable);
        assert!(items[0].error.as_deref().unwrap().contains("kind"));

        // source_override：从手选 agent 文件生成计划并打包，指纹往返一致
        let plan = build_export_plan(
            "claude",
            AssetKind::Agent,
            Scope::User,
            "manual-agent",
            None,
            Some(agent.as_path()),
        )
        .unwrap();
        assert_eq!(plan.native_id, "manual-agent");
        assert_eq!(plan.entry_path, "manual-agent.md");
        assert_eq!(std::path::Path::new(&plan.source_root), root.as_path());
        let envelope = bundle::pack_plan(&plan).unwrap();
        let parsed = bundle::parse_bundle(&envelope).unwrap();
        assert_eq!(parsed.files[0].1, agent_body);
        assert_eq!(
            bundle::manifest_digest(&parsed.manifest),
            envelope.manifest_sha256
        );
        assert_eq!(bundle::bundle_digest(&parsed.files), envelope.bundle_sha256);

        // native_id 不匹配时明确拒绝
        let err = build_export_plan(
            "claude",
            AssetKind::Agent,
            Scope::User,
            "other",
            None,
            Some(agent.as_path()),
        )
        .unwrap_err();
        assert_eq!(err.code(), "VALIDATION_FAILED");

        // source_override：skill 目录同样可打包
        let plan = build_export_plan(
            "claude",
            AssetKind::Skill,
            Scope::User,
            "manual-skill",
            None,
            Some(skill.as_path()),
        )
        .unwrap();
        let envelope = bundle::pack_plan(&plan).unwrap();
        let parsed = bundle::parse_bundle(&envelope).unwrap();
        assert_eq!(parsed.files.len(), 3);
        assert_eq!(bundle::bundle_digest(&parsed.files), envelope.bundle_sha256);

        let _ = std::fs::remove_dir_all(&root);
    }
}
