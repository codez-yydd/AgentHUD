//! Kimi Code 适配器（D 工作包，契约 §5.2/§6）。
//!
//! 官方目录与格式（2026-11-09 按 WebFetch 核对）：
//!
//! - 当前版子代理：Markdown + YAML frontmatter。用户级
//!   `$KIMI_CODE_HOME/agents/`（默认 `~/.kimi-code/agents/`）与共享
//!   `~/.agents/agents/`；项目级 `<project>/.kimi-code/agents/` 与
//!   `<project>/.agents/agents/`。目录递归扫描 `.md`。frontmatter 的
//!   `name` 缺省时回退文件名；`description` 必填；保留 `override`/`whenToUse`。
//!   官方文档：<https://www.kimi.com/code/docs/en/kimi-code-cli/customization/agents.html>
//! - 当前版 Skill：目录型 `SKILL.md`。用户级 `$KIMI_CODE_HOME/skills/` 与
//!   `~/.agents/skills/`；项目级 `.kimi-code/skills/` 与 `.agents/skills/`。
//!   官方文档：<https://www.kimi.com/code/docs/en/kimi-code-cli/customization/skills.html>
//! - 旧 kimi-cli（Python，已归档）：YAML 格式，`version: 1` + `agent:` 映射，
//!   字段 `extend`/`name`/`system_prompt_path`/`system_prompt_args`/`tools`/
//!   `exclude_tools`/`subagents`，通过 `kimi --agent-file <path>` 显式加载
//!   （旧文档未定义默认扫描目录）。官方旧文档：
//!   <https://moonshotai.github.io/kimi-cli/en/customization/agents.html>
//!
//! 因此 `.yaml`/`.yml`（无 frontmatter 围栏的纯 YAML）在 agent 目录中被识别为
//! `native_format=yaml-legacy`：**仅归档与原样导出，installable=false**。
//! 相对路径的 `system_prompt_path` 与 `subagents.<name>.path` 若安全且位于同
//! 目录内，会一并归档；绝对路径或包外引用只报告不递归（契约 §3.5）。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::agent_assets::adapter::{
    require_project_root, Adapter, AssetMetadata, DiscoveredAsset, ExportPlan, ExportPlanBuilder,
    FileBrief, InspectionReport, PlannedFile,
};
use crate::agent_assets::adapters::common;
use crate::agent_assets::adapters::zcode::parse_frontmatter;
use crate::agent_assets::capabilities::{entries_for, CapabilityEntry, KIMI};
use crate::agent_assets::{AssetError, AssetKind, Scope};

const FM: &str = "markdown-frontmatter";
const SKILL_DIR: &str = "skill-dir";
const YAML_LEGACY: &str = "yaml-legacy";

/// Kimi Code 内置子代理（目录文件需 `override: true` 才能覆盖）。
const BUILTIN_AGENTS: [&str; 3] = ["coder", "explore", "plan"];

pub struct KimiAdapter {
    home: PathBuf,
    kimi_home: PathBuf,
}

impl KimiAdapter {
    pub fn new() -> Self {
        let home = common::home_dir();
        let kimi_home =
            common::env_path("KIMI_CODE_HOME").unwrap_or_else(|| home.join(".kimi-code"));
        Self { home, kimi_home }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn with_home(home: PathBuf) -> Self {
        Self {
            kimi_home: home.join(".kimi-code"),
            home,
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn with_roots(home: PathBuf, kimi_home: PathBuf) -> Self {
        Self { home, kimi_home }
    }

    /// 用户级/项目级 agent 根，按官方优先级排列（工具专属目录在前）。
    fn agent_roots(&self, scope: Scope, project_root: Option<&Path>) -> Vec<PathBuf> {
        match scope {
            Scope::User => vec![
                self.kimi_home.join("agents"),
                self.home.join(".agents").join("agents"),
            ],
            Scope::Project => {
                let p = project_root.unwrap_or_else(|| Path::new(""));
                vec![
                    p.join(".kimi-code").join("agents"),
                    p.join(".agents").join("agents"),
                ]
            }
        }
    }

    fn skill_roots(&self, scope: Scope, project_root: Option<&Path>) -> Vec<PathBuf> {
        match scope {
            Scope::User => vec![
                self.kimi_home.join("skills"),
                self.home.join(".agents").join("skills"),
            ],
            Scope::Project => {
                let p = project_root.unwrap_or_else(|| Path::new(""));
                vec![
                    p.join(".kimi-code").join("skills"),
                    p.join(".agents").join("skills"),
                ]
            }
        }
    }
}

impl Default for KimiAdapter {
    fn default() -> Self {
        Self::new()
    }
}

/// Kimi 要求 kebab-case；非法时仍归档但记 warning（早期版本会跳过）。
fn kebab_case(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !name.starts_with('-')
        && !name.ends_with('-')
        && !name.contains("--")
}

fn builtin_warning(name: &str, override_true: bool) -> Option<String> {
    if BUILTIN_AGENTS.iter().any(|b| b.eq_ignore_ascii_case(name)) {
        let mut msg = format!(
            "Kimi Code 内置 Agent `{name}`：目录文件仅在 frontmatter 声明 override: true 时覆盖内置定义"
        );
        if !override_true {
            msg.push_str("；当前文件未声明 override: true，内置 Agent 不会被覆盖");
        }
        Some(msg)
    } else {
        None
    }
}

// ===== 旧 kimi-cli YAML 最小解析 =====

#[derive(Debug, Default)]
struct LegacyYaml {
    name: Option<String>,
    system_prompt_path: Option<String>,
    extend: Option<String>,
    tools: Vec<String>,
    /// `subagents.<name>.path` 引用（旧版子代理定义文件）。
    subagent_paths: Vec<String>,
    /// 所有出现过的键（敏感字段提示用）。
    keys: Vec<String>,
}

fn unquote(v: &str) -> String {
    v.trim().trim_matches('"').trim_matches('\'').to_string()
}

/// 解析旧 kimi-cli agent 的 YAML 子集：`version` + `agent:` 映射。
///
/// 只关心标量、`tools` 列表与 `subagents.<name>.path` 引用；其余嵌套结构
/// 原样保留在文件字节中，不做求值、不递归读取。
fn parse_legacy_yaml(text: &str) -> LegacyYaml {
    let mut out = LegacyYaml::default();
    let mut in_agent = false;
    let mut current_key: Option<String> = None;
    let mut current_indent = 0usize;
    // subagents 映射的缩进：更深层的 `path:` 是子代理定义文件引用。
    let mut subagents_indent: Option<usize> = None;
    for raw_line in text.lines() {
        let line = raw_line.trim_end_matches('\r');
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        if indent == 0 {
            current_key = None;
            current_indent = 0;
            subagents_indent = None;
            if trimmed == "agent:" || trimmed.starts_with("agent:") {
                in_agent = true;
            } else if !trimmed.starts_with("version:") {
                // 没有 agent: 包裹时，把顶层键也当作 agent 字段（兼容手写样例）。
                in_agent = true;
                if let Some((k, v)) = trimmed.split_once(':') {
                    record_legacy_field(&mut out, k.trim(), v.trim());
                }
            }
            continue;
        }
        if !in_agent {
            continue;
        }
        // subagents 映射内部：只记录 `path:` 引用，不混入顶层字段。
        if let Some(base_indent) = subagents_indent {
            if indent > base_indent {
                if let Some((k, v)) = trimmed.split_once(':') {
                    if k.trim() == "path" {
                        let path = unquote(v.trim());
                        if !path.is_empty() {
                            out.subagent_paths.push(path);
                        }
                    }
                }
                continue;
            }
            subagents_indent = None;
        }
        if let Some(item) = trimmed.strip_prefix("- ") {
            // 只处理当前键下的列表项；更深层嵌套的列表忽略。
            if current_key.as_deref() == Some("tools") && indent > current_indent {
                out.tools.push(unquote(item));
            }
            continue;
        }
        let Some((k, v)) = trimmed.split_once(':') else {
            continue;
        };
        let key = k.trim().to_string();
        // 比当前字段更深的键属于嵌套映射（如 subagents 的子项），忽略。
        if current_key.is_some() && indent > current_indent {
            continue;
        }
        current_key = Some(key.clone());
        current_indent = indent;
        record_legacy_field(&mut out, &key, v.trim());
        if key == "subagents" && v.trim().is_empty() {
            subagents_indent = Some(indent);
        }
    }
    out
}

fn record_legacy_field(out: &mut LegacyYaml, key: &str, value: &str) {
    if !key.is_empty() {
        out.keys.push(key.to_string());
    }
    let v = unquote(value);
    match key {
        "name" if !v.is_empty() => out.name = Some(v),
        "system_prompt_path" if !v.is_empty() => out.system_prompt_path = Some(v),
        "extend" if !v.is_empty() => out.extend = Some(v),
        "tools" if !v.is_empty() => {
            if let Some(inner) = v.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
                for part in inner.split(',') {
                    let p = unquote(part);
                    if !p.is_empty() {
                        out.tools.push(p);
                    }
                }
            }
        }
        _ => {}
    }
}

/// 相对引用是否可安全纳入包：仅普通相对路径，无 `..`/绝对路径/盘符。
fn safe_relative_ref(p: &str) -> Option<String> {
    let mut normalized = p.replace('\\', "/");
    while let Some(rest) = normalized.strip_prefix("./") {
        normalized = rest.to_string();
    }
    if normalized.starts_with('/')
        || normalized.starts_with("~/")
        || normalized.contains(':')
        || normalized.is_empty()
    {
        return None;
    }
    if !common::safe_relative_path(&normalized) {
        return None;
    }
    Some(normalized)
}

/// 旧的 `system_prompt_path` 若在同目录内且为普通文件，则纳入包。
fn legacy_included_prompt(base: &Path, prompt: &str) -> Option<(String, PathBuf)> {
    let rel = safe_relative_ref(prompt)?;
    let path = base.join(&rel);
    if path.is_file() {
        Some((rel, path))
    } else {
        None
    }
}

struct AgentCandidate {
    native_id: String,
    rel: String,
    root_idx: usize,
    format: &'static str,
    display: String,
    description: Option<String>,
    warnings: Vec<String>,
    source_root: PathBuf,
    legacy: bool,
}

// ===== Adapter 实现 =====

impl Adapter for KimiAdapter {
    fn tool(&self) -> &'static str {
        KIMI
    }

    fn capabilities(&self) -> Vec<CapabilityEntry> {
        entries_for(KIMI)
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
                    "未找到 Kimi Code {} 资源 {native_id}（作用域 {}）",
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
        let (root, entry_path, native_id, display, description, native_format) = match kind {
            AssetKind::Agent => {
                common::ensure_agent_file(path, &md, &[".md", ".yaml", ".yml"])?;
                let file_name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let stem = path
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let lower = file_name.to_ascii_lowercase();
                let legacy = lower.ends_with(".yaml") || lower.ends_with(".yml");
                let (native_id, display, description, native_format) = if legacy {
                    let yaml = common::read_text(path)
                        .as_deref()
                        .map(parse_legacy_yaml)
                        .unwrap_or_default();
                    if yaml.system_prompt_path.is_some() {
                        warnings.push(
                            "system_prompt_path 相对且位于同目录时会一并归档；绝对/包外引用只报告不递归"
                                .to_string(),
                        );
                    }
                    let name = yaml.name.unwrap_or_else(|| stem.clone());
                    (name.clone(), name, None, YAML_LEGACY)
                } else {
                    let fm = common::read_text(path)
                        .as_deref()
                        .and_then(parse_frontmatter)
                        .ok_or_else(|| {
                            AssetError::validation(format!(
                                "Kimi Code 当前 agent 需要 YAML frontmatter: {}",
                                path.display()
                            ))
                        })?;
                    let name = common::scalar_field(&fm, "name").unwrap_or_else(|| {
                        warnings.push(format!("frontmatter 缺少 name，回退文件名 {stem}"));
                        stem.clone()
                    });
                    if !kebab_case(&name) {
                        warnings.push(format!(
                            "name `{name}` 不是 kebab-case；部分 Kimi Code 版本会跳过该文件"
                        ));
                    }
                    if common::scalar_field(&fm, "description").is_none() {
                        warnings.push("frontmatter 缺少必填字段 description".to_string());
                    }
                    let override_true = fm
                        .entries
                        .iter()
                        .find(|(k, _)| k == "override")
                        .map(|(_, v)| v.as_str().eq_ignore_ascii_case("true"))
                        .unwrap_or(false);
                    if let Some(w) = builtin_warning(&name, override_true) {
                        warnings.push(w);
                    }
                    let description = common::scalar_field(&fm, "description");
                    (name.clone(), name, description, FM)
                };
                if native_id.trim().is_empty() {
                    return Err(AssetError::validation(format!(
                        "无法从文件推断 agent native_id: {}",
                        path.display()
                    )));
                }
                let root = path.parent().unwrap_or_else(|| Path::new(""));
                (
                    root.to_path_buf(),
                    file_name,
                    native_id,
                    display,
                    description,
                    native_format,
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
                    SKILL_DIR,
                )
            }
        };
        let asset = DiscoveredAsset {
            tool: KIMI.into(),
            kind,
            scope: Scope::User,
            native_id,
            native_format: native_format.into(),
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

impl KimiAdapter {
    fn discover_agents(
        &self,
        scope: Scope,
        project_root: Option<&Path>,
    ) -> Result<Vec<DiscoveredAsset>, AssetError> {
        let roots = self.agent_roots(scope, project_root);
        let mut grouped: BTreeMap<String, Vec<AgentCandidate>> = BTreeMap::new();
        for (root_idx, root) in roots.iter().enumerate() {
            let mut walk_warnings = Vec::new();
            // 当前版 Markdown
            for (rel, path) in common::walk_files_with_ext(root, ".md", &mut walk_warnings) {
                let text = match common::read_text(&path) {
                    Some(t) => t,
                    None => continue,
                };
                let fm = match parse_frontmatter(&text) {
                    Some(fm) => fm,
                    None => continue,
                };
                let stem = Path::new(&rel)
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let mut warnings = if root_idx > 0 {
                    vec![format!(
                        "来源：共享 .agents 目录 {}（Kimi Code 兼容读取）",
                        root.display()
                    )]
                } else {
                    Vec::new()
                };
                let name = common::scalar_field(&fm, "name").unwrap_or_else(|| {
                    warnings.push(format!(
                        "frontmatter 缺少 name，回退文件名 {stem}（Kimi Code 允许缺省）"
                    ));
                    stem.clone()
                });
                if !kebab_case(&name) {
                    warnings.push(format!(
                        "name `{name}` 不是 kebab-case；部分 Kimi Code 版本会跳过该文件"
                    ));
                }
                if common::scalar_field(&fm, "description").is_none() {
                    warnings.push("frontmatter 缺少必填字段 description".to_string());
                }
                let override_true = fm
                    .entries
                    .iter()
                    .find(|(k, _)| k == "override")
                    .map(|(_, v)| v.as_str().eq_ignore_ascii_case("true"))
                    .unwrap_or(false);
                if let Some(w) = builtin_warning(&name, override_true) {
                    warnings.push(w);
                }
                let native_id = name.clone();
                grouped
                    .entry(native_id.clone())
                    .or_default()
                    .push(AgentCandidate {
                        native_id,
                        rel,
                        root_idx,
                        format: FM,
                        display: name,
                        description: common::scalar_field(&fm, "description"),
                        warnings,
                        source_root: root.clone(),
                        legacy: false,
                    });
            }
            // 旧 kimi-cli YAML
            for ext in [".yaml", ".yml"] {
                for (rel, path) in common::walk_files_with_ext(root, ext, &mut walk_warnings) {
                    let text = match common::read_text(&path) {
                        Some(t) => t,
                        None => continue,
                    };
                    let yaml = parse_legacy_yaml(&text);
                    let stem = Path::new(&rel)
                        .file_stem()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    let name = yaml.name.clone().unwrap_or_else(|| stem.clone());
                    let mut warnings = vec![
                        "旧 kimi-cli YAML 格式（yaml-legacy）：仅归档与原样导出，安装待验证"
                            .to_string(),
                        "旧版通过 kimi --agent-file <path> 显式加载；本目录扫描为兼容发现"
                            .to_string(),
                    ];
                    if root_idx > 0 {
                        warnings.push(format!(
                            "来源：共享 .agents 目录 {}（Kimi Code 兼容读取）",
                            root.display()
                        ));
                    }
                    if yaml.system_prompt_path.is_some() {
                        warnings.push(
                            "system_prompt_path 相对且位于同目录时会一并归档；绝对/包外引用只报告不递归"
                                .to_string(),
                        );
                    }
                    let base = path.parent().unwrap_or(root);
                    grouped
                        .entry(name.clone())
                        .or_default()
                        .push(AgentCandidate {
                            native_id: name.clone(),
                            rel: path
                                .file_name()
                                .map(|s| s.to_string_lossy().into_owned())
                                .unwrap_or(rel),
                            root_idx,
                            format: YAML_LEGACY,
                            display: name,
                            description: None,
                            warnings,
                            source_root: base.to_path_buf(),
                            legacy: true,
                        });
                }
            }
        }

        let mut out = Vec::new();
        for (_, mut group) in grouped {
            // 当前 Markdown 优先于旧 YAML；同格式按工具专属根 > 共享根 > 相对路径。
            group.sort_by(|a, b| {
                (a.legacy, a.root_idx, a.rel.as_bytes()).cmp(&(
                    b.legacy,
                    b.root_idx,
                    b.rel.as_bytes(),
                ))
            });
            if group.len() > 1 {
                let others: Vec<String> = group
                    .iter()
                    .skip(1)
                    .map(|c| format!("{}（{}）", c.rel, c.source_root.display()))
                    .collect();
                if let Some(first) = group.first_mut() {
                    first.warnings.push(format!(
                        "native_id 冲突：同名定义也存在于 {}；按「当前 Markdown > 工具专属根 > 共享根」仅纳管本条",
                        others.join("、")
                    ));
                }
            }
            let first = group.into_iter().next().unwrap();
            out.push(DiscoveredAsset {
                tool: KIMI.into(),
                kind: AssetKind::Agent,
                scope,
                native_id: first.native_id,
                native_format: first.format.into(),
                display_name: first.display,
                description: first.description,
                entry_path: first.rel,
                warnings: first.warnings,
                source_root: first.source_root.to_string_lossy().into_owned(),
            });
        }
        Ok(out)
    }

    fn discover_skills(
        &self,
        scope: Scope,
        project_root: Option<&Path>,
    ) -> Result<Vec<DiscoveredAsset>, AssetError> {
        let roots = self.skill_roots(scope, project_root);
        let mut grouped: BTreeMap<String, Vec<(usize, DiscoveredAsset)>> = BTreeMap::new();
        for (root_idx, root) in roots.iter().enumerate() {
            let rd = match std::fs::read_dir(root) {
                Ok(rd) => rd,
                Err(_) => continue,
            };
            let mut names: Vec<_> = rd.filter_map(|e| e.ok()).collect();
            names.sort_by_key(|e| e.file_name());
            for entry in names {
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.starts_with('.') || !entry.path().is_dir() {
                    continue;
                }
                if !common::safe_segment(&name) || !entry.path().join("SKILL.md").is_file() {
                    continue;
                }
                let mut warnings = Vec::new();
                if root_idx > 0 {
                    warnings.push(format!(
                        "来源：共享 .agents 目录 {}（Kimi Code 兼容读取；建议安装到工具专属目录）",
                        root.display()
                    ));
                }
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
                grouped.entry(name.clone()).or_default().push((
                    root_idx,
                    DiscoveredAsset {
                        tool: KIMI.into(),
                        kind: AssetKind::Skill,
                        scope,
                        native_id: name,
                        native_format: SKILL_DIR.into(),
                        display_name: display,
                        description,
                        entry_path: "SKILL.md".into(),
                        warnings,
                        source_root: entry.path().to_string_lossy().into_owned(),
                    },
                ));
            }
        }
        let mut out = Vec::new();
        for (_, mut group) in grouped {
            group.sort_by_key(|(idx, _)| *idx);
            if group.len() > 1 {
                let others: Vec<String> = group
                    .iter()
                    .skip(1)
                    .map(|(_, a)| a.source_root.clone())
                    .collect();
                if let Some((_, first)) = group.first_mut() {
                    first.warnings.push(format!(
                        "同名 skill 也存在于 {}；按工具专属目录优先，仅纳管本条",
                        others.join("、")
                    ));
                }
            }
            if let Some((_, first)) = group.into_iter().next() {
                out.push(first);
            }
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
        let mut external_references = Vec::new();

        match kind {
            AssetKind::Agent => {
                let path = root.join(&found.entry_path);
                let text = common::read_text(&path);
                if found.native_format == YAML_LEGACY {
                    match text.as_deref() {
                        Some(t) => {
                            let yaml = parse_legacy_yaml(t);
                            metadata.parsed_ok = true;
                            metadata.name = yaml.name.clone();
                            metadata.tools = yaml.tools.clone();
                            metadata.frontmatter_raw = Some(t.to_string());
                            for key in &yaml.keys {
                                if common::sensitive_key(key) {
                                    common::push_sensitive(
                                        &mut sensitive,
                                        &found.entry_path,
                                        key,
                                        "旧 kimi-cli YAML 含疑似敏感字段，上传前请确认",
                                    );
                                }
                            }
                            // 相对 system_prompt_path / subagents path 纳入包；
                            // 其余引用只报告（契约 §3.5，不递归读取）。
                            if let Some(prompt) = &yaml.system_prompt_path {
                                match legacy_included_prompt(root, prompt) {
                                    Some((rel, prompt_path)) if rel != found.entry_path => {
                                        let size = std::fs::metadata(&prompt_path)
                                            .map(|m| m.len())
                                            .unwrap_or(0);
                                        files.push(FileBrief {
                                            path: rel.clone(),
                                            size,
                                            executable: common::has_executable_ext(&rel),
                                            binary: false,
                                        });
                                    }
                                    None => external_references.push(prompt.clone()),
                                    // 引用文件与入口相同：入口已在 files 中，无需重复。
                                    Some(_) => {}
                                }
                            }
                            for subagent in &yaml.subagent_paths {
                                match legacy_included_prompt(root, subagent) {
                                    Some((rel, prompt_path))
                                        if rel != found.entry_path
                                            && !files.iter().any(|f| f.path == rel) =>
                                    {
                                        let size = std::fs::metadata(&prompt_path)
                                            .map(|m| m.len())
                                            .unwrap_or(0);
                                        files.push(FileBrief {
                                            path: rel.clone(),
                                            size,
                                            executable: common::has_executable_ext(&rel),
                                            binary: false,
                                        });
                                    }
                                    None => {
                                        external_references.push(format!("subagents: {subagent}"))
                                    }
                                    Some(_) => {}
                                }
                            }
                            if !yaml.subagent_paths.is_empty() {
                                warnings.push(
                                    "subagents 相对引用位于同目录时会一并归档；绝对/包外引用只报告不递归"
                                        .to_string(),
                                );
                            }
                            if let Some(extend) = &yaml.extend {
                                if safe_relative_ref(extend).is_none() || extend != "default" {
                                    external_references.push(format!("extend: {extend}"));
                                }
                            }
                        }
                        None => {
                            metadata.parse_error = Some("文件不可读或不是 UTF-8".to_string());
                            warnings
                                .push("旧 YAML 文件不可读或不是 UTF-8（归档不受影响）".to_string());
                        }
                    }
                } else {
                    match text.as_deref().and_then(parse_frontmatter) {
                        Some(fm) => {
                            metadata.parsed_ok = true;
                            metadata.name = common::scalar_field(&fm, "name");
                            metadata.description = common::scalar_field(&fm, "description");
                            metadata.model = common::scalar_field(&fm, "model");
                            metadata.tools = common::tools_from_frontmatter(&fm);
                            metadata.frontmatter_raw = Some(fm.raw.clone());
                            sensitive = common::frontmatter_sensitive_hints(&found.entry_path, &fm);
                        }
                        None => {
                            metadata.parse_error = Some("frontmatter 缺失或未闭合".to_string());
                            warnings.push(
                                "frontmatter 解析失败，已降级为纯文本（归档不受影响）".to_string(),
                            );
                        }
                    }
                }
                let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                files.insert(
                    0,
                    FileBrief {
                        path: found.entry_path.clone(),
                        size,
                        executable: false,
                        binary: false,
                    },
                );
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
                        sensitive = common::frontmatter_sensitive_hints(&skill_md, &fm);
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
            external_references,
        })
    }
}

// ============================================================
// 单测：当前 Markdown、旧 YAML、共享根遮蔽与 skill 往返。
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_assets::bundle;

    const AGENT_SAMPLE: &str = "---\r\nname: reviewer\r\ndescription: 严格代码审查\r\nwhenToUse: 代码评审与 PR 检查\r\noverride: false\r\ntools:\r\n  - Read\r\n  - Grep\r\napi_key: sk-xxx\r\nunknown_field: 原样保留\r\n---\r\n你是严格代码审查员。\r\n";

    fn adapter_at(home: &Path) -> KimiAdapter {
        KimiAdapter::with_home(home.to_path_buf())
    }

    #[test]
    fn current_agent_round_trip_preserves_override_and_when_to_use() {
        let home = crate::agent_assets::tests_support::temp_root("kimi-agent");
        let dir = home.join(".kimi-code/agents");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("reviewer.md"), AGENT_SAMPLE).unwrap();

        let ad = adapter_at(&home);
        let found = ad.discover(AssetKind::Agent, Scope::User, None).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].native_id, "reviewer");
        assert_eq!(found[0].native_format, FM);

        let report = ad
            .inspect(AssetKind::Agent, Scope::User, "reviewer", None)
            .unwrap();
        assert!(report.metadata.parsed_ok);
        assert!(report
            .metadata
            .frontmatter_raw
            .as_deref()
            .unwrap()
            .contains("whenToUse"));
        assert!(report
            .metadata
            .frontmatter_raw
            .as_deref()
            .unwrap()
            .contains("override"));
        assert!(report.sensitive_fields.iter().any(|h| h.field == "api_key"));

        let plan = ad
            .plan_export(AssetKind::Agent, Scope::User, "reviewer", None)
            .unwrap();
        let envelope = bundle::pack_from_disk(Path::new(&plan.source_root), &plan).unwrap();
        let parsed = bundle::parse_bundle(&envelope).unwrap();
        assert_eq!(bundle::bundle_digest(&parsed.files), envelope.bundle_sha256);
        assert_eq!(parsed.files[0].1, AGENT_SAMPLE.as_bytes());
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn legacy_yaml_is_archived_with_referenced_prompt() {
        let home = crate::agent_assets::tests_support::temp_root("kimi-legacy");
        let dir = home.join(".kimi-code/agents/legacy");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("my-agent.yaml"),
            "version: 1\nagent:\n  name: legacy-agent\n  system_prompt_path: ./system.md\n  tools:\n    - kimi_cli.tools.shell:Shell\n    - kimi_cli.tools.file:ReadFile\n",
        )
        .unwrap();
        std::fs::write(dir.join("system.md"), "# Legacy\n\n你是旧版代理。\n").unwrap();

        let ad = adapter_at(&home);
        let found = ad.discover(AssetKind::Agent, Scope::User, None).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].native_id, "legacy-agent");
        assert_eq!(found[0].native_format, YAML_LEGACY);

        let report = ad
            .inspect(AssetKind::Agent, Scope::User, "legacy-agent", None)
            .unwrap();
        assert!(report.metadata.parsed_ok);
        assert_eq!(report.metadata.tools.len(), 2);
        let paths: Vec<&str> = report.files.iter().map(|f| f.path.as_str()).collect();
        assert!(paths.contains(&"my-agent.yaml"));
        assert!(paths.contains(&"system.md"), "相对引用文件应一并归档");

        let plan = ad
            .plan_export(AssetKind::Agent, Scope::User, "legacy-agent", None)
            .unwrap();
        assert_eq!(plan.native_format, YAML_LEGACY);
        let envelope = bundle::pack_from_disk(Path::new(&plan.source_root), &plan).unwrap();
        assert_eq!(envelope.manifest.files.len(), 2);
        assert!(bundle::parse_bundle(&envelope).is_ok());
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn legacy_yaml_absolute_prompt_is_external_reference_only() {
        let home = crate::agent_assets::tests_support::temp_root("kimi-legacy-ext");
        let dir = home.join(".kimi-code/agents");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("ext.yaml"),
            "version: 1\nagent:\n  name: ext-agent\n  system_prompt_path: ~/outside/system.md\n",
        )
        .unwrap();
        let ad = adapter_at(&home);
        let report = ad
            .inspect(AssetKind::Agent, Scope::User, "ext-agent", None)
            .unwrap();
        assert_eq!(report.files.len(), 1, "包外引用不得读入");
        assert!(report
            .external_references
            .iter()
            .any(|r| r.contains("outside/system.md")));
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn shared_agent_root_is_marked_and_shadowed_by_tool_specific_root() {
        let home = crate::agent_assets::tests_support::temp_root("kimi-shared");
        for root in [".kimi-code/agents", ".agents/agents"] {
            let dir = home.join(root);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join("reviewer.md"),
                "---\nname: reviewer\ndescription: 重复\n---\n正文",
            )
            .unwrap();
        }
        let ad = adapter_at(&home);
        let found = ad.discover(AssetKind::Agent, Scope::User, None).unwrap();
        assert_eq!(found.len(), 1);
        assert!(
            found[0].source_root.contains(".kimi-code"),
            "工具专属根优先"
        );
        assert!(found[0]
            .warnings
            .iter()
            .any(|w| w.contains("native_id 冲突")));
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn skill_shared_root_source_warning_and_round_trip() {
        let home = crate::agent_assets::tests_support::temp_root("kimi-skill");
        let dir = home.join(".agents/skills/demo-skill");
        std::fs::create_dir_all(dir.join("scripts")).unwrap();
        std::fs::create_dir_all(dir.join("assets")).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            "---\r\nname: demo-skill\r\ndescription: 演示技能\r\n---\r\n正文\r\n",
        )
        .unwrap();
        std::fs::write(dir.join("scripts/run.sh"), b"#!/bin/sh\necho hi\n").unwrap();
        std::fs::write(dir.join("assets/logo.png"), [0x89u8, 0x50, 0x4E, 0x00]).unwrap();

        let ad = adapter_at(&home);
        let found = ad.discover(AssetKind::Skill, Scope::User, None).unwrap();
        assert_eq!(found.len(), 1);
        assert!(found[0].warnings.iter().any(|w| w.contains("共享 .agents")));

        let plan = ad
            .plan_export(AssetKind::Skill, Scope::User, "demo-skill", None)
            .unwrap();
        let envelope = bundle::pack_from_disk(Path::new(&plan.source_root), &plan).unwrap();
        let mf: std::collections::BTreeMap<&str, &bundle::ManifestFile> = envelope
            .manifest
            .files
            .iter()
            .map(|f| (f.path.as_str(), f))
            .collect();
        assert!(mf["assets/logo.png"].binary);
        assert!(mf["scripts/run.sh"].executable);
        let parsed = bundle::parse_bundle(&envelope).unwrap();
        assert_eq!(bundle::bundle_digest(&parsed.files), envelope.bundle_sha256);
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn project_scope_uses_kimi_and_shared_roots() {
        let home = crate::agent_assets::tests_support::temp_root("kimi-project");
        let project = home.join("proj");
        std::fs::create_dir_all(project.join(".kimi-code/agents")).unwrap();
        std::fs::write(
            project.join(".kimi-code/agents/p.md"),
            "---\nname: p\ndescription: 项目级\n---\n正文",
        )
        .unwrap();
        std::fs::create_dir_all(project.join(".kimi-code/skills/ps")).unwrap();
        std::fs::write(
            project.join(".kimi-code/skills/ps/SKILL.md"),
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
            .discover(AssetKind::Agent, Scope::Project, None)
            .unwrap_err();
        assert_eq!(err.code(), "VALIDATION_FAILED");
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn legacy_yaml_parser_reads_agent_mapping() {
        let yaml = parse_legacy_yaml(
            "version: 1\nagent:\n  extend: default\n  name: demo\n  system_prompt_path: ./p.md\n  tools:\n    - a\n    - b\n  subagents:\n    coder:\n      path: ./c.yaml\n",
        );
        assert_eq!(yaml.name.as_deref(), Some("demo"));
        assert_eq!(yaml.extend.as_deref(), Some("default"));
        assert_eq!(yaml.system_prompt_path.as_deref(), Some("./p.md"));
        assert_eq!(yaml.tools, vec!["a", "b"]);
        assert_eq!(yaml.subagent_paths, vec!["./c.yaml"]);
        assert!(!yaml.keys.contains(&"path".to_string()), "嵌套键不应混入");
    }

    #[test]
    fn legacy_yaml_archives_local_subagent_files_and_reports_external() {
        let home = crate::agent_assets::tests_support::temp_root("kimi-subagents");
        let dir = home.join(".kimi-code/agents");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("parent.yaml"),
            "version: 1\nagent:\n  name: parent\n  system_prompt_path: ./system.md\n  subagents:\n    coder:\n      path: ./coder.yaml\n    outside:\n      path: ~/elsewhere/other.yaml\n",
        )
        .unwrap();
        std::fs::write(dir.join("system.md"), b"# parent prompt\n").unwrap();
        std::fs::write(
            dir.join("coder.yaml"),
            b"version: 1\nagent:\n  name: coder\n",
        )
        .unwrap();

        let ad = adapter_at(&home);
        let report = ad
            .inspect(AssetKind::Agent, Scope::User, "parent", None)
            .unwrap();
        let paths: Vec<&str> = report.files.iter().map(|f| f.path.as_str()).collect();
        assert!(paths.contains(&"parent.yaml"));
        assert!(paths.contains(&"system.md"));
        assert!(paths.contains(&"coder.yaml"), "同目录 subagents 引用应归档");
        assert_eq!(
            report.files.len(),
            3,
            "重复/包外引用不得增加条目: {paths:?}"
        );
        assert!(report
            .external_references
            .iter()
            .any(|r| r.contains("subagents:") && r.contains("elsewhere/other.yaml")));

        // 打包往返仍成立
        let plan = ad
            .plan_export(AssetKind::Agent, Scope::User, "parent", None)
            .unwrap();
        let envelope = bundle::pack_from_disk(Path::new(&plan.source_root), &plan).unwrap();
        assert_eq!(envelope.manifest.files.len(), 3);
        assert!(bundle::parse_bundle(&envelope).is_ok());
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn kimi_code_home_override_is_respected() {
        let home = crate::agent_assets::tests_support::temp_root("kimi-home");
        let custom = home.join("custom-kimi");
        std::fs::create_dir_all(custom.join("agents")).unwrap();
        std::fs::write(
            custom.join("agents/custom.md"),
            "---\nname: custom\ndescription: 自定义根\n---\n正文",
        )
        .unwrap();
        let ad = KimiAdapter::with_roots(home.clone(), custom);
        let found = ad.discover(AssetKind::Agent, Scope::User, None).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].native_id, "custom");
        assert!(found[0].source_root.contains("custom-kimi"));
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn import_path_current_legacy_and_skill() {
        let root = crate::agent_assets::tests_support::temp_root("kimi-import");
        let current = root.join("reviewer.md");
        let current_body = "---\nname: reviewer\ndescription: 手选当前版\n---\n正文".as_bytes();
        std::fs::write(&current, current_body).unwrap();
        let legacy = root.join("legacy.yaml");
        std::fs::write(
            &legacy,
            b"version: 1\nagent:\n  name: legacy-agent\n  system_prompt_path: ./system.md\n",
        )
        .unwrap();
        std::fs::write(root.join("system.md"), b"# prompt\n").unwrap();
        let skill_dir = root.join("my-skill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            b"---\nname: my-skill\ndescription: manual skill\n---\nbody",
        )
        .unwrap();
        let bad = root.join("bad.txt");
        std::fs::write(&bad, b"x").unwrap();

        let ad = adapter_at(&root);
        let report = ad.import_path(AssetKind::Agent, &current).unwrap();
        assert_eq!(report.asset.native_id, "reviewer");
        assert_eq!(report.asset.native_format, FM);
        let plan = ad
            .plan_export_from_source(AssetKind::Agent, Scope::User, &current)
            .unwrap();
        let envelope = bundle::pack_from_disk(Path::new(&plan.source_root), &plan).unwrap();
        let parsed = bundle::parse_bundle(&envelope).unwrap();
        assert_eq!(parsed.files[0].1, current_body);
        assert_eq!(bundle::bundle_digest(&parsed.files), envelope.bundle_sha256);

        let report = ad.import_path(AssetKind::Agent, &legacy).unwrap();
        assert_eq!(report.asset.native_id, "legacy-agent");
        assert_eq!(report.asset.native_format, YAML_LEGACY);
        assert_eq!(report.files.len(), 2, "YAML + 相对提示词");
        let plan = ad
            .plan_export_from_source(AssetKind::Agent, Scope::User, &legacy)
            .unwrap();
        let envelope = bundle::pack_from_disk(Path::new(&plan.source_root), &plan).unwrap();
        let parsed = bundle::parse_bundle(&envelope).unwrap();
        assert_eq!(parsed.files.len(), 2);
        assert_eq!(bundle::bundle_digest(&parsed.files), envelope.bundle_sha256);

        let report = ad.import_path(AssetKind::Skill, &skill_dir).unwrap();
        assert_eq!(report.asset.native_id, "my-skill");
        assert!(ad.import_path(AssetKind::Agent, &bad).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn real_directory_discovery_gated() {
        if std::env::var_os("AGENTHUD_AGENT_ASSETS_REALDIR").is_none() {
            return;
        }
        let ad = KimiAdapter::new();
        let agents = ad.discover(AssetKind::Agent, Scope::User, None).unwrap();
        println!("真实目录 Kimi agents: {} 个", agents.len());
        for a in &agents {
            println!(
                "  - {} ({}, {})",
                a.native_id, a.native_format, a.source_root
            );
            let _ = ad
                .inspect(AssetKind::Agent, Scope::User, &a.native_id, None)
                .unwrap();
        }
        let skills = ad.discover(AssetKind::Skill, Scope::User, None).unwrap();
        println!("真实目录 Kimi skills: {} 个", skills.len());
    }
}
