//! Agent 展示开关的 Rust 侧镜像。
//!
//! 主面板的设置开关存在 localStorage（见 src/agentVisibility.ts），而注入
//! 到 ZCode 的 usage.js 在另一个 webview 源里读不到这份偏好。主面板在启动
//! 与每次开关变化时经 `save_agent_visibility` 命令把同一份 map 原子写到
//! `~/.agenthud/agent-visibility.json`，额度线程（usage_feed）与 HUD 数据
//! 线程据此过滤「设置里开启的 agent」。
//!
//! 文件缺失/损坏时回退内置默认值（与前端 DEFAULT_VISIBILITY 对齐：首批
//! 5 个默认开启，新 provider 默认关闭）。这样升级后即使用户没打开过设置
//! 页，也不会把隐藏的 provider 突然显示出来。

use std::collections::BTreeMap;
use std::path::PathBuf;

/// 与 src/agentVisibility.ts 的 DEFAULT_VISIBILITY 保持一致。
const DEFAULT_VISIBLE: &[&str] = &["zai", "codex", "claude", "cursor", "kimi"];

fn path() -> Result<PathBuf, String> {
    Ok(crate::pricing::config_dir()?.join("agent-visibility.json"))
}

/// 读取开关 map；文件不存在/解析失败时返回空 map（调用方按默认值判定）。
pub fn load() -> BTreeMap<String, bool> {
    let Ok(path) = path() else {
        return BTreeMap::new();
    };
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<BTreeMap<String, bool>>(&text).ok())
        .unwrap_or_default()
}

/// 某个来源是否在设置里开启。未写入的键按默认名单判定。
pub fn is_visible(source: &str) -> bool {
    is_visible_in(&load(), source)
}

/// 基于已加载的开关 map 判定（一拍里要判多个来源时先用 `load()` 读一次，
/// 避免每个来源都读一遍文件——HUD 数据线程每拍要判全量 HUD_SOURCES）。
/// 未写入的键按默认名单判定。
pub fn is_visible_in(map: &BTreeMap<String, bool>, source: &str) -> bool {
    if let Some(value) = map.get(source) {
        return *value;
    }
    DEFAULT_VISIBLE.iter().any(|s| *s == source)
}

/// 写入开关 map（调用方已保证是完整快照，不是增量）。原子写与其它配置
/// 同口径：先写临时文件再改名。
pub fn save(map: &BTreeMap<String, bool>) -> Result<(), String> {
    let path = path()?;
    let json =
        serde_json::to_string_pretty(map).map_err(|e| format!("序列化 agent 开关失败: {e}"))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json).map_err(|e| format!("写入 agent 开关失败: {e}"))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("替换 agent 开关失败: {e}"))
}

/// 只保留可见来源（用于 HUD/注入额度数据组装）。

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 默认名单_首批可见新provider默认关闭() {
        // is_visible 读真实文件，测试环境不写文件时走默认；这里直接验证
        // 常量语义，避免依赖用户 home 目录的当前内容。
        assert!(DEFAULT_VISIBLE.contains(&"zai"));
        assert!(DEFAULT_VISIBLE.contains(&"codex"));
        assert!(!DEFAULT_VISIBLE.contains(&"opencodego"));
    }
}
