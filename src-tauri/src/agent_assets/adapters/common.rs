//! C/D 工作包适配器的共用工具（只读扫描与元数据辅助）。
//!
//! 与参考实现 `zcode.rs` 保持同一形态：frontmatter 解析直接复用
//! [`super::zcode::parse_frontmatter`]（零依赖最小实现），本模块只补充
//! 目录遍历、敏感键判断、列表拆分与路径段校验等重复逻辑。
//!
//! 所有函数**只读**，不写任何文件；skill 目录在发现/归档阶段遇到符号链接/
//! junction 或超深目录会**硬拒绝**（`PATH_UNSAFE` / `VALIDATION_FAILED`），
//! 绝不静默跳过——被跳过的条目不会进入 `plan.files`，事后 `pack` 无从察觉，
//! 只会产出缺文件的“成功”包（契约 §9.7）。

use std::path::{Path, PathBuf};

use crate::agent_assets::adapter::{FileBrief, SensitiveHint};
use crate::agent_assets::adapters::zcode::{FrontValue, Frontmatter};
use crate::agent_assets::AssetError;

/// skill 目录遍历深度上限（防失控目录）。
pub(crate) const MAX_WALK_DEPTH: usize = 8;
/// skill 目录文件数软上限（pack 阶段还有 200 条硬限制）。
pub(crate) const MAX_WALK_ENTRIES: usize = 500;

/// 读取 UTF-8 文本；不可读或非 UTF-8 返回 None（inspect 降级用）。
pub(crate) fn read_text(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    String::from_utf8(bytes).ok()
}

/// 是否单段安全路径（用于把 native_id 拼进包内相对路径）。
pub(crate) fn safe_segment(seg: &str) -> bool {
    crate::agent_assets::safety::validate_relative_path(seg).is_ok()
        && !seg.contains('/')
        && !seg.starts_with('.')
}

/// 是否可作为多段相对路径（OpenCode 嵌套 ID 等；用于安装目标拼接）。
pub(crate) fn safe_relative_path(rel: &str) -> bool {
    crate::agent_assets::safety::validate_relative_path(rel).is_ok() && !rel.starts_with('.')
}

/// 敏感键判断（契约 §9 末段 + 各工具补充）：
/// 键名含 `token`/`secret`/`key`/`api`/`env`/`credential`（大小写不敏感）。
pub(crate) fn sensitive_key(key: &str) -> bool {
    let k = key.to_ascii_lowercase();
    ["token", "secret", "key", "api", "env", "credential"]
        .iter()
        .any(|w| k.contains(w))
}

/// frontmatter 敏感字段提示。
pub(crate) fn frontmatter_sensitive_hints(file: &str, fm: &Frontmatter) -> Vec<SensitiveHint> {
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

/// 追加一条敏感提示（去重由调用方按需处理；这里只做简单过滤）。
pub(crate) fn push_sensitive(out: &mut Vec<SensitiveHint>, file: &str, field: &str, reason: &str) {
    if out.iter().any(|h| h.file == file && h.field == field) {
        return;
    }
    out.push(SensitiveHint {
        file: file.to_string(),
        field: field.to_string(),
        reason: reason.to_string(),
    });
}

/// 把 frontmatter 的 `tools` 字段解析成列表：
/// 同时兼容 YAML 列表与 `tools: Read, Grep` 逗号字符串。
pub(crate) fn tools_from_frontmatter(fm: &Frontmatter) -> Vec<String> {
    let mut out = Vec::new();
    for (k, v) in &fm.entries {
        if k != "tools" && k != "allowed-tools" {
            continue;
        }
        match v {
            FrontValue::List(items) => {
                for item in items {
                    for part in item.split(',') {
                        let p = part.trim();
                        if !p.is_empty() {
                            out.push(p.to_string());
                        }
                    }
                }
            }
            FrontValue::Scalar(s) => {
                for part in s.split(',') {
                    let p = part.trim();
                    if !p.is_empty() {
                        out.push(p.to_string());
                    }
                }
            }
        }
    }
    out
}

/// 从 frontmatter 取标量字段（首个同名键）。
pub(crate) fn scalar_field(fm: &Frontmatter, key: &str) -> Option<String> {
    fm.entries
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
        .filter(|s| !s.trim().is_empty())
}

/// 遍历 skill 目录：只读、拒符号链接、限深限量；返回文件简要列表。
///
/// 目录内出现符号链接/junction（文件本体或中间目录）→ `PATH_UNSAFE`
/// （`details.path` 指明条目）；目录层级超过 [`MAX_WALK_DEPTH`] →
/// `VALIDATION_FAILED`。绝不跳过条目继续——跳过会导致导出计划缺文件而打包
/// 仍然“成功”。
///
/// 与 `zcode.rs` 的遍历规则一致：脚本按扩展名标 `executable`，`binary` 留待
/// pack 阶段内容嗅探（发现阶段避免全量读文件）。
pub(crate) fn walk_skill_dir(
    dir: &Path,
    warnings: &mut Vec<String>,
) -> Result<Vec<FileBrief>, AssetError> {
    let mut out = Vec::new();
    walk_skill_dir_rec(dir, "", 0, &mut out, warnings)?;
    Ok(out)
}

fn walk_skill_dir_rec(
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
        if md.file_type().is_symlink() {
            return Err(AssetError::path_unsafe(format!(
                "资源目录内存在符号链接/junction，拒绝归档: {rel}"
            ))
            .with_details(serde_json::json!({ "path": rel, "rule": "symlink" })));
        }
        if md.is_dir() {
            walk_skill_dir_rec(&entry.path(), &rel, depth + 1, out, warnings)?;
        } else if md.is_file() {
            if looks_like_credential_file(&rel) {
                warnings.push(format!(
                    "疑似凭据/私钥文件 {rel}：按契约只做预览提示、不自动排除，上传前请确认"
                ));
            }
            out.push(FileBrief {
                path: rel.clone(),
                size: md.len(),
                executable: has_executable_ext(&rel),
                binary: false,
            });
        }
    }
    Ok(())
}

/// 文件名是否疑似凭据/私钥（契约 §9 末段；只提示不拒绝）。
pub(crate) fn looks_like_credential_file(rel: &str) -> bool {
    let base = rel.rsplit('/').next().unwrap_or(rel).to_ascii_lowercase();
    base == ".env"
        || base.starts_with(".env.")
        || base == "credentials"
        || base == "credentials.json"
        || base == "id_rsa"
        || base == "id_ed25519"
        || base.ends_with(".pem")
        || base.ends_with(".key")
}

/// 按扩展名判断可执行（安装/打包阶段还会叠加 Unix 执行位与内容嗅探）。
pub(crate) fn has_executable_ext(rel: &str) -> bool {
    let lower = rel.to_ascii_lowercase();
    [
        ".sh", ".py", ".pl", ".rb", ".js", ".mjs", ".cjs", ".ps1", ".bat", ".cmd", ".exe",
    ]
    .iter()
    .any(|ext| lower.ends_with(ext))
}

/// 递归收集指定扩展名的普通文件（只读；跳过隐藏项与符号链接）。
///
/// 返回 `(相对路径, 绝对路径)`，按相对路径字节序排序，保证 discover 结果稳定。
pub(crate) fn walk_files_with_ext(
    root: &Path,
    ext: &str,
    warnings: &mut Vec<String>,
) -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    walk_files_rec(root, "", 0, ext, &mut out, warnings);
    out.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    out
}

fn walk_files_rec(
    dir: &Path,
    prefix: &str,
    depth: usize,
    ext: &str,
    out: &mut Vec<(String, PathBuf)>,
    warnings: &mut Vec<String>,
) {
    if depth > MAX_WALK_DEPTH {
        warnings.push(format!(
            "目录层级超过 {MAX_WALK_DEPTH}，已停止深入: {prefix}"
        ));
        return;
    }
    let rd = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(_) => return, // 目录不存在 = 没有资源
    };
    let mut items: Vec<_> = rd.filter_map(|e| e.ok()).collect();
    items.sort_by_key(|e| e.file_name());
    for entry in items {
        if out.len() >= MAX_WALK_ENTRIES {
            warnings.push(format!("文件数超过软上限 {MAX_WALK_ENTRIES}，已停止收集"));
            return;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let rel = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}/{name}")
        };
        let md = match entry.metadata() {
            Ok(md) => md,
            Err(_) => continue,
        };
        if md.file_type().is_symlink() {
            warnings.push(format!("跳过符号链接/junction: {rel}"));
            continue;
        }
        if md.is_dir() {
            walk_files_rec(&entry.path(), &rel, depth + 1, ext, out, warnings);
        } else if md.is_file() && rel.to_ascii_lowercase().ends_with(ext) {
            out.push((rel, entry.path()));
        }
    }
}

/// 用户真实 HOME（各适配器 `new()` 使用；测试一律注入临时目录）。
pub(crate) fn home_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_default()
}

/// 读取环境变量路径（空串视为未设置）。
pub(crate) fn env_path(var: &str) -> Option<PathBuf> {
    std::env::var_os(var)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// epoch 毫秒（备份目录命名与测试辅助）。
pub(crate) fn epoch_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// 把 native_id 转成可用作单层目录名的 slug（导出目录命名用）。
pub(crate) fn slugify(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last_dash = false;
    for ch in s.chars() {
        let ok = ch.is_alphanumeric() || ch == '_' || ch == '.' || ch == '-';
        if ok {
            out.push(ch);
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    let trimmed = out.trim_matches('-').trim_matches('.').to_string();
    if trimmed.is_empty() {
        "asset".to_string()
    } else {
        trimmed
    }
}

/// 计算文件的 SHA-256 短哈希（内联片段文件名防碰撞用）。
pub(crate) fn short_hash(input: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(input.as_bytes());
    let out = h.finalize();
    let mut s = String::with_capacity(8);
    for b in out.iter().take(4) {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

// ===== 手选原生文件导入（`agent_assets_import_files`）=====

/// 手选路径的基础安全检查（契约 §9.7）：绝对路径、存在、本体不是符号链接。
///
/// 父目录链的符号链接由打包阶段 `read_file_safely` 对资源根内部逐级检查；
/// 这里只校验用户实际选中的本体，避免把合法的家目录软链接误判为不安全。
pub(crate) fn validate_import_source(path: &Path) -> Result<std::fs::Metadata, AssetError> {
    if path.as_os_str().is_empty() {
        return Err(AssetError::validation("手选路径不能为空"));
    }
    if !path.is_absolute() {
        return Err(AssetError::validation(format!(
            "手选路径必须是绝对路径: {}",
            path.display()
        )));
    }
    let md = std::fs::symlink_metadata(path).map_err(|e| {
        AssetError::not_found(format!("路径不存在或不可访问: {} ({e})", path.display()))
    })?;
    if md.file_type().is_symlink() {
        return Err(AssetError::path_unsafe(format!(
            "手选路径是符号链接/junction，拒绝导入: {}",
            path.display()
        )));
    }
    Ok(md)
}

/// 校验 skill 目录：必须是目录且直接包含普通文件 `SKILL.md`。
pub(crate) fn ensure_skill_dir(dir: &Path) -> Result<(), AssetError> {
    let md = validate_import_source(dir)?;
    if !md.is_dir() {
        return Err(AssetError::validation(format!(
            "skill 需选择包含 SKILL.md 的目录: {}",
            dir.display()
        )));
    }
    let skill_md = dir.join("SKILL.md");
    let smd = std::fs::symlink_metadata(&skill_md).map_err(|e| {
        AssetError::validation(format!(
            "目录缺少入口文件 SKILL.md: {} ({e})",
            dir.display()
        ))
    })?;
    if smd.file_type().is_symlink() {
        return Err(AssetError::path_unsafe(format!(
            "SKILL.md 是符号链接/junction，拒绝导入: {}",
            skill_md.display()
        )));
    }
    if !smd.is_file() {
        return Err(AssetError::validation(format!(
            "SKILL.md 不是普通文件: {}",
            skill_md.display()
        )));
    }
    Ok(())
}

/// 手选 skill 根：接受目录，或用户直接选中的 `SKILL.md` 文件（取其父目录）。
pub(crate) fn import_skill_root(
    path: &Path,
    md: &std::fs::Metadata,
) -> Result<PathBuf, AssetError> {
    if md.is_dir() {
        ensure_skill_dir(path)?;
        return Ok(path.to_path_buf());
    }
    let is_skill_md = path
        .file_name()
        .map(|n| n.to_string_lossy().eq_ignore_ascii_case("SKILL.md"))
        .unwrap_or(false);
    if is_skill_md {
        let dir = path
            .parent()
            .ok_or_else(|| AssetError::validation("SKILL.md 缺少父目录"))?;
        ensure_skill_dir(dir)?;
        return Ok(dir.to_path_buf());
    }
    Err(AssetError::validation(format!(
        "skill 需选择包含 SKILL.md 的目录（或直接选择 SKILL.md）: {}",
        path.display()
    )))
}

/// 手选 agent 文件：必须是普通文件且扩展名在允许列表内。
pub(crate) fn ensure_agent_file(
    path: &Path,
    md: &std::fs::Metadata,
    allowed_exts: &[&str],
) -> Result<(), AssetError> {
    if !md.is_file() {
        return Err(AssetError::validation(format!(
            "agent 需选择单个原生文件，不能是目录: {}",
            path.display()
        )));
    }
    let lower = path
        .file_name()
        .map(|n| n.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    if !allowed_exts.iter().any(|ext| lower.ends_with(ext)) {
        return Err(AssetError::validation(format!(
            "agent 文件扩展名不匹配（允许 {}）: {}",
            allowed_exts.join(" / "),
            path.display()
        )));
    }
    // 文件名必须本身可作为包内相对路径（拒绝冒号/控制字符/设备名等）。
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    if crate::agent_assets::safety::validate_relative_path(&file_name).is_err() {
        return Err(AssetError::path_unsafe(format!(
            "agent 文件名不是安全的包内路径: {file_name}"
        )));
    }
    Ok(())
}

// ============================================================
// 单测：列表拆分、敏感键、slug 与目录遍历边界。
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sensitive_keys_match_contract_words() {
        for k in [
            "api_key",
            "API-KEY",
            "token",
            "secret",
            "credential",
            "env",
            "environment",
        ] {
            assert!(sensitive_key(k), "应判敏感: {k}");
        }
        for k in ["name", "description", "model", "tools", "whenToUse"] {
            assert!(!sensitive_key(k), "不应判敏感: {k}");
        }
    }

    #[test]
    fn tools_field_accepts_list_and_comma_string() {
        let fm = super::super::zcode::parse_frontmatter(
            "---\nname: x\ntools:\n  - Read\n  - Grep\n---\nbody",
        )
        .unwrap();
        assert_eq!(tools_from_frontmatter(&fm), vec!["Read", "Grep"]);
        let fm = super::super::zcode::parse_frontmatter(
            "---\nname: x\ntools: Read, Grep, Glob\n---\nbody",
        )
        .unwrap();
        assert_eq!(tools_from_frontmatter(&fm), vec!["Read", "Grep", "Glob"]);
    }

    #[test]
    fn slugify_and_short_hash_are_safe() {
        assert_eq!(slugify("team/reviewer"), "team-reviewer");
        assert_eq!(slugify(".."), "asset");
        assert_eq!(slugify("中文 名"), "中文-名");
        assert_eq!(short_hash("abc").len(), 8);
    }

    #[test]
    fn walk_skill_dir_marks_scripts_and_reports_credentials() {
        let root = crate::agent_assets::tests_support::temp_root("common-walk");
        std::fs::create_dir_all(root.join("scripts")).unwrap();
        std::fs::write(root.join("SKILL.md"), b"x").unwrap();
        std::fs::write(root.join("scripts/run.sh"), b"#!/bin/sh\n").unwrap();
        std::fs::write(root.join(".env"), b"SECRET=1\n").unwrap();
        let mut warnings = Vec::new();
        let files = walk_skill_dir(&root, &mut warnings).unwrap();
        assert!(files
            .iter()
            .any(|f| f.path == "scripts/run.sh" && f.executable));
        assert!(files.iter().any(|f| f.path == "SKILL.md" && !f.executable));
        assert!(warnings.iter().any(|w| w.contains("疑似凭据/私钥文件")));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn walk_skill_dir_rejects_too_deep_directories() {
        let root = crate::agent_assets::tests_support::temp_root("common-depth");
        let mut dir = root.clone();
        for index in 0..(MAX_WALK_DEPTH + 1) {
            dir = dir.join(format!("d{index}"));
        }
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("too-deep.md"), b"x").unwrap();
        let mut warnings = Vec::new();
        let err = walk_skill_dir(&root, &mut warnings).unwrap_err();
        assert_eq!(err.code(), "VALIDATION_FAILED");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    #[cfg(unix)]
    fn walk_skill_dir_rejects_symlinked_entries() {
        let root = crate::agent_assets::tests_support::temp_root("common-symlink");
        std::fs::create_dir_all(root.join("real")).unwrap();
        std::fs::write(root.join("real/target.md"), b"x").unwrap();
        std::os::unix::fs::symlink("real/target.md", root.join("link.md")).unwrap();
        let mut warnings = Vec::new();
        let err = walk_skill_dir(&root, &mut warnings).unwrap_err();
        assert_eq!(err.code(), "PATH_UNSAFE");
        assert_eq!(err.details.as_ref().unwrap()["path"], "link.md");

        // 中间目录是符号链接同样拒绝
        std::fs::remove_file(root.join("link.md")).unwrap();
        std::os::unix::fs::symlink("real", root.join("dirlink")).unwrap();
        let mut warnings = Vec::new();
        let err = walk_skill_dir(&root, &mut warnings).unwrap_err();
        assert_eq!(err.code(), "PATH_UNSAFE");
        assert_eq!(err.details.as_ref().unwrap()["path"], "dirlink");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn credential_file_names_are_recognized() {
        for p in [
            ".env",
            "config/.env.local",
            "credentials",
            "credentials.json",
            "id_rsa",
            "keys/server.pem",
            "client.key",
        ] {
            assert!(looks_like_credential_file(p), "应识别: {p}");
        }
        for p in ["SKILL.md", "scripts/run.sh", "README.md", "keymap.json"] {
            assert!(!looks_like_credential_file(p), "不应识别: {p}");
        }
    }
}
