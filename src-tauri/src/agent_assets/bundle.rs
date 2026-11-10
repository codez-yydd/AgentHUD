//! §3 资源包（bundle）：打包、解析与指纹复验（契约 `doc/agent-assets-contract.md`）。
//!
//! 打包 = 磁盘收集 → 路径安全检查（safety）→ SHA-256 → 清单 + 内容信封；
//! 解析 = 结构校验 + 逐文件哈希复验 + 指纹复验，用于下载/安装前的强制校验
//! （契约 §3.3 往返不变量）。原文件字节不经任何重新序列化：换行、未知字段、
//! 二进制附件逐字节保留。

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::Path;

use crate::agent_assets::adapter::ExportPlan;
use crate::agent_assets::{AssetError, AssetKind, Scope};

/// 当前清单 schema 版本（契约 §3.2）。
pub const SCHEMA_VERSION: u32 = 1;

// ---- 客户端预检用的容量上限（与服务端建议默认值一致，契约 §8.7）----
// 服务端真实限制以 GET /agent-assets/capabilities 返回为准；这里只做本地
// 快速失败，避免打了一个注定超限的大包才在服务端被 413。

/// 单文件上限：8 MiB。
pub const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;
/// 单包总内容上限：16 MiB。
pub const MAX_BUNDLE_BYTES: u64 = 16 * 1024 * 1024;
/// 单包条目数上限：200。
pub const MAX_ENTRIES: usize = 200;

// ===== wire 结构（与契约 §3.1/§3.2 字段一一对应）=====

/// 清单条目（契约 §3.2 ManifestFile）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ManifestFile {
    /// 包内相对路径（以资源自身根为基准）。
    pub path: String,
    /// 原始字节数。
    pub size: u64,
    /// 该文件内容的 SHA-256（小写 hex）。
    pub sha256: String,
    /// 可执行标记（Unix 权限位或 Windows 可执行后缀）。
    pub executable: bool,
    /// 二进制标志（前 8 KiB 含 0x00 字节或适配器声明）。
    pub binary: bool,
}

/// 资源清单（契约 §3.2 AssetManifest）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssetManifest {
    pub schema_version: u32,
    pub tool: String,
    pub kind: AssetKind,
    pub native_format: String,
    pub native_id: String,
    pub display_name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub origin_scope: Scope,
    #[serde(default)]
    pub target_versions: Vec<String>,
    /// 入口文件相对路径，必须在 files 内。
    pub entry_path: String,
    /// 按 path 升序排列。
    pub files: Vec<ManifestFile>,
}

/// 逐文件内容（字节数组的标准 base64 编码，含 padding）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileContent {
    pub path: String,
    pub content_base64: String,
}

/// 传输信封（契约 §3.1 AssetBundle）。
///
/// `raw_manifest` 保存反序列化时收到的原始 manifest JSON 子树：服务端允许
/// 清单携带未知字段（契约 §3.2），结构体解析会丢弃这些字段、并为缺省可选
/// 字段补默认值；`manifest_sha256` 必须对**收到的原文**复验，否则第三方包
/// 会被误判为指纹不符。
///
/// 序列化时若存在原文则原样输出（`fetch_bundle → 前端 → install` 往返也
/// 不能丢未知字段）；`pack` 产出的信封为 `None`，按结构体规范序列化计算
/// 指纹——两条路径在自有包上逐位一致。
#[derive(Debug, Clone)]
pub struct AssetBundle {
    pub manifest: AssetManifest,
    pub manifest_sha256: String,
    pub bundle_sha256: String,
    /// 与 manifest.files 一一对应、同序（按 path 升序）。
    pub contents: Vec<FileContent>,
    raw_manifest: Option<serde_json::Value>,
}

impl Serialize for AssetBundle {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("AssetBundle", 4)?;
        match &self.raw_manifest {
            Some(raw) => state.serialize_field("manifest", raw)?,
            None => state.serialize_field("manifest", &self.manifest)?,
        }
        state.serialize_field("manifest_sha256", &self.manifest_sha256)?;
        state.serialize_field("bundle_sha256", &self.bundle_sha256)?;
        state.serialize_field("contents", &self.contents)?;
        state.end()
    }
}

impl<'de> Deserialize<'de> for AssetBundle {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct WireEnvelope {
            manifest: serde_json::Value,
            manifest_sha256: String,
            bundle_sha256: String,
            contents: Vec<FileContent>,
        }
        let wire = WireEnvelope::deserialize(deserializer)?;
        let manifest: AssetManifest =
            serde_json::from_value(wire.manifest.clone()).map_err(serde::de::Error::custom)?;
        Ok(AssetBundle {
            manifest,
            manifest_sha256: wire.manifest_sha256,
            bundle_sha256: wire.bundle_sha256,
            contents: wire.contents,
            raw_manifest: Some(wire.manifest),
        })
    }
}

/// 解析复验后的资源包：内容已解码为原始字节。
///
/// 安装器与下载复验的主要消费方（安装前强制指纹复验，契约 §3.3）。
#[derive(Debug, Clone)]
pub struct ParsedBundle {
    pub manifest: AssetManifest,
    /// (path, 原始字节)，与 manifest.files 同序。
    pub files: Vec<(String, Vec<u8>)>,
}

// ===== 指纹（契约 §3.3）=====

fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    let out = h.finalize();
    let mut s = String::with_capacity(64);
    for b in out {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// 清单规范 JSON：紧凑分隔符、键按字典序（serde_json 默认 BTreeMap）、files 需
/// 调用前已按 path 升序。与契约 §3.3 给出的 Python 参考实现逐字节一致。
fn canonical_manifest_json(manifest: &AssetManifest) -> String {
    let value = serde_json::to_value(manifest).expect("清单序列化不会失败");
    serde_json::to_string(&value).expect("规范 JSON 序列化不会失败")
}

/// `manifest_sha256`：清单对象规范 JSON 的 SHA-256（小写 hex）。
pub fn manifest_digest(manifest: &AssetManifest) -> String {
    sha256_hex(canonical_manifest_json(manifest).as_bytes())
}

/// 对**收到的原始 manifest JSON**（含未知字段、保留缺省形态）计算规范 JSON
/// 指纹；用于解析侧复验（契约 §3.3）。
fn manifest_digest_value(value: &serde_json::Value) -> Result<String, AssetError> {
    // serde_json 默认 Map 为 BTreeMap（未启用 preserve_order），to_string 输出
    // 紧凑分隔符与按 UTF-8 字节序的键序，与 Python 参考实现逐位一致。
    let canonical = serde_json::to_string(value)
        .map_err(|e| AssetError::bundle_invalid(format!("清单规范 JSON 序列化失败: {e}")))?;
    Ok(sha256_hex(canonical.as_bytes()))
}

/// `bundle_sha256`：按 path 升序，逐条喂 `path` UTF-8 字节 + `0x00` + 原始内容。
pub fn bundle_digest(files: &[(String, Vec<u8>)]) -> String {
    let mut sorted: Vec<&(String, Vec<u8>)> = files.iter().collect();
    sorted.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    let mut h = Sha256::new();
    for (path, content) in sorted {
        h.update(path.as_bytes());
        h.update([0u8]);
        h.update(content);
    }
    let out = h.finalize();
    let mut s = String::with_capacity(64);
    for b in out {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

// ===== 打包（磁盘 → 信封）=====

/// 内容嗅探是否二进制：前 8 KiB 含 0x00 字节。
fn sniff_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(8 * 1024).any(|&b| b == 0)
}

/// 文件是否可执行：Windows 按后缀；Unix 任意 owner/group/other 执行位。
fn detect_executable(rel: &str, md: &std::fs::Metadata) -> bool {
    let lower = rel.to_ascii_lowercase();
    if lower.ends_with(".sh")
        || lower.ends_with(".py")
        || lower.ends_with(".pl")
        || lower.ends_with(".rb")
        || lower.ends_with(".js")
        || lower.ends_with(".mjs")
        || lower.ends_with(".cjs")
        || lower.ends_with(".ps1")
        || lower.ends_with(".bat")
        || lower.ends_with(".cmd")
        || lower.ends_with(".exe")
    {
        return true;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if md.permissions().mode() & 0o111 != 0 {
            return true;
        }
    }
    #[cfg(not(unix))]
    let _ = md;
    false
}

/// 按导出计划打包：对 plan.files 逐条安全读取、哈希、限流，产出信封。
///
/// plan.entry_path 必须在文件清单内；单文件/单包/条目数超限立即报错
/// （契约 §8.7 客户端预检）；路径违规（含大小写碰撞）由 safety 拒绝。
pub fn pack_from_disk(root: &Path, plan: &ExportPlan) -> Result<AssetBundle, AssetError> {
    if plan.files.is_empty() {
        return Err(AssetError::validation("导出计划没有文件条目"));
    }
    if plan.files.len() > MAX_ENTRIES {
        return Err(AssetError::too_many_entries(format!(
            "文件条目 {} 超过上限 {MAX_ENTRIES}",
            plan.files.len()
        )));
    }
    let paths: Vec<String> = plan.files.iter().map(|f| f.path.clone()).collect();
    for p in &paths {
        crate::agent_assets::safety::validate_relative_path(p)?;
    }
    crate::agent_assets::safety::check_case_collisions(&paths)?;
    if !paths.contains(&plan.entry_path) {
        return Err(AssetError::validation(format!(
            "入口文件 {} 不在文件清单内",
            plan.entry_path
        )));
    }

    let mut collected: Vec<(String, Vec<u8>, bool, bool)> = Vec::with_capacity(paths.len());
    for pf in &plan.files {
        let bytes = crate::agent_assets::safety::read_file_safely(root, &pf.path)?;
        // 可执行：适配器声明或文件系统实检取或；二进制：声明或内容嗅探
        let exec = pf.executable || {
            let md = std::fs::metadata(root.join(&pf.path))
                .map_err(|e| AssetError::io(format!("读取 {} 元数据失败: {e}", pf.path)))?;
            detect_executable(&pf.path, &md)
        };
        let binary = pf.binary || sniff_binary(&bytes);
        collected.push((pf.path.clone(), bytes, exec, binary));
    }
    assemble_bundle(plan, collected)
}

/// 按导出计划打包（统一入口）：普通资源从磁盘收集；`jsonc-inline` 使用
/// 适配器在 `plan_export` 中提取并序列化好的内存片段（契约 §3.4 例外）。
///
/// `inline_fragment` 是进程内字段（`#[serde(skip)]`），不改变命令返回的
/// `ExportPlan` wire 形状；打包仍走同一套路径安全检查与指纹计算。
pub fn pack_plan(plan: &ExportPlan) -> Result<AssetBundle, AssetError> {
    if let Some(fragment) = &plan.inline_fragment {
        let path = plan.entry_path.clone();
        return assemble_bundle(
            plan,
            vec![(path, fragment.as_bytes().to_vec(), false, false)],
        );
    }
    pack_from_disk(Path::new(&plan.source_root), plan)
}

/// 把已收集的内容组装成信封：路径安全、大小限制、排序、哈希与 base64。
fn assemble_bundle(
    plan: &ExportPlan,
    mut collected: Vec<(String, Vec<u8>, bool, bool)>,
) -> Result<AssetBundle, AssetError> {
    if collected.is_empty() {
        return Err(AssetError::validation("导出计划没有文件条目"));
    }
    if collected.len() > MAX_ENTRIES {
        return Err(AssetError::too_many_entries(format!(
            "文件条目 {} 超过上限 {MAX_ENTRIES}",
            collected.len()
        )));
    }
    let paths: Vec<String> = collected.iter().map(|(p, _, _, _)| p.clone()).collect();
    for p in &paths {
        crate::agent_assets::safety::validate_relative_path(p)?;
    }
    crate::agent_assets::safety::check_case_collisions(&paths)?;
    if !paths.iter().any(|p| p == &plan.entry_path) {
        return Err(AssetError::validation(format!(
            "入口文件 {} 不在文件清单内",
            plan.entry_path
        )));
    }
    let mut total: u64 = 0;
    for (path, bytes, _, _) in &collected {
        if bytes.len() as u64 > MAX_FILE_BYTES {
            return Err(AssetError::payload_too_large(format!(
                "文件 {path} 为 {} 字节，超过单文件上限 {MAX_FILE_BYTES}",
                bytes.len()
            )));
        }
        total += bytes.len() as u64;
        if total > MAX_BUNDLE_BYTES {
            return Err(AssetError::payload_too_large(format!(
                "资源包总大小超过上限 {MAX_BUNDLE_BYTES} 字节"
            )));
        }
    }

    // 清单条目按 path 的 UTF-8 字节序升序（契约 §3.2/§3.3）
    collected.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));

    let files: Vec<ManifestFile> = collected
        .iter()
        .map(|(path, bytes, exec, binary)| ManifestFile {
            path: path.clone(),
            size: bytes.len() as u64,
            sha256: sha256_hex(bytes),
            executable: *exec,
            binary: *binary,
        })
        .collect();
    let manifest = AssetManifest {
        schema_version: SCHEMA_VERSION,
        tool: plan.tool.clone(),
        kind: plan.kind,
        native_format: plan.native_format.clone(),
        native_id: plan.native_id.clone(),
        display_name: if plan.display_name.is_empty() {
            plan.native_id.clone()
        } else {
            plan.display_name.clone()
        },
        description: plan.description.clone(),
        origin_scope: plan.origin_scope,
        target_versions: plan.target_versions.clone(),
        entry_path: plan.entry_path.clone(),
        files,
    };
    let manifest_sha256 = manifest_digest(&manifest);
    let raw_files: Vec<(String, Vec<u8>)> =
        collected.into_iter().map(|(p, b, _, _)| (p, b)).collect();
    let bundle_sha256 = bundle_digest(&raw_files);
    let contents = raw_files
        .into_iter()
        .map(|(path, bytes)| FileContent {
            path,
            content_base64: base64::engine::general_purpose::STANDARD.encode(&bytes),
        })
        .collect();
    Ok(AssetBundle {
        manifest,
        manifest_sha256,
        bundle_sha256,
        contents,
        raw_manifest: None,
    })
}

// ===== 解析与复验（下载/安装前强制走）=====

/// 解析并复验一个信封：schema、路径安全、条目↔内容一一对应、逐文件
/// size/sha256、`manifest_sha256`、`bundle_sha256` 全部通过才返回内容字节。
/// 任何不一致按 `BUNDLE_INVALID`（或 `PATH_UNSAFE`）拒绝。
///
/// 调用方是下载/预览/安装链路；命令层刻意不在下载时强制复验——下载只要
/// 信封可反序列化即返回，复验留给预览/安装动作，避免双倍解码大包。
pub fn parse_bundle(envelope: &AssetBundle) -> Result<ParsedBundle, AssetError> {
    let m = &envelope.manifest;
    if m.schema_version != SCHEMA_VERSION {
        return Err(AssetError::bundle_invalid(format!(
            "不支持的清单 schema_version {}（当前 {SCHEMA_VERSION}）",
            m.schema_version
        )));
    }
    if m.files.is_empty() {
        return Err(AssetError::bundle_invalid("文件条目为空"));
    }
    if m.files.len() > MAX_ENTRIES {
        return Err(AssetError::too_many_entries(format!(
            "文件条目 {} 超过上限 {MAX_ENTRIES}",
            m.files.len()
        )));
    }
    if m.display_name.trim().is_empty() {
        return Err(AssetError::bundle_invalid("display_name 为空"));
    }
    // 路径安全 + 严格升序（无重复）+ 大小写碰撞
    let paths: Vec<String> = m.files.iter().map(|f| f.path.clone()).collect();
    for p in &paths {
        crate::agent_assets::safety::validate_relative_path(p)?;
    }
    for w in paths.windows(2) {
        if w[0].as_bytes() >= w[1].as_bytes() {
            return Err(AssetError::bundle_invalid(format!(
                "files 未按 path 严格升序或存在重复: {} / {}",
                w[0], w[1]
            )));
        }
    }
    crate::agent_assets::safety::check_case_collisions(&paths)?;
    if !paths.contains(&m.entry_path) {
        return Err(AssetError::bundle_invalid(format!(
            "entry_path {} 不在 files 内",
            m.entry_path
        )));
    }
    // 内容与条目一一对应、同序
    if envelope.contents.len() != m.files.len() {
        return Err(AssetError::bundle_invalid(format!(
            "contents 数量 {} 与 files {} 不一致",
            envelope.contents.len(),
            m.files.len()
        )));
    }
    let mut decoded: Vec<(String, Vec<u8>)> = Vec::with_capacity(m.files.len());
    let mut total: u64 = 0;
    for (f, c) in m.files.iter().zip(envelope.contents.iter()) {
        if f.path != c.path {
            return Err(AssetError::bundle_invalid(format!(
                "contents 与 files 顺序不一致: {} vs {}",
                c.path, f.path
            )));
        }
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(c.content_base64.as_bytes())
            .map_err(|e| AssetError::bundle_invalid(format!("{} base64 解码失败: {e}", f.path)))?;
        if bytes.len() as u64 != f.size {
            return Err(AssetError::bundle_invalid(format!(
                "{} 大小不符：清单 {}，实际 {}",
                f.path,
                f.size,
                bytes.len()
            )));
        }
        if bytes.len() as u64 > MAX_FILE_BYTES {
            return Err(AssetError::payload_too_large(format!(
                "文件 {} 超过单文件上限",
                f.path
            )));
        }
        total += bytes.len() as u64;
        if total > MAX_BUNDLE_BYTES {
            return Err(AssetError::payload_too_large("资源包总大小超过上限"));
        }
        let actual = sha256_hex(&bytes);
        if actual != f.sha256 {
            return Err(AssetError::bundle_invalid(format!(
                "{} 指纹不符：清单 {}，实际 {actual}",
                f.path, f.sha256
            )));
        }
        decoded.push((f.path.clone(), bytes));
    }
    // 信封级指纹复验：解析侧对收到的原始 manifest JSON 计算（未知字段与
    // 缺省可选字段不受结构体默认值影响）；pack 自建信封按结构体序列化。
    let m_digest = match &envelope.raw_manifest {
        Some(value) => manifest_digest_value(value)?,
        None => manifest_digest(m),
    };
    if m_digest != envelope.manifest_sha256 {
        return Err(AssetError::bundle_invalid(format!(
            "manifest_sha256 不符：信封 {}，复验 {m_digest}",
            envelope.manifest_sha256
        )));
    }
    let b_digest = bundle_digest(&decoded);
    if b_digest != envelope.bundle_sha256 {
        return Err(AssetError::bundle_invalid(format!(
            "bundle_sha256 不符：信封 {}，复验 {b_digest}",
            envelope.bundle_sha256
        )));
    }
    Ok(ParsedBundle {
        manifest: m.clone(),
        files: decoded,
    })
}

// ============================================================
// 单测：往返样例（最小 agent + 带 脚本/二进制附件/CRLF 的 skill）
// 与篡改检测。适配器层面的端到端用例在 adapters/zcode.rs。
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_assets::adapter::{ExportPlanBuilder, PlannedFile};

    fn plan_for(paths: &[&str], entry: &str, kind: AssetKind, native_id: &str) -> ExportPlan {
        let mut b = ExportPlanBuilder::new("zcode", kind, Scope::User, native_id);
        b = b.entry_path(entry);
        for p in paths {
            b = b.file(PlannedFile {
                path: (*p).to_string(),
                executable: false,
                binary: false,
            });
        }
        b.build()
    }

    #[test]
    fn pack_then_parse_round_trip_identical_fingerprints() {
        let root = crate::agent_assets::tests_support::temp_root("bundle-agent");
        std::fs::create_dir_all(&root).unwrap();
        // 最小 agent 样例（契约 §10.3）：frontmatter + Markdown 正文（CRLF 混入）
        let body = "---\r\nname: demo-agent\r\ndescription: 演示用最小子代理\r\nmodel: glm-4.7\r\n---\r\n你是演示子代理。\r\n";
        std::fs::write(root.join("demo-agent.md"), body.as_bytes()).unwrap();

        let plan = plan_for(
            &["demo-agent.md"],
            "demo-agent.md",
            AssetKind::Agent,
            "demo-agent",
        );
        let envelope = pack_from_disk(&root, &plan).unwrap();
        assert_eq!(envelope.manifest.files.len(), 1);
        assert_eq!(envelope.manifest.files[0].size, body.len() as u64);
        assert!(!envelope.manifest.files[0].binary);

        // 序列化 → 反序列化（模拟跨设备传输）→ 复验
        let wire = serde_json::to_string(&envelope).unwrap();
        let back: AssetBundle = serde_json::from_str(&wire).unwrap();
        let parsed = parse_bundle(&back).unwrap();
        // 打包 → 解析 指纹逐位一致（契约 §3.3 往返不变量）
        assert_eq!(manifest_digest(&parsed.manifest), envelope.manifest_sha256);
        assert_eq!(bundle_digest(&parsed.files), envelope.bundle_sha256);
        assert_eq!(parsed.files[0].1, body.as_bytes()); // 原文件字节不经重新序列化
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn parse_accepts_manifest_with_unknown_fields_from_other_clients() {
        // 服务端允许清单携带未知字段（契约 §3.2）；下载侧必须按收到的原始
        // manifest JSON 复验指纹，而不是按结构体重序列化（后者会丢未知字段）。
        let root = crate::agent_assets::tests_support::temp_root("bundle-unknown-fields");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("a.md"), b"---\nname: a\n---\nbody").unwrap();
        let plan = plan_for(&["a.md"], "a.md", AssetKind::Agent, "a");
        let envelope = pack_from_disk(&root, &plan).unwrap();

        let mut wire = serde_json::to_value(&envelope).unwrap();
        let manifest = wire
            .get_mut("manifest")
            .and_then(|v| v.as_object_mut())
            .unwrap();
        manifest.insert(
            "future_field".to_string(),
            serde_json::json!({"nested": [1, 2, 3], "text": "第三方客户端扩展"}),
        );
        let raw_manifest = wire["manifest"].clone();
        wire["manifest_sha256"] = serde_json::json!(manifest_digest_value(&raw_manifest).unwrap());
        let back: AssetBundle = serde_json::from_value(wire).unwrap();

        let parsed = parse_bundle(&back).expect("未知字段不得导致指纹复验失败");
        assert_eq!(parsed.files.len(), 1);
        assert_eq!(parsed.files[0].1, b"---\nname: a\n---\nbody");

        // fetch_bundle → 前端 → install 的 Tauri 序列化往返也必须保留原始
        // manifest（含未知字段），否则回传后复验会再次失败。
        let relayed = serde_json::to_value(&back).unwrap();
        assert_eq!(
            relayed["manifest"]["future_field"]["text"],
            "第三方客户端扩展"
        );
        let back_again: AssetBundle = serde_json::from_value(relayed).unwrap();
        assert!(parse_bundle(&back_again).is_ok());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn pack_skill_with_script_and_binary_attachment() {
        let root = crate::agent_assets::tests_support::temp_root("bundle-skill");
        std::fs::create_dir_all(root.join("scripts")).unwrap();
        std::fs::create_dir_all(root.join("assets")).unwrap();
        std::fs::create_dir_all(root.join("references")).unwrap();
        std::fs::write(root.join("SKILL.md"), b"---\nname: demo-skill\n---\nbody").unwrap();
        std::fs::write(root.join("scripts/run.sh"), b"#!/bin/sh\necho hi\n").unwrap();
        // 4 字节含 0x00 的伪 PNG → binary 嗅探命中
        std::fs::write(
            root.join("assets/logo.png"),
            [0x89, 0x50, 0x4E, 0x00].as_slice(),
        )
        .unwrap();
        std::fs::write(root.join("references/tips.md"), b"tips").unwrap();

        let plan = plan_for(
            &[
                "SKILL.md",
                "scripts/run.sh",
                "assets/logo.png",
                "references/tips.md",
            ],
            "SKILL.md",
            AssetKind::Skill,
            "demo-skill",
        );
        let envelope = pack_from_disk(&root, &plan).unwrap();
        let by_path: std::collections::BTreeMap<&str, &ManifestFile> = envelope
            .manifest
            .files
            .iter()
            .map(|f| (f.path.as_str(), f))
            .collect();
        assert!(by_path["assets/logo.png"].binary, "伪 PNG 应嗅探为二进制");
        assert!(by_path["scripts/run.sh"].executable, "脚本应标记可执行");
        assert!(!by_path["SKILL.md"].binary);
        // 清单按 path 升序
        let paths: Vec<&str> = envelope
            .manifest
            .files
            .iter()
            .map(|f| f.path.as_str())
            .collect();
        assert_eq!(
            paths,
            vec![
                "SKILL.md",
                "assets/logo.png",
                "references/tips.md",
                "scripts/run.sh"
            ]
        );

        let parsed = parse_bundle(&envelope).unwrap();
        assert_eq!(parsed.files.len(), 4);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn tampered_content_fails_verification() {
        let root = crate::agent_assets::tests_support::temp_root("bundle-tamper");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("a.md"), b"original").unwrap();
        let plan = plan_for(&["a.md"], "a.md", AssetKind::Agent, "a");
        let mut envelope = pack_from_disk(&root, &plan).unwrap();
        // 篡改内容但保留清单哈希 → 逐文件复验失败
        envelope.contents[0].content_base64 =
            base64::engine::general_purpose::STANDARD.encode(b"tampered");
        let err = parse_bundle(&envelope).unwrap_err();
        assert_eq!(err.code(), "BUNDLE_INVALID");
        // 篡改 bundle_sha256 → 信封级复验失败
        let mut envelope2 = pack_from_disk(&root, &plan).unwrap();
        envelope2.bundle_sha256 = "0".repeat(64);
        assert_eq!(
            parse_bundle(&envelope2).unwrap_err().code(),
            "BUNDLE_INVALID"
        );
        // 篡改 manifest_sha256 → 信封级复验失败
        let mut envelope3 = pack_from_disk(&root, &plan).unwrap();
        envelope3.manifest_sha256 = "0".repeat(64);
        assert_eq!(
            parse_bundle(&envelope3).unwrap_err().code(),
            "BUNDLE_INVALID"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn pack_rejects_unsafe_and_colliding_plans() {
        let root = crate::agent_assets::tests_support::temp_root("bundle-unsafe");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("ok.md"), b"x").unwrap();
        // 穿越
        let plan = plan_for(&["../x.md"], "../x.md", AssetKind::Agent, "a");
        assert_eq!(
            pack_from_disk(&root, &plan).unwrap_err().code(),
            "PATH_UNSAFE"
        );
        // 大小写碰撞
        let plan = plan_for(&["A.md", "a.md"], "A.md", AssetKind::Agent, "a");
        assert_eq!(
            pack_from_disk(&root, &plan).unwrap_err().code(),
            "PATH_UNSAFE"
        );
        // 设备名
        let plan = plan_for(&["CON"], "CON", AssetKind::Agent, "a");
        assert_eq!(
            pack_from_disk(&root, &plan).unwrap_err().code(),
            "PATH_UNSAFE"
        );
        // 入口不在清单内
        let plan = plan_for(&["ok.md"], "other.md", AssetKind::Agent, "a");
        assert_eq!(
            pack_from_disk(&root, &plan).unwrap_err().code(),
            "VALIDATION_FAILED"
        );
        // 空清单
        let plan = plan_for(&[], "", AssetKind::Agent, "a");
        assert_eq!(
            pack_from_disk(&root, &plan).unwrap_err().code(),
            "VALIDATION_FAILED"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn parse_rejects_unsorted_and_duplicate_files() {
        let root = crate::agent_assets::tests_support::temp_root("bundle-order");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("b.md"), b"bb").unwrap();
        std::fs::write(root.join("a.md"), b"aa").unwrap();
        // 手工构造乱序清单（正常 pack 不会产出）
        let mut plan = plan_for(&["b.md", "a.md"], "b.md", AssetKind::Agent, "a");
        plan.entry_path = "b.md".into();
        let envelope = pack_from_disk(&root, &plan).unwrap();
        // pack 已排序，这里手动打乱 manifest.files 与 contents 再复验
        let mut broken = envelope.clone();
        broken.manifest.files.swap(0, 1);
        broken.contents.swap(0, 1);
        // 打乱后 manifest_sha256 也不再匹配，无论哪个检查先触发都必须是 BUNDLE_INVALID
        let err = parse_bundle(&broken).unwrap_err();
        assert_eq!(err.code(), "BUNDLE_INVALID");
        let _ = std::fs::remove_dir_all(&root);
    }
}
