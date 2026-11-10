//! §9 路径安全（契约 `doc/agent-assets-contract.md`）。
//!
//! 对资源包内每条相对路径执行**与契约逐条对应**的检查；服务端（工作包 B）按
//! 同一规则复验。任何违规都拒绝归档/安装，绝不静默改名或跳过。
//!
//! 规则概览：
//! 1. 非空、≤512 字节、仅 `/` 分隔、无前导 `/`；
//! 2. 逐段：无空段/`.`/`..`、无 `:`（含盘符与 NTFS 备用数据流）、无控制字符、
//!    段不以点或空格结尾；
//! 3. Windows 设备名（`CON`/`PRN`/`AUX`/`NUL`/`COM1-9`/`LPT1-9`，含带扩展名形态）；
//! 4. 8.3 短名形态（基名以 `~<数字>` 结尾）；
//! 5. Windows 保留字符 `< > " | ? *`；
//! 6. 同包大小写折叠碰撞；
//! 7. 符号链接与 junction（收集/安装时用 `symlink_metadata` 检查文件与其全部
//!    父目录；Windows 上 std 对 junction 与 symlink 均报 `is_symlink()`）。

use std::collections::BTreeMap;
use std::path::Path;

use crate::agent_assets::AssetError;

/// 单条路径的最大字节数（契约 §9.1）。
pub const MAX_PATH_BYTES: usize = 512;

/// Windows 设备名集合（不含扩展名；比较用小写）。
const DEVICE_NAMES: [&str; 22] = [
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// Windows 保留字符。
const RESERVED_CHARS: [char; 6] = ['<', '>', '"', '|', '?', '*'];

/// 生成 `PATH_UNSAFE` 错误（details.path 指明违规条目，契约 §9）。
fn violation(path: &str, rule: &str, detail: impl Into<String>) -> AssetError {
    AssetError::path_unsafe(format!("路径安全检查失败（{rule}）: {}", detail.into()))
        .with_details(serde_json::json!({ "path": path, "rule": rule }))
}

/// 校验一条包内相对路径。通过返回 `Ok(())`。
pub fn validate_relative_path(path: &str) -> Result<(), AssetError> {
    if path.is_empty() {
        return Err(violation(path, "empty", "路径为空".to_string()));
    }
    if path.len() > MAX_PATH_BYTES {
        return Err(violation(
            path,
            "too_long",
            format!("路径超过 {MAX_PATH_BYTES} 字节"),
        ));
    }
    if path.starts_with('/') {
        return Err(violation(
            path,
            "absolute",
            "不允许绝对路径（前导 /）".to_string(),
        ));
    }
    if path.contains('\\') {
        return Err(violation(
            path,
            "separator",
            "只允许 / 作为分隔符（拒绝反斜杠）".to_string(),
        ));
    }
    if path.contains(':') {
        // 同时覆盖盘符（C:）与 NTFS 备用数据流（file:stream）
        return Err(violation(
            path,
            "colon",
            "路径不允许包含冒号（盘符/备用数据流）".to_string(),
        ));
    }
    for c in path.chars() {
        if (c as u32) < 0x20 || c as u32 == 0x7f {
            return Err(violation(
                path,
                "control_char",
                format!("包含控制字符 U+{:04X}", c as u32),
            ));
        }
        if RESERVED_CHARS.contains(&c) {
            return Err(violation(
                path,
                "reserved_char",
                format!("包含 Windows 保留字符 {c}"),
            ));
        }
    }
    for seg in path.split('/') {
        if seg.is_empty() {
            return Err(violation(
                path,
                "empty_segment",
                "存在空路径段（// 或尾随 /）".to_string(),
            ));
        }
        if seg == "." || seg == ".." {
            return Err(violation(
                path,
                "dot_segment",
                "不允许 . 或 .. 路径段".to_string(),
            ));
        }
        if seg.ends_with('.') || seg.ends_with(' ') {
            return Err(violation(
                path,
                "trailing_dot_space",
                "路径段以点或空格结尾".to_string(),
            ));
        }
        // 设备名：取去掉首个 `.` 之前的部分比较（CON.txt 与 CON 同拒）
        let base = seg.split('.').next().unwrap_or("");
        let lower = base.to_ascii_lowercase();
        if DEVICE_NAMES.contains(&lower.as_str()) {
            return Err(violation(
                path,
                "device_name",
                format!("Windows 设备名 {base}"),
            ));
        }
        // 8.3 短名：把路径段按 `.` 切分，任一切分片以 `~` + 一个或多个 ASCII
        // 数字结尾即拒绝（`^.*~[0-9]+$`；`NAME~1`、`name~1.md`、`a.b~1.md`、
        // `name~1.tar.gz`、`~1` 同拒；`wave~front.md` 放行）。与服务端 §9.4
        // 的定义逐条一致，且只认 ASCII 数字。
        for part in seg.split('.') {
            if let Some(pos) = part.rfind('~') {
                let suffix = &part[pos + 1..];
                if !suffix.is_empty() && suffix.bytes().all(|b| b.is_ascii_digit()) {
                    return Err(violation(path, "short_name", format!("8.3 短名形态 {seg}")));
                }
            }
        }
    }
    Ok(())
}

/// 同包大小写折叠碰撞检查：两条不同路径折叠（小写）后相同即拒绝。
/// 保护 Windows / 大小写不敏感文件系统上的相互覆盖。
pub fn check_case_collisions(paths: &[String]) -> Result<(), AssetError> {
    let mut folded: BTreeMap<String, &str> = BTreeMap::new();
    for p in paths {
        let key = p.to_lowercase();
        if let Some(prev) = folded.get(key.as_str()) {
            return Err(violation(
                p,
                "case_collision",
                format!("与 `{prev}` 大小写折叠后冲突"),
            ));
        }
        folded.insert(key, p.as_str());
    }
    Ok(())
}

/// 元数据是否指向符号链接/junction（Windows 上 std 对两者均返回 true）。
fn is_symlink_md(md: &std::fs::Metadata) -> bool {
    md.file_type().is_symlink()
}

/// 安全读取 `root/rel` 的文件内容：
/// - `rel` 必须先通过 [`validate_relative_path`]（无 `..` 等，join 无穿越面）；
/// - 文件本体与其**每一个父目录**都不得是符号链接/junction（先 `symlink_metadata`
///   逐级检查，再读取；避免「先打开后检查」的 TOCTOU 面）。
///
/// 外部引用（包外路径）不在本函数职责内——适配器只报告不递归（契约 §3.5）。
pub fn read_file_safely(root: &Path, rel: &str) -> Result<Vec<u8>, AssetError> {
    validate_relative_path(rel)?;
    let mut cur = root.to_path_buf();
    let segs: Vec<&str> = rel.split('/').collect();
    // 逐级检查父目录（最后一段是文件本体）
    for seg in &segs[..segs.len() - 1] {
        cur.push(seg);
        match std::fs::symlink_metadata(&cur) {
            Ok(md) => {
                if is_symlink_md(&md) {
                    return Err(violation(
                        rel,
                        "symlink",
                        format!("父目录 {seg} 是符号链接/junction"),
                    ));
                }
                if !md.is_dir() {
                    return Err(AssetError::not_found(format!(
                        "路径中段 {seg} 不是目录: {rel}"
                    )));
                }
            }
            Err(e) => return Err(AssetError::io(format!("读取 {rel} 元数据失败: {e}"))),
        }
    }
    cur.push(segs[segs.len() - 1]);
    let md = std::fs::symlink_metadata(&cur)
        .map_err(|e| AssetError::not_found(format!("文件不存在 {rel}: {e}")))?;
    if is_symlink_md(&md) {
        return Err(violation(rel, "symlink", "文件本体是符号链接/junction"));
    }
    if !md.is_file() {
        return Err(AssetError::io(format!("{rel} 不是普通文件")));
    }
    std::fs::read(&cur).map_err(|e| AssetError::io(format!("读取 {rel} 失败: {e}")))
}

// ============================================================
// 单测：契约 §9 全部拒绝用例 + 通过用例 + 大小写碰撞
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_normal_relative_paths() {
        for ok in [
            "a.md",
            "SKILL.md",
            "scripts/run.sh",
            "dir/sub/file.tar.gz",
            "ä-中文/文件.md",
        ] {
            assert!(validate_relative_path(ok).is_ok(), "应当通过: {ok}");
        }
    }

    #[test]
    fn rejects_traversal_and_absolute() {
        for bad in [
            "",
            "/abs.md",
            "../x.md",
            "a/../b.md",
            "a/./b.md",
            "a//b.md",
            "a/",
            "a\\b.md",
            "C:/x.md",
            "C:\\x.md",
            "file:stream",
        ] {
            let err = validate_relative_path(bad).unwrap_err();
            assert_eq!(err.code(), "PATH_UNSAFE", "应当拒绝: {bad:?}");
            assert!(err.details.is_some(), "details 应携带违规路径: {bad:?}");
        }
    }

    #[test]
    fn rejects_windows_device_names() {
        for bad in [
            "CON", "con", "CON.txt", "PRN.md", "aux/b.md", "NUL", "com1", "COM9.cfg", "lpt9",
            "LPT1.md", "dir/CON",
        ] {
            assert!(
                validate_relative_path(bad).unwrap_err().code() == "PATH_UNSAFE",
                "应当拒绝设备名: {bad}"
            );
        }
        // 形近但合法：console.md（基名不是设备名）、com10（超出 COM1-9）
        assert!(validate_relative_path("console.md").is_ok());
        assert!(validate_relative_path("com10.md").is_ok());
    }

    #[test]
    fn rejects_short_names_and_reserved_chars() {
        for bad in [
            "NAME~1",
            "name~1.md",
            "longna~2.txt",
            // 最后一个点之前 / 任一点分隔片含 `~数字` 的形态一律拒绝（契约 §9.4）
            "a.b~1.md",
            "name~1.tar.gz",
            "~1",
            "dir/NAME~1",
            "a<b.md",
            "a>b",
            "a\"b",
            "a|b",
            "a?b",
            "a*b.md",
            "a\u{01}b.md",
            "a\u{7f}.md",
            "a.",
            "a ",
            "dir/a.",
            "dir/a ",
        ] {
            assert!(
                validate_relative_path(bad).unwrap_err().code() == "PATH_UNSAFE",
                "应当拒绝: {bad:?}"
            );
        }
        // ~ 不在结尾数字形态则合法（如 wave~front.md、a~1b.md）
        assert!(validate_relative_path("wave~front.md").is_ok());
        assert!(validate_relative_path("a~1b.md").is_ok());
        // 非 ASCII 数字不算 8.3 短名（Rust 与服务端只认 [0-9]）
        assert!(validate_relative_path("name~١.md").is_ok());
    }

    #[test]
    fn case_collisions() {
        // 同目录大小写折叠冲突 → 拒绝
        let bad = vec!["A/x.md".to_string(), "a/X.md".to_string()];
        let err = check_case_collisions(&bad).unwrap_err();
        assert_eq!(err.code(), "PATH_UNSAFE");
        // 大小写不同目录 / 完全不同名 → 通过
        let ok = vec![
            "A/x.md".to_string(),
            "a/y.md".to_string(),
            "b/X.md".to_string(),
        ];
        assert!(check_case_collisions(&ok).is_ok());
        // 自身重复（同一路径两条）也算碰撞
        let dup = vec!["a.md".to_string(), "a.md".to_string()];
        assert!(check_case_collisions(&dup).is_err());
    }

    #[test]
    fn read_file_safely_rejects_unsafe_rel_before_fs() {
        // 不触碰文件系统就应先被路径规则拒绝
        let err = read_file_safely(Path::new("Z:/不存在也无所谓"), "../escape.md").unwrap_err();
        assert_eq!(err.code(), "PATH_UNSAFE");
        let err = read_file_safely(Path::new("Z:/x"), "CON").unwrap_err();
        assert_eq!(err.code(), "PATH_UNSAFE");
    }

    #[test]
    #[cfg(unix)]
    fn read_file_safely_rejects_symlink() {
        // 仅 Unix 可无特权建符号链接；Windows 的链接拒绝走同一 is_symlink 分支
        let root = super::super::tests_support::temp_root("safety-symlink");
        std::fs::create_dir_all(root.join("d")).unwrap();
        std::fs::write(root.join("d/real.md"), b"hi").unwrap();
        std::os::unix::fs::symlink("real.md", root.join("d/link.md")).unwrap();
        let err = read_file_safely(&root, "d/link.md").unwrap_err();
        assert_eq!(err.code(), "PATH_UNSAFE");
        assert!(err.message.contains("符号链接"));
        // 符号链接目录同样拒绝
        std::os::unix::fs::symlink("d", root.join("dlink")).unwrap();
        let err = read_file_safely(&root, "dlink/real.md").unwrap_err();
        assert_eq!(err.code(), "PATH_UNSAFE");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn read_file_safely_reads_normal_file() {
        let root = super::super::tests_support::temp_root("safety-read");
        std::fs::create_dir_all(root.join("d")).unwrap();
        std::fs::write(root.join("d/f.md"), b"hello").unwrap();
        assert_eq!(read_file_safely(&root, "d/f.md").unwrap(), b"hello");
        let _ = std::fs::remove_dir_all(&root);
    }
}
