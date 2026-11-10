//! 工具 ID → 适配器实例的分发注册表。
//!
//! 已登记：ZCode（A 参考实现）、Codex/Claude Code（C）、Kimi Code/OpenCode（D）。
//! DSH / MiniMax 不建适配器，直到 E 工作包完成发行版验证。

use std::sync::OnceLock;

use crate::agent_assets::adapter::Adapter;
use crate::agent_assets::adapters::claude::ClaudeAdapter;
use crate::agent_assets::adapters::codex::CodexAdapter;
use crate::agent_assets::adapters::kimi::KimiAdapter;
use crate::agent_assets::adapters::opencode::OpencodeAdapter;
use crate::agent_assets::adapters::zcode::ZcodeAdapter;
use crate::agent_assets::capabilities::{self, CapabilityEntry};
use crate::agent_assets::{AssetError, AssetKind, Scope};

/// 已注册的适配器（顺序无关；查找按 tool 精确匹配）。
fn registry() -> &'static Vec<Box<dyn Adapter>> {
    static REGISTRY: OnceLock<Vec<Box<dyn Adapter>>> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        vec![
            Box::new(ZcodeAdapter::new()),
            Box::new(CodexAdapter::new()),
            Box::new(ClaudeAdapter::new()),
            Box::new(KimiAdapter::new()),
            Box::new(OpencodeAdapter::new()),
        ]
    })
}

/// 按工具 ID 取适配器。
///
/// 错误口径：未知 ID → `ADAPTER_NOT_FOUND`；已知但适配器未合并（仅剩 DSH /
/// MiniMax 等 E 工作包）→ 同码但提示具体等待哪个工作包。前端据此显示
/// 「适配器开发中」。
pub fn adapter_for(tool: &str) -> Result<&'static dyn Adapter, AssetError> {
    if let Some(a) = registry().iter().find(|a| a.tool() == tool) {
        return Ok(a.as_ref());
    }
    if capabilities::tool_display_name(tool).is_some() {
        Err(AssetError::adapter_not_found(format!(
            "工具 {tool} 的适配器尚未合并（DSH/MiniMax 见 E 工作包）"
        )))
    } else {
        Err(AssetError::adapter_not_found(format!(
            "未知工具 ID: {tool}"
        )))
    }
}

/// 全量能力矩阵（含尚无适配器的工具与 pending 占位；契约 §5 单一事实来源）。
pub fn all_capabilities() -> Vec<CapabilityEntry> {
    capabilities::all_entries()
}

/// 便捷转发：命令层本地预检某组合是否可扫描/可归档（契约 §5.1）。
pub fn ensure_scannable(tool: &str, kind: AssetKind, scope: Scope) -> Result<(), AssetError> {
    if capabilities::has_capability(tool, kind, scope, capabilities::CapabilityBit::Scannable) {
        Ok(())
    } else {
        Err(AssetError::capability_unsupported(format!(
            "工具 {tool} 的 {} @ {} 组合未声明可扫描能力",
            kind.as_str(),
            scope.as_str()
        )))
    }
}

/// 同上：归档能力预检（plan_export / pack 入口共用）。
pub fn ensure_archivable(tool: &str, kind: AssetKind, scope: Scope) -> Result<(), AssetError> {
    if capabilities::has_capability(tool, kind, scope, capabilities::CapabilityBit::Archivable) {
        Ok(())
    } else {
        Err(AssetError::capability_unsupported(format!(
            "工具 {tool} 的 {} @ {} 组合未声明可归档能力",
            kind.as_str(),
            scope.as_str()
        )))
    }
}
