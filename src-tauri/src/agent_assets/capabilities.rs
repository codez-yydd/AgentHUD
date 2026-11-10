//! §5 能力位四档静态矩阵（契约 `doc/agent-assets-contract.md`）。
//!
//! 这是**单一事实来源**：前端按它显示动作、适配器按它拒绝越界请求。
//! C/D/E 工作包在各自 PR 中校正对应工具的行并补 `verified_versions`；
//! DSH / MiniMax 首期只是 `pending` 占位（全 false，前端不可操作），
//! 待发行版验证后由 E 打开。
//!
//! 目录/格式核对来源（2026-11-09，C/D 工作包）：
//!
//! - Codex：<https://learn.chatgpt.com/docs/agent-configuration/subagents>、
//!   <https://learn.chatgpt.com/docs/build-skills>、`CODEX_HOME`：
//!   <https://learn.chatgpt.com/docs/config-file/environment-variables>
//! - Claude Code：<https://code.claude.com/docs/en/sub-agents>、
//!   <https://code.claude.com/docs/en/skills>
//! - Kimi Code：<https://www.kimi.com/code/docs/en/kimi-code-cli/customization/agents.html>、
//!   <https://www.kimi.com/code/docs/en/kimi-code-cli/customization/skills.html>、
//!   旧版：<https://moonshotai.github.io/kimi-cli/en/customization/agents.html>
//! - OpenCode：<https://opencode.ai/docs/agents/>、<https://opencode.ai/docs/skills/>、
//!   <https://opencode.ai/docs/migrate-v1/>
//!
//! 本机未检测到上述四款 CLI（`codex`/`claude`/`kimi`/`opencode` 均不在 PATH），
//! 因此 `verified_versions` 留空并在 notes 标注「目录与格式按官方文档核对
//! （2026-11-09）；本机未检测到四款 CLI 版本，仅做真实目录只读发现，未做
//! 工具内加载验证」。

use serde::{Deserialize, Serialize};

use crate::agent_assets::{AssetKind, Scope};

// ===== 工具 ID 常量（单一口径，防止字符串漂移；风格同 local_agents.rs）=====

pub const ZCODE: &str = "zcode";
pub const CODEX: &str = "codex";
pub const CLAUDE: &str = "claude";
pub const KIMI: &str = "kimi";
pub const OPENCODE: &str = "opencode";
pub const DSH: &str = "dsh";
pub const MINIMAX: &str = "minimax";

/// 界面显示名（契约 §2.1）。
pub fn tool_display_name(tool: &str) -> Option<&'static str> {
    match tool {
        ZCODE => Some("ZCode"),
        CODEX => Some("Codex"),
        CLAUDE => Some("Claude Code"),
        KIMI => Some("Kimi Code"),
        OPENCODE => Some("OpenCode"),
        DSH => Some("DeepSeek Harness"),
        MINIMAX => Some("MiniMax Code"),
        _ => None,
    }
}

// ===== 能力条目（契约 §5.2）=====

/// 单个 工具 × 资源类型 × 作用域 的四档能力声明。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityEntry {
    pub tool: String,
    pub display_name: String,
    pub kind: AssetKind,
    pub scope: Scope,
    /// 可扫描：discover 能列出本机资源。
    pub scannable: bool,
    /// 可归档：plan_export + pack 能产出合法资源包并上传。
    pub archivable: bool,
    /// 可原生导出：能下载原生文件并还原出与原目录字节一致的文件。
    pub native_exportable: bool,
    /// 可安装：适配器能在本机安全写入并注册。
    pub installable: bool,
    pub native_format: String,
    /// `verified`（已按实际版本验证）/ `pending`（占位待验证，一律不可操作）。
    pub status: String,
    /// 实测过的适用版本（C/D/E 在 PR 中补齐；A 期留空）。
    #[serde(default)]
    pub verified_versions: Vec<String>,
    #[serde(default)]
    pub notes: String,
}

/// 四档全开条目。
fn full(
    tool: &str,
    kind: AssetKind,
    scope: Scope,
    native_format: &str,
    notes: &str,
) -> CapabilityEntry {
    CapabilityEntry {
        tool: tool.to_string(),
        display_name: tool_display_name(tool).unwrap_or(tool).to_string(),
        kind,
        scope,
        scannable: true,
        archivable: true,
        native_exportable: true,
        installable: true,
        native_format: native_format.to_string(),
        status: "verified".to_string(),
        verified_versions: Vec::new(),
        notes: notes.to_string(),
    }
}

/// 可扫描 + 可归档 + 可原生导出（安装待验证/首期不开放）。
fn scan_archive(
    tool: &str,
    kind: AssetKind,
    scope: Scope,
    native_format: &str,
    notes: &str,
) -> CapabilityEntry {
    CapabilityEntry {
        installable: false,
        ..full(tool, kind, scope, native_format, notes)
    }
}

/// `pending` 占位条目（DSH / MiniMax：待发行版验证，四档全 false）。
fn pending(
    tool: &str,
    kind: AssetKind,
    scope: Scope,
    native_format: &str,
    notes: &str,
) -> CapabilityEntry {
    CapabilityEntry {
        tool: tool.to_string(),
        display_name: tool_display_name(tool).unwrap_or(tool).to_string(),
        kind,
        scope,
        scannable: false,
        archivable: false,
        native_exportable: false,
        installable: false,
        native_format: native_format.to_string(),
        status: "pending".to_string(),
        verified_versions: Vec::new(),
        notes: notes.to_string(),
    }
}

const FM: &str = "markdown-frontmatter";
const SKILL: &str = "skill-dir";

/// C/D 工作包对四款工具的文档核对结论（本机均未安装 CLI）。
const DOC_CHECK_NOTE: &str =
    "目录与格式按官方文档核对（2026-11-09）；本机未检测到四款 CLI 版本，仅做真实目录只读发现，未做工具内加载验证";

/// 首期能力矩阵（契约 §5.2 表格的代码化；顺序即前端展示顺序）。
pub fn all_entries() -> Vec<CapabilityEntry> {
    let mut entries = vec![
        // ---- ZCode（A 参考实现）----
        full(
            ZCODE,
            AssetKind::Agent,
            Scope::User,
            FM,
            "安装 = 写入 ~/.zcode/agents/<name>.md，同卷暂存 + 原子发布；子代理首期仅用户级",
        ),
        full(
            ZCODE,
            AssetKind::Skill,
            Scope::User,
            SKILL,
            "双根 ~/.zcode/skills/ 与 ~/.agents/skills/，同名按 .zcode 优先（其余记入遮蔽警告）",
        ),
        scan_archive(
            ZCODE,
            AssetKind::Skill,
            Scope::Project,
            SKILL,
            "项目级 Skill 首期仅发现与归档；目录格式待实际版本样例验证后开放安装",
        ),
        // ---- Codex（C 工作包）----
        full(
            CODEX,
            AssetKind::Agent,
            Scope::User,
            "toml",
            "独立 TOML；内置 Agent 可被同名自定义覆盖，安装预览须显示",
        ),
        scan_archive(
            CODEX,
            AssetKind::Agent,
            Scope::Project,
            "toml",
            "首期用户级限定；项目级只扫描与归档",
        ),
        full(
            CODEX,
            AssetKind::Skill,
            Scope::User,
            SKILL,
            "保留附属资源及 agents/openai.yaml",
        ),
        full(CODEX, AssetKind::Skill, Scope::Project, SKILL, ""),
        // ---- Claude Code（C 工作包）----
        full(
            CLAUDE,
            AssetKind::Agent,
            Scope::User,
            FM,
            "frontmatter 的 name 决定身份；保留 tools/permissionMode/hooks 等字段并提示敏感配置",
        ),
        scan_archive(
            CLAUDE,
            AssetKind::Agent,
            Scope::Project,
            FM,
            "首期用户级限定",
        ),
        full(CLAUDE, AssetKind::Skill, Scope::User, SKILL, "保留子目录"),
        full(CLAUDE, AssetKind::Skill, Scope::Project, SKILL, ""),
        // ---- Kimi Code（D 工作包）----
        full(
            KIMI,
            AssetKind::Agent,
            Scope::User,
            FM,
            "当前版 Markdown；保留 override/whenToUse",
        ),
        scan_archive(
            KIMI,
            AssetKind::Agent,
            Scope::User,
            "yaml-legacy",
            "旧 kimi-cli 的 YAML agent：仅归档与原样导出，旧版恢复按需验证",
        ),
        scan_archive(KIMI, AssetKind::Agent, Scope::Project, FM, "首期用户级限定"),
        full(
            KIMI,
            AssetKind::Skill,
            Scope::User,
            SKILL,
            "$KIMI_CODE_HOME/skills/ 与共享 .agents/skills/",
        ),
        full(
            KIMI,
            AssetKind::Skill,
            Scope::Project,
            SKILL,
            ".kimi-code/skills/ 与共享目录",
        ),
        // ---- OpenCode（D 工作包）----
        full(
            OPENCODE,
            AssetKind::Agent,
            Scope::User,
            FM,
            "Markdown 优先；V2 用 agents、旧版常见 agent",
        ),
        scan_archive(
            OPENCODE,
            AssetKind::Agent,
            Scope::User,
            "jsonc-inline",
            "内联条目仅提取归档为独立 JSONC 片段；首期由用户手动合并回配置",
        ),
        scan_archive(
            OPENCODE,
            AssetKind::Agent,
            Scope::Project,
            FM,
            "首期用户级限定；V1/V2 权限差异与嵌套 ID 由 D 验证",
        ),
        full(
            OPENCODE,
            AssetKind::Skill,
            Scope::User,
            SKILL,
            "也读 Claude 与共享目录",
        ),
        full(OPENCODE, AssetKind::Skill, Scope::Project, SKILL, ""),
        // ---- DSH（E 工作包；占位）----
        pending(
            DSH,
            AssetKind::Agent,
            Scope::User,
            "dsh-preset",
            "待发行版验证：Agent Preset 的导入入口与 bundle/profile 安装流程",
        ),
        pending(
            DSH,
            AssetKind::Agent,
            Scope::Project,
            "dsh-preset",
            "待发行版验证",
        ),
        pending(
            DSH,
            AssetKind::Skill,
            Scope::User,
            SKILL,
            "待发行版验证：<dshHome>/skills 与 .dsh/skills；预览禁止求值 !!js",
        ),
        pending(DSH, AssetKind::Skill, Scope::Project, SKILL, "待发行版验证"),
        // ---- MiniMax Code（E 工作包；占位）----
        pending(
            MINIMAX,
            AssetKind::Agent,
            Scope::User,
            FM,
            "待发行版验证：原生 agent.md 与导入格式仅做已确认样例的原样归档",
        ),
        pending(
            MINIMAX,
            AssetKind::Agent,
            Scope::Project,
            FM,
            "待发行版验证",
        ),
        pending(
            MINIMAX,
            AssetKind::Skill,
            Scope::User,
            SKILL,
            "待发行版验证：~/.agents/skills/ 与项目 .minimax/skills/；Skill 可先行",
        ),
        pending(
            MINIMAX,
            AssetKind::Skill,
            Scope::Project,
            SKILL,
            "待发行版验证",
        ),
    ];

    // C/D：四款工具已按官方文档核对目录与格式；本机未安装 CLI，版本列表留空。
    for e in entries.iter_mut() {
        if matches!(e.tool.as_str(), CODEX | CLAUDE | KIMI | OPENCODE) {
            if e.notes.is_empty() {
                e.notes = DOC_CHECK_NOTE.to_string();
            } else {
                e.notes.push('；');
                e.notes.push_str(DOC_CHECK_NOTE);
            }
        }
    }
    entries
}

/// 查某工具的能力切片（适配器 `capabilities()` 实现转发到这里）。
pub fn entries_for(tool: &str) -> Vec<CapabilityEntry> {
    all_entries()
        .into_iter()
        .filter(|e| e.tool == tool)
        .collect()
}

/// 查单个组合是否声明了某能力位（命令入口的本地预检）。
pub fn has_capability(tool: &str, kind: AssetKind, scope: Scope, bit: CapabilityBit) -> bool {
    all_entries().into_iter().any(|e| {
        e.tool == tool
            && e.kind == kind
            && e.scope == scope
            && e.status == "verified"
            && bit.get(&e)
    })
}

/// 能力位选择器（命令层预检与安装/导出的格式判定共用）。
#[derive(Debug, Clone, Copy)]
pub enum CapabilityBit {
    Scannable,
    Archivable,
    NativeExportable,
    Installable,
}

impl CapabilityBit {
    fn get(self, e: &CapabilityEntry) -> bool {
        match self {
            CapabilityBit::Scannable => e.scannable,
            CapabilityBit::Archivable => e.archivable,
            CapabilityBit::NativeExportable => e.native_exportable,
            CapabilityBit::Installable => e.installable,
        }
    }
}
