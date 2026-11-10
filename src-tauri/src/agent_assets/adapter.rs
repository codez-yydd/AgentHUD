//! §6 适配器 trait 与 DTO（契约 `doc/agent-assets-contract.md`）。
//!
//! 适配器只做三件事：`capabilities`（静态能力切片）、`discover`（只读扫描）、
//! `inspect`（元数据解析，失败不阻断归档）、`plan_export`（导出计划：文件清单 +
//! 敏感字段提示）。真正读内容、算哈希、路径安全检查、写盘安装都由共用层
//! （`bundle` / `safety`）完成——适配器永不自行读包外文件。

use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::agent_assets::{AssetError, AssetKind, Scope};

// ===== DTO（serde 序列化即契约 §7.1 的 JSON 形状）=====

/// discover 的一条结果：一个本机资源。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredAsset {
    pub tool: String,
    pub kind: AssetKind,
    pub scope: Scope,
    pub native_id: String,
    pub native_format: String,
    /// 显示名（frontmatter/配置缺失时取 native_id，不为空）。
    pub display_name: String,
    #[serde(default)]
    pub description: Option<String>,
    /// 入口文件相对路径（资源自身根为基准）。
    pub entry_path: String,
    /// 非致命提示（遮蔽、frontmatter 警告等），不阻断后续操作。
    #[serde(default)]
    pub warnings: Vec<String>,
    /// 资源根的本机绝对路径（仅本地展示与打包收集用，不上传服务端）。
    pub source_root: String,
}

/// inspect 的文件简要（不含内容）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileBrief {
    pub path: String,
    pub size: u64,
    pub executable: bool,
    pub binary: bool,
}

/// frontmatter/配置解析出的关键元数据（解析失败只置 `parsed_ok=false`）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AssetMetadata {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub tools: Vec<String>,
    /// frontmatter 原文（含未知字段，逐字节保留，预览用）。
    #[serde(default)]
    pub frontmatter_raw: Option<String>,
    pub parsed_ok: bool,
    #[serde(default)]
    pub parse_error: Option<String>,
}

/// 敏感字段提示（上传预览展示；契约 §9 末段）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SensitiveHint {
    pub file: String,
    pub field: String,
    pub reason: String,
}

/// inspect 结果：发现条目 + 元数据 + 文件清单 + 提示。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InspectionReport {
    pub asset: DiscoveredAsset,
    pub metadata: AssetMetadata,
    #[serde(default)]
    pub files: Vec<FileBrief>,
    #[serde(default)]
    pub sensitive_fields: Vec<SensitiveHint>,
    #[serde(default)]
    pub warnings: Vec<String>,
    /// 包外引用（只报告不递归，契约 §3.5）。
    #[serde(default)]
    pub external_references: Vec<String>,
}

/// 手选原生文件/目录导入的单项预览（`agent_assets_import_files`）。
///
/// 成功项复用 [`DiscoveredAsset`] / [`InspectionReport`] 的字段；失败项
/// `importable=false`、`asset=None`，原因同时写入
/// `metadata.parse_error` 与 `error`，保证整批不因单项失败而中断。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportCandidate {
    pub tool: String,
    /// 请求的 kind 原文（合法时为 `agent` / `skill`，非法时原样回显）。
    pub kind: String,
    /// 用户手选路径原文。
    pub path: String,
    pub importable: bool,
    /// 成功时的发现条目（含 native_id/native_format/source_root 等）。
    #[serde(default)]
    pub asset: Option<DiscoveredAsset>,
    /// 成功时为解析结果；失败时为 `parsed_ok=false` + `parse_error`。
    #[serde(default)]
    pub metadata: Option<AssetMetadata>,
    #[serde(default)]
    pub files: Vec<FileBrief>,
    #[serde(default)]
    pub sensitive_fields: Vec<SensitiveHint>,
    #[serde(default)]
    pub warnings: Vec<String>,
    #[serde(default)]
    pub external_references: Vec<String>,
    #[serde(default)]
    pub source_root: Option<String>,
    /// 不可导入原因（与 metadata.parse_error 同义，便于前端直接展示）。
    #[serde(default)]
    pub error: Option<String>,
}

impl ImportCandidate {
    /// 成功项：把 inspect 报告摊平为候选；`path` 为手选输入原文。
    pub fn from_report(report: InspectionReport, path: impl Into<String>) -> Self {
        Self {
            tool: report.asset.tool.clone(),
            kind: report.asset.kind.as_str().to_string(),
            path: path.into(),
            importable: true,
            source_root: Some(report.asset.source_root.clone()),
            asset: Some(report.asset),
            metadata: Some(report.metadata),
            files: report.files,
            sensitive_fields: report.sensitive_fields,
            warnings: report.warnings,
            external_references: report.external_references,
            error: None,
        }
    }

    /// 失败项：单项输入不识别/不安全时使用，不阻断同批其他路径。
    pub fn error(tool: &str, kind: &str, path: &str, reason: impl Into<String>) -> Self {
        let reason = reason.into();
        let metadata = AssetMetadata {
            parsed_ok: false,
            parse_error: Some(reason.clone()),
            ..Default::default()
        };
        Self {
            tool: tool.to_string(),
            kind: kind.to_string(),
            path: path.to_string(),
            importable: false,
            asset: None,
            metadata: Some(metadata),
            files: Vec::new(),
            sensitive_fields: Vec::new(),
            warnings: vec![reason.clone()],
            external_references: Vec::new(),
            source_root: None,
            error: Some(reason),
        }
    }
}

/// plan_export 的单文件计划（相对路径 + 声明位；哈希由打包阶段补齐）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlannedFile {
    pub path: String,
    pub executable: bool,
    pub binary: bool,
}

/// 导出计划：上传预览与打包的共同输入。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportPlan {
    pub tool: String,
    pub kind: AssetKind,
    pub native_format: String,
    pub native_id: String,
    pub display_name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub origin_scope: Scope,
    #[serde(default)]
    pub target_versions: Vec<String>,
    pub entry_path: String,
    /// 资源根本机绝对路径（bundle::pack_from_disk 的收集根）。
    pub source_root: String,
    pub files: Vec<PlannedFile>,
    #[serde(default)]
    pub sensitive_fields: Vec<SensitiveHint>,
    #[serde(default)]
    pub warnings: Vec<String>,
    #[serde(default)]
    pub external_references: Vec<String>,
    /// 内联配置资源（`jsonc-inline`）从配置中提取并序列化出的单个片段。
    ///
    /// 契约 §3.4 的例外：内联定义不整体打包，只打包提取出的条目。该字段只在
    /// 进程内传递给 `bundle::pack_plan`，**不参与序列化**（前端的 `ExportPlan`
    /// 预览不需要片段原文），因此不改变契约 §7.1 的 wire 形状。
    #[serde(skip)]
    pub inline_fragment: Option<String>,
}

/// [`ExportPlan`] 的链式构建器（适配器实现用，减少字段噪音）。
pub struct ExportPlanBuilder {
    plan: ExportPlan,
}

impl ExportPlanBuilder {
    pub fn new(tool: &str, kind: AssetKind, scope: Scope, native_id: &str) -> Self {
        Self {
            plan: ExportPlan {
                tool: tool.to_string(),
                kind,
                native_format: String::new(),
                native_id: native_id.to_string(),
                display_name: native_id.to_string(),
                description: None,
                origin_scope: scope,
                target_versions: Vec::new(),
                entry_path: String::new(),
                source_root: String::new(),
                files: Vec::new(),
                sensitive_fields: Vec::new(),
                warnings: Vec::new(),
                external_references: Vec::new(),
                inline_fragment: None,
            },
        }
    }

    pub fn native_format(mut self, v: &str) -> Self {
        self.plan.native_format = v.to_string();
        self
    }
    pub fn display_name(mut self, v: &str) -> Self {
        if !v.trim().is_empty() {
            self.plan.display_name = v.to_string();
        }
        self
    }
    pub fn description(mut self, v: Option<&str>) -> Self {
        self.plan.description = v.filter(|s| !s.trim().is_empty()).map(|s| s.to_string());
        self
    }
    pub fn entry_path(mut self, v: &str) -> Self {
        self.plan.entry_path = v.to_string();
        self
    }
    pub fn source_root(mut self, v: &Path) -> Self {
        self.plan.source_root = v.to_string_lossy().into_owned();
        self
    }
    pub fn file(mut self, f: PlannedFile) -> Self {
        self.plan.files.push(f);
        self
    }
    pub fn warning(mut self, v: String) -> Self {
        self.plan.warnings.push(v);
        self
    }
    pub fn sensitive(mut self, hints: Vec<SensitiveHint>) -> Self {
        self.plan.sensitive_fields.extend(hints);
        self
    }
    /// 内联配置资源的内存片段（`jsonc-inline` 专用，见字段注释）。
    pub fn inline_fragment(mut self, v: Option<String>) -> Self {
        self.plan.inline_fragment = v;
        self
    }
    pub fn build(self) -> ExportPlan {
        self.plan
    }
}

/// 由一次 inspect 报告生成导出计划（手选导入的 `source_override` 流程复用）。
pub fn plan_from_inspection(
    kind: AssetKind,
    scope: Scope,
    report: &InspectionReport,
) -> ExportPlan {
    let a = &report.asset;
    let root = Path::new(&a.source_root);
    let mut builder = ExportPlanBuilder::new(&a.tool, kind, scope, &a.native_id)
        .native_format(&a.native_format)
        .display_name(&a.display_name)
        .description(report.metadata.description.as_deref())
        .entry_path(&a.entry_path)
        .source_root(root)
        .sensitive(report.sensitive_fields.clone());
    for w in &report.warnings {
        builder = builder.warning(w.clone());
    }
    for f in &report.files {
        builder = builder.file(PlannedFile {
            path: f.path.clone(),
            executable: f.executable,
            binary: f.binary,
        });
    }
    builder.build()
}

// ===== trait（契约 §6）=====

/// 工具适配器。实现方注意：
///
/// - 三个数据方法一律**只读**，不写任何文件；
/// - `scope = Project` 时必须校验 `project_root` 是否提供；
/// - 未声明对应能力位的组合返回 `CAPABILITY_UNSUPPORTED`；
/// - `inspect` 的解析失败只进 warnings/metadata.parsed_ok，**绝不 Err**
///   （归档以原文件字节为真，契约 §6 约定）；
/// - `plan_export` 只列清单不读内容（大小/哈希由 bundle 阶段补）。
pub trait Adapter: Send + Sync {
    /// §2.1 工具 ID。
    fn tool(&self) -> &'static str;

    /// 该工具的能力矩阵切片（来自 capabilities 静态表，不得运行时编造）。
    ///
    /// 当前主线按 `registry::all_capabilities()` 整表下发前端，逐适配器调用
    /// `capabilities()` 的路径尚未接（留给 C/D 的「按工具过滤」页面形态）。
    /// 声明在 trait 里是为了契约完整性（同 local_agents.rs 的处理方式）。
    #[allow(dead_code)]
    fn capabilities(&self) -> Vec<crate::agent_assets::capabilities::CapabilityEntry>;

    /// 只读扫描某作用域下的资源。
    fn discover(
        &self,
        kind: AssetKind,
        scope: Scope,
        project_root: Option<&Path>,
    ) -> Result<Vec<DiscoveredAsset>, AssetError>;

    /// 解析单个资源元数据；资源不存在返回 NOT_FOUND。
    fn inspect(
        &self,
        kind: AssetKind,
        scope: Scope,
        native_id: &str,
        project_root: Option<&Path>,
    ) -> Result<InspectionReport, AssetError>;

    /// 生成导出计划（上传预览 + 打包输入）。
    fn plan_export(
        &self,
        kind: AssetKind,
        scope: Scope,
        native_id: &str,
        project_root: Option<&Path>,
    ) -> Result<ExportPlan, AssetError>;

    /// 手选导入：对任意绝对路径做只读解析，返回与 `discover + inspect` 同形的
    /// 报告。默认不支持；C/D 适配器各自实现（ZCode 参考实现同时补齐）。
    ///
    /// 失败必须是可读的 `VALIDATION_FAILED` / `PATH_UNSAFE` / `NOT_FOUND`，
    /// 命令层会折成单项 `ImportCandidate { importable: false, ... }`，不阻断
    /// 同批其他路径。
    fn import_path(&self, _kind: AssetKind, _path: &Path) -> Result<InspectionReport, AssetError> {
        Err(AssetError::capability_unsupported(format!(
            "工具 {} 暂不支持手选原生文件导入",
            self.tool()
        )))
    }

    /// 从显式资源根生成导出计划（`agent_assets_plan_export` /
    /// `agent_assets_pack` 的 `source_override` 参数）。默认不支持。
    fn plan_export_from_source(
        &self,
        _kind: AssetKind,
        _scope: Scope,
        _source: &Path,
    ) -> Result<ExportPlan, AssetError> {
        Err(AssetError::capability_unsupported(format!(
            "工具 {} 暂不支持从显式路径导出",
            self.tool()
        )))
    }
}

/// project 作用域缺少 project_root 时的统一报错（各适配器复用）。
pub fn require_project_root(scope: Scope, project_root: Option<&Path>) -> Result<(), AssetError> {
    if scope == Scope::Project && project_root.is_none() {
        return Err(AssetError::validation(
            "project 作用域必须提供 project_root（项目根绝对路径）",
        ));
    }
    Ok(())
}
