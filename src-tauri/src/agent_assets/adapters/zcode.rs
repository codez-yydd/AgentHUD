//! ZCode 适配器（A 工作包参考实现，契约 §10）。
//!
//! 目录与发现规则（依据 ZCode 官方配置指南，C/D 各自按目标发行版复核）：
//!
//! | 资源 | 作用域 | 根目录 | 规则 |
//! | --- | --- | --- | --- |
//! | agent | user | `~/.zcode/agents/` | 非递归扫 `*.md`；native_id = 文件名去扩展 |
//! | skill | user | `~/.zcode/skills/` → `~/.agents/skills/` | 子目录含 `SKILL.md` 即资源；双根同名按 `.zcode` 优先（遮蔽记入警告） |
//! | skill | project | `<root>/.zcode/skills/` → `<root>/.agents/skills/` | 同上；首期仅发现与归档 |
//!
//! 包内相对路径：agent 以 agents 目录为根（`<name>.md`）；skill 以 skill 目录
//! 为根（`SKILL.md`、`scripts/…`）。inspect 的 frontmatter 解析失败只降级为
//! `parsed_ok=false`，**不阻断归档**（原文件字节为真）。

use std::path::{Path, PathBuf};

use crate::agent_assets::adapter::{
    plan_from_inspection, require_project_root, Adapter, AssetMetadata, DiscoveredAsset,
    ExportPlan, ExportPlanBuilder, FileBrief, InspectionReport, PlannedFile, SensitiveHint,
};
use crate::agent_assets::adapters::common;
use crate::agent_assets::capabilities::{entries_for, CapabilityEntry};
use crate::agent_assets::{AssetError, AssetKind, Scope};

/// 首期格式标识（契约 §2.3）。
const FM_AGENT: &str = "markdown-frontmatter";
const SKILL_DIR: &str = "skill-dir";
/// skill 目录的清单深度上限（防失控目录；硬上限由 bundle 的条目数限制兜底）。
const MAX_WALK_DEPTH: usize = 8;
/// walk 软上限：超出后停止收集并给出警告（pack 阶段还有 200 条硬限制）。
const MAX_WALK_ENTRIES: usize = 500;

/// ZCode 适配器。`home` 用于测试注入（绝不碰真实 `~/.zcode` 的写操作，
/// 本适配器全部只读）。
pub struct ZcodeAdapter {
    home: PathBuf,
}

impl ZcodeAdapter {
    pub fn new() -> Self {
        Self {
            home: dirs::home_dir().unwrap_or_default(),
        }
    }

    /// 测试/验证注入自定义 HOME（发现与归档的只读扫描用真实目录的门控
    /// 测试也走这里）。
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn with_home(home: PathBuf) -> Self {
        Self { home }
    }

    fn agents_root(&self) -> PathBuf {
        self.home.join(".zcode").join("agents")
    }

    fn skill_roots(&self, scope: Scope, project_root: Option<&Path>) -> Vec<PathBuf> {
        match scope {
            Scope::User => vec![
                self.home.join(".zcode").join("skills"),
                self.home.join(".agents").join("skills"),
            ],
            Scope::Project => {
                let p = project_root.unwrap_or_else(|| Path::new(""));
                vec![
                    p.join(".zcode").join("skills"),
                    p.join(".agents").join("skills"),
                ]
            }
        }
    }
}

impl Default for ZcodeAdapter {
    fn default() -> Self {
        Self::new()
    }
}

// ===== frontmatter 最小解析（C/D 可复制的零依赖实现）=====

/// 解析结果：原文逐字节保留 + 顶层键值（标量 / 字符串列表）。
pub(crate) struct Frontmatter {
    pub raw: String,
    pub entries: Vec<(String, FrontValue)>,
}

pub(crate) enum FrontValue {
    Scalar(String),
    List(Vec<String>),
}

impl FrontValue {
    /// 标量值或列表的逗号连接（展示用）。
    pub fn as_str(&self) -> String {
        match self {
            FrontValue::Scalar(s) => s.clone(),
            FrontValue::List(v) => v.join(", "),
        }
    }
    pub fn as_list(&self) -> Vec<String> {
        match self {
            FrontValue::Scalar(s) => vec![s.clone()],
            FrontValue::List(v) => v.clone(),
        }
    }
}

/// 解析 `---` 围栏 YAML frontmatter 的最小子集：
/// - 首行必须是 `---`（容忍 BOM 与 `\r`）；
/// - 顶层 `key: value` 标量、`key:` 后跟 `- item` 列表、`[a, b]` 行内列表；
/// - 其余行（含嵌套结构）不解释，原文保留在 `raw`；
/// - 缺少闭合围栏 → None（调用方降级为 parsed_ok=false，不报错）。
pub(crate) fn parse_frontmatter(text: &str) -> Option<Frontmatter> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut lines = text.split('\n');
    let first = lines.next()?.trim_end_matches('\r');
    if first.trim() != "---" {
        return None;
    }
    let mut entries: Vec<(String, FrontValue)> = Vec::new();
    let mut raw_lines: Vec<&str> = vec![first];
    let mut closed = false;
    for line in lines {
        let trimmed = line.trim_end_matches('\r');
        raw_lines.push(trimmed);
        if trimmed.trim() == "---" {
            closed = true;
            break;
        }
        let t = trimmed.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        if let Some(item) = t.strip_prefix("- ") {
            if let Some((k, FrontValue::List(v))) = entries.last_mut() {
                let _ = k;
                v.push(item.trim().trim_matches('"').trim_matches('\'').to_string());
            }
            continue;
        }
        if let Some((k, v)) = t.split_once(':') {
            let key = k.trim().to_string();
            if key.is_empty() {
                continue;
            }
            let val = v.trim();
            if val.is_empty() {
                entries.push((key, FrontValue::List(Vec::new())));
            } else if let Some(inner) = val.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
                let list = inner
                    .split(',')
                    .map(|s| s.trim().trim_matches('"').trim_matches('\'').to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
                entries.push((key, FrontValue::List(list)));
            } else {
                entries.push((
                    key,
                    FrontValue::Scalar(val.trim_matches('"').trim_matches('\'').to_string()),
                ));
            }
        }
        // 其余行（嵌套映射等）忽略：只取顶层标量/列表
    }
    if !closed {
        return None;
    }
    Some(Frontmatter {
        raw: raw_lines.join("\n"),
        entries,
    })
}

/// frontmatter 键是否疑似敏感（契约 §10.2：token/secret/key/api/env/credential）。
fn sensitive_key(key: &str) -> bool {
    let k = key.to_ascii_lowercase();
    [
        "token",
        "secret",
        "api_key",
        "apikey",
        "key",
        "credential",
        "password",
        "env",
    ]
    .iter()
    .any(|w| k.contains(w))
}

fn sensitive_hints(file: &str, fm: &Frontmatter) -> Vec<SensitiveHint> {
    fm.entries
        .iter()
        .filter(|(k, _)| sensitive_key(k))
        .map(|(k, _)| SensitiveHint {
            file: file.to_string(),
            field: k.clone(),
            reason: "frontmatter 含疑似敏感字段，上传前请确认".to_string(),
        })
        .collect()
}

/// native_id 必须是单段安全路径（用来拼包内相对路径）。
fn safe_segment(seg: &str) -> bool {
    crate::agent_assets::safety::validate_relative_path(seg).is_ok()
        && !seg.contains('/')
        && !seg.starts_with('.')
}

// ===== 目录遍历（skill 目录；只读、拒符号链接、限深限量）=====
//
// 目录内遇到符号链接/junction 或超过深度上限时**硬拒绝**（不静默跳过）：
// 跳过的条目不会进入 plan.files，pack 只读清单，只会产出缺文件的“成功”包。

fn walk_skill_dir(dir: &Path, warnings: &mut Vec<String>) -> Result<Vec<FileBrief>, AssetError> {
    let mut out = Vec::new();
    fn rec(
        dir: &Path,
        prefix: &str,
        depth: usize,
        out: &mut Vec<FileBrief>,
        warnings: &mut Vec<String>,
    ) -> Result<(), AssetError> {
        if depth > MAX_WALK_DEPTH {
            return Err(AssetError::validation(format!(
                "目录层级超过 {MAX_WALK_DEPTH}，拒绝归档: {prefix}"
            )));
        }
        let rd = match std::fs::read_dir(dir) {
            Ok(rd) => rd,
            Err(e) => {
                warnings.push(format!("读取目录 {prefix} 失败: {e}"));
                return Ok(());
            }
        };
        let mut items: Vec<_> = rd.filter_map(|e| e.ok()).collect();
        items.sort_by_key(|e| e.file_name());
        for entry in items {
            if out.len() >= MAX_WALK_ENTRIES {
                warnings.push(format!("文件数超过软上限 {MAX_WALK_ENTRIES}，已停止收集"));
                return Ok(());
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            let rel = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            };
            let md = match entry.metadata() {
                Ok(md) => md,
                Err(_) => continue,
            };
            // 符号链接/junction：文件本体或中间目录一律硬拒绝（契约 §9.7）
            if md.file_type().is_symlink() {
                return Err(AssetError::path_unsafe(format!(
                    "资源目录内存在符号链接/junction，拒绝归档: {rel}"
                ))
                .with_details(serde_json::json!({ "path": rel, "rule": "symlink" })));
            }
            if md.is_dir() {
                rec(&entry.path(), &rel, depth + 1, out, warnings)?;
            } else if md.is_file() {
                let lower = rel.to_ascii_lowercase();
                let executable = lower.ends_with(".sh")
                    || lower.ends_with(".py")
                    || lower.ends_with(".pl")
                    || lower.ends_with(".rb")
                    || lower.ends_with(".js")
                    || lower.ends_with(".mjs")
                    || lower.ends_with(".cjs")
                    || lower.ends_with(".ps1")
                    || lower.ends_with(".bat")
                    || lower.ends_with(".cmd")
                    || lower.ends_with(".exe");
                out.push(FileBrief {
                    path: rel,
                    size: md.len(),
                    executable,
                    binary: false, // 精确嗅探在 pack 阶段做（避免发现期全量读文件）
                });
            }
        }
        Ok(())
    }
    rec(dir, "", 0, &mut out, warnings)?;
    Ok(out)
}

/// 读 UTF-8 文本（失败返回 None；inspect 对不可读文件只降级元数据）。
fn read_text(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    String::from_utf8(bytes).ok()
}

// ===== Adapter 实现 =====

impl Adapter for ZcodeAdapter {
    fn tool(&self) -> &'static str {
        crate::agent_assets::capabilities::ZCODE
    }

    fn capabilities(&self) -> Vec<CapabilityEntry> {
        entries_for(crate::agent_assets::capabilities::ZCODE)
    }

    fn discover(
        &self,
        kind: AssetKind,
        scope: Scope,
        project_root: Option<&Path>,
    ) -> Result<Vec<DiscoveredAsset>, AssetError> {
        require_project_root(scope, project_root)?;
        match kind {
            AssetKind::Agent => {
                if scope == Scope::Project {
                    return Err(AssetError::capability_unsupported(
                        "ZCode 子代理首期仅支持用户级（~/.zcode/agents/）",
                    ));
                }
                self.discover_agents()
            }
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
        // discover 是唯一的定位来源（保证 discover/inspect/plan 三者一致）
        let found = self
            .discover(kind, scope, project_root)?
            .into_iter()
            .find(|a| a.native_id == native_id)
            .ok_or_else(|| {
                AssetError::not_found(format!(
                    "未找到 ZCode {} 资源 {native_id}（作用域 {}）",
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
        let (root, entry_path, native_id, display, description, mut warnings) = match kind {
            AssetKind::Agent => {
                common::ensure_agent_file(path, &md, &[".md"])?;
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
                let fm = common::read_text(path)
                    .as_deref()
                    .and_then(parse_frontmatter);
                let display = fm
                    .as_ref()
                    .and_then(|fm| fm.entries.iter().find(|(k, _)| k == "name"))
                    .map(|(_, v)| v.as_str())
                    .filter(|s| !s.trim().is_empty())
                    .unwrap_or_else(|| stem.clone());
                let description = fm.as_ref().and_then(|fm| {
                    fm.entries
                        .iter()
                        .find(|(k, _)| k == "description")
                        .map(|(_, v)| v.as_str())
                });
                let warnings = if fm.is_none() {
                    vec!["frontmatter 缺失或未闭合，元数据降级（不影响归档）".to_string()]
                } else {
                    Vec::new()
                };
                let root = path.parent().unwrap_or_else(|| Path::new(""));
                (
                    root.to_path_buf(),
                    file_name,
                    stem,
                    display,
                    description,
                    warnings,
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
                let mut warnings = Vec::new();
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
                    name,
                    display,
                    description,
                    warnings,
                )
            }
        };
        let asset = DiscoveredAsset {
            tool: crate::agent_assets::capabilities::ZCODE.into(),
            kind,
            scope: Scope::User,
            native_id,
            native_format: if kind == AssetKind::Agent {
                FM_AGENT.into()
            } else {
                SKILL_DIR.into()
            },
            display_name: display,
            description,
            entry_path,
            warnings: {
                warnings.insert(0, "来源：手选原生文件导入".to_string());
                warnings
            },
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
        Ok(plan_from_inspection(kind, scope, &report))
    }
}

impl ZcodeAdapter {
    // ---- agent（用户级）----

    fn discover_agents(&self) -> Result<Vec<DiscoveredAsset>, AssetError> {
        let root = self.agents_root();
        let mut out = Vec::new();
        let rd = match std::fs::read_dir(&root) {
            Ok(rd) => rd,
            Err(_) => return Ok(out), // 目录不存在 = 没有资源，不是错误
        };
        let mut names: Vec<_> = rd.filter_map(|e| e.ok()).collect();
        names.sort_by_key(|e| e.file_name());
        for entry in names {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue; // 隐藏文件/目录不是 agent
            }
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            if !name.to_ascii_lowercase().ends_with(".md") {
                continue;
            }
            let stem = name
                .trim_end_matches(".md")
                .trim_end_matches(".MD")
                .to_string();
            if !safe_segment(&stem) {
                continue; // 文件名无法构成安全包内路径（设备名/短名等）→ 不纳管
            }
            let (display, desc, mut warnings) = (stem.clone(), None, Vec::new());
            let (display, desc) = match read_text(&path).as_deref().and_then(parse_frontmatter) {
                Some(fm) => {
                    let display = fm
                        .entries
                        .iter()
                        .find(|(k, _)| k == "name")
                        .map(|(_, v)| v.as_str())
                        .filter(|s| !s.trim().is_empty())
                        .unwrap_or(display);
                    let desc = fm
                        .entries
                        .iter()
                        .find(|(k, _)| k == "description")
                        .map(|(_, v)| v.as_str());
                    (display, desc)
                }
                None => {
                    warnings.push("frontmatter 缺失或未闭合，元数据降级（不影响归档）".to_string());
                    (display, desc)
                }
            };
            let entry_path = format!("{stem}.md");
            out.push(DiscoveredAsset {
                tool: crate::agent_assets::capabilities::ZCODE.into(),
                kind: AssetKind::Agent,
                scope: Scope::User,
                native_id: stem,
                native_format: FM_AGENT.into(),
                display_name: display,
                description: desc,
                entry_path,
                warnings,
                source_root: root.to_string_lossy().into_owned(),
            });
        }
        Ok(out)
    }

    // ---- skill（用户级/项目级，双根）----

    fn discover_skills(
        &self,
        scope: Scope,
        project_root: Option<&Path>,
    ) -> Result<Vec<DiscoveredAsset>, AssetError> {
        let mut out: Vec<DiscoveredAsset> = Vec::new();
        let mut seen: std::collections::BTreeMap<String, String> =
            std::collections::BTreeMap::new();
        for root in self.skill_roots(scope, project_root) {
            let rd = match std::fs::read_dir(&root) {
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
                if !entry.path().join("SKILL.md").is_file() {
                    continue;
                }
                if !safe_segment(&name) {
                    continue;
                }
                let mut warnings = Vec::new();
                if let Some(prev_root) = seen.get(&name) {
                    // 双根同名：按官方优先级取先出现者（.zcode 优先），后到的记遮蔽警告
                    warnings.push(format!(
                        "同名 skill 也存在于 {prev_root}（按 .zcode 优先级被遮蔽，本条为高优先级副本）"
                    ));
                    continue;
                }
                seen.insert(name.clone(), root.to_string_lossy().into_owned());
                let mut display = name.clone();
                let mut desc: Option<String> = None;
                if let Some(fm) = read_text(&entry.path().join("SKILL.md"))
                    .as_deref()
                    .and_then(parse_frontmatter)
                {
                    if let Some((_, v)) = fm.entries.iter().find(|(k, _)| k == "name") {
                        let n = v.as_str();
                        if !n.trim().is_empty() {
                            display = n;
                        }
                    }
                    desc = fm
                        .entries
                        .iter()
                        .find(|(k, _)| k == "description")
                        .map(|(_, v)| v.as_str());
                } else {
                    warnings.push(
                        "SKILL.md frontmatter 缺失或未闭合，元数据降级（不影响归档）".to_string(),
                    );
                }
                out.push(DiscoveredAsset {
                    tool: crate::agent_assets::capabilities::ZCODE.into(),
                    kind: AssetKind::Skill,
                    scope,
                    native_id: name,
                    native_format: SKILL_DIR.into(),
                    display_name: display,
                    description: desc,
                    entry_path: "SKILL.md".into(),
                    warnings,
                    source_root: entry.path().to_string_lossy().into_owned(),
                });
            }
        }
        Ok(out)
    }

    // ---- inspect（定位后解析）----

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
        let mut files: Vec<FileBrief> = Vec::new();

        match kind {
            AssetKind::Agent => {
                let path = root.join(&found.entry_path);
                let text = read_text(&path);
                match text.as_deref().and_then(parse_frontmatter) {
                    Some(fm) => {
                        metadata.parsed_ok = true;
                        metadata.name = fm
                            .entries
                            .iter()
                            .find(|(k, _)| k == "name")
                            .map(|(_, v)| v.as_str())
                            .into();
                        metadata.description = fm
                            .entries
                            .iter()
                            .find(|(k, _)| k == "description")
                            .map(|(_, v)| v.as_str())
                            .into();
                        metadata.model = fm
                            .entries
                            .iter()
                            .find(|(k, _)| k == "model")
                            .map(|(_, v)| v.as_str())
                            .into();
                        metadata.tools = fm
                            .entries
                            .iter()
                            .find(|(k, _)| k == "tools")
                            .map(|(_, v)| v.as_list())
                            .unwrap_or_default();
                        metadata.frontmatter_raw = Some(fm.raw.clone());
                        sensitive = sensitive_hints(&found.entry_path, &fm);
                    }
                    None => {
                        let reason = match &text {
                            None => "文件不可读或不是 UTF-8".to_string(),
                            Some(_) => "frontmatter 缺失或未闭合".to_string(),
                        };
                        metadata.parse_error = Some(reason);
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
                let skill_md = found.entry_path.clone(); // "SKILL.md"
                let path = root.join(&skill_md);
                match read_text(&path).as_deref().and_then(parse_frontmatter) {
                    Some(fm) => {
                        metadata.parsed_ok = true;
                        metadata.name = fm
                            .entries
                            .iter()
                            .find(|(k, _)| k == "name")
                            .map(|(_, v)| v.as_str())
                            .into();
                        metadata.description = fm
                            .entries
                            .iter()
                            .find(|(k, _)| k == "description")
                            .map(|(_, v)| v.as_str())
                            .into();
                        metadata.frontmatter_raw = Some(fm.raw.clone());
                        sensitive = sensitive_hints(&skill_md, &fm);
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
                files = walk_skill_dir(root, &mut warnings)?;
                // 发现后入口文件被删（竞态）→ 明确报错而不是打出空入口的包
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
            external_references: Vec::new(), // ZCode 首期无包外引用形态（契约 §3.5 允许为空）
        })
    }
}

// ============================================================
// 单测：契约 §10.3 最小样例的完整往返 + 遮蔽/降级/边界。
// 全部使用临时 HOME，绝不触碰用户真实 ~/.zcode。
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_assets::bundle::{self, AssetBundle};

    fn adapter_at(home: &Path) -> ZcodeAdapter {
        ZcodeAdapter::with_home(home.to_path_buf())
    }

    /// 契约 §10.3 的最小 agent 样例（frontmatter 含列表与 CRLF）。
    const AGENT_SAMPLE: &str = "---\r\nname: demo-agent\r\ndescription: 演示用最小子代理\r\nmodel: glm-4.7\r\ntools:\r\n  - read\r\n  - grep\r\n---\r\n你是演示子代理。正文支持任意 Markdown，含未知 frontmatter 字段时原样保留。\r\n";

    /// 契约 §10.3 的最小 skill 样例（SKILL.md + 脚本 + 二进制附件 + 引用文档）。
    fn write_skill_sample(home: &Path) {
        let dir = home.join(".zcode/skills/demo-skill");
        std::fs::create_dir_all(dir.join("scripts")).unwrap();
        std::fs::create_dir_all(dir.join("assets")).unwrap();
        std::fs::create_dir_all(dir.join("references")).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            "---\r\nname: demo-skill\r\ndescription: 演示技能\r\n---\r\n正文 CRLF 也要逐字节保留\r\n".as_bytes(),
        )
        .unwrap();
        std::fs::write(dir.join("scripts/run.sh"), b"#!/bin/sh\necho demo\n").unwrap();
        std::fs::write(
            dir.join("assets/logo.png"),
            [0x89u8, 0x50, 0x4E, 0x00].as_slice(),
        )
        .unwrap();
        std::fs::write(dir.join("references/tips.md"), b"tips").unwrap();
    }

    #[test]
    fn agent_discover_inspect_plan_round_trip() {
        let home = crate::agent_assets::tests_support::temp_root("zc-agent");
        std::fs::create_dir_all(home.join(".zcode/agents")).unwrap();
        std::fs::write(home.join(".zcode/agents/demo-agent.md"), AGENT_SAMPLE).unwrap();

        let ad = adapter_at(&home);
        let found = ad.discover(AssetKind::Agent, Scope::User, None).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].native_id, "demo-agent");
        assert_eq!(found[0].display_name, "demo-agent");

        let report = ad
            .inspect(AssetKind::Agent, Scope::User, "demo-agent", None)
            .unwrap();
        assert!(report.metadata.parsed_ok);
        assert_eq!(report.metadata.model.as_deref(), Some("glm-4.7"));
        assert_eq!(report.metadata.tools, vec!["read", "grep"]);
        assert!(report
            .metadata
            .frontmatter_raw
            .as_deref()
            .unwrap()
            .starts_with("---"));
        assert_eq!(report.files.len(), 1);
        assert_eq!(report.files[0].path, "demo-agent.md");

        // plan → pack → 传输序列化 → parse 复验：指纹逐位一致
        let plan = ad
            .plan_export(AssetKind::Agent, Scope::User, "demo-agent", None)
            .unwrap();
        assert_eq!(plan.entry_path, "demo-agent.md");
        let envelope = bundle::pack_from_disk(Path::new(&plan.source_root), &plan).unwrap();
        let wire = serde_json::to_string(&envelope).unwrap();
        let back: AssetBundle = serde_json::from_str(&wire).unwrap();
        let parsed = bundle::parse_bundle(&back).unwrap();
        assert_eq!(
            bundle::manifest_digest(&parsed.manifest),
            envelope.manifest_sha256
        );
        assert_eq!(bundle::bundle_digest(&parsed.files), envelope.bundle_sha256);
        // CRLF 与中文逐字节保留（原文件不经重新序列化）
        assert_eq!(parsed.files[0].1, AGENT_SAMPLE.as_bytes());
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn skill_round_trip_with_script_and_binary_attachment() {
        let home = crate::agent_assets::tests_support::temp_root("zc-skill");
        write_skill_sample(&home);

        let ad = adapter_at(&home);
        let found = ad.discover(AssetKind::Skill, Scope::User, None).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].native_id, "demo-skill");
        assert_eq!(found[0].entry_path, "SKILL.md");

        let report = ad
            .inspect(AssetKind::Skill, Scope::User, "demo-skill", None)
            .unwrap();
        assert!(report.metadata.parsed_ok);
        assert_eq!(report.files.len(), 4, "SKILL.md + 脚本 + 二进制 + 引用");
        let by_path: std::collections::BTreeMap<&str, &FileBrief> =
            report.files.iter().map(|f| (f.path.as_str(), f)).collect();
        assert!(by_path["scripts/run.sh"].executable);
        assert!(!by_path["SKILL.md"].executable);

        let plan = ad
            .plan_export(AssetKind::Skill, Scope::User, "demo-skill", None)
            .unwrap();
        let envelope = bundle::pack_from_disk(Path::new(&plan.source_root), &plan).unwrap();
        assert_eq!(envelope.manifest.entry_path, "SKILL.md");
        // 伪 PNG 嗅探为二进制；脚本标可执行
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
        // CRLF 逐字节保留
        let skill_md = parsed
            .files
            .iter()
            .find(|(p, _)| p == "SKILL.md")
            .map(|(_, b)| b.clone())
            .unwrap();
        assert!(
            skill_md.windows(2).any(|w| w == b"\r\n"),
            "SKILL.md 应保留 CRLF"
        );
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    #[cfg(unix)]
    fn skill_plan_export_rejects_symlink_entries() {
        // 符号链接被静默跳过会让 plan.files 缺条目，pack 仍“成功”产出缺文件
        // 的包；因此必须在 plan_export 阶段直接返回 PATH_UNSAFE（契约 §9.7）。
        let home = crate::agent_assets::tests_support::temp_root("zc-symlink");
        let dir = home.join(".zcode/skills/demo-skill");
        std::fs::create_dir_all(dir.join("references")).unwrap();
        std::fs::write(dir.join("SKILL.md"), b"---\nname: demo-skill\n---\nbody").unwrap();
        std::fs::write(dir.join("references/tips.md"), b"tips").unwrap();
        std::os::unix::fs::symlink("tips.md", dir.join("references/link.md")).unwrap();
        let ad = adapter_at(&home);
        let err = ad
            .plan_export(AssetKind::Skill, Scope::User, "demo-skill", None)
            .unwrap_err();
        assert_eq!(err.code(), "PATH_UNSAFE");
        assert_eq!(err.details.as_ref().unwrap()["path"], "references/link.md");
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn shadowed_skill_between_roots_warns() {
        let home = crate::agent_assets::tests_support::temp_root("zc-shadow");
        for root in [".zcode/skills", ".agents/skills"] {
            let dir = home.join(root).join("dup");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("SKILL.md"), b"---\nname: dup\n---\nbody").unwrap();
        }
        let ad = adapter_at(&home);
        let found = ad.discover(AssetKind::Skill, Scope::User, None).unwrap();
        assert_eq!(found.len(), 1, "双根同名只出一个（.zcode 优先）");
        assert!(found[0].source_root.contains(".zcode"), "应取 .zcode 副本");
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn project_agent_unsupported_and_project_skill_works() {
        let home = crate::agent_assets::tests_support::temp_root("zc-proj");
        std::fs::create_dir_all(&home).unwrap();
        let project = home.join("proj");
        let dir = project.join(".zcode/skills/ps-skill");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("SKILL.md"), b"---\nname: ps-skill\n---\nbody").unwrap();

        let ad = adapter_at(&home);
        // 子代理项目级：能力未声明 → CAPABILITY_UNSUPPORTED
        let err = ad
            .discover(AssetKind::Agent, Scope::Project, Some(&project))
            .unwrap_err();
        assert_eq!(err.code(), "CAPABILITY_UNSUPPORTED");
        // 缺 project_root → VALIDATION_FAILED
        let err = ad
            .discover(AssetKind::Skill, Scope::Project, None)
            .unwrap_err();
        assert_eq!(err.code(), "VALIDATION_FAILED");
        // 项目级 skill：可发现、可归档（打包走通）
        let found = ad
            .discover(AssetKind::Skill, Scope::Project, Some(&project))
            .unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].native_id, "ps-skill");
        assert_eq!(found[0].scope, Scope::Project);
        let plan = ad
            .plan_export(AssetKind::Skill, Scope::Project, "ps-skill", Some(&project))
            .unwrap();
        let envelope = bundle::pack_from_disk(Path::new(&plan.source_root), &plan).unwrap();
        assert_eq!(envelope.manifest.origin_scope, Scope::Project);
        assert!(bundle::parse_bundle(&envelope).is_ok());
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn broken_frontmatter_degrades_but_does_not_block() {
        let home = crate::agent_assets::tests_support::temp_root("zc-broken-fm");
        std::fs::create_dir_all(home.join(".zcode/agents")).unwrap();
        // 只有起始围栏、没有闭合 → 解析失败但不阻断发现与归档
        std::fs::write(
            home.join(".zcode/agents/broken.md"),
            "---\nname: broken\n正文吞掉了闭合围栏",
        )
        .unwrap();
        let ad = adapter_at(&home);
        let found = ad.discover(AssetKind::Agent, Scope::User, None).unwrap();
        assert_eq!(found.len(), 1);
        assert!(!found[0].warnings.is_empty());
        let report = ad
            .inspect(AssetKind::Agent, Scope::User, "broken", None)
            .unwrap();
        assert!(!report.metadata.parsed_ok);
        assert!(report.metadata.parse_error.is_some());
        // 归档链路照常走通（原文件字节为真）
        let plan = ad
            .plan_export(AssetKind::Agent, Scope::User, "broken", None)
            .unwrap();
        assert!(bundle::pack_from_disk(Path::new(&plan.source_root), &plan).is_ok());
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn sensitive_frontmatter_fields_hinted() {
        let home = crate::agent_assets::tests_support::temp_root("zc-sensitive");
        std::fs::create_dir_all(home.join(".zcode/agents")).unwrap();
        std::fs::write(
            home.join(".zcode/agents/secretive.md"),
            "---\nname: secretive\ndescription: 带密钥的示例\napi_key: sk-xxx\n---\n正文",
        )
        .unwrap();
        let ad = adapter_at(&home);
        let report = ad
            .inspect(AssetKind::Agent, Scope::User, "secretive", None)
            .unwrap();
        assert!(report.metadata.parsed_ok);
        assert_eq!(report.sensitive_fields.len(), 1);
        assert_eq!(report.sensitive_fields[0].field, "api_key");
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn unknown_native_id_is_not_found() {
        let home = crate::agent_assets::tests_support::temp_root("zc-missing");
        std::fs::create_dir_all(home.join(".zcode/agents")).unwrap();
        let ad = adapter_at(&home);
        let err = ad
            .inspect(AssetKind::Agent, Scope::User, "nope", None)
            .unwrap_err();
        assert_eq!(err.code(), "NOT_FOUND");
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn frontmatter_parser_basics() {
        // 行内列表与未知行容忍
        let fm =
            parse_frontmatter("---\nname: x\ntools: [a, b]\nnested:\n  k: v\n---\nbody").unwrap();
        assert_eq!(
            fm.entries
                .iter()
                .find(|(k, _)| k == "tools")
                .map(|(_, v)| v.as_list()),
            Some(vec!["a".to_string(), "b".to_string()])
        );
        // 无 frontmatter
        assert!(parse_frontmatter("只是正文").is_none());
        // 未闭合
        assert!(parse_frontmatter("---\nname: x\n").is_none());
    }

    #[test]
    fn import_path_custom_agent_and_skill_locations() {
        let root = crate::agent_assets::tests_support::temp_root("zc-import");
        let agent_file = root.join("custom-file.md");
        let agent_body = "---\nname: 显示名\ndescription: 手选\n---\n正文".as_bytes();
        std::fs::write(&agent_file, agent_body).unwrap();
        let skill_dir = root.join("my-skill");
        std::fs::create_dir_all(skill_dir.join("scripts")).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            b"---\nname: my-skill\ndescription: manual skill\n---\nbody",
        )
        .unwrap();
        std::fs::write(skill_dir.join("scripts/run.sh"), b"#!/bin/sh\n").unwrap();
        let bad = root.join("bad.txt");
        std::fs::write(&bad, b"x").unwrap();

        let ad = adapter_at(&root);
        let report = ad.import_path(AssetKind::Agent, &agent_file).unwrap();
        assert_eq!(
            report.asset.native_id, "custom-file",
            "ZCode 取文件名去扩展"
        );
        assert_eq!(report.asset.display_name, "显示名");
        assert_eq!(report.asset.entry_path, "custom-file.md");
        assert_eq!(Path::new(&report.asset.source_root), root.as_path());
        let plan = ad
            .plan_export_from_source(AssetKind::Agent, Scope::User, &agent_file)
            .unwrap();
        let envelope = bundle::pack_from_disk(Path::new(&plan.source_root), &plan).unwrap();
        let parsed = bundle::parse_bundle(&envelope).unwrap();
        assert_eq!(parsed.files[0].1, agent_body);
        assert_eq!(bundle::bundle_digest(&parsed.files), envelope.bundle_sha256);

        let report = ad.import_path(AssetKind::Skill, &skill_dir).unwrap();
        assert_eq!(report.asset.native_id, "my-skill");
        assert_eq!(report.files.len(), 2);
        assert!(ad.import_path(AssetKind::Agent, &bad).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 真实目录发现验证（环境变量门控，不进常规断言链）：
    /// `AGENTHUD_AGENT_ASSETS_REALDIR=1 cargo test agent_assets` 时才扫真实
    /// `~/.zcode`，只读且只断言「不崩溃」，结果打印到测试输出供人工核对。
    #[test]
    fn real_directory_discovery_gated() {
        if std::env::var_os("AGENTHUD_AGENT_ASSETS_REALDIR").is_none() {
            return; // 常规 cargo test 直接跳过，绝不碰用户真实 ~/.zcode
        }
        let ad = ZcodeAdapter::new();
        let agents = ad.discover(AssetKind::Agent, Scope::User, None).unwrap();
        println!("真实目录 ZCode agents: {} 个", agents.len());
        for a in &agents {
            println!("  - {} ({})", a.native_id, a.source_root);
            let _ = ad
                .inspect(AssetKind::Agent, Scope::User, &a.native_id, None)
                .unwrap();
        }
        let skills = ad.discover(AssetKind::Skill, Scope::User, None).unwrap();
        println!("真实目录 ZCode skills: {} 个", skills.len());
        for s in &skills {
            println!("  - {} ({})", s.native_id, s.source_root);
            let _ = ad
                .inspect(AssetKind::Skill, Scope::User, &s.native_id, None)
                .unwrap();
        }
    }
}
