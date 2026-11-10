//! OpenCode 适配器（D 工作包，契约 §5.2/§6）。
//!
//! 官方目录与格式（2026-11-09 按 WebFetch 核对）：
//!
//! - Markdown 子代理：用户级 `~/.config/opencode/agents/<name>.md`（尊重
//!   `XDG_CONFIG_HOME`），项目级 `<project>/.opencode/agents/<name>.md`；
//!   V2 递归发现，嵌套相对路径成为 Agent ID（`agents/team/reviewer.md` →
//!   `team/reviewer`）。V2 仍兼容 `agent/`、`mode/`、`modes/` 旧目录。
//!   官方文档：<https://opencode.ai/docs/agents/>
//! - JSON/JSONC 内联定义：配置文件中的 `agents`（V2）或 `agent`（V1）映射；
//!   首期**只提取选中条目**序列化为独立 JSONC 片段（`jsonc-inline`，
//!   installable=false，用户手动合并）。V1→V2 差异：
//!   <https://opencode.ai/docs/migrate-v1/>
//! - Skill：用户配置目录 `skills/`、项目 `.opencode/skills/`（V2 也认
//!   `skill/`），并兼容读取 `~/.claude/skills/` 与 `.agents/skills/`
//!   （发现时标注来源）。官方文档：<https://opencode.ai/docs/skills/>

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::agent_assets::adapter::{
    require_project_root, Adapter, AssetMetadata, DiscoveredAsset, ExportPlan, ExportPlanBuilder,
    FileBrief, InspectionReport, PlannedFile, SensitiveHint,
};
use crate::agent_assets::adapters::common;
use crate::agent_assets::adapters::zcode::parse_frontmatter;
use crate::agent_assets::capabilities::{entries_for, CapabilityEntry, OPENCODE};
use crate::agent_assets::{AssetError, AssetKind, Scope};

const FM: &str = "markdown-frontmatter";
const SKILL_DIR: &str = "skill-dir";
const JSONC_INLINE: &str = "jsonc-inline";

pub struct OpencodeAdapter {
    home: PathBuf,
    config_home: PathBuf,
}

impl OpencodeAdapter {
    pub fn new() -> Self {
        let home = common::home_dir();
        let config_home = common::env_path("XDG_CONFIG_HOME")
            .map(|p| p.join("opencode"))
            .unwrap_or_else(|| home.join(".config").join("opencode"));
        Self { home, config_home }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn with_home(home: PathBuf) -> Self {
        Self {
            config_home: home.join(".config").join("opencode"),
            home,
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn with_roots(home: PathBuf, config_home: PathBuf) -> Self {
        Self { home, config_home }
    }

    /// Markdown agent 根（主目录在前；V1 旧目录在后）。
    fn agent_roots(
        &self,
        scope: Scope,
        project_root: Option<&Path>,
    ) -> Vec<(PathBuf, &'static str)> {
        let base = match scope {
            Scope::User => self.config_home.clone(),
            Scope::Project => project_root
                .unwrap_or_else(|| Path::new(""))
                .join(".opencode"),
        };
        vec![
            (base.join("agents"), "agents（V2 首选）"),
            (base.join("agent"), "agent（V1 兼容目录）"),
            (base.join("mode"), "mode（V1 主 Agent 目录）"),
            (base.join("modes"), "modes（V1 主 Agent 目录）"),
        ]
    }

    /// skill 根（主目录在前；兼容根在后）。
    fn skill_roots(
        &self,
        scope: Scope,
        project_root: Option<&Path>,
    ) -> Vec<(PathBuf, &'static str)> {
        match scope {
            Scope::User => vec![
                (self.config_home.join("skills"), "OpenCode 用户配置目录"),
                (self.config_home.join("skill"), "OpenCode V1 旧 skills 目录"),
                (self.home.join(".claude").join("skills"), "Claude 兼容根"),
                (
                    self.home.join(".agents").join("skills"),
                    "共享 .agents 目录",
                ),
            ],
            Scope::Project => {
                let p = project_root.unwrap_or_else(|| Path::new(""));
                vec![
                    (p.join(".opencode").join("skills"), "OpenCode 项目目录"),
                    (
                        p.join(".opencode").join("skill"),
                        "OpenCode V1 旧 skills 目录",
                    ),
                    (p.join(".claude").join("skills"), "Claude 兼容根"),
                    (p.join(".agents").join("skills"), "共享 .agents 目录"),
                ]
            }
        }
    }

    /// 内联配置候选文件（按优先级排序）。
    fn config_files(&self, scope: Scope, project_root: Option<&Path>) -> Vec<PathBuf> {
        match scope {
            Scope::User => [
                "opencode.jsonc",
                "opencode.json",
                "config.jsonc",
                "config.json",
            ]
            .iter()
            .map(|n| self.config_home.join(n))
            .collect(),
            Scope::Project => {
                let p = project_root.unwrap_or_else(|| Path::new(""));
                let mut out = Vec::new();
                for n in ["opencode.jsonc", "opencode.json"] {
                    out.push(p.join(n));
                }
                for n in [
                    "opencode.jsonc",
                    "opencode.json",
                    "config.jsonc",
                    "config.json",
                ] {
                    out.push(p.join(".opencode").join(n));
                }
                out
            }
        }
    }
}

impl Default for OpencodeAdapter {
    fn default() -> Self {
        Self::new()
    }
}

// ===== 零依赖 JSONC 解析（去注释 + 去尾逗号后交给 serde_json）=====

fn strip_jsonc(input: &str) -> Result<String, String> {
    let input = input.strip_prefix('\u{feff}').unwrap_or(input);
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    let mut in_string = false;
    let mut escape = false;
    while let Some(c) = chars.next() {
        if in_string {
            out.push(c);
            if escape {
                escape = false;
            } else if c == '\\' {
                escape = true;
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        match c {
            '"' => {
                in_string = true;
                out.push(c);
            }
            '/' => match chars.peek() {
                Some('/') => {
                    chars.next();
                    while let Some(&n) = chars.peek() {
                        if n == '\n' {
                            break;
                        }
                        chars.next();
                    }
                }
                Some('*') => {
                    chars.next();
                    let mut prev = '\0';
                    while let Some(n) = chars.next() {
                        if prev == '*' && n == '/' {
                            break;
                        }
                        prev = n;
                    }
                }
                _ => out.push(c),
            },
            _ => out.push(c),
        }
    }
    if in_string {
        return Err("JSONC 字符串未闭合".to_string());
    }
    // 去掉对象/数组尾逗号（字符串内部不处理）。
    let chars: Vec<char> = out.chars().collect();
    let mut cleaned = String::with_capacity(out.len());
    let mut in_string = false;
    let mut escape = false;
    let mut i = 0usize;
    while i < chars.len() {
        let c = chars[i];
        if in_string {
            cleaned.push(c);
            if escape {
                escape = false;
            } else if c == '\\' {
                escape = true;
            } else if c == '"' {
                in_string = false;
            }
            i += 1;
            continue;
        }
        if c == '"' {
            in_string = true;
            cleaned.push(c);
            i += 1;
            continue;
        }
        if c == ',' {
            let mut j = i + 1;
            while j < chars.len() && chars[j].is_whitespace() {
                j += 1;
            }
            if j < chars.len() && (chars[j] == '}' || chars[j] == ']') {
                i += 1;
                continue;
            }
        }
        cleaned.push(c);
        i += 1;
    }
    Ok(cleaned)
}

fn parse_jsonc(text: &str) -> Result<Value, String> {
    let cleaned = strip_jsonc(text)?;
    serde_json::from_str(&cleaned).map_err(|e| format!("JSONC 解析失败: {e}"))
}

// ===== 内联条目提取 =====

#[derive(Debug, Clone)]
struct InlineEntry {
    id: String,
    /// V2 用 `agents`，V1 用 `agent`。
    key: &'static str,
    value: Value,
    file: PathBuf,
    file_rank: usize,
    key_rank: usize,
    warnings: Vec<String>,
}

fn array_entry_id(item: &Value) -> Option<String> {
    for key in ["name", "id", "agent"] {
        if let Some(id) = item.get(key).and_then(Value::as_str) {
            let id = id.trim();
            if !id.is_empty() {
                return Some(id.to_string());
            }
        }
    }
    None
}

/// 从一份配置值中提取 `agents`（V2）/`agent`（V1）条目，只保留对象值。
fn entries_from_config(root: &Value, file: &Path, file_rank: usize, out: &mut Vec<InlineEntry>) {
    for (key, key_rank) in [("agents", 0usize), ("agent", 1usize)] {
        let Some(section) = root.get(key) else {
            continue;
        };
        match section {
            Value::Object(map) => {
                for (id, value) in map {
                    let id = id.trim();
                    if id.is_empty() || !value.is_object() {
                        continue;
                    }
                    out.push(InlineEntry {
                        id: id.to_string(),
                        key,
                        value: value.clone(),
                        file: file.to_path_buf(),
                        file_rank,
                        key_rank,
                        warnings: Vec::new(),
                    });
                }
            }
            Value::Array(items) => {
                for item in items {
                    let Some(id) = array_entry_id(item) else {
                        continue;
                    };
                    if !item.is_object() {
                        continue;
                    }
                    out.push(InlineEntry {
                        id,
                        key,
                        value: item.clone(),
                        file: file.to_path_buf(),
                        file_rank,
                        key_rank,
                        warnings: vec![format!(
                            "内联 `{key}` 为数组形态（非官方 V2 对象映射），导出片段按对象条目合并"
                        )],
                    });
                }
            }
            _ => {}
        }
    }
}

fn inline_fragment_text(entry: &InlineEntry) -> String {
    let mut wrapper = Map::new();
    let mut inner = Map::new();
    inner.insert(entry.id.clone(), entry.value.clone());
    wrapper.insert(entry.key.to_string(), Value::Object(inner));
    let body =
        serde_json::to_string_pretty(&Value::Object(wrapper)).unwrap_or_else(|_| "{}".to_string());
    let file_name = entry
        .file
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "opencode 配置".to_string());
    format!(
        "// AgentHUD 从 {file_name} 提取的 OpenCode 内联 agent `{}`。\n// 请手动把下面的条目合并到配置文件的 \"{}\" 中（V2 用 agents，V1 用 agent）。\n{}\n",
        entry.id, entry.key, body
    )
}

fn inline_entry_path(id: &str) -> String {
    let slug = common::slugify(id);
    let candidate = format!("{slug}.jsonc");
    if crate::agent_assets::safety::validate_relative_path(&candidate).is_ok() {
        candidate
    } else {
        format!("agent-{}.jsonc", common::short_hash(id))
    }
}

/// 手选 Markdown agent 的 native_id：若路径位于 OpenCode 的
/// `agents/`/`agent/`/`mode/`/`modes/` 目录之下，保留相对路径（与 discover
/// 的嵌套 ID 一致）；否则取文件名去扩展。
fn import_agent_id(path: &Path) -> String {
    for ancestor in path.ancestors().skip(1) {
        let Some(name) = ancestor.file_name() else {
            continue;
        };
        let name = name.to_string_lossy();
        if !matches!(name.as_ref(), "agents" | "agent" | "mode" | "modes") {
            continue;
        }
        if let Ok(rel) = path.strip_prefix(ancestor) {
            let rel = rel.to_string_lossy().replace('\\', "/");
            if rel.to_ascii_lowercase().ends_with(".md") {
                let id = rel[..rel.len() - 3].to_string();
                if common::safe_relative_path(&id) {
                    return id;
                }
            }
        }
    }
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn inline_model(value: &Value) -> Option<String> {
    match value.get("model") {
        Some(Value::String(s)) if !s.trim().is_empty() => Some(s.trim().to_string()),
        Some(Value::Object(m)) => {
            let provider = m.get("providerID").and_then(Value::as_str).unwrap_or("");
            let model = m.get("model").and_then(Value::as_str).unwrap_or("");
            let variant = m.get("variant").and_then(Value::as_str).unwrap_or("");
            if model.is_empty() {
                None
            } else {
                let mut s = format!("{provider}/{model}");
                if !variant.is_empty() {
                    s.push('#');
                    s.push_str(variant);
                }
                Some(s)
            }
        }
        _ => None,
    }
}

fn collect_json_sensitive(
    file: &str,
    path: &str,
    value: &Value,
    depth: usize,
    out: &mut Vec<SensitiveHint>,
) {
    if depth > 16 {
        return;
    }
    match value {
        Value::Object(map) => {
            for (k, v) in map {
                let field = if path.is_empty() {
                    k.clone()
                } else {
                    format!("{path}.{k}")
                };
                let lower = k.to_ascii_lowercase();
                let specific = matches!(
                    lower.as_str(),
                    "permissions" | "permission" | "tools" | "request"
                );
                if common::sensitive_key(k) || specific {
                    common::push_sensitive(
                        out,
                        file,
                        &field,
                        "内联配置含疑似敏感字段（权限/工具/请求或密钥），上传前请确认",
                    );
                }
                collect_json_sensitive(file, &field, v, depth + 1, out);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_json_sensitive(file, path, item, depth + 1, out);
            }
        }
        _ => {}
    }
}

/// 收集某作用域全部内联条目，按「配置文件顺序 → V2 agents → V1 agent」去重。
fn collect_inline_entries(
    ad: &OpencodeAdapter,
    scope: Scope,
    project_root: Option<&Path>,
) -> Vec<InlineEntry> {
    let mut all = Vec::new();
    for (rank, file) in ad.config_files(scope, project_root).iter().enumerate() {
        let Some(text) = common::read_text(file) else {
            continue;
        };
        let value = match parse_jsonc(&text) {
            Ok(v) => v,
            Err(_) => continue, // 配置解析失败：不纳管内联条目，Markdown 仍可发现
        };
        entries_from_config(&value, file, rank, &mut all);
    }
    let mut grouped: BTreeMap<String, Vec<InlineEntry>> = BTreeMap::new();
    for entry in all {
        grouped.entry(entry.id.clone()).or_default().push(entry);
    }
    let mut out = Vec::new();
    for (_, mut group) in grouped {
        group.sort_by_key(|e| (e.file_rank, e.key_rank));
        if group.len() > 1 {
            let others: Vec<String> = group
                .iter()
                .skip(1)
                .map(|e| format!("{}（{}）", e.key, e.file.display()))
                .collect();
            if let Some(first) = group.first_mut() {
                first.warnings.push(format!(
                    "同一内联 ID 在多个位置重复定义：{}；按配置文件顺序优先，仅纳管本条",
                    others.join("、")
                ));
            }
        }
        if let Some(first) = group.into_iter().next() {
            out.push(first);
        }
    }
    out
}

/// OpenCode 敏感字段：通用敏感词 + 权限/工具/请求相关。
fn opencode_sensitive_hints(
    file: &str,
    fm: &crate::agent_assets::adapters::zcode::Frontmatter,
) -> Vec<SensitiveHint> {
    let mut out = Vec::new();
    for (k, _) in &fm.entries {
        let lower = k.to_ascii_lowercase();
        let specific = matches!(
            lower.as_str(),
            "permissions" | "permission" | "tools" | "request" | "disallowedtools"
        );
        if common::sensitive_key(k) || specific {
            common::push_sensitive(
                &mut out,
                file,
                k,
                "OpenCode 权限/工具/请求配置，安装或合并前请确认",
            );
        }
    }
    out
}

// ===== Adapter 实现 =====

impl Adapter for OpencodeAdapter {
    fn tool(&self) -> &'static str {
        OPENCODE
    }

    fn capabilities(&self) -> Vec<CapabilityEntry> {
        entries_for(OPENCODE)
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
                    "未找到 OpenCode {} 资源 {native_id}（作用域 {}）",
                    kind.as_str(),
                    scope.as_str()
                ))
            })?;
        if found.native_format == JSONC_INLINE {
            self.inspect_inline(&found, scope, project_root)
        } else {
            self.inspect_markdown(kind, &found)
        }
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
        if a.native_format == JSONC_INLINE {
            let entry = collect_inline_entries(self, scope, project_root)
                .into_iter()
                .find(|e| e.id == native_id)
                .ok_or_else(|| {
                    AssetError::not_found(format!(
                        "内联配置条目 {native_id} 在预览期间已变化，请重新扫描"
                    ))
                })?;
            builder = builder.inline_fragment(Some(inline_fragment_text(&entry)));
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
                let native_id = import_agent_id(path);
                if !common::safe_relative_path(&native_id) {
                    return Err(AssetError::validation(format!(
                        "agent 文件名/相对路径不能作为安全 native_id: {native_id}"
                    )));
                }
                let fm = common::read_text(path)
                    .as_deref()
                    .and_then(parse_frontmatter);
                let description = match &fm {
                    Some(fm) => {
                        let v1 = fm.entries.iter().any(|(k, _)| {
                            matches!(
                                k.as_str(),
                                "prompt"
                                    | "permission"
                                    | "tools"
                                    | "temperature"
                                    | "top_p"
                                    | "maxSteps"
                                    | "disable"
                                    | "variant"
                            )
                        });
                        let v2 = fm.entries.iter().any(|(k, _)| {
                            matches!(
                                k.as_str(),
                                "permissions" | "request" | "system" | "disabled"
                            )
                        });
                        if v1 {
                            warnings.push(
                                "检测到 V1 frontmatter 字段（已弃用）：OpenCode V2 仍会自动翻译；建议一次性迁移到 V2 字段"
                                    .to_string(),
                            );
                        }
                        if v2 && v1 {
                            warnings.push(
                                "同一文件混用 V1/V2 字段：OpenCode 按 V2 读取并忽略 V1 字段（日志有警告）"
                                    .to_string(),
                            );
                        }
                        common::scalar_field(fm, "description")
                    }
                    None => {
                        warnings.push(
                            "frontmatter 缺失或未闭合，已降级为纯文本（归档不受影响）".to_string(),
                        );
                        None
                    }
                };
                let root = path.parent().unwrap_or_else(|| Path::new(""));
                (
                    root.to_path_buf(),
                    file_name,
                    native_id.clone(),
                    native_id,
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
            tool: OPENCODE.into(),
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
        self.inspect_markdown(kind, &asset)
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

impl OpencodeAdapter {
    fn discover_agents(
        &self,
        scope: Scope,
        project_root: Option<&Path>,
    ) -> Result<Vec<DiscoveredAsset>, AssetError> {
        let mut by_id: BTreeMap<String, DiscoveredAsset> = BTreeMap::new();
        let mut walk_warnings = Vec::new();
        for (root_idx, (root, label)) in self.agent_roots(scope, project_root).iter().enumerate() {
            for (rel, path) in common::walk_files_with_ext(root, ".md", &mut walk_warnings) {
                let id = if rel.to_ascii_lowercase().ends_with(".md") {
                    rel[..rel.len() - 3].to_string()
                } else {
                    rel.clone()
                };
                if !common::safe_relative_path(&id) {
                    continue;
                }
                let text = match common::read_text(&path) {
                    Some(t) => t,
                    None => continue,
                };
                let fm = parse_frontmatter(&text);
                let mut warnings = Vec::new();
                if root_idx > 0 {
                    warnings.push(format!(
                        "来源：{label}（V2 首选 .opencode/agents/；旧目录仍兼容读取）"
                    ));
                    if label.starts_with("mode") {
                        warnings.push(
                            "该文件来自 V1 mode/modes 主 Agent 目录；安装到 V2 agents/ 时请补充 mode: primary 以保持主 Agent 语义"
                                .to_string(),
                        );
                    }
                }
                let (description, model, tools) = match &fm {
                    Some(fm) => {
                        let v1 = fm.entries.iter().any(|(k, _)| {
                            matches!(
                                k.as_str(),
                                "prompt"
                                    | "permission"
                                    | "tools"
                                    | "temperature"
                                    | "top_p"
                                    | "maxSteps"
                                    | "disable"
                                    | "variant"
                            )
                        });
                        let v2 = fm.entries.iter().any(|(k, _)| {
                            matches!(
                                k.as_str(),
                                "permissions" | "request" | "system" | "disabled"
                            )
                        });
                        if v1 {
                            warnings.push(
                                "检测到 V1 frontmatter 字段（已弃用）：OpenCode V2 仍会自动翻译；建议一次性迁移到 V2 字段"
                                    .to_string(),
                            );
                        }
                        if v2 && v1 {
                            warnings.push(
                                "同一文件混用 V1/V2 字段：OpenCode 按 V2 读取并忽略 V1 字段（日志有警告）"
                                    .to_string(),
                            );
                        }
                        (
                            common::scalar_field(fm, "description"),
                            common::scalar_field(fm, "model"),
                            common::tools_from_frontmatter(fm),
                        )
                    }
                    None => {
                        warnings.push(
                            "frontmatter 缺失或未闭合，已降级为纯文本（归档不受影响）".to_string(),
                        );
                        (None, None, Vec::new())
                    }
                };
                let asset = DiscoveredAsset {
                    tool: OPENCODE.into(),
                    kind: AssetKind::Agent,
                    scope,
                    native_id: id.clone(),
                    native_format: FM.into(),
                    display_name: id.clone(),
                    description,
                    entry_path: rel.clone(),
                    warnings,
                    source_root: root.to_string_lossy().into_owned(),
                };
                // 记录 tools/model 的解析结果供 inspect 使用（不额外存储：重新解析）。
                let _ = (model, tools);
                if let Some(prev) = by_id.get_mut(&id) {
                    prev.warnings.push(format!(
                        "native_id 冲突：同一 ID 也存在于 {}（按 agents > agent > mode > modes 取本条）",
                        path.display()
                    ));
                    continue;
                }
                by_id.insert(id, asset);
            }
        }

        // 内联条目：Markdown 优先，同 ID 不再产生内联资源。
        let inline = collect_inline_entries(self, scope, project_root);
        let mut inline_assets = Vec::new();
        for entry in inline {
            if let Some(prev) = by_id.get_mut(&entry.id) {
                prev.warnings.push(format!(
                    "内联配置 {} 中也有同 ID 定义（Markdown 优先，内联条目不再单列）",
                    entry.file.display()
                ));
                continue;
            }
            let mut warnings = vec![
                "内联配置条目：首期仅归档与原样导出为独立 JSONC 片段，需手动合并回配置".to_string(),
                format!("来源配置文件：{}", entry.file.display()),
            ];
            warnings.extend(entry.warnings.clone());
            let entry_path = inline_entry_path(&entry.id);
            inline_assets.push(DiscoveredAsset {
                tool: OPENCODE.into(),
                kind: AssetKind::Agent,
                scope,
                native_id: entry.id.clone(),
                native_format: JSONC_INLINE.into(),
                display_name: entry.id.clone(),
                description: entry
                    .value
                    .get("description")
                    .and_then(Value::as_str)
                    .map(|s| s.to_string()),
                entry_path,
                warnings,
                source_root: entry
                    .file
                    .parent()
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_default(),
            });
        }
        let mut out: Vec<DiscoveredAsset> = by_id.into_values().collect();
        out.extend(inline_assets);
        out.sort_by(|a, b| {
            (a.native_id.as_bytes(), a.native_format.as_bytes())
                .cmp(&(b.native_id.as_bytes(), b.native_format.as_bytes()))
        });
        Ok(out)
    }

    fn discover_skills(
        &self,
        scope: Scope,
        project_root: Option<&Path>,
    ) -> Result<Vec<DiscoveredAsset>, AssetError> {
        let mut grouped: BTreeMap<String, Vec<(usize, DiscoveredAsset)>> = BTreeMap::new();
        for (root_idx, (root, label)) in self.skill_roots(scope, project_root).iter().enumerate() {
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
                        "来源：{label}（OpenCode 兼容读取；安装请走 OpenCode 专属目录以避免影响其他工具）"
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
                        tool: OPENCODE.into(),
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
                        "同名 skill 也存在于 {}；按 OpenCode 专属目录优先，仅纳管本条",
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

    fn inspect_markdown(
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
                        metadata.name = Some(found.native_id.clone());
                        metadata.description = common::scalar_field(&fm, "description");
                        metadata.model = common::scalar_field(&fm, "model");
                        metadata.tools = common::tools_from_frontmatter(&fm);
                        metadata.frontmatter_raw = Some(fm.raw.clone());
                        sensitive = opencode_sensitive_hints(&found.entry_path, &fm);
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
                        sensitive = opencode_sensitive_hints(&skill_md, &fm);
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

    fn inspect_inline(
        &self,
        found: &DiscoveredAsset,
        scope: Scope,
        project_root: Option<&Path>,
    ) -> Result<InspectionReport, AssetError> {
        let entry = collect_inline_entries(self, scope, project_root)
            .into_iter()
            .find(|e| e.id == found.native_id)
            .ok_or_else(|| {
                AssetError::not_found(format!(
                    "内联配置条目 {} 已不存在（配置可能已被修改）",
                    found.native_id
                ))
            })?;
        let fragment = inline_fragment_text(&entry);
        let mut warnings = found.warnings.clone();
        warnings.extend(entry.warnings.clone());
        let mut sensitive = Vec::new();
        collect_json_sensitive(&found.entry_path, "", &entry.value, 0, &mut sensitive);
        let pretty = serde_json::to_string_pretty(&entry.value).unwrap_or_default();
        let metadata = AssetMetadata {
            name: Some(entry.id.clone()),
            description: entry
                .value
                .get("description")
                .and_then(Value::as_str)
                .map(|s| s.to_string()),
            model: inline_model(&entry.value),
            tools: Vec::new(),
            frontmatter_raw: Some(pretty),
            parsed_ok: true,
            parse_error: None,
        };
        Ok(InspectionReport {
            asset: found.clone(),
            metadata,
            files: vec![FileBrief {
                path: found.entry_path.clone(),
                size: fragment.len() as u64,
                executable: false,
                binary: false,
            }],
            sensitive_fields: sensitive,
            warnings,
            external_references: Vec::new(),
        })
    }
}

// ============================================================
// 单测：Markdown 嵌套 ID、V2/V1 内联提取、Markdown 优先、兼容 skill 根。
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_assets::bundle;
    use base64::Engine as _;

    fn adapter_at(home: &Path) -> OpencodeAdapter {
        OpencodeAdapter::with_home(home.to_path_buf())
    }

    #[test]
    fn nested_markdown_agent_round_trip() {
        let home = crate::agent_assets::tests_support::temp_root("oc-md");
        let project = home.join("proj");
        let dir = project.join(".opencode/agents/team");
        std::fs::create_dir_all(&dir).unwrap();
        let sample = "---\r\ndescription: 审查当前改动\r\nmode: subagent\r\nmodel: anthropic/claude-sonnet-5-5#high\r\npermissions:\r\n  - action: edit\r\n    resource: \"*\"\r\n    effect: deny\r\nunknown_field: 原样保留\r\n---\r\n按严重程度列出问题。\r\n";
        std::fs::write(dir.join("reviewer.md"), sample).unwrap();

        let ad = adapter_at(&home);
        let found = ad
            .discover(AssetKind::Agent, Scope::Project, Some(&project))
            .unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].native_id, "team/reviewer");
        assert_eq!(found[0].entry_path, "team/reviewer.md");
        assert_eq!(found[0].native_format, FM);

        let report = ad
            .inspect(
                AssetKind::Agent,
                Scope::Project,
                "team/reviewer",
                Some(&project),
            )
            .unwrap();
        assert!(report.metadata.parsed_ok);
        assert_eq!(
            report.metadata.model.as_deref(),
            Some("anthropic/claude-sonnet-5-5#high")
        );
        assert!(report
            .sensitive_fields
            .iter()
            .any(|h| h.field == "permissions"));

        let plan = ad
            .plan_export(
                AssetKind::Agent,
                Scope::Project,
                "team/reviewer",
                Some(&project),
            )
            .unwrap();
        let envelope = bundle::pack_from_disk(Path::new(&plan.source_root), &plan).unwrap();
        let parsed = bundle::parse_bundle(&envelope).unwrap();
        assert_eq!(parsed.files[0].1, sample.as_bytes());
        assert_eq!(bundle::bundle_digest(&parsed.files), envelope.bundle_sha256);
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn v2_inline_entry_is_extracted_as_single_fragment() {
        let home = crate::agent_assets::tests_support::temp_root("oc-inline-v2");
        let cfg_dir = home.join(".config/opencode");
        std::fs::create_dir_all(&cfg_dir).unwrap();
        std::fs::write(
            cfg_dir.join("opencode.jsonc"),
            r#"{
  // 注释与尾逗号都要能解析
  "$schema": "https://opencode.ai/config.json",
  "agents": {
    "reviewer": {
      "description": "审查改动",
      "mode": "subagent",
      "model": "anthropic/claude-sonnet-5-5#high",
      "permissions": [{ "action": "edit", "resource": "*", "effect": "deny" },],
    },
    "other": { "description": "其他条目" },
  },
}
"#,
        )
        .unwrap();

        let ad = adapter_at(&home);
        let found = ad.discover(AssetKind::Agent, Scope::User, None).unwrap();
        assert_eq!(found.len(), 2, "两个内联条目都应被列出");
        let reviewer = found
            .iter()
            .find(|a| a.native_id == "reviewer")
            .expect("reviewer 条目");
        assert_eq!(reviewer.native_format, JSONC_INLINE);
        assert!(reviewer.entry_path.ends_with(".jsonc"));

        let report = ad
            .inspect(AssetKind::Agent, Scope::User, "reviewer", None)
            .unwrap();
        assert!(report.metadata.parsed_ok);
        assert_eq!(report.metadata.description.as_deref(), Some("审查改动"));
        assert!(report
            .sensitive_fields
            .iter()
            .any(|h| h.field == "permissions"));
        assert_eq!(report.files.len(), 1);

        let plan = ad
            .plan_export(AssetKind::Agent, Scope::User, "reviewer", None)
            .unwrap();
        assert!(
            plan.inline_fragment
                .as_deref()
                .unwrap()
                .contains("reviewer"),
            "片段应包含选中条目"
        );
        assert!(
            !plan
                .inline_fragment
                .as_deref()
                .unwrap()
                .contains("其他条目"),
            "片段不得包含未选中的条目"
        );
        let envelope = bundle::pack_plan(&plan).unwrap();
        assert_eq!(envelope.manifest.native_format, JSONC_INLINE);
        assert_eq!(envelope.manifest.files.len(), 1);
        assert_eq!(envelope.manifest.entry_path, plan.entry_path);
        let parsed = bundle::parse_bundle(&envelope).unwrap();
        let content = String::from_utf8(parsed.files[0].1.clone()).unwrap();
        assert!(content.contains("reviewer"));
        assert!(!content.contains("其他条目"));
        assert_eq!(bundle::bundle_digest(&parsed.files), envelope.bundle_sha256);
        // base64 内容与片段一致（原样字节）
        let raw = base64::engine::general_purpose::STANDARD
            .decode(&envelope.contents[0].content_base64)
            .unwrap();
        assert_eq!(raw, plan.inline_fragment.as_deref().unwrap().as_bytes());
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn v1_inline_singular_agent_key_is_preserved() {
        let home = crate::agent_assets::tests_support::temp_root("oc-inline-v1");
        let project = home.join("proj");
        std::fs::create_dir_all(project.join(".opencode")).unwrap();
        std::fs::write(
            project.join(".opencode/opencode.json"),
            r#"{"agent":{"legacy":{"prompt":"旧版提示词","model":"anthropic/claude-sonnet-5-5","permission":{"edit":"deny"}}}}"#,
        )
        .unwrap();
        let ad = adapter_at(&home);
        let found = ad
            .discover(AssetKind::Agent, Scope::Project, Some(&project))
            .unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].native_id, "legacy");
        let plan = ad
            .plan_export(AssetKind::Agent, Scope::Project, "legacy", Some(&project))
            .unwrap();
        let fragment = plan.inline_fragment.as_deref().unwrap();
        assert!(fragment.contains("\"agent\""));
        assert!(!fragment.contains("\"agents\""));
        let envelope = bundle::pack_plan(&plan).unwrap();
        assert!(bundle::parse_bundle(&envelope).is_ok());
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn markdown_wins_over_same_inline_id() {
        let home = crate::agent_assets::tests_support::temp_root("oc-priority");
        let project = home.join("proj");
        std::fs::create_dir_all(project.join(".opencode/agents")).unwrap();
        std::fs::write(
            project.join(".opencode/agents/reviewer.md"),
            "---\ndescription: Markdown 定义\n---\n正文",
        )
        .unwrap();
        std::fs::write(
            project.join(".opencode/opencode.jsonc"),
            r#"{"agents":{"reviewer":{"description":"内联定义"}}}"#,
        )
        .unwrap();
        let ad = adapter_at(&home);
        let found = ad
            .discover(AssetKind::Agent, Scope::Project, Some(&project))
            .unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].native_format, FM);
        assert!(found[0]
            .warnings
            .iter()
            .any(|w| w.contains("Markdown 优先")));
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn compat_skill_root_is_marked_and_shadowed() {
        let home = crate::agent_assets::tests_support::temp_root("oc-skill");
        for root in [
            ".config/opencode/skills/demo",
            ".claude/skills/demo",
            ".agents/skills/demo",
        ] {
            let dir = home.join(root);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join("SKILL.md"),
                "---\nname: demo\ndescription: 演示\n---\n正文",
            )
            .unwrap();
        }
        std::fs::write(
            home.join(".config/opencode/skills/demo/run.sh"),
            b"#!/bin/sh\n",
        )
        .unwrap();
        let ad = adapter_at(&home);
        let found = ad.discover(AssetKind::Skill, Scope::User, None).unwrap();
        assert_eq!(found.len(), 1);
        assert!(
            found[0].source_root.contains(".config") && found[0].source_root.contains("opencode"),
            "OpenCode 专属目录优先"
        );
        assert!(found[0].warnings.iter().any(|w| w.contains("同名 skill")));

        // 只有兼容根时也标注来源
        let home2 = crate::agent_assets::tests_support::temp_root("oc-skill-compat");
        let dir = home2.join(".claude/skills/only");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            "---\nname: only\ndescription: 兼容\n---\n正文",
        )
        .unwrap();
        let ad2 = adapter_at(&home2);
        let found2 = ad2.discover(AssetKind::Skill, Scope::User, None).unwrap();
        assert_eq!(found2.len(), 1);
        assert!(found2[0]
            .warnings
            .iter()
            .any(|w| w.contains("Claude 兼容根")));
        let _ = std::fs::remove_dir_all(&home);
        let _ = std::fs::remove_dir_all(&home2);
    }

    #[test]
    fn skill_round_trip_and_project_scope() {
        let home = crate::agent_assets::tests_support::temp_root("oc-skill-rt");
        let project = home.join("proj");
        let dir = project.join(".opencode/skills/demo-skill");
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
        let plan = ad
            .plan_export(
                AssetKind::Skill,
                Scope::Project,
                "demo-skill",
                Some(&project),
            )
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
        assert_eq!(envelope.manifest.files.len(), 3);
        let err = ad
            .discover(AssetKind::Skill, Scope::Project, None)
            .unwrap_err();
        assert_eq!(err.code(), "VALIDATION_FAILED");
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn jsonc_parser_handles_comments_trailing_commas_and_strings() {
        let v = parse_jsonc(
            "{\n // line\n \"url\": \"https://example.com/a//b\",\n /* block */\n \"list\": [1,2,],\n}",
        )
        .unwrap();
        assert_eq!(v["url"], "https://example.com/a//b");
        assert_eq!(v["list"].as_array().unwrap().len(), 2);
        // 未闭合字符串报错
        assert!(parse_jsonc("{\"a\": \"x}").is_err());
    }

    #[test]
    fn xdg_config_home_override_is_respected() {
        let home = crate::agent_assets::tests_support::temp_root("oc-xdg");
        let config = home.join("custom-xdg/opencode");
        std::fs::create_dir_all(config.join("agents")).unwrap();
        std::fs::write(
            config.join("agents/custom.md"),
            "---\ndescription: 自定义 XDG\n---\n正文",
        )
        .unwrap();
        let ad = OpencodeAdapter::with_roots(home.clone(), config);
        let found = ad.discover(AssetKind::Agent, Scope::User, None).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].native_id, "custom");
        assert!(found[0].source_root.contains("custom-xdg"));
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn import_path_markdown_and_skill_only() {
        let root = crate::agent_assets::tests_support::temp_root("oc-import");
        let agent_file = root.join("reviewer.md");
        let agent_body = "---\ndescription: 手选 Markdown\nmode: subagent\n---\n正文".as_bytes();
        std::fs::write(&agent_file, agent_body).unwrap();
        let skill_dir = root.join("my-skill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            b"---\nname: my-skill\ndescription: manual skill\n---\nbody",
        )
        .unwrap();
        let jsonc = root.join("fragment.jsonc");
        std::fs::write(&jsonc, b"{\"agents\":{\"x\":{}}}").unwrap();
        let nested_dir = root.join(".opencode/agents/team");
        std::fs::create_dir_all(&nested_dir).unwrap();
        let nested = nested_dir.join("nested.md");
        std::fs::write(&nested, b"---\ndescription: nested\n---\nbody").unwrap();

        let ad = adapter_at(&root);
        let report = ad.import_path(AssetKind::Agent, &agent_file).unwrap();
        assert_eq!(report.asset.native_id, "reviewer", "单文件取文件名去扩展");
        assert_eq!(report.asset.native_format, FM);
        // 与 discover 一致：位于 agents 目录下时保留嵌套 ID
        let report = ad.import_path(AssetKind::Agent, &nested).unwrap();
        assert_eq!(report.asset.native_id, "team/nested");
        let plan = ad
            .plan_export_from_source(AssetKind::Agent, Scope::User, &agent_file)
            .unwrap();
        let envelope = bundle::pack_from_disk(Path::new(&plan.source_root), &plan).unwrap();
        let parsed = bundle::parse_bundle(&envelope).unwrap();
        assert_eq!(parsed.files[0].1, agent_body);
        assert_eq!(bundle::bundle_digest(&parsed.files), envelope.bundle_sha256);

        let report = ad.import_path(AssetKind::Skill, &skill_dir).unwrap();
        assert_eq!(report.asset.native_id, "my-skill");
        // 内联配置片段不属于可直接导入的原生 agent 文件
        let err = ad.import_path(AssetKind::Agent, &jsonc).unwrap_err();
        assert_eq!(err.code(), "VALIDATION_FAILED");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn real_directory_discovery_gated() {
        if std::env::var_os("AGENTHUD_AGENT_ASSETS_REALDIR").is_none() {
            return;
        }
        let ad = OpencodeAdapter::new();
        let agents = ad.discover(AssetKind::Agent, Scope::User, None).unwrap();
        println!("真实目录 OpenCode agents: {} 个", agents.len());
        for a in &agents {
            println!("  - {} ({})", a.native_id, a.native_format);
            let _ = ad
                .inspect(AssetKind::Agent, Scope::User, &a.native_id, None)
                .unwrap();
        }
        let skills = ad.discover(AssetKind::Skill, Scope::User, None).unwrap();
        println!("真实目录 OpenCode skills: {} 个", skills.len());
    }
}
