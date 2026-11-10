//! Claude Code 适配器（C 工作包，契约 §5.2/§6）。
//!
//! 官方目录与格式（2026-11-09 按 WebFetch 核对）：
//!
//! - 子代理：用户级 `~/.claude/agents/`、项目级 `<project>/.claude/agents/`，
//!   **递归**扫描 `*.md`；frontmatter 的 `name` 是唯一身份（文件名只是发现键，
//!   子目录不影响身份）。官方文档：
//!   <https://code.claude.com/docs/en/sub-agents>
//! - Skill：用户级 `~/.claude/skills/<name>/SKILL.md`、项目级
//!   `<project>/.claude/skills/<name>/SKILL.md`，保留子目录与附属文件。官方文档：
//!   <https://code.claude.com/docs/en/skills>
//!
//! 关键行为：
//! - `tools`/`permissionMode`/`hooks`/`mcpServers` 等字段**原样字节保留**
//!   （适配器从不重新序列化），inspect 只做敏感配置提示；
//! - 同一 `~/.claude/agents/` 树下多个文件声明同一个 `name` 时，Claude Code
//!   只加载其中一个且优先级未文档化；discover 只产出一个资源并记
//!   `native_id 冲突` 警告；
//! - 无 `name` 或 `name` 非法的文件会被 Claude Code 当作文档跳过，本适配器
//!   不纳管（discover 不返回）。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::agent_assets::adapter::{
    require_project_root, Adapter, AssetMetadata, DiscoveredAsset, ExportPlan, ExportPlanBuilder,
    FileBrief, InspectionReport, PlannedFile, SensitiveHint,
};
use crate::agent_assets::adapters::common;
use crate::agent_assets::adapters::zcode::{parse_frontmatter, Frontmatter};
use crate::agent_assets::capabilities::{entries_for, CapabilityEntry, CLAUDE};
use crate::agent_assets::{AssetError, AssetKind, Scope};

const FM: &str = "markdown-frontmatter";
const SKILL_DIR: &str = "skill-dir";

/// Claude Code 内置子代理（自定义同名文件会覆盖内置定义）。
const BUILTIN_AGENTS: [&str; 6] = [
    "explore",
    "plan",
    "general-purpose",
    "claude",
    "statusline-setup",
    "claude-code-guide",
];

/// Claude Code 适配器；`home` 供测试注入，全部只读。
pub struct ClaudeAdapter {
    home: PathBuf,
}

impl ClaudeAdapter {
    pub fn new() -> Self {
        Self {
            home: common::home_dir(),
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn with_home(home: PathBuf) -> Self {
        Self { home }
    }

    fn agents_root(&self, scope: Scope, project_root: Option<&Path>) -> PathBuf {
        match scope {
            Scope::User => self.home.join(".claude").join("agents"),
            Scope::Project => project_root
                .unwrap_or_else(|| Path::new(""))
                .join(".claude")
                .join("agents"),
        }
    }

    fn skills_root(&self, scope: Scope, project_root: Option<&Path>) -> PathBuf {
        match scope {
            Scope::User => self.home.join(".claude").join("skills"),
            Scope::Project => project_root
                .unwrap_or_else(|| Path::new(""))
                .join(".claude")
                .join("skills"),
        }
    }
}

impl Default for ClaudeAdapter {
    fn default() -> Self {
        Self::new()
    }
}

/// `name` 是否满足 Claude Code 的加载条件（非空、≤256、不以 `-` 开头、不含 `:`）。
fn valid_agent_name(name: &str) -> bool {
    let n = name.trim();
    !n.is_empty() && n.chars().count() <= 256 && !n.starts_with('-') && !n.contains(':')
}

fn builtin_agent_warning(name: &str) -> Option<String> {
    if BUILTIN_AGENTS.iter().any(|b| b.eq_ignore_ascii_case(name)) {
        Some(format!(
            "Claude Code 内置 Agent `{name}` 会被同名自定义 Agent 覆盖（官方文档：user/project subagent overrides built-in）"
        ))
    } else {
        None
    }
}

/// Claude Code 敏感配置字段：通用敏感词 + hooks/权限/MCP/环境变量相关。
fn claude_sensitive_hints(file: &str, fm: &Frontmatter) -> Vec<SensitiveHint> {
    let mut out = Vec::new();
    for (k, _) in &fm.entries {
        let lower = k.to_ascii_lowercase();
        let specific = matches!(
            lower.as_str(),
            "hooks"
                | "permissionmode"
                | "permissions"
                | "mcpservers"
                | "allowed-tools"
                | "disallowedtools"
        );
        if common::sensitive_key(k) || specific {
            common::push_sensitive(
                &mut out,
                file,
                k,
                "Claude Code 敏感配置字段（hooks/权限/MCP/环境变量），安装前请确认",
            );
        }
    }
    out
}

struct AgentCandidate {
    rel: String,
    name: String,
    description: Option<String>,
    warnings: Vec<String>,
}

// ===== Adapter 实现 =====

impl Adapter for ClaudeAdapter {
    fn tool(&self) -> &'static str {
        CLAUDE
    }

    fn capabilities(&self) -> Vec<CapabilityEntry> {
        entries_for(CLAUDE)
    }

    fn discover(
        &self,
        kind: AssetKind,
        scope: Scope,
        project_root: Option<&Path>,
    ) -> Result<Vec<DiscoveredAsset>, AssetError> {
        require_project_root(scope, project_root)?;
        match kind {
            AssetKind::Agent => self.discover_agents(scope, project_root),
            AssetKind::Skill => self.discover_skills(scope, project_root),
        }
    }

    fn inspect(
        &self,
        kind: AssetKind,
        scope: Scope,
        native_id: &str,
        project_root: Option<&Path>,
    ) -> Result<InspectionReport, AssetError> {
        let found = self
            .discover(kind, scope, project_root)?
            .into_iter()
            .find(|a| a.native_id == native_id)
            .ok_or_else(|| {
                AssetError::not_found(format!(
                    "未找到 Claude Code {} 资源 {native_id}（作用域 {}）",
                    kind.as_str(),
                    scope.as_str()
                ))
            })?;
        self.inspect_found(kind, &found)
    }

    fn plan_export(
        &self,
        kind: AssetKind,
        scope: Scope,
        native_id: &str,
        project_root: Option<&Path>,
    ) -> Result<ExportPlan, AssetError> {
        let report = self.inspect(kind, scope, native_id, project_root)?;
        let a = &report.asset;
        let root = Path::new(&a.source_root);
        let mut builder = ExportPlanBuilder::new(a.tool.as_str(), kind, scope, native_id)
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
        Ok(builder.build())
    }

    fn import_path(&self, kind: AssetKind, path: &Path) -> Result<InspectionReport, AssetError> {
        let md = common::validate_import_source(path)?;
        let mut warnings = vec!["来源：手选原生文件导入".to_string()];
        let (root, entry_path, native_id, display, description) = match kind {
            AssetKind::Agent => {
                common::ensure_agent_file(path, &md, &[".md"])?;
                let file_name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let fm = common::read_text(path)
                    .as_deref()
                    .and_then(parse_frontmatter)
                    .ok_or_else(|| {
                        AssetError::validation(format!(
                            "Claude Code 只加载带 YAML frontmatter 的 .md: {}",
                            path.display()
                        ))
                    })?;
                let name = common::scalar_field(&fm, "name")
                    .filter(|n| valid_agent_name(n))
                    .ok_or_else(|| {
                        AssetError::validation(format!(
                            "Claude Code 需要有效的 frontmatter name（非空、≤256、不含 :）: {}",
                            path.display()
                        ))
                    })?;
                if let Some(w) = builtin_agent_warning(&name) {
                    warnings.push(w);
                }
                let description = common::scalar_field(&fm, "description");
                if description.is_none() {
                    warnings.push(
                        "frontmatter 缺少 description，Claude Code 可能跳过该子代理".to_string(),
                    );
                }
                let root = path.parent().unwrap_or_else(|| Path::new(""));
                (
                    root.to_path_buf(),
                    file_name,
                    name.clone(),
                    name,
                    description,
                )
            }
            AssetKind::Skill => {
                let root = common::import_skill_root(path, &md)?;
                let name = root
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                if !common::safe_segment(&name) {
                    return Err(AssetError::validation(format!(
                        "skill 目录名不能作为安全 native_id: {name}"
                    )));
                }
                let mut display = name.clone();
                let mut description = None;
                match common::read_text(&root.join("SKILL.md"))
                    .as_deref()
                    .and_then(parse_frontmatter)
                {
                    Some(fm) => {
                        if let Some(n) = common::scalar_field(&fm, "name") {
                            display = n;
                        }
                        description = common::scalar_field(&fm, "description");
                    }
                    None => warnings
                        .push("SKILL.md frontmatter 缺失或未闭合，元数据降级（不影响归档）".into()),
                }
                (
                    root,
                    "SKILL.md".to_string(),
                    name.clone(),
                    display,
                    description,
                )
            }
        };
        let asset = DiscoveredAsset {
            tool: CLAUDE.into(),
            kind,
            scope: Scope::User,
            native_id,
            native_format: if kind == AssetKind::Agent {
                FM.into()
            } else {
                SKILL_DIR.into()
            },
            display_name: display,
            description,
            entry_path,
            warnings,
            source_root: root.to_string_lossy().into_owned(),
        };
        self.inspect_found(kind, &asset)
    }

    fn plan_export_from_source(
        &self,
        kind: AssetKind,
        scope: Scope,
        source: &Path,
    ) -> Result<ExportPlan, AssetError> {
        let report = self.import_path(kind, source)?;
        Ok(crate::agent_assets::adapter::plan_from_inspection(
            kind, scope, &report,
        ))
    }
}

impl ClaudeAdapter {
    fn discover_agents(
        &self,
        scope: Scope,
        project_root: Option<&Path>,
    ) -> Result<Vec<DiscoveredAsset>, AssetError> {
        let root = self.agents_root(scope, project_root);
        let mut walk_warnings = Vec::new();
        let files = common::walk_files_with_ext(&root, ".md", &mut walk_warnings);
        let mut grouped: BTreeMap<String, Vec<AgentCandidate>> = BTreeMap::new();
        for (rel, path) in files {
            let text = match common::read_text(&path) {
                Some(t) => t,
                None => continue,
            };
            let fm = match parse_frontmatter(&text) {
                Some(fm) => fm,
                None => continue, // 无 frontmatter 或无 name：Claude Code 视为文档跳过
            };
            let name = match common::scalar_field(&fm, "name") {
                Some(n) if valid_agent_name(&n) => n.trim().to_string(),
                _ => continue,
            };
            let mut warnings = Vec::new();
            if let Some(w) = builtin_agent_warning(&name) {
                warnings.push(w);
            }
            grouped
                .entry(name.clone())
                .or_default()
                .push(AgentCandidate {
                    rel,
                    name,
                    description: common::scalar_field(&fm, "description"),
                    warnings,
                });
        }

        let mut out = Vec::new();
        for (_, mut group) in grouped {
            if group.len() > 1 {
                let others: Vec<String> = group.iter().skip(1).map(|c| c.rel.clone()).collect();
                if let Some(first) = group.first_mut() {
                    first.warnings.push(format!(
                        "native_id 冲突：多个文件声明了同一个 name `{}`（Claude Code 只加载其中一个，结果取决于文件系统读取顺序）；其余文件: {}",
                        first.name,
                        others.join("、")
                    ));
                }
            }
            let first = group.into_iter().next().unwrap();
            out.push(DiscoveredAsset {
                tool: CLAUDE.into(),
                kind: AssetKind::Agent,
                scope,
                native_id: first.name.clone(),
                native_format: FM.into(),
                display_name: first.name,
                description: first.description,
                entry_path: first.rel,
                warnings: first.warnings,
                source_root: root.to_string_lossy().into_owned(),
            });
        }
        Ok(out)
    }

    fn discover_skills(
        &self,
        scope: Scope,
        project_root: Option<&Path>,
    ) -> Result<Vec<DiscoveredAsset>, AssetError> {
        let root = self.skills_root(scope, project_root);
        let rd = match std::fs::read_dir(&root) {
            Ok(rd) => rd,
            Err(_) => return Ok(Vec::new()),
        };
        let mut names: Vec<_> = rd.filter_map(|e| e.ok()).collect();
        names.sort_by_key(|e| e.file_name());
        let mut out = Vec::new();
        for entry in names {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') || !entry.path().is_dir() {
                continue;
            }
            if !common::safe_segment(&name) || !entry.path().join("SKILL.md").is_file() {
                continue;
            }
            let mut warnings = Vec::new();
            let mut display = name.clone();
            let mut description = None;
            match common::read_text(&entry.path().join("SKILL.md"))
                .as_deref()
                .and_then(parse_frontmatter)
            {
                Some(fm) => {
                    if let Some(n) = common::scalar_field(&fm, "name") {
                        display = n;
                    }
                    description = common::scalar_field(&fm, "description");
                }
                None => warnings.push(
                    "SKILL.md frontmatter 缺失或未闭合，元数据降级（不影响归档）".to_string(),
                ),
            }
            out.push(DiscoveredAsset {
                tool: CLAUDE.into(),
                kind: AssetKind::Skill,
                scope,
                native_id: name,
                native_format: SKILL_DIR.into(),
                display_name: display,
                description,
                entry_path: "SKILL.md".into(),
                warnings,
                source_root: entry.path().to_string_lossy().into_owned(),
            });
        }
        Ok(out)
    }

    fn inspect_found(
        &self,
        kind: AssetKind,
        found: &DiscoveredAsset,
    ) -> Result<InspectionReport, AssetError> {
        let root = Path::new(&found.source_root);
        let mut warnings = found.warnings.clone();
        let mut metadata = AssetMetadata {
            name: None,
            description: None,
            model: None,
            tools: Vec::new(),
            frontmatter_raw: None,
            parsed_ok: false,
            parse_error: None,
        };
        let mut sensitive = Vec::new();
        let mut files = Vec::new();

        match kind {
            AssetKind::Agent => {
                let path = root.join(&found.entry_path);
                match common::read_text(&path)
                    .as_deref()
                    .and_then(parse_frontmatter)
                {
                    Some(fm) => {
                        metadata.parsed_ok = true;
                        metadata.name = common::scalar_field(&fm, "name");
                        metadata.description = common::scalar_field(&fm, "description");
                        metadata.model = common::scalar_field(&fm, "model");
                        metadata.tools = common::tools_from_frontmatter(&fm);
                        metadata.frontmatter_raw = Some(fm.raw.clone());
                        sensitive = claude_sensitive_hints(&found.entry_path, &fm);
                    }
                    None => {
                        metadata.parse_error = Some("frontmatter 缺失或未闭合".to_string());
                        warnings.push(
                            "frontmatter 解析失败，已降级为纯文本（归档不受影响）".to_string(),
                        );
                    }
                }
                let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                files.push(FileBrief {
                    path: found.entry_path.clone(),
                    size,
                    executable: false,
                    binary: false,
                });
            }
            AssetKind::Skill => {
                let skill_md = found.entry_path.clone();
                match common::read_text(&root.join(&skill_md))
                    .as_deref()
                    .and_then(parse_frontmatter)
                {
                    Some(fm) => {
                        metadata.parsed_ok = true;
                        metadata.name = common::scalar_field(&fm, "name");
                        metadata.description = common::scalar_field(&fm, "description");
                        metadata.model = common::scalar_field(&fm, "model");
                        metadata.tools = common::tools_from_frontmatter(&fm);
                        metadata.frontmatter_raw = Some(fm.raw.clone());
                        sensitive = claude_sensitive_hints(&skill_md, &fm);
                    }
                    None => {
                        metadata.parse_error =
                            Some("SKILL.md frontmatter 缺失或未闭合".to_string());
                        warnings.push(
                            "SKILL.md frontmatter 解析失败，已降级为纯文本（归档不受影响）"
                                .to_string(),
                        );
                    }
                }
                files = common::walk_skill_dir(root, &mut warnings)?;
                if !files.iter().any(|f| f.path == skill_md) {
                    return Err(AssetError::not_found(format!(
                        "skill 目录缺少入口文件 {skill_md}"
                    )));
                }
            }
        }

        Ok(InspectionReport {
            asset: found.clone(),
            metadata,
            files,
            sensitive_fields: sensitive,
            warnings,
            external_references: Vec::new(),
        })
    }
}

// ============================================================
// 单测：递归发现、name 冲突、字段原样保留、skill 往返。
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_assets::bundle::{self, AssetBundle};

    const AGENT_SAMPLE: &str = "---\r\nname: security-reviewer\r\ndescription: 安全审查子代理\r\ntools: Read, Grep, Glob\r\nmodel: sonnet\r\npermissionMode: plan\r\nunknown_field: 未知字段原样保留\r\n---\r\n你是安全审查子代理。\r\n";

    fn adapter_at(home: &Path) -> ClaudeAdapter {
        ClaudeAdapter::with_home(home.to_path_buf())
    }

    #[test]
    fn recursive_agent_discovery_and_round_trip() {
        let home = crate::agent_assets::tests_support::temp_root("cc-agent");
        let dir = home.join(".claude/agents/review");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("security.md"), AGENT_SAMPLE).unwrap();

        let ad = adapter_at(&home);
        let found = ad.discover(AssetKind::Agent, Scope::User, None).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].native_id, "security-reviewer");
        assert_eq!(found[0].entry_path, "review/security.md");

        let report = ad
            .inspect(AssetKind::Agent, Scope::User, "security-reviewer", None)
            .unwrap();
        assert!(report.metadata.parsed_ok);
        assert_eq!(
            report.metadata.tools,
            vec!["Read", "Grep", "Glob"],
            "逗号字符串形式的 tools 应拆分"
        );
        assert!(report
            .sensitive_fields
            .iter()
            .any(|h| h.field.eq_ignore_ascii_case("permissionMode")));

        let plan = ad
            .plan_export(AssetKind::Agent, Scope::User, "security-reviewer", None)
            .unwrap();
        let envelope = bundle::pack_from_disk(Path::new(&plan.source_root), &plan).unwrap();
        let wire = serde_json::to_string(&envelope).unwrap();
        let back: AssetBundle = serde_json::from_str(&wire).unwrap();
        let parsed = bundle::parse_bundle(&back).unwrap();
        assert_eq!(
            bundle::manifest_digest(&parsed.manifest),
            envelope.manifest_sha256
        );
        assert_eq!(bundle::bundle_digest(&parsed.files), envelope.bundle_sha256);
        // CRLF、中文、未知字段逐字节保留
        assert_eq!(parsed.files[0].1, AGENT_SAMPLE.as_bytes());
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn duplicate_name_yields_one_asset_with_conflict_warning() {
        let home = crate::agent_assets::tests_support::temp_root("cc-dup");
        let dir = home.join(".claude/agents");
        std::fs::create_dir_all(&dir).unwrap();
        for file in ["a.md", "b.md"] {
            std::fs::write(
                dir.join(file),
                format!("---\nname: same\ndescription: {file}\n---\n正文"),
            )
            .unwrap();
        }
        let ad = adapter_at(&home);
        let found = ad.discover(AssetKind::Agent, Scope::User, None).unwrap();
        assert_eq!(found.len(), 1);
        assert!(found[0]
            .warnings
            .iter()
            .any(|w| w.contains("native_id 冲突")));
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn missing_name_files_are_skipped() {
        let home = crate::agent_assets::tests_support::temp_root("cc-noname");
        let dir = home.join(".claude/agents");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("doc.md"), "---\ndescription: 文档\n---\n正文").unwrap();
        std::fs::write(dir.join("bad.md"), "---\nname: has:colon\n---\n正文").unwrap();
        let ad = adapter_at(&home);
        assert!(ad
            .discover(AssetKind::Agent, Scope::User, None)
            .unwrap()
            .is_empty());
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn skill_round_trip_and_sensitive_hooks() {
        let home = crate::agent_assets::tests_support::temp_root("cc-skill");
        let dir = home.join(".claude/skills/demo-skill");
        std::fs::create_dir_all(dir.join("scripts")).unwrap();
        std::fs::create_dir_all(dir.join("references")).unwrap();
        std::fs::create_dir_all(dir.join("assets")).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            "---\r\nname: demo-skill\r\ndescription: 演示技能\r\nhooks:\r\n  - pre_tool_use\r\n---\r\n正文\r\n",
        )
        .unwrap();
        std::fs::write(dir.join("scripts/run.sh"), b"#!/bin/sh\necho hi\n").unwrap();
        std::fs::write(dir.join("references/tips.md"), b"tips").unwrap();
        std::fs::write(dir.join("assets/logo.png"), [0x89u8, 0x50, 0x4E, 0x00]).unwrap();

        let ad = adapter_at(&home);
        let report = ad
            .inspect(AssetKind::Skill, Scope::User, "demo-skill", None)
            .unwrap();
        assert!(report.metadata.parsed_ok);
        assert!(report.sensitive_fields.iter().any(|h| h.field == "hooks"));
        assert_eq!(report.files.len(), 4);

        let plan = ad
            .plan_export(AssetKind::Skill, Scope::User, "demo-skill", None)
            .unwrap();
        let envelope = bundle::pack_from_disk(Path::new(&plan.source_root), &plan).unwrap();
        assert!(bundle::parse_bundle(&envelope).is_ok());
        let mf: std::collections::BTreeMap<&str, &bundle::ManifestFile> = envelope
            .manifest
            .files
            .iter()
            .map(|f| (f.path.as_str(), f))
            .collect();
        assert!(mf["assets/logo.png"].binary);
        assert!(mf["scripts/run.sh"].executable);
        let parsed = bundle::parse_bundle(&envelope).unwrap();
        let skill_md = parsed
            .files
            .iter()
            .find(|(p, _)| p == "SKILL.md")
            .map(|(_, b)| b.clone())
            .unwrap();
        assert!(skill_md.windows(2).any(|w| w == b"\r\n"));
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn project_scope_reads_project_claude_dirs() {
        let home = crate::agent_assets::tests_support::temp_root("cc-project");
        let project = home.join("proj");
        std::fs::create_dir_all(project.join(".claude/agents")).unwrap();
        std::fs::write(
            project.join(".claude/agents/p.md"),
            "---\nname: p\ndescription: 项目级\n---\n正文",
        )
        .unwrap();
        std::fs::create_dir_all(project.join(".claude/skills/ps")).unwrap();
        std::fs::write(
            project.join(".claude/skills/ps/SKILL.md"),
            b"---\nname: ps\n---\nbody",
        )
        .unwrap();
        let ad = adapter_at(&home);
        assert_eq!(
            ad.discover(AssetKind::Agent, Scope::Project, Some(&project))
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            ad.discover(AssetKind::Skill, Scope::Project, Some(&project))
                .unwrap()
                .len(),
            1
        );
        let err = ad
            .discover(AssetKind::Skill, Scope::Project, None)
            .unwrap_err();
        assert_eq!(err.code(), "VALIDATION_FAILED");
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn real_directory_discovery_gated() {
        if std::env::var_os("AGENTHUD_AGENT_ASSETS_REALDIR").is_none() {
            return;
        }
        let ad = ClaudeAdapter::new();
        let agents = ad.discover(AssetKind::Agent, Scope::User, None).unwrap();
        println!("真实目录 Claude agents: {} 个", agents.len());
        for a in &agents {
            println!("  - {} ({})", a.native_id, a.entry_path);
            let _ = ad
                .inspect(AssetKind::Agent, Scope::User, &a.native_id, None)
                .unwrap();
        }
        let skills = ad.discover(AssetKind::Skill, Scope::User, None).unwrap();
        println!("真实目录 Claude skills: {} 个", skills.len());
    }
}
