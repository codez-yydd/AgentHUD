//! 各工具的适配器实现（契约 §6/§10）。
//!
//! - `zcode`：A 工作包的**参考实现**；
//! - `codex` / `claude`：C 工作包（TOML / Markdown）；
//! - `kimi` / `opencode`：D 工作包（当前 Markdown、旧 YAML、JSONC 内联）；
//! - `common`：C/D 共用的只读扫描与元数据辅助；
//! - DSH / MiniMax 不建模块，直到 E 工作包完成发行版验证（能力矩阵里是
//!   `pending` 占位）。

pub mod claude;
pub mod codex;
pub mod common;
pub mod kimi;
pub mod opencode;
pub mod zcode;
