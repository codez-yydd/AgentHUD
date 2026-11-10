//! 共用安装 / 原生导出层（契约 §3.3、§9、§3.4）。
//!
//! 管线：`parse_bundle` 复验指纹 → 路径安全（bundle 内已校验）→ 目标根解析
//! （与各适配器 discover 根一致）→ 冲突默认跳过 / `overwrite` 先备份 →
//! 同卷暂存 → 原子 rename 发布；失败保留或恢复原内容。
//!
//! 设计边界：
//! - 安装/导出都只消费**已复验**的 `AssetBundle`，不信任调用方；
//! - 目标根解析按 工具 × kind × 目标作用域；共享目录（`.agents/skills/`、
//!   `~/.claude/skills/` 兼容根）安装时强制 warnings 提示影响其他工具；
//! - 内联格式（`jsonc-inline`）与旧 YAML（`yaml-legacy`）不提供安装，
//!   只做归档与原样导出（契约 §3.4 / §5.2）。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use crate::agent_assets::adapters::common;
use crate::agent_assets::bundle::{self, AssetBundle, ParsedBundle};
use crate::agent_assets::capabilities::{
    self, CapabilityBit, CLAUDE, CODEX, KIMI, OPENCODE, ZCODE,
};
use crate::agent_assets::{AssetError, AssetKind, Scope};

// ===== 返回结构（契约 §7 追加命令）=====

/// 一条跳过记录。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkipItem {
    /// 目标绝对路径（或目标目录）。
    pub path: String,
    /// 跳过原因（人读中文）。
    pub reason: String,
}

/// `agent_assets_export_to_disk` 的返回。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiskExportResult {
    /// 已写出的文件绝对路径。
    pub written: Vec<String>,
    #[serde(default)]
    pub skipped: Vec<SkipItem>,
    /// 覆盖前备份目录（未覆盖时为 null）。
    #[serde(default)]
    pub backup_dir: Option<String>,
    #[serde(default)]
    pub warnings: Vec<String>,
}

/// `agent_assets_install` 的返回。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallResult {
    /// 已安装的文件绝对路径（目录型 skill 逐文件列出）。
    pub installed_paths: Vec<String>,
    #[serde(default)]
    pub skipped: Vec<SkipItem>,
    /// 覆盖前备份目录（未覆盖时为 null）。
    #[serde(default)]
    pub backup_dir: Option<String>,
    /// 安装后用户需手动完成的事项（四款工具当前均为 null）。
    #[serde(default)]
    pub tool_instructions: Option<String>,
    #[serde(default)]
    pub warnings: Vec<String>,
}

// ===== 格式与能力 =====

const FM: &str = "markdown-frontmatter";
const TOML_FMT: &str = "toml";
const SKILL_DIR: &str = "skill-dir";
const YAML_LEGACY: &str = "yaml-legacy";
const JSONC_INLINE: &str = "jsonc-inline";

/// 该 工具 × kind × native_format 是否可原生导出（契约 §5.2）。
fn format_native_exportable(tool: &str, kind: AssetKind, native_format: &str) -> bool {
    match (tool, kind) {
        (ZCODE, AssetKind::Agent) => native_format == FM,
        (CODEX, AssetKind::Agent) => native_format == TOML_FMT,
        (CLAUDE, AssetKind::Agent) => native_format == FM,
        (KIMI, AssetKind::Agent) => native_format == FM || native_format == YAML_LEGACY,
        (OPENCODE, AssetKind::Agent) => native_format == FM || native_format == JSONC_INLINE,
        (ZCODE | CODEX | CLAUDE | KIMI | OPENCODE, AssetKind::Skill) => native_format == SKILL_DIR,
        _ => false,
    }
}

/// 该 工具 × kind × native_format 是否可安装（契约 §5.2）。
///
/// `yaml-legacy` 与 `jsonc-inline` 与当前 Markdown 共用 tool/kind/scope，
/// 只在格式维度上区分；仅靠 `has_capability` 会把它们误判为可安装。
fn format_installable(tool: &str, kind: AssetKind, native_format: &str) -> bool {
    match (tool, kind) {
        (ZCODE, AssetKind::Agent) => native_format == FM,
        (CODEX, AssetKind::Agent) => native_format == TOML_FMT,
        (CLAUDE, AssetKind::Agent) => native_format == FM,
        (KIMI, AssetKind::Agent) => native_format == FM,
        (OPENCODE, AssetKind::Agent) => native_format == FM,
        (ZCODE | CODEX | CLAUDE | KIMI | OPENCODE, AssetKind::Skill) => native_format == SKILL_DIR,
        _ => false,
    }
}

fn ensure_verified(
    tool: &str,
    kind: AssetKind,
    scope: Scope,
    bit: CapabilityBit,
    format: &str,
    action: &str,
) -> Result<(), AssetError> {
    if !capabilities::has_capability(tool, kind, scope, bit) {
        return Err(AssetError::capability_unsupported(format!(
            "工具 {tool} 的 {} @ {} 未声明{action}能力",
            kind.as_str(),
            scope.as_str()
        )));
    }
    let ok = match bit {
        CapabilityBit::NativeExportable => format_native_exportable(tool, kind, format),
        CapabilityBit::Installable => format_installable(tool, kind, format),
        _ => true,
    };
    if !ok {
        return Err(AssetError::capability_unsupported(format!(
            "工具 {tool} 的 {} @ {} 不支持格式 {format} 的{action}",
            kind.as_str(),
            scope.as_str()
        )));
    }
    Ok(())
}

// ===== 目标根解析（与各适配器 discover 根一致）=====

/// 目标布局解析所需的目录来源（默认读进程环境；测试注入临时 HOME）。
#[derive(Debug, Clone)]
pub(crate) struct LayoutEnv {
    pub home: PathBuf,
    /// Codex 状态根（`CODEX_HOME`，默认 `~/.codex`）。
    pub codex_home: PathBuf,
    /// Kimi Code 数据根（`KIMI_CODE_HOME`，默认 `~/.kimi-code`）。
    pub kimi_home: PathBuf,
    /// OpenCode 用户配置目录（尊重 `XDG_CONFIG_HOME`）。
    pub opencode_config: PathBuf,
}

impl LayoutEnv {
    pub(crate) fn from_process() -> Self {
        let home = common::home_dir();
        let codex_home = common::env_path("CODEX_HOME").unwrap_or_else(|| home.join(".codex"));
        let kimi_home =
            common::env_path("KIMI_CODE_HOME").unwrap_or_else(|| home.join(".kimi-code"));
        let opencode_config = common::env_path("XDG_CONFIG_HOME")
            .map(|p| p.join("opencode"))
            .unwrap_or_else(|| home.join(".config").join("opencode"));
        Self {
            home,
            codex_home,
            kimi_home,
            opencode_config,
        }
    }
}

/// 安装目标根。
#[derive(Debug, Clone)]
pub(crate) struct InstallLayout {
    pub root: PathBuf,
    /// 该目录同时被其他工具读取（安装须提示影响范围）。
    pub shared: bool,
}

/// 解析安装目标根；未声明 installable 的组合返回 CAPABILITY_UNSUPPORTED。
pub(crate) fn resolve_layout_with(
    env: &LayoutEnv,
    tool: &str,
    kind: AssetKind,
    scope: Scope,
    project_root: Option<&Path>,
) -> Result<InstallLayout, AssetError> {
    if scope == Scope::Project {
        match project_root {
            None => {
                return Err(AssetError::validation(
                    "project 作用域必须提供 project_root（项目根绝对路径）",
                ))
            }
            Some(p) if !p.is_absolute() => {
                return Err(AssetError::validation(format!(
                    "project_root 必须是绝对路径: {}",
                    p.display()
                )))
            }
            _ => {}
        }
    }
    let project = project_root.unwrap_or_else(|| Path::new(""));
    let layout = match (tool, kind, scope) {
        // ZCode：仅用户级；skill 安装到工具专属目录（不写共享 .agents）。
        (ZCODE, AssetKind::Agent, Scope::User) => InstallLayout {
            root: env.home.join(".zcode").join("agents"),
            shared: false,
        },
        (ZCODE, AssetKind::Skill, Scope::User) => InstallLayout {
            root: env.home.join(".zcode").join("skills"),
            shared: false,
        },
        // Codex：agent 在 CODEX_HOME/agents；skill 官方用户级是共享 ~/.agents/skills。
        (CODEX, AssetKind::Agent, Scope::User) => InstallLayout {
            root: env.codex_home.join("agents"),
            shared: false,
        },
        (CODEX, AssetKind::Skill, Scope::User) => InstallLayout {
            root: env.home.join(".agents").join("skills"),
            shared: true,
        },
        (CODEX, AssetKind::Skill, Scope::Project) => InstallLayout {
            root: project.join(".agents").join("skills"),
            shared: true,
        },
        // Claude Code：用户级 agent；skill 目录同时被 OpenCode 作为兼容根读取。
        (CLAUDE, AssetKind::Agent, Scope::User) => InstallLayout {
            root: env.home.join(".claude").join("agents"),
            shared: false,
        },
        (CLAUDE, AssetKind::Skill, Scope::User) => InstallLayout {
            root: env.home.join(".claude").join("skills"),
            shared: true,
        },
        (CLAUDE, AssetKind::Skill, Scope::Project) => InstallLayout {
            root: project.join(".claude").join("skills"),
            shared: true,
        },
        // Kimi Code：用户级安装到 $KIMI_CODE_HOME 下的工具专属目录。
        (KIMI, AssetKind::Agent, Scope::User) => InstallLayout {
            root: env.kimi_home.join("agents"),
            shared: false,
        },
        (KIMI, AssetKind::Skill, Scope::User) => InstallLayout {
            root: env.kimi_home.join("skills"),
            shared: false,
        },
        (KIMI, AssetKind::Skill, Scope::Project) => InstallLayout {
            root: project.join(".kimi-code").join("skills"),
            shared: false,
        },
        // OpenCode：用户配置目录（XDG）；项目级 skill 在 .opencode/skills。
        (OPENCODE, AssetKind::Agent, Scope::User) => InstallLayout {
            root: env.opencode_config.join("agents"),
            shared: false,
        },
        (OPENCODE, AssetKind::Skill, Scope::User) => InstallLayout {
            root: env.opencode_config.join("skills"),
            shared: false,
        },
        (OPENCODE, AssetKind::Skill, Scope::Project) => InstallLayout {
            root: project.join(".opencode").join("skills"),
            shared: false,
        },
        _ => {
            return Err(AssetError::capability_unsupported(format!(
                "工具 {tool} 的 {} @ {} 未声明可安装能力（首期仅用户级 agent / 已开放作用域）",
                kind.as_str(),
                scope.as_str()
            )))
        }
    };
    Ok(layout)
}

// ===== 安装目标（文件型 agent / 目录型 skill）=====

#[derive(Debug)]
enum PutTarget {
    /// 单文件 agent：包内只允许入口文件。
    File { path: PathBuf, package_path: String },
    /// 目录型 skill：包内全部文件写入资源目录。
    Dir { path: PathBuf },
}

fn resolve_put_target(
    tool: &str,
    kind: AssetKind,
    manifest: &bundle::AssetManifest,
    root: &Path,
) -> Result<PutTarget, AssetError> {
    match kind {
        AssetKind::Skill => {
            if !common::safe_segment(&manifest.native_id) {
                return Err(AssetError::validation(format!(
                    "skill native_id 不是安全单段目录名: {}",
                    manifest.native_id
                )));
            }
            Ok(PutTarget::Dir {
                path: root.join(&manifest.native_id),
            })
        }
        AssetKind::Agent => {
            let (rel, package_path) = match tool {
                ZCODE => {
                    if !common::safe_segment(&manifest.native_id) {
                        return Err(AssetError::validation(format!(
                            "ZCode agent native_id 不是安全单段名: {}",
                            manifest.native_id
                        )));
                    }
                    (
                        format!("{}.md", manifest.native_id),
                        manifest.entry_path.clone(),
                    )
                }
                CODEX => {
                    if !common::safe_segment(&manifest.native_id) {
                        return Err(AssetError::validation(format!(
                            "Codex agent native_id 不是安全单段名: {}",
                            manifest.native_id
                        )));
                    }
                    (
                        format!("{}.toml", manifest.native_id),
                        manifest.entry_path.clone(),
                    )
                }
                CLAUDE | KIMI => {
                    // 身份来自 frontmatter.name；文件名仅作发现键，安装保留原相对路径。
                    if manifest.entry_path.is_empty()
                        || !common::safe_relative_path(&manifest.entry_path)
                    {
                        return Err(AssetError::validation(format!(
                            "agent entry_path 不是安全相对路径: {}",
                            manifest.entry_path
                        )));
                    }
                    (manifest.entry_path.clone(), manifest.entry_path.clone())
                }
                OPENCODE => {
                    // V2 嵌套 ID（team/reviewer）直接映射为嵌套文件名。
                    let rel = format!("{}.md", manifest.native_id);
                    if !common::safe_relative_path(&rel) {
                        return Err(AssetError::validation(format!(
                            "OpenCode agent native_id 不是安全相对路径: {}",
                            manifest.native_id
                        )));
                    }
                    (rel, manifest.entry_path.clone())
                }
                _ => {
                    return Err(AssetError::capability_unsupported(format!(
                        "工具 {tool} 未实现 agent 安装"
                    )))
                }
            };
            Ok(PutTarget::File {
                path: root.join(rel),
                package_path,
            })
        }
    }
}

// ===== 安装 =====

/// 安装一个已下载/已打包的资源包到本机默认目录。
pub fn install_bundle(
    bundle: &AssetBundle,
    target_scope: Scope,
    project_root: Option<&Path>,
    overwrite: bool,
) -> Result<InstallResult, AssetError> {
    let parsed = bundle::parse_bundle(bundle)?;
    let m = &parsed.manifest;
    ensure_verified(
        &m.tool,
        m.kind,
        target_scope,
        CapabilityBit::Installable,
        &m.native_format,
        "安装",
    )?;
    let env = LayoutEnv::from_process();
    let layout = resolve_layout_with(&env, &m.tool, m.kind, target_scope, project_root)?;
    install_parsed(&parsed, &layout, overwrite)
}

/// 安装核心（测试注入目标布局用）。
pub(crate) fn install_parsed(
    parsed: &ParsedBundle,
    layout: &InstallLayout,
    overwrite: bool,
) -> Result<InstallResult, AssetError> {
    let m = &parsed.manifest;
    let root = &layout.root;
    ensure_dir_root(root)?;
    let target = resolve_put_target(&m.tool, m.kind, m, root)?;
    let target_path = match &target {
        PutTarget::File { path, .. } => path.clone(),
        PutTarget::Dir { path } => path.clone(),
    };

    let mut warnings = builtin_override_warnings(&m.tool, m.kind, &m.native_id, &parsed.files);
    // 四款工具的能力按官方文档实现、本机未安装目标 CLI 实测（verified_versions
    // 为空）：安装照常可用，但必须让用户知情并在目标工具内自行确认。
    if let Some(hint) = unverified_tool_warning(&m.tool) {
        warnings.push(hint);
    }
    if layout.shared {
        warnings.push(format!(
            "目标目录 {} 为共享目录：安装将影响同样读取该目录的其他工具",
            root.display()
        ));
    }

    let exists = std::fs::symlink_metadata(&target_path).is_ok();
    if exists && !overwrite {
        return Ok(InstallResult {
            installed_paths: Vec::new(),
            skipped: vec![SkipItem {
                path: target_path.to_string_lossy().into_owned(),
                reason: "目标已存在（overwrite=false，默认跳过）".to_string(),
            }],
            backup_dir: None,
            tool_instructions: None,
            warnings,
        });
    }

    // 文件型 agent 只允许一个入口文件（未知附件形态不静默丢失）。
    if let PutTarget::File { package_path, .. } = &target {
        if parsed.files.len() != 1 || parsed.files[0].0 != *package_path {
            return Err(AssetError::validation(format!(
                "单文件 agent 的资源包应只含入口文件 {package_path}，实际 {} 个条目",
                parsed.files.len()
            )));
        }
    }

    // 目标父目录：创建前检查符号链接/junction，创建后再检查一次。
    if let Some(parent) = target_path.parent() {
        create_dir_no_symlink(root, parent)?;
    }

    let staging = unique_staging_dir(root, "inst");
    std::fs::create_dir(&staging)
        .map_err(|e| AssetError::io(format!("创建暂存目录 {} 失败: {e}", staging.display())))?;
    let staged_entry = match &target {
        PutTarget::File { path, .. } => {
            let file_name = path
                .file_name()
                .ok_or_else(|| AssetError::validation("目标文件缺少文件名"))?;
            let staged = staging.join(file_name);
            let (_, bytes) = &parsed.files[0];
            let executable = m
                .files
                .iter()
                .find(|f| f.path == parsed.files[0].0)
                .map(|f| f.executable)
                .unwrap_or(false);
            if let Err(e) = write_file(&staged, bytes, executable) {
                let _ = std::fs::remove_dir_all(&staging);
                return Err(e);
            }
            Some(staged)
        }
        PutTarget::Dir { .. } => {
            for (rel, bytes) in &parsed.files {
                let dest = staging.join(rel);
                let executable = m
                    .files
                    .iter()
                    .find(|f| f.path == *rel)
                    .map(|f| f.executable)
                    .unwrap_or(false);
                if let Err(e) = write_file(&dest, bytes, executable) {
                    let _ = std::fs::remove_dir_all(&staging);
                    return Err(e);
                }
            }
            None
        }
    };

    // 备份原内容（默认跳过路径已在上面返回；这里只处理 overwrite）。
    let mut backup_dir: Option<PathBuf> = None;
    let mut backup_entry: Option<PathBuf> = None;
    if exists {
        let dir = unique_backup_dir(root);
        std::fs::create_dir_all(&dir)
            .map_err(|e| AssetError::io(format!("创建备份目录 {} 失败: {e}", dir.display())))?;
        let name = target_path
            .file_name()
            .ok_or_else(|| AssetError::validation("目标缺少文件名，无法备份"))?;
        let moved = dir.join(name);
        std::fs::rename(&target_path, &moved).map_err(|e| {
            let _ = std::fs::remove_dir_all(&staging);
            AssetError::io(format!(
                "备份 {} 到 {} 失败: {e}",
                target_path.display(),
                moved.display()
            ))
        })?;
        backup_dir = Some(dir);
        backup_entry = Some(moved);
    }

    // 原子发布：同卷 rename。
    let publish = match &target {
        PutTarget::File { .. } => std::fs::rename(staged_entry.as_ref().unwrap(), &target_path),
        PutTarget::Dir { .. } => std::fs::rename(&staging, &target_path),
    };
    if let Err(e) = publish {
        let _ = std::fs::remove_dir_all(&staging);
        // 失败恢复原内容。
        if let (Some(entry), Some(dir)) = (&backup_entry, &backup_dir) {
            let _ = std::fs::create_dir_all(root);
            if let Err(re) = std::fs::rename(entry, &target_path) {
                return Err(AssetError::io(format!(
                    "发布失败: {e}；恢复备份失败: {re}（备份目录 {}）",
                    dir.display()
                )));
            }
        }
        return Err(AssetError::io(format!(
            "发布 {} 失败: {e}",
            target_path.display()
        )));
    }

    if matches!(target, PutTarget::File { .. }) {
        let _ = std::fs::remove_dir_all(&staging);
    }

    let installed_paths = match &target {
        PutTarget::File { path, .. } => vec![path.to_string_lossy().into_owned()],
        PutTarget::Dir { path } => parsed
            .files
            .iter()
            .map(|(rel, _)| path.join(rel).to_string_lossy().into_owned())
            .collect(),
    };
    Ok(InstallResult {
        installed_paths,
        skipped: Vec::new(),
        backup_dir: backup_dir.map(|p| p.to_string_lossy().into_owned()),
        tool_instructions: None,
        warnings,
    })
}

// ===== 原生导出 =====

/// 把资源包还原为 `dest_dir/<tool>-<kind>-<native_id>/` 下的原生文件。
pub fn export_bundle(
    bundle: &AssetBundle,
    dest_dir: &Path,
    overwrite: bool,
) -> Result<DiskExportResult, AssetError> {
    let parsed = bundle::parse_bundle(bundle)?;
    let m = &parsed.manifest;
    ensure_verified(
        &m.tool,
        m.kind,
        m.origin_scope,
        CapabilityBit::NativeExportable,
        &m.native_format,
        "原生导出",
    )?;
    if dest_dir.as_os_str().is_empty() {
        return Err(AssetError::validation("导出目标目录不能为空"));
    }
    // 统一为绝对路径，保证返回的 written/backup_dir 可直接定位。
    let dest_dir = if dest_dir.is_absolute() {
        dest_dir.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|e| AssetError::io(format!("读取当前目录失败: {e}")))?
            .join(dest_dir)
    };
    std::fs::create_dir_all(&dest_dir)
        .map_err(|e| AssetError::io(format!("创建导出目录 {} 失败: {e}", dest_dir.display())))?;
    ensure_dir_root(&dest_dir)?;

    let dir_name = format!(
        "{}-{}-{}",
        m.tool,
        m.kind.as_str(),
        common::slugify(&m.native_id)
    );
    let target = dest_dir.join(&dir_name);
    let mut warnings = Vec::new();
    if m.native_format == JSONC_INLINE {
        warnings.push(
            "内联配置片段已按单条提取导出，请手动合并回 OpenCode 配置（首期不改写整份配置）"
                .to_string(),
        );
    }
    if m.native_format == YAML_LEGACY {
        warnings.push(
            "旧 kimi-cli YAML 已原样导出，需按旧版 --agent-file 显式加载（安装待验证）".to_string(),
        );
    }

    let exists = std::fs::symlink_metadata(&target).is_ok();
    if exists && !overwrite {
        return Ok(DiskExportResult {
            written: Vec::new(),
            skipped: vec![SkipItem {
                path: target.to_string_lossy().into_owned(),
                reason: "目标目录已存在（overwrite=false，默认跳过）".to_string(),
            }],
            backup_dir: None,
            warnings,
        });
    }

    let staging = unique_staging_dir(&dest_dir, "exp");
    std::fs::create_dir(&staging)
        .map_err(|e| AssetError::io(format!("创建暂存目录 {} 失败: {e}", staging.display())))?;
    for (rel, bytes) in &parsed.files {
        let dest = staging.join(rel);
        let executable = m
            .files
            .iter()
            .find(|f| f.path == *rel)
            .map(|f| f.executable)
            .unwrap_or(false);
        if let Err(e) = write_file(&dest, bytes, executable) {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(e);
        }
    }

    let mut backup_dir: Option<PathBuf> = None;
    let mut backup_entry: Option<PathBuf> = None;
    if exists {
        let dir = unique_backup_dir(&dest_dir);
        std::fs::create_dir_all(&dir)
            .map_err(|e| AssetError::io(format!("创建备份目录 {} 失败: {e}", dir.display())))?;
        let moved = dir.join(&dir_name);
        std::fs::rename(&target, &moved).map_err(|e| {
            let _ = std::fs::remove_dir_all(&staging);
            AssetError::io(format!(
                "备份 {} 到 {} 失败: {e}",
                target.display(),
                moved.display()
            ))
        })?;
        backup_dir = Some(dir);
        backup_entry = Some(moved);
    }

    if let Err(e) = std::fs::rename(&staging, &target) {
        let _ = std::fs::remove_dir_all(&staging);
        if let (Some(entry), Some(dir)) = (&backup_entry, &backup_dir) {
            if let Err(re) = std::fs::rename(entry, &target) {
                return Err(AssetError::io(format!(
                    "发布失败: {e}；恢复备份失败: {re}（备份目录 {}）",
                    dir.display()
                )));
            }
        }
        return Err(AssetError::io(format!(
            "发布 {} 失败: {e}",
            target.display()
        )));
    }

    Ok(DiskExportResult {
        written: parsed
            .files
            .iter()
            .map(|(rel, _)| target.join(rel).to_string_lossy().into_owned())
            .collect(),
        skipped: Vec::new(),
        backup_dir: backup_dir.map(|p| p.to_string_lossy().into_owned()),
        warnings,
    })
}

// ===== 落盘工具 =====

/// 确保目录存在且本体不是符号链接/junction。
fn ensure_dir_root(root: &Path) -> Result<(), AssetError> {
    std::fs::create_dir_all(root)
        .map_err(|e| AssetError::io(format!("创建目标根 {} 失败: {e}", root.display())))?;
    let md = std::fs::symlink_metadata(root)
        .map_err(|e| AssetError::io(format!("读取目标根 {} 元数据失败: {e}", root.display())))?;
    if md.file_type().is_symlink() {
        return Err(AssetError::path_unsafe(format!(
            "目标根 {} 是符号链接/junction，拒绝写入",
            root.display()
        )));
    }
    if !md.is_dir() {
        return Err(AssetError::io(format!(
            "目标根 {} 不是目录",
            root.display()
        )));
    }
    Ok(())
}

/// 逐级创建 `dir`（必须位于 `root` 下），路径上任何已存在的段都不得是符号链接。
fn create_dir_no_symlink(root: &Path, dir: &Path) -> Result<(), AssetError> {
    if !dir.starts_with(root) {
        return Err(AssetError::path_unsafe(format!(
            "目标目录逃逸出目标根: {}",
            dir.display()
        )));
    }
    let rel = dir
        .strip_prefix(root)
        .map_err(|_| AssetError::path_unsafe("无法计算目标相对路径"))?;
    let mut cur = root.to_path_buf();
    for seg in rel.components() {
        cur.push(seg);
        match std::fs::symlink_metadata(&cur) {
            Ok(md) => {
                if md.file_type().is_symlink() {
                    return Err(AssetError::path_unsafe(format!(
                        "目标路径段 {} 是符号链接/junction",
                        cur.display()
                    )));
                }
                if !md.is_dir() {
                    return Err(AssetError::io(format!(
                        "目标路径段 {} 不是目录",
                        cur.display()
                    )));
                }
            }
            Err(_) => {
                std::fs::create_dir(&cur)
                    .map_err(|e| AssetError::io(format!("创建目录 {} 失败: {e}", cur.display())))?;
            }
        }
    }
    Ok(())
}

/// 写一个文件并创建父目录；Unix 上按 executable 设置权限位。
fn write_file(path: &Path, bytes: &[u8], executable: bool) -> Result<(), AssetError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| AssetError::io(format!("创建目录 {} 失败: {e}", parent.display())))?;
    }
    std::fs::write(path, bytes)
        .map_err(|e| AssetError::io(format!("写入 {} 失败: {e}", path.display())))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = if executable { 0o755 } else { 0o644 };
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
    }
    #[cfg(not(unix))]
    let _ = executable;
    Ok(())
}

static SEQ: AtomicU64 = AtomicU64::new(0);

fn unique_staging_dir(root: &Path, tag: &str) -> PathBuf {
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    root.join(format!(
        ".agenthud-tmp-{tag}-{}-{}-{seq}",
        std::process::id(),
        common::epoch_ms()
    ))
}

fn unique_backup_dir(root: &Path) -> PathBuf {
    // 契约要求 `<目标根>/.agenthud-bak-<epoch_ms>/`；同一毫秒内多次覆盖时
    // 依次 +1 毫秒，仍保持同一命名形态，避免覆盖既有备份。
    let mut epoch = common::epoch_ms();
    loop {
        let dir = root.join(format!(".agenthud-bak-{epoch}"));
        if !dir.exists() {
            return dir;
        }
        epoch += 1;
    }
}

/// 未实测工具提示：该工具在能力矩阵中的 `verified_versions` 全为空时返回。
fn unverified_tool_warning(tool: &str) -> Option<String> {
    let entries = capabilities::entries_for(tool);
    if entries.is_empty() || entries.iter().any(|e| !e.verified_versions.is_empty()) {
        return None;
    }
    Some(
        "该工具能力按官方文档实现，未在本机实测目标工具版本，安装后请在目标工具中确认识别"
            .to_string(),
    )
}

/// 内置 Agent 同名覆盖提示（只提示，不阻断）。
fn builtin_override_warnings(
    tool: &str,
    kind: AssetKind,
    native_id: &str,
    files: &[(String, Vec<u8>)],
) -> Vec<String> {
    if kind != AssetKind::Agent {
        return Vec::new();
    }
    let id = native_id.to_ascii_lowercase();
    match tool {
        CODEX => {
            if ["default", "worker", "explorer"].contains(&id.as_str()) {
                vec![format!(
                    "Codex 内置 Agent `{native_id}` 将被同名自定义 Agent 覆盖（官方文档：custom agent takes precedence）"
                )]
            } else {
                Vec::new()
            }
        }
        CLAUDE => {
            if [
                "explore",
                "plan",
                "general-purpose",
                "claude",
                "statusline-setup",
                "claude-code-guide",
            ]
            .contains(&id.as_str())
            {
                vec![format!(
                    "Claude Code 内置 Agent `{native_id}` 将被同名自定义 Agent 覆盖"
                )]
            } else {
                Vec::new()
            }
        }
        KIMI => {
            if ["coder", "explore", "plan"].contains(&id.as_str()) {
                let overrides = files.iter().any(|(path, bytes)| {
                    path.to_ascii_lowercase().ends_with(".md")
                        && String::from_utf8_lossy(bytes)
                            .to_ascii_lowercase()
                            .contains("override: true")
                });
                let mut out = vec![format!(
                    "Kimi Code 内置 Agent `{native_id}`：仅当 frontmatter 声明 override: true 时才会覆盖"
                )];
                if !overrides {
                    out.push("当前文件未检测到 override: true，内置 Agent 不会被覆盖".to_string());
                }
                out
            } else {
                Vec::new()
            }
        }
        OPENCODE => {
            if [
                "build",
                "plan",
                "general",
                "explore",
                "compaction",
                "title",
                "summary",
            ]
            .contains(&id.as_str())
            {
                vec![format!(
                    "OpenCode 内置 Agent `{native_id}` 将被同 ID 自定义 Agent 覆盖"
                )]
            } else {
                Vec::new()
            }
        }
        _ => Vec::new(),
    }
}

// ============================================================
// 单测：安装/导出往返、冲突跳过、覆盖备份、能力与指纹拒绝。
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_assets::adapter::{ExportPlanBuilder, PlannedFile};

    fn temp_root(tag: &str) -> PathBuf {
        crate::agent_assets::tests_support::temp_root(tag)
    }

    fn env_at(home: &Path) -> LayoutEnv {
        LayoutEnv {
            home: home.to_path_buf(),
            codex_home: home.join(".codex"),
            kimi_home: home.join(".kimi-code"),
            opencode_config: home.join(".config").join("opencode"),
        }
    }

    /// 造一个最小 skill 目录并打包成信封。
    fn skill_bundle(native_id: &str) -> (PathBuf, AssetBundle) {
        let root = temp_root("install-skill-src");
        let dir = root.join(native_id);
        std::fs::create_dir_all(dir.join("scripts")).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            "---\r\nname: demo-skill\r\ndescription: 演示技能\r\n---\r\n正文\r\n".as_bytes(),
        )
        .unwrap();
        std::fs::write(dir.join("scripts/run.sh"), b"#!/bin/sh\necho hi\n").unwrap();
        let mut b = ExportPlanBuilder::new("claude", AssetKind::Skill, Scope::User, native_id)
            .native_format(SKILL_DIR)
            .entry_path("SKILL.md")
            .source_root(&dir);
        b = b.file(PlannedFile {
            path: "SKILL.md".into(),
            executable: false,
            binary: false,
        });
        b = b.file(PlannedFile {
            path: "scripts/run.sh".into(),
            executable: true,
            binary: false,
        });
        let plan = b.build();
        let envelope = bundle::pack_from_disk(&dir, &plan).unwrap();
        (root, envelope)
    }

    #[test]
    fn install_skill_writes_files_and_skips_by_default() {
        let (src, envelope) = skill_bundle("demo-skill");
        let home = temp_root("install-home");
        let layout =
            resolve_layout_with(&env_at(&home), CLAUDE, AssetKind::Skill, Scope::User, None)
                .unwrap();
        let first =
            install_parsed(&bundle::parse_bundle(&envelope).unwrap(), &layout, false).unwrap();
        assert_eq!(first.installed_paths.len(), 2);
        assert!(first.skipped.is_empty());
        assert!(
            layout.shared,
            "~/.claude/skills 是 OpenCode 兼容根，应提示共享"
        );
        assert!(first.warnings.iter().any(|w| w.contains("共享目录")));
        let skill_md = home.join(".claude/skills/demo-skill/SKILL.md");
        assert!(skill_md.is_file());
        assert!(home
            .join(".claude/skills/demo-skill/scripts/run.sh")
            .is_file());

        // 冲突默认跳过
        let second =
            install_parsed(&bundle::parse_bundle(&envelope).unwrap(), &layout, false).unwrap();
        assert!(second.installed_paths.is_empty());
        assert_eq!(second.skipped.len(), 1);
        let _ = std::fs::remove_dir_all(&src);
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn install_warns_when_tool_versions_are_not_verified() {
        let (src, envelope) = skill_bundle("demo-skill");
        let home = temp_root("install-unverified");
        let layout =
            resolve_layout_with(&env_at(&home), CLAUDE, AssetKind::Skill, Scope::User, None)
                .unwrap();
        let result =
            install_parsed(&bundle::parse_bundle(&envelope).unwrap(), &layout, false).unwrap();
        assert!(
            result
                .warnings
                .iter()
                .any(|w| w.contains("未在本机实测目标工具版本")),
            "verified_versions 为空时必须提示未实测: {:?}",
            result.warnings
        );
        // 已实测（verified_versions 非空）的工具不再追加该提示
        assert!(unverified_tool_warning("nope").is_none());
        let _ = std::fs::remove_dir_all(&src);
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn install_overwrite_backs_up_and_replaces() {
        let (src, envelope) = skill_bundle("demo-skill");
        let home = temp_root("install-overwrite");
        let layout =
            resolve_layout_with(&env_at(&home), CLAUDE, AssetKind::Skill, Scope::User, None)
                .unwrap();
        install_parsed(&bundle::parse_bundle(&envelope).unwrap(), &layout, false).unwrap();
        let result =
            install_parsed(&bundle::parse_bundle(&envelope).unwrap(), &layout, true).unwrap();
        let backup = result.backup_dir.expect("覆盖应产生备份目录");
        assert!(Path::new(&backup).is_dir());
        assert!(Path::new(&backup).join("demo-skill/SKILL.md").is_file());
        assert!(home.join(".claude/skills/demo-skill/SKILL.md").is_file());
        let _ = std::fs::remove_dir_all(&src);
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn install_agent_file_uses_tool_specific_target_name() {
        let src = temp_root("install-agent-src");
        std::fs::write(
            src.join("reviewer.toml"),
            "name = \"reviewer\"\ndescription = \"审查\"\ndeveloper_instructions = \"\"\"\n只读审查。\n\"\"\"\n",
        )
        .unwrap();
        let plan = ExportPlanBuilder::new(CODEX, AssetKind::Agent, Scope::User, "reviewer")
            .native_format(TOML_FMT)
            .entry_path("reviewer.toml")
            .source_root(&src)
            .file(PlannedFile {
                path: "reviewer.toml".into(),
                executable: false,
                binary: false,
            })
            .build();
        let envelope = bundle::pack_from_disk(&src, &plan).unwrap();

        let home = temp_root("install-agent-home");
        let layout =
            resolve_layout_with(&env_at(&home), CODEX, AssetKind::Agent, Scope::User, None)
                .unwrap();
        let result =
            install_parsed(&bundle::parse_bundle(&envelope).unwrap(), &layout, false).unwrap();
        assert_eq!(result.installed_paths.len(), 1);
        assert!(home.join(".codex/agents/reviewer.toml").is_file());
        let _ = std::fs::remove_dir_all(&src);
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn zcode_user_agent_install_goes_to_zcode_agents() {
        let src = temp_root("install-zcode-src");
        std::fs::write(
            src.join("demo-agent.md"),
            "---\nname: demo-agent\ndescription: 演示\n---\n正文",
        )
        .unwrap();
        let plan = ExportPlanBuilder::new(ZCODE, AssetKind::Agent, Scope::User, "demo-agent")
            .native_format(FM)
            .entry_path("demo-agent.md")
            .source_root(&src)
            .file(PlannedFile {
                path: "demo-agent.md".into(),
                executable: false,
                binary: false,
            })
            .build();
        let envelope = bundle::pack_from_disk(&src, &plan).unwrap();
        let home = temp_root("install-zcode-home");
        let layout =
            resolve_layout_with(&env_at(&home), ZCODE, AssetKind::Agent, Scope::User, None)
                .unwrap();
        install_parsed(&bundle::parse_bundle(&envelope).unwrap(), &layout, false).unwrap();
        assert!(home.join(".zcode/agents/demo-agent.md").is_file());
        let _ = std::fs::remove_dir_all(&src);
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn opencode_nested_agent_and_claude_entry_path_targets() {
        // OpenCode：native_id 自带嵌套路径 → 安装目标为 agents/team/reviewer.md。
        let src = temp_root("install-opencode-src");
        std::fs::write(src.join("reviewer.md"), "---\ndescription: 审查\n---\n正文").unwrap();
        let plan = ExportPlanBuilder::new(OPENCODE, AssetKind::Agent, Scope::User, "team/reviewer")
            .native_format(FM)
            .entry_path("reviewer.md")
            .source_root(&src)
            .file(PlannedFile {
                path: "reviewer.md".into(),
                executable: false,
                binary: false,
            })
            .build();
        let envelope = bundle::pack_from_disk(&src, &plan).unwrap();
        let home = temp_root("install-opencode-home");
        let layout = InstallLayout {
            root: home.join(".config/opencode/agents"),
            shared: false,
        };
        let result =
            install_parsed(&bundle::parse_bundle(&envelope).unwrap(), &layout, false).unwrap();
        assert_eq!(result.installed_paths.len(), 1);
        assert!(home
            .join(".config/opencode/agents/team/reviewer.md")
            .is_file());

        // Claude：身份来自 frontmatter.name，安装保留发现时的相对路径。
        let src2 = temp_root("install-claude-src");
        std::fs::create_dir_all(src2.join("review")).unwrap();
        std::fs::write(
            src2.join("review/security.md"),
            "---\nname: security\ndescription: 安全\n---\n正文",
        )
        .unwrap();
        let plan = ExportPlanBuilder::new(CLAUDE, AssetKind::Agent, Scope::User, "security")
            .native_format(FM)
            .entry_path("review/security.md")
            .source_root(&src2)
            .file(PlannedFile {
                path: "review/security.md".into(),
                executable: false,
                binary: false,
            })
            .build();
        let envelope = bundle::pack_from_disk(&src2, &plan).unwrap();
        let home2 = temp_root("install-claude-home");
        let layout = InstallLayout {
            root: home2.join(".claude/agents"),
            shared: false,
        };
        install_parsed(&bundle::parse_bundle(&envelope).unwrap(), &layout, false).unwrap();
        assert!(home2.join(".claude/agents/review/security.md").is_file());
        let _ = std::fs::remove_dir_all(&src);
        let _ = std::fs::remove_dir_all(&src2);
        let _ = std::fs::remove_dir_all(&home);
        let _ = std::fs::remove_dir_all(&home2);
    }

    #[test]
    fn install_bundle_rejects_jsonc_inline_and_legacy_yaml_formats() {
        // jsonc-inline：能力位为 installable=false，命令必须在落盘前拒绝。
        let plan = ExportPlanBuilder::new(OPENCODE, AssetKind::Agent, Scope::User, "reviewer")
            .native_format(JSONC_INLINE)
            .entry_path("reviewer.jsonc")
            .file(PlannedFile {
                path: "reviewer.jsonc".into(),
                executable: false,
                binary: false,
            })
            .inline_fragment(Some(
                "{\"agents\":{\"reviewer\":{\"description\":\"x\"}}}\n".into(),
            ))
            .build();
        let envelope = bundle::pack_plan(&plan).unwrap();
        let err = install_bundle(&envelope, Scope::User, None, false).unwrap_err();
        assert_eq!(err.code(), "CAPABILITY_UNSUPPORTED");

        // yaml-legacy：同样只允许归档与原样导出。
        let src = temp_root("install-legacy-src");
        std::fs::write(
            src.join("legacy.yaml"),
            "version: 1\nagent:\n  name: legacy\n  system_prompt_path: ./p.md\n",
        )
        .unwrap();
        let plan = ExportPlanBuilder::new(KIMI, AssetKind::Agent, Scope::User, "legacy")
            .native_format(YAML_LEGACY)
            .entry_path("legacy.yaml")
            .source_root(&src)
            .file(PlannedFile {
                path: "legacy.yaml".into(),
                executable: false,
                binary: false,
            })
            .build();
        let envelope = bundle::pack_from_disk(&src, &plan).unwrap();
        let err = install_bundle(&envelope, Scope::User, None, false).unwrap_err();
        assert_eq!(err.code(), "CAPABILITY_UNSUPPORTED");
        let _ = std::fs::remove_dir_all(&src);
    }

    #[test]
    fn export_jsonc_inline_writes_fragment_and_manual_merge_warning() {
        let plan = ExportPlanBuilder::new(OPENCODE, AssetKind::Agent, Scope::User, "reviewer")
            .native_format(JSONC_INLINE)
            .entry_path("reviewer.jsonc")
            .file(PlannedFile {
                path: "reviewer.jsonc".into(),
                executable: false,
                binary: false,
            })
            .inline_fragment(Some(
                "// fragment\n{\"agents\":{\"reviewer\":{\"description\":\"x\"}}}\n".into(),
            ))
            .build();
        let envelope = bundle::pack_plan(&plan).unwrap();
        let dest = temp_root("install-export-inline");
        let result = export_bundle(&envelope, &dest, false).unwrap();
        assert_eq!(result.written.len(), 1);
        assert!(result.warnings.iter().any(|w| w.contains("手动合并")));
        let content =
            std::fs::read_to_string(dest.join("opencode-agent-reviewer/reviewer.jsonc")).unwrap();
        assert!(content.contains("\"agents\""));
        assert!(content.contains("reviewer"));
        let _ = std::fs::remove_dir_all(&dest);
    }

    #[test]
    fn install_rejects_jsonc_inline_and_legacy_yaml() {
        let home = temp_root("install-reject");
        let env = env_at(&home);
        // 能力位检查依赖 manifest，构造最小信封走 install_bundle 会先看能力；
        // 这里直接验证格式判定与布局解析的口径一致。
        assert!(!format_installable(
            OPENCODE,
            AssetKind::Agent,
            JSONC_INLINE
        ));
        assert!(format_native_exportable(
            OPENCODE,
            AssetKind::Agent,
            JSONC_INLINE
        ));
        assert!(!format_installable(KIMI, AssetKind::Agent, YAML_LEGACY));
        assert!(format_native_exportable(
            KIMI,
            AssetKind::Agent,
            YAML_LEGACY
        ));
        // 项目级 agent 一律未开放安装
        assert!(
            resolve_layout_with(&env, CODEX, AssetKind::Agent, Scope::Project, Some(&home))
                .is_err()
        );
        assert!(
            resolve_layout_with(&env, CLAUDE, AssetKind::Agent, Scope::Project, Some(&home))
                .is_err()
        );
        assert!(
            resolve_layout_with(&env, KIMI, AssetKind::Agent, Scope::Project, Some(&home)).is_err()
        );
        assert!(resolve_layout_with(
            &env,
            OPENCODE,
            AssetKind::Agent,
            Scope::Project,
            Some(&home)
        )
        .is_err());
        // 缺 project_root
        assert!(resolve_layout_with(&env, CLAUDE, AssetKind::Skill, Scope::Project, None).is_err());
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn export_writes_native_dir_and_skips_then_backs_up() {
        let (src, envelope) = skill_bundle("demo-skill");
        let dest = temp_root("export-dest");
        let first = export_bundle(&envelope, &dest, false).unwrap();
        assert_eq!(first.written.len(), 2);
        assert!(dest.join("claude-skill-demo-skill/SKILL.md").is_file());
        assert!(dest
            .join("claude-skill-demo-skill/scripts/run.sh")
            .is_file());

        let second = export_bundle(&envelope, &dest, false).unwrap();
        assert!(second.written.is_empty());
        assert_eq!(second.skipped.len(), 1);

        let third = export_bundle(&envelope, &dest, true).unwrap();
        let backup = third.backup_dir.expect("覆盖应产生备份");
        assert!(Path::new(&backup)
            .join("claude-skill-demo-skill/SKILL.md")
            .is_file());
        let _ = std::fs::remove_dir_all(&src);
        let _ = std::fs::remove_dir_all(&dest);
    }

    #[test]
    fn tampered_bundle_is_rejected_before_install() {
        let (src, mut envelope) = skill_bundle("demo-skill");
        envelope.bundle_sha256 = "0".repeat(64);
        let home = temp_root("install-tamper");
        let err = install_bundle(&envelope, Scope::User, None, false).unwrap_err();
        assert_eq!(err.code(), "BUNDLE_INVALID");
        assert!(!home.join(".claude/skills/demo-skill").exists());
        let _ = std::fs::remove_dir_all(&src);
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn built_in_override_warning_for_codex() {
        let warnings = builtin_override_warnings(CODEX, AssetKind::Agent, "explorer", &[]);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("内置 Agent"));
        let warnings = builtin_override_warnings(CODEX, AssetKind::Agent, "custom", &[]);
        assert!(warnings.is_empty());
    }
}
