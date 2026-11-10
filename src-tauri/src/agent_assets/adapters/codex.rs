//! Codex 适配器（C 工作包，契约 §5.2/§6）。
//!
//! 官方目录与格式（2026-11-09 按 WebFetch 核对）：
//!
//! - 子代理：独立 TOML 文件，用户级 `~/.codex/agents/`（`CODEX_HOME` 覆盖时
//!   为 `$CODEX_HOME/agents/`），项目级 `<project>/.codex/agents/`；必填
//!   `name`/`description`/`developer_instructions`。官方文档：
//!   <https://learn.chatgpt.com/docs/agent-configuration/subagents>
//! - `CODEX_HOME` 官方定义（默认 `~/.codex`）：
//!   <https://learn.chatgpt.com/docs/config-file/environment-variables>
//! - Skill：用户级 `~/.agents/skills/`，项目级从工作目录到仓库根的
//!   `.agents/skills/`；目录含 `SKILL.md`，可选 `scripts/`、`references/`、
//!   `assets/`、`agents/openai.yaml`。官方文档：
//!   <https://learn.chatgpt.com/docs/build-skills#where-codex-loads-local-skills>
//!
//! 与 ZCode 参考实现的差异：
//! - agent 身份取 TOML `name`（官方明确 name 字段是 source of truth，文件名仅
//!   约定），缺失/非法时回退文件名并记 warning；
//! - 内置 Agent `default`/`worker`/`explorer` 可被同名自定义 Agent 覆盖，
//!   discover/inspect 记入 warnings（不阻断归档）；
//! - TOML 解析用零依赖最小实现（只取顶层标量/多行字符串），解析失败降级为
//!   `parsed_ok=false`，原文件字节仍逐字节归档。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::agent_assets::adapter::{
    require_project_root, Adapter, AssetMetadata, DiscoveredAsset, ExportPlan, ExportPlanBuilder,
    FileBrief, InspectionReport, PlannedFile,
};
use crate::agent_assets::adapters::common;
use crate::agent_assets::adapters::zcode::parse_frontmatter;
use crate::agent_assets::capabilities::{entries_for, CapabilityEntry, CODEX};
use crate::agent_assets::{AssetError, AssetKind, Scope};

const TOML_FMT: &str = "toml";
const SKILL_DIR: &str = "skill-dir";

/// Codex 内置 Agent（同名自定义文件会覆盖内置定义，官方文档明确）。
const BUILTIN_AGENTS: [&str; 3] = ["default", "worker", "explorer"];

/// Codex 适配器；`home`/`codex_home` 供测试注入，全部只读。
pub struct CodexAdapter {
    home: PathBuf,
    codex_home: PathBuf,
}

impl CodexAdapter {
    pub fn new() -> Self {
        let home = common::home_dir();
        let codex_home = common::env_path("CODEX_HOME").unwrap_or_else(|| home.join(".codex"));
        Self { home, codex_home }
    }

    /// 测试注入：`~` 与会话根都落在临时目录。
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn with_home(home: PathBuf) -> Self {
        Self {
            codex_home: home.join(".codex"),
            home,
        }
    }

    /// 测试注入：分别指定真实 HOME 与 `CODEX_HOME`（验证覆盖语义）。
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn with_roots(home: PathBuf, codex_home: PathBuf) -> Self {
        Self { home, codex_home }
    }

    fn agents_root(&self, scope: Scope, project_root: Option<&Path>) -> PathBuf {
        match scope {
            Scope::User => self.codex_home.join("agents"),
            Scope::Project => project_root
                .unwrap_or_else(|| Path::new(""))
                .join(".codex")
                .join("agents"),
        }
    }

    fn skills_root(&self, scope: Scope, project_root: Option<&Path>) -> PathBuf {
        match scope {
            Scope::User => self.home.join(".agents").join("skills"),
            Scope::Project => project_root
                .unwrap_or_else(|| Path::new(""))
                .join(".agents")
                .join("skills"),
        }
    }
}

impl Default for CodexAdapter {
    fn default() -> Self {
        Self::new()
    }
}

// ===== 零依赖 TOML 顶层解析（只服务元数据，不重新序列化）=====

#[derive(Debug, Default)]
struct TomlTopLevel {
    values: BTreeMap<String, String>,
}

fn skip_inline_ws(s: &str, i: &mut usize) {
    let b = s.as_bytes();
    while *i < b.len() && (b[*i] == b' ' || b[*i] == b'\t' || b[*i] == b'\r') {
        *i += 1;
    }
}

fn skip_to_newline(s: &str, i: &mut usize) {
    let b = s.as_bytes();
    while *i < b.len() && b[*i] != b'\n' {
        *i += 1;
    }
}

fn decode_escape(s: &str, i: &mut usize) -> Result<String, String> {
    let b = s.as_bytes();
    if *i >= b.len() {
        return Err("字符串转义不完整".to_string());
    }
    let c = b[*i];
    *i += 1;
    let out = match c {
        b'n' => "\n".to_string(),
        b't' => "\t".to_string(),
        b'r' => "\r".to_string(),
        b'"' => "\"".to_string(),
        b'\\' => "\\".to_string(),
        b'u' => {
            if *i + 4 > b.len() {
                return Err("\\u 转义不足 4 位".to_string());
            }
            let hex = &s[*i..*i + 4];
            *i += 4;
            let code = u32::from_str_radix(hex, 16).map_err(|_| format!("非法 \\u 转义: {hex}"))?;
            char::from_u32(code)
                .ok_or_else(|| format!("非法 Unicode 码点: {hex}"))?
                .to_string()
        }
        other => (other as char).to_string(),
    };
    Ok(out)
}

/// 解析 TOML 基础字符串（`"..."`，i 指向开引号之后）。
fn parse_basic_string(s: &str, i: &mut usize) -> Result<String, String> {
    let b = s.as_bytes();
    let mut out = String::new();
    while *i < b.len() {
        let c = b[*i];
        if c == b'"' {
            *i += 1;
            return Ok(out);
        }
        if c == b'\n' {
            return Err("单行字符串中出现换行".to_string());
        }
        if c == b'\\' {
            *i += 1;
            out.push_str(&decode_escape(s, i)?);
        } else {
            let ch = s[*i..].chars().next().unwrap();
            out.push(ch);
            *i += ch.len_utf8();
        }
    }
    Err("字符串未闭合".to_string())
}

fn parse_literal_string(s: &str, i: &mut usize) -> Result<String, String> {
    if let Some(pos) = s[*i..].find('\'') {
        let out = s[*i..*i + pos].to_string();
        *i += pos + 1;
        Ok(out)
    } else {
        Err("字面量字符串未闭合".to_string())
    }
}

fn parse_multiline_basic(s: &str, i: &mut usize) -> Result<String, String> {
    let b = s.as_bytes();
    // TOML 规则：紧随开头的换行会被裁掉。
    if *i < b.len() && b[*i] == b'\r' {
        *i += 1;
        if *i < b.len() && b[*i] == b'\n' {
            *i += 1;
        }
    } else if *i < b.len() && b[*i] == b'\n' {
        *i += 1;
    }
    let mut out = String::new();
    while *i < b.len() {
        if s[*i..].starts_with("\"\"\"") {
            *i += 3;
            return Ok(out);
        }
        let c = b[*i];
        if c == b'\\' {
            *i += 1;
            // 行尾反斜杠 = 续行（吃掉换行与后续空白）；其余按普通转义。
            if *i < b.len() && (b[*i] == b'\n' || b[*i] == b'\r') {
                while *i < b.len()
                    && (b[*i] == b'\n' || b[*i] == b'\r' || b[*i] == b' ' || b[*i] == b'\t')
                {
                    *i += 1;
                }
                continue;
            }
            out.push_str(&decode_escape(s, i)?);
        } else {
            let ch = s[*i..].chars().next().unwrap();
            out.push(ch);
            *i += ch.len_utf8();
        }
    }
    Err("多行字符串未闭合".to_string())
}

fn parse_multiline_literal(s: &str, i: &mut usize) -> Result<String, String> {
    let b = s.as_bytes();
    if *i < b.len() && b[*i] == b'\r' {
        *i += 1;
        if *i < b.len() && b[*i] == b'\n' {
            *i += 1;
        }
    } else if *i < b.len() && b[*i] == b'\n' {
        *i += 1;
    }
    if let Some(pos) = s[*i..].find("'''") {
        let out = s[*i..*i + pos].to_string();
        *i += pos + 3;
        Ok(out)
    } else {
        Err("多行字面量字符串未闭合".to_string())
    }
}

/// 消费数组或内联表（含字符串），返回原文；元数据用不到其内容。
fn consume_bracketed(s: &str, i: &mut usize, open: u8, close: u8) -> Result<String, String> {
    let b = s.as_bytes();
    let start = *i;
    let mut depth = 0usize;
    let mut in_str: Option<u8> = None;
    while *i < b.len() {
        let c = b[*i];
        if let Some(q) = in_str {
            if c == b'\\' && q == b'"' {
                *i += 2;
                continue;
            }
            if c == q {
                in_str = None;
            }
            *i += 1;
            continue;
        }
        match c {
            b'"' | b'\'' => in_str = Some(c),
            _ if c == open => depth += 1,
            _ if c == close => {
                depth -= 1;
                *i += 1;
                if depth == 0 {
                    return Ok(s[start..*i].to_string());
                }
                continue;
            }
            _ => {}
        }
        *i += 1;
    }
    Err("数组/内联表未闭合".to_string())
}

fn parse_toml_key(s: &str, i: &mut usize) -> Result<String, String> {
    skip_inline_ws(s, i);
    let b = s.as_bytes();
    if *i >= b.len() {
        return Err("缺少键".to_string());
    }
    if b[*i] == b'"' {
        *i += 1;
        return parse_basic_string(s, i);
    }
    if b[*i] == b'\'' {
        *i += 1;
        return parse_literal_string(s, i);
    }
    let start = *i;
    while *i < b.len() && !matches!(b[*i], b' ' | b'\t' | b'\r' | b'\n' | b'=') {
        *i += 1;
    }
    if *i == start {
        return Err("键为空".to_string());
    }
    Ok(s[start..*i].to_string())
}

fn parse_toml_value(s: &str, i: &mut usize) -> Result<String, String> {
    let b = s.as_bytes();
    if *i >= b.len() {
        return Err("缺少值".to_string());
    }
    if s[*i..].starts_with("\"\"\"") {
        *i += 3;
        return parse_multiline_basic(s, i);
    }
    if s[*i..].starts_with("'''") {
        *i += 3;
        return parse_multiline_literal(s, i);
    }
    match b[*i] {
        b'"' => {
            *i += 1;
            parse_basic_string(s, i)
        }
        b'\'' => {
            *i += 1;
            parse_literal_string(s, i)
        }
        b'[' => consume_bracketed(s, i, b'[', b']'),
        b'{' => consume_bracketed(s, i, b'{', b'}'),
        _ => {
            let start = *i;
            while *i < b.len() && b[*i] != b'\n' && b[*i] != b'#' {
                *i += 1;
            }
            Ok(s[start..*i].trim().to_string())
        }
    }
}

fn parse_toml_top_level(text: &str) -> Result<TomlTopLevel, String> {
    let s = text.strip_prefix('\u{feff}').unwrap_or(text);
    let b = s.as_bytes();
    let mut i = 0usize;
    let mut values = BTreeMap::new();
    let mut in_table = false;
    while i < b.len() {
        // 跳过空白、换行与整行注释
        loop {
            skip_inline_ws(s, &mut i);
            if i < b.len() && (b[i] == b'\n' || b[i] == b'\r') {
                i += 1;
                continue;
            }
            if i < b.len() && b[i] == b'#' {
                skip_to_newline(s, &mut i);
                continue;
            }
            break;
        }
        if i >= b.len() {
            break;
        }
        if b[i] == b'[' {
            // 表头开始后，后续键都属于表内，不再作为顶层元数据采集。
            in_table = true;
            skip_to_newline(s, &mut i);
            continue;
        }
        if in_table {
            skip_to_newline(s, &mut i);
            continue;
        }
        let key = parse_toml_key(s, &mut i)?;
        skip_inline_ws(s, &mut i);
        if i >= b.len() || b[i] != b'=' {
            return Err(format!("键 {key} 后缺少 ="));
        }
        i += 1;
        skip_inline_ws(s, &mut i);
        let value = parse_toml_value(s, &mut i)?;
        values.insert(key, value);
        skip_to_newline(s, &mut i);
    }
    Ok(TomlTopLevel { values })
}

fn codex_builtin_warning(name: &str) -> Option<String> {
    if BUILTIN_AGENTS.iter().any(|b| b.eq_ignore_ascii_case(name)) {
        Some(format!(
            "Codex 内置 Agent `{name}` 会被同名自定义 Agent 覆盖（官方文档：custom agent takes precedence）"
        ))
    } else {
        None
    }
}

// ===== Adapter 实现 =====

impl Adapter for CodexAdapter {
    fn tool(&self) -> &'static str {
        CODEX
    }

    fn capabilities(&self) -> Vec<CapabilityEntry> {
        entries_for(CODEX)
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
                    "未找到 Codex {} 资源 {native_id}（作用域 {}）",
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
                common::ensure_agent_file(path, &md, &[".toml"])?;
                let file_name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let stem = path
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                if !common::safe_segment(&stem) {
                    return Err(AssetError::validation(format!(
                        "agent 文件名不能作为安全 native_id: {stem}"
                    )));
                }
                let doc = common::read_text(path)
                    .as_deref()
                    .and_then(|t| parse_toml_top_level(t).ok());
                let name = doc
                    .as_ref()
                    .and_then(|d| d.values.get("name"))
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty() && common::safe_segment(s));
                let native_id = match name {
                    Some(n) => n,
                    None => {
                        warnings.push(format!(
                            "TOML 缺少合法 name 字段，回退文件名 {stem}（官方要求 name 必填）"
                        ));
                        stem.clone()
                    }
                };
                if let Some(w) = codex_builtin_warning(&native_id) {
                    warnings.push(w);
                }
                let description = doc
                    .as_ref()
                    .and_then(|d| d.values.get("description"))
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty());
                if let Some(d) = &doc {
                    if description.is_none() {
                        warnings.push("TOML 缺少必填字段 description".to_string());
                    }
                    let dev = d
                        .values
                        .get("developer_instructions")
                        .map(|s| s.trim())
                        .unwrap_or("");
                    if dev.is_empty() {
                        warnings.push("TOML 缺少必填字段 developer_instructions".to_string());
                    }
                }
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
            tool: CODEX.into(),
            kind,
            scope: Scope::User,
            native_id,
            native_format: if kind == AssetKind::Agent {
                TOML_FMT.into()
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

impl CodexAdapter {
    fn discover_agents(
        &self,
        scope: Scope,
        project_root: Option<&Path>,
    ) -> Result<Vec<DiscoveredAsset>, AssetError> {
        let root = self.agents_root(scope, project_root);
        let rd = match std::fs::read_dir(&root) {
            Ok(rd) => rd,
            Err(_) => return Ok(Vec::new()),
        };
        let mut entries: Vec<_> = rd.filter_map(|e| e.ok()).collect();
        entries.sort_by_key(|e| e.file_name());

        // 先收集全部候选，再按 native_id 归并：同名文件只出一个资源并记冲突。
        let mut grouped: BTreeMap<String, Vec<(String, DiscoveredAsset)>> = BTreeMap::new();
        for entry in entries {
            let file_name = entry.file_name().to_string_lossy().into_owned();
            if file_name.starts_with('.') || !entry.path().is_file() {
                continue;
            }
            if !file_name.to_ascii_lowercase().ends_with(".toml") {
                continue;
            }
            let stem = file_name
                .trim_end_matches(".toml")
                .trim_end_matches(".TOML")
                .to_string();
            if !common::safe_segment(&stem) {
                continue;
            }
            let mut warnings = Vec::new();
            let text = common::read_text(&entry.path());
            let parsed = match text.as_deref() {
                Some(t) => parse_toml_top_level(t),
                None => Err("文件不可读或不是 UTF-8".to_string()),
            };
            let (name, description) = match parsed {
                Ok(doc) => {
                    let mut name = doc
                        .values
                        .get("name")
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty());
                    if name.as_deref().map_or(true, |n| !common::safe_segment(n)) {
                        warnings.push(format!(
                            "TOML 缺少合法 name 字段，回退文件名 {stem}（官方要求 name 必填）"
                        ));
                        name = Some(stem.clone());
                    }
                    let description = doc
                        .values
                        .get("description")
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty());
                    if description.is_none() {
                        warnings.push("TOML 缺少必填字段 description".to_string());
                    }
                    let dev = doc
                        .values
                        .get("developer_instructions")
                        .map(|s| s.trim())
                        .unwrap_or("");
                    if dev.is_empty() {
                        warnings.push("TOML 缺少必填字段 developer_instructions".to_string());
                    }
                    (name.unwrap(), description)
                }
                Err(e) => {
                    warnings.push(format!("TOML 解析失败，元数据降级（归档不受影响）: {e}"));
                    (stem.clone(), None)
                }
            };
            let native_id = name;
            if let Some(w) = codex_builtin_warning(&native_id) {
                warnings.push(w);
            }
            let asset = DiscoveredAsset {
                tool: CODEX.into(),
                kind: AssetKind::Agent,
                scope,
                native_id: native_id.clone(),
                native_format: TOML_FMT.into(),
                display_name: native_id.clone(),
                description,
                entry_path: file_name.clone(),
                warnings,
                source_root: root.to_string_lossy().into_owned(),
            };
            grouped
                .entry(native_id)
                .or_default()
                .push((file_name, asset));
        }

        let mut out = Vec::new();
        for (_, mut group) in grouped {
            if group.len() > 1 {
                let others: Vec<String> = group.iter().skip(1).map(|(f, _)| f.clone()).collect();
                if let Some((_, first)) = group.first_mut() {
                    first.warnings.push(format!(
                        "native_id 冲突：多个 TOML 文件声明了同一 name，Codex 只加载其中一个；其余文件: {}",
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
            if !common::safe_segment(&name) {
                continue;
            }
            if !entry.path().join("SKILL.md").is_file() {
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
                tool: CODEX.into(),
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
                let text = common::read_text(&path);
                match text.as_deref() {
                    Some(t) => match parse_toml_top_level(t) {
                        Ok(doc) => {
                            metadata.parsed_ok = true;
                            metadata.name = doc
                                .values
                                .get("name")
                                .map(|s| s.trim().to_string())
                                .filter(|s| !s.is_empty());
                            metadata.description = doc
                                .values
                                .get("description")
                                .map(|s| s.trim().to_string())
                                .filter(|s| !s.is_empty());
                            metadata.model = doc
                                .values
                                .get("model")
                                .map(|s| s.trim().to_string())
                                .filter(|s| !s.is_empty());
                            metadata.frontmatter_raw = Some(t.to_string());
                            for key in doc.values.keys() {
                                if common::sensitive_key(key) {
                                    common::push_sensitive(
                                        &mut sensitive,
                                        &found.entry_path,
                                        key,
                                        "TOML 含疑似敏感字段，上传前请确认",
                                    );
                                }
                            }
                        }
                        Err(e) => {
                            metadata.parse_error = Some(e.clone());
                            warnings.push(format!(
                                "TOML 解析失败，已降级为纯文本（归档不受影响）: {e}"
                            ));
                        }
                    },
                    None => {
                        metadata.parse_error = Some("文件不可读或不是 UTF-8".to_string());
                        warnings.push(
                            "agent 文件不可读或不是 UTF-8，已降级为纯文本（归档不受影响）"
                                .to_string(),
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
            external_references: Vec::new(),
        })
    }
}

// ============================================================
// 单测：TOML agent + skill 完整往返、必填校验、内置覆盖提示。
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_assets::bundle::{self, AssetBundle};

    const AGENT_SAMPLE: &str = "name = \"demo-agent\"\r\ndescription = \"演示用最小子代理\"\r\nmodel = \"gpt-6.1-sol\"\r\nmodel_reasoning_effort = \"high\"\r\nunknown_field = \"未知字段也要保留\"\r\ndeveloper_instructions = \"\"\"\r\n你是演示子代理。\r\n正文支持任意 Markdown。\r\n\"\"\"\r\n";

    fn adapter_at(home: &Path) -> CodexAdapter {
        CodexAdapter::with_home(home.to_path_buf())
    }

    fn write_skill_sample(home: &Path) {
        let dir = home.join(".agents/skills/demo-skill");
        std::fs::create_dir_all(dir.join("scripts")).unwrap();
        std::fs::create_dir_all(dir.join("assets")).unwrap();
        std::fs::create_dir_all(dir.join("agents")).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            "---\r\nname: demo-skill\r\ndescription: 演示技能\r\n---\r\n正文 CRLF 逐字节保留\r\n"
                .as_bytes(),
        )
        .unwrap();
        std::fs::write(dir.join("scripts/run.sh"), b"#!/bin/sh\necho demo\n").unwrap();
        std::fs::write(dir.join("assets/logo.png"), [0x89u8, 0x50, 0x4E, 0x00]).unwrap();
        std::fs::write(
            dir.join("agents/openai.yaml"),
            b"interface:\n  display_name: Demo\n",
        )
        .unwrap();
    }

    #[test]
    fn agent_discover_inspect_plan_round_trip() {
        let home = crate::agent_assets::tests_support::temp_root("cx-agent");
        std::fs::create_dir_all(home.join(".codex/agents")).unwrap();
        std::fs::write(home.join(".codex/agents/demo-agent.toml"), AGENT_SAMPLE).unwrap();

        let ad = adapter_at(&home);
        let found = ad.discover(AssetKind::Agent, Scope::User, None).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].native_id, "demo-agent");
        assert_eq!(found[0].entry_path, "demo-agent.toml");

        let report = ad
            .inspect(AssetKind::Agent, Scope::User, "demo-agent", None)
            .unwrap();
        assert!(report.metadata.parsed_ok);
        assert_eq!(report.metadata.model.as_deref(), Some("gpt-6.1-sol"));
        assert_eq!(report.files.len(), 1);

        let plan = ad
            .plan_export(AssetKind::Agent, Scope::User, "demo-agent", None)
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
    fn skill_round_trip_keeps_openai_yaml_and_binary() {
        let home = crate::agent_assets::tests_support::temp_root("cx-skill");
        write_skill_sample(&home);
        let ad = adapter_at(&home);
        let found = ad.discover(AssetKind::Skill, Scope::User, None).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].native_id, "demo-skill");

        let report = ad
            .inspect(AssetKind::Skill, Scope::User, "demo-skill", None)
            .unwrap();
        assert!(report.metadata.parsed_ok);
        let paths: Vec<&str> = report.files.iter().map(|f| f.path.as_str()).collect();
        assert!(paths.contains(&"agents/openai.yaml"));
        assert!(paths.contains(&"scripts/run.sh"));

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
    fn missing_required_toml_fields_only_warn() {
        let home = crate::agent_assets::tests_support::temp_root("cx-required");
        std::fs::create_dir_all(home.join(".codex/agents")).unwrap();
        std::fs::write(
            home.join(".codex/agents/partial.toml"),
            "name = \"partial\"\n",
        )
        .unwrap();
        let ad = adapter_at(&home);
        let found = ad.discover(AssetKind::Agent, Scope::User, None).unwrap();
        assert_eq!(found.len(), 1);
        assert!(found[0].warnings.iter().any(|w| w.contains("description")));
        assert!(found[0]
            .warnings
            .iter()
            .any(|w| w.contains("developer_instructions")));
        // 归档链路照常走通
        let plan = ad
            .plan_export(AssetKind::Agent, Scope::User, "partial", None)
            .unwrap();
        assert!(bundle::pack_from_disk(Path::new(&plan.source_root), &plan).is_ok());
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn sensitive_toml_key_is_hinted() {
        let home = crate::agent_assets::tests_support::temp_root("cx-sensitive");
        std::fs::create_dir_all(home.join(".codex/agents")).unwrap();
        std::fs::write(
            home.join(".codex/agents/s.toml"),
            "name = \"s\"\ndescription = \"敏感\"\ndeveloper_instructions = \"ok\"\nenv_key = \"OPENAI_API_KEY\"\n",
        )
        .unwrap();
        let ad = adapter_at(&home);
        let report = ad
            .inspect(AssetKind::Agent, Scope::User, "s", None)
            .unwrap();
        assert!(report.sensitive_fields.iter().any(|h| h.field == "env_key"));
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn builtin_agent_name_warns_and_duplicate_name_is_marked() {
        let home = crate::agent_assets::tests_support::temp_root("cx-builtin");
        std::fs::create_dir_all(home.join(".codex/agents")).unwrap();
        std::fs::write(
            home.join(".codex/agents/explorer.toml"),
            "name = \"explorer\"\ndescription = \"覆盖\"\ndeveloper_instructions = \"只读\"\n",
        )
        .unwrap();
        std::fs::write(
            home.join(".codex/agents/dup.toml"),
            "name = \"explorer\"\ndescription = \"重复\"\ndeveloper_instructions = \"只读\"\n",
        )
        .unwrap();
        let ad = adapter_at(&home);
        let found = ad.discover(AssetKind::Agent, Scope::User, None).unwrap();
        assert_eq!(found.len(), 1, "同名 name 只出一个资源");
        assert!(found[0].warnings.iter().any(|w| w.contains("内置 Agent")));
        assert!(found[0]
            .warnings
            .iter()
            .any(|w| w.contains("native_id 冲突")));
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn codex_home_override_is_respected() {
        let home = crate::agent_assets::tests_support::temp_root("cx-home");
        let codex_home = home.join("custom-codex");
        std::fs::create_dir_all(codex_home.join("agents")).unwrap();
        std::fs::write(
            codex_home.join("agents/custom.toml"),
            "name = \"custom\"\ndescription = \"自定义根\"\ndeveloper_instructions = \"ok\"\n",
        )
        .unwrap();
        let ad = CodexAdapter::with_roots(home.clone(), codex_home);
        let found = ad.discover(AssetKind::Agent, Scope::User, None).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].native_id, "custom");
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn project_scope_uses_project_dirs() {
        let home = crate::agent_assets::tests_support::temp_root("cx-project");
        let project = home.join("proj");
        std::fs::create_dir_all(project.join(".codex/agents")).unwrap();
        std::fs::write(
            project.join(".codex/agents/p.toml"),
            "name = \"p\"\ndescription = \"项目级\"\ndeveloper_instructions = \"ok\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(project.join(".agents/skills/ps")).unwrap();
        std::fs::write(
            project.join(".agents/skills/ps/SKILL.md"),
            b"---\nname: ps\n---\nbody",
        )
        .unwrap();
        let ad = adapter_at(&home);
        let agents = ad
            .discover(AssetKind::Agent, Scope::Project, Some(&project))
            .unwrap();
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].scope, Scope::Project);
        let plan = ad
            .plan_export(AssetKind::Agent, Scope::Project, "p", Some(&project))
            .unwrap();
        assert_eq!(plan.origin_scope, Scope::Project);
        let skills = ad
            .discover(AssetKind::Skill, Scope::Project, Some(&project))
            .unwrap();
        assert_eq!(skills.len(), 1);
        let err = ad
            .discover(AssetKind::Agent, Scope::Project, None)
            .unwrap_err();
        assert_eq!(err.code(), "VALIDATION_FAILED");
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn toml_parser_handles_multiline_and_tables() {
        let doc = parse_toml_top_level(
            "# 注释\nname = \"x\"\ndescription = 'literal'\n[features]\nshell_tool = false\n",
        )
        .unwrap();
        assert_eq!(doc.values.get("name").map(String::as_str), Some("x"));
        assert_eq!(
            doc.values.get("description").map(String::as_str),
            Some("literal")
        );
        assert!(!doc.values.contains_key("shell_tool"));
        // 未闭合字符串降级为错误
        assert!(parse_toml_top_level("name = \"unterminated\n").is_err());
    }

    #[test]
    fn import_path_custom_agent_and_skill_locations() {
        let root = crate::agent_assets::tests_support::temp_root("cx-import");
        let agent_file = root.join("whatever.toml");
        let agent_body =
            "name = \"manual-codex\"\ndescription = \"手选\"\ndeveloper_instructions = \"ok\"\n"
                .as_bytes();
        std::fs::write(&agent_file, agent_body).unwrap();
        let skill_dir = root.join("my-skill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            b"---\nname: my-skill\ndescription: manual skill\n---\nbody",
        )
        .unwrap();
        let bad = root.join("bad.md");
        std::fs::write(&bad, b"x").unwrap();

        let ad = adapter_at(&root);
        let report = ad.import_path(AssetKind::Agent, &agent_file).unwrap();
        assert_eq!(report.asset.native_id, "manual-codex", "Codex 取 TOML name");
        assert_eq!(report.asset.entry_path, "whatever.toml");
        assert!(report.metadata.parsed_ok);
        let plan = ad
            .plan_export_from_source(AssetKind::Agent, Scope::User, &agent_file)
            .unwrap();
        let envelope = bundle::pack_from_disk(Path::new(&plan.source_root), &plan).unwrap();
        let parsed = bundle::parse_bundle(&envelope).unwrap();
        assert_eq!(parsed.files[0].1, agent_body);
        assert_eq!(bundle::bundle_digest(&parsed.files), envelope.bundle_sha256);

        let report = ad.import_path(AssetKind::Skill, &skill_dir).unwrap();
        assert_eq!(report.asset.native_id, "my-skill");
        assert!(ad.import_path(AssetKind::Agent, &bad).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 真实目录发现验证（环境变量门控，只读）：
    /// `AGENTHUD_AGENT_ASSETS_REALDIR=1 cargo test agent_assets` 时扫描真实
    /// `$CODEX_HOME/agents` 与 `~/.agents/skills`。
    #[test]
    fn real_directory_discovery_gated() {
        if std::env::var_os("AGENTHUD_AGENT_ASSETS_REALDIR").is_none() {
            return;
        }
        let ad = CodexAdapter::new();
        let agents = ad.discover(AssetKind::Agent, Scope::User, None).unwrap();
        println!("真实目录 Codex agents: {} 个", agents.len());
        for a in &agents {
            println!("  - {} ({})", a.native_id, a.source_root);
            let _ = ad
                .inspect(AssetKind::Agent, Scope::User, &a.native_id, None)
                .unwrap();
        }
        let skills = ad.discover(AssetKind::Skill, Scope::User, None).unwrap();
        println!("真实目录 Codex skills: {} 个", skills.len());
        for s in &skills {
            println!("  - {} ({})", s.native_id, s.source_root);
            let _ = ad
                .inspect(AssetKind::Skill, Scope::User, &s.native_id, None)
                .unwrap();
        }
    }
}
