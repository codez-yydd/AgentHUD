# AgentHUD 子代理与 Skill 管理 —— 公共契约（工作包 A）

本文是「Agent 与 Skill」功能的**唯一公共契约**，供四个并行工作包独立开发：

- **B（服务端）**：按 §8 实现 Flask API 与存储；
- **C/D/E（适配器）**：按 §5、§6、§9、§10 实现 Rust 适配器；
- **F（前端）**：按 §4、§5、§7 对接 Tauri 命令与服务端状态。

契约自包含：不读方案文档也能完成开发。与 `doc/agent-skill-management-plan.md` 冲突时以本文为准并回报 A 集成负责人修订。所有字段名 snake_case，JSON 一律 UTF-8，无 BOM。

## 1. 术语

| 术语 | 含义 |
| --- | --- |
| 资源（asset） | 一个子代理（agent）或一个 Skill（skill），是上传、版本化、下载的最小单位 |
| 资源包（bundle） | 清单（manifest）+ 逐文件原始内容字节，跨设备传输的载体 |
| 工具（tool） | 资源所属的 coding agent，见 §2 工具 ID |
| 作用域（scope） | `user`（用户级）或 `project`（项目级） |
| 原生标识（native_id） | 资源在目标工具内的原生名字（agent 名 / skill 目录名 / 配置条目名） |
| 能力位 | 工具 × 资源类型 × 作用域上四档布尔声明，见 §5 |

## 2. 标识与词汇表

### 2.1 工具 ID（固定小写，不许别名）

| ID | 界面显示名 | 首期适配器 |
| --- | --- | --- |
| `zcode` | ZCode | A（参考实现） |
| `codex` | Codex | C |
| `claude` | Claude Code | C |
| `kimi` | Kimi Code | D |
| `opencode` | OpenCode | D |
| `dsh` | DeepSeek Harness | E（占位，待发行版验证） |
| `minimax` | MiniMax Code | E（占位，待发行版验证） |

### 2.2 资源类型 `kind`

`agent`（子代理）| `skill`（技能）。每类工具的行为可能不同，逐组合声明能力。

### 2.3 原生格式 `native_format`（开放词汇，未知值必须容忍）

| 值 | 含义 |
| --- | --- |
| `markdown-frontmatter` | YAML frontmatter + Markdown 正文（zcode/claude/kimi 当前版 agent、opencode Markdown agent） |
| `toml` | 独立 TOML 文件（codex agent） |
| `jsonc-inline` | 从内联 JSON/JSONC 配置中提取的单个条目（opencode 内联 agent；原文件不整体打包，见 §3.4） |
| `yaml-legacy` | 旧 kimi-cli 的 YAML agent 声明 |
| `skill-dir` | `SKILL.md` + 目录内脚本/引用/附件（各工具 Skill 目录） |
| `dsh-preset` | DSH Agent Preset 声明（待 E 验证） |

消费方（服务端、前端）不得因未知 `native_format` 报错；只有适配器理解其语义。

## 3. 资源包与清单 schema

### 3.1 顶层传输信封 `AssetBundle`

```json
{
  "manifest": { "…见 3.2…" },
  "manifest_sha256": "小写 hex（64 位）",
  "bundle_sha256": "小写 hex（64 位）",
  "contents": [
    { "path": "SKILL.md", "content_base64": "…文件原始字节的标准 base64…" }
  ]
}
```

- `contents` 与 `manifest.files` **一一对应、同序**（均按 `path` 升序，见 §3.3）。
- 文件内容以**字节数组随清单传输**（wire 上为字节数组的标准 base64 编码，含 padding），**原文件不经任何重新序列化**：换行（CRLF/LF）、未知字段、二进制附件逐字节保留。base64 解码后即原始字节。
- 包内相对路径以**资源自身根**为基准：单文件型 agent 以其所在目录为根（如 `code-reviewer.md`）；目录型 skill 以 skill 目录为根（如 `SKILL.md`、`scripts/run.sh`）。跨设备恢复时由目标适配器解析新根目录，包内路径不改写。

### 3.2 清单 `AssetManifest`（`schema_version = 1`）

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `schema_version` | u32 | 是 | 当前 `1`；不认识的版本必须拒绝（`BUNDLE_INVALID`） |
| `tool` | string | 是 | §2.1 工具 ID |
| `kind` | string | 是 | `agent` \| `skill` |
| `native_format` | string | 是 | §2.3 |
| `native_id` | string | 是 | 原生标识；同一工具内按 §10 各适配器规则唯一 |
| `display_name` | string | 是 | 界面显示名；缺失元数据时取 `native_id`，不得为空串 |
| `description` | string \| null | 否 | 描述（来自 frontmatter/配置），可为 null |
| `origin_scope` | string | 是 | `user` \| `project`（采集时的作用域；恢复时项目级需重选目标项目） |
| `target_versions` | string[] | 否 | 验证过的适用工具版本（如 `["Codex CLI 0.2x"]`），可空 |
| `entry_path` | string | 是 | 入口文件相对路径（agent 的 md/toml；skill 的 `SKILL.md`），必须在 `files` 内 |
| `files` | ManifestFile[] | 是 | 非空，按 `path` 升序 |

`ManifestFile`：

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `path` | string | 包内相对路径，必须通过 §9 全部路径安全检查 |
| `size` | u64 | 原始字节数 |
| `sha256` | string | 该文件原始内容的 SHA-256，小写 hex |
| `executable` | bool | 可执行标记（Unix 权限位或 Windows `.bat/.cmd/.exe/.ps1` 后缀），恢复时按目标平台落地 |
| `binary` | bool | 二进制标志（内容嗅探：前 8 KiB 含 `0x00` 字节，或适配器声明），纯文本预览据此分流 |

清单本身**不含** `manifest_sha256`/`bundle_sha256`（自引用），二者只在信封层。

### 3.3 指纹规则（B 服务端复验、安装前复验都按此实现）

- `manifest_sha256` = 对**清单对象的规范 JSON** 计算 SHA-256：
  - UTF-8、紧凑分隔符（无空白、无换行、无尾随换行）；
  - 对象键按 Unicode 码点**字典序**排序；`files` 数组按 `path` 的 UTF-8 字节序升序；
  - 参考实现：Python `json.dumps(manifest, ensure_ascii=False, separators=(",", ":"), sort_keys=True).encode("utf-8")`；Rust 序列化到 `serde_json::Value` 后 `to_string`（serde_json 默认 Map 为 BTreeMap，键天然有序）。
- `bundle_sha256` = 对**全部文件内容**计算 SHA-256：按 `path` 升序逐条喂入 `path` 的 UTF-8 字节、单个 `0x00` 分隔字节、该文件原始内容字节；条目间无额外分隔（路径安全规则禁止路径含 `0x00`，无歧义）。
- 所有哈希一律**小写 hex**。
- 往返不变量：任意一方 `pack → 传输 → parse` 后，三个层次（逐文件 sha256、`manifest_sha256`、`bundle_sha256`）必须逐位一致；任何不一致按 `BUNDLE_INVALID` 拒绝。
- Rust 解析侧按**收到的原始 manifest JSON 子树**（`serde_json::Value`）计算 `manifest_sha256`，而不是结构体重序列化：服务端允许未知字段、且允许 `description`/`target_versions` 缺省，结构体补默认值/丢未知字段会导致第三方包误判。打包侧（`pack`）自建清单按结构体规范序列化计算；两条路径在自有包上逐位一致。服务端必须拒绝 `schema_version` 的非整数字面量（`1.0`/`"1"`），避免浮点/整数规范形态差异。

### 3.4 内联配置例外

`native_format = jsonc-inline` 等内联定义：资源包只含**提取出的单个条目**（序列化为独立 JSONC 片段文件），`entry_path` 指向该片段；恢复时**首期由用户手动合并**回配置（适配器 `installable=false`），AgentHUD 不改写整份配置。

### 3.5 外部引用

包外路径引用（绝对路径、`~` 路径、其他资源目录）**只报告不递归**：出现在 `ExportPlan.external_references` 与 `InspectionReport.external_references`（字符串数组），绝不读入包内。

## 4. 统一错误码

### 4.1 码表（稳定字符串，前端据此分支；`message` 为可读中文，可改）

| code | 场景 | HTTP（服务端发出时） |
| --- | --- | --- |
| `UNAUTHORIZED` | 设备凭证缺失/无效 | 401 |
| `FORBIDDEN` | 设备已被撤销 | 403 |
| `NOT_FOUND` | 资源或版本不存在 | 404 |
| `VERSION_CONFLICT` | PUT 携带的 `expected_version` 与服务端当前版本不一致 | 409 |
| `VALIDATION_FAILED` | 请求体不合法（含清单 schema 校验失败、参数错误） | 400 |
| `PATH_UNSAFE` | 任一包内路径违反 §9 | 400 |
| `TOO_MANY_ENTRIES` | 文件条目数超过限制 | 400 |
| `PAYLOAD_TOO_LARGE` | 单文件/单包/请求体超过限制 | 413 |
| `QUOTA_EXCEEDED` | 资源库总容量超限 | 507 |
| `CAPABILITY_UNSUPPORTED` | 请求了能力矩阵未支持的 工具×类型×作用域 组合 | 400 |
| `INTERNAL` | 服务端内部错误 | 500 |
| `CONFIG_MISSING` | 本机未注册设备（无 `device_token`），仅客户端发出 | — |
| `OLD_SERVER_UNSUPPORTED` | 服务端无 §8.1 能力接口（版本过旧），仅客户端发出 | — |
| `NETWORK_ERROR` | 网络层失败（连接/超时），仅客户端发出 | — |
| `ADAPTER_NOT_FOUND` | 工具 ID 无已注册适配器（C/D 未合并或 ID 拼错） | — |
| `NOT_IMPLEMENTED` | 适配器已注册但该能力位未实现 | — |
| `IO_ERROR` | 本地文件读写失败 | — |
| `BUNDLE_INVALID` | 打包/解析/指纹复验失败 | — |

> 首期 `FORBIDDEN` 不会出现：当前设备撤销机制直接删除设备行，被撤销设备的旧凭证表现为无效凭证（401 `UNAUTHORIZED`）。将来引入设备墓碑后再启用 403。

### 4.2 线上错误体（服务端所有非 2xx 都用此形状）

```json
{ "error": { "code": "VERSION_CONFLICT", "message": "预期版本 3，服务端当前为 5", "details": { "current_version": 5 } } }
```

`details` 可省略；`VERSION_CONFLICT` 必须带 `details.current_version`（u64）。

### 4.3 Tauri 命令错误

所有 §7 命令返回 `Result<T, AssetError>`，`AssetError` 即 `{ "code": "…", "message": "…", "details": {…}? }`（前端 `catch` 收到的就是该对象）。服务端错误码原样透传（§4.1 HTTP 列非空的码）。

## 5. 能力位四档与首期矩阵

### 5.1 四档定义（布尔位，按 工具 × kind × 作用域 声明，未声明组合一律视为全 false）

| 位 | 名称 | 含义 |
| --- | --- | --- |
| 1 | `scannable` 可扫描 | `discover` 能列出该组合的本机资源 |
| 2 | `archivable` 可归档 | `plan_export`+`pack` 能产出合法资源包并上传 |
| 3 | `native_exportable` 可原生导出 | 能下载原生文件/文件包并还原出与原目录字节一致的文件 |
| 4 | `installable` 可安装 | 适配器能在本机**安全写入并注册**（§9 路径检查、同卷暂存+原子发布、默认跳过冲突） |

依赖关系：`installable ⊆ native_exportable ⊆ archivable ⊆ scannable`（高层位为 true 时低位必为 true）。

### 5.2 `CapabilityEntry` 形状（本地矩阵与服务端可提供的扩展共用）

```json
{
  "tool": "zcode",
  "display_name": "ZCode",
  "kind": "agent",
  "scope": "user",
  "scannable": true, "archivable": true, "native_exportable": true, "installable": true,
  "native_format": "markdown-frontmatter",
  "status": "verified",
  "verified_versions": [],
  "notes": "安装 = 写入 ~/.zcode/agents/<name>.md，同卷暂存 + 原子发布"
}
```

- `status`：`verified`（已按实际版本验证）| `pending`（占位，待 E 工作包发行版验证）。**`pending` 条目前端一律不可操作**，只显示「待验证」。
- `verified_versions`：适配器 PR 必须填实测版本；A 交付的矩阵留空，由 C/D/E 合并时补。
- 首期矩阵（A 交付，代码在 `capabilities.rs`；C/D/E 在各自 PR 内校正）：

| tool | kind | scope | scan | archive | native | install | 说明 |
| --- | --- | --- | --- | --- | --- | --- | --- |
| zcode | agent | user | 1 | 1 | 1 | 1 | 仅用户级；项目级子代理首期不开放 |
| zcode | skill | user | 1 | 1 | 1 | 1 | `~/.zcode/skills/` 与 `~/.agents/skills/` 双根，见 §10 |
| zcode | skill | project | 1 | 1 | 1 | 0 | 项目级 Skill 仅发现与归档；目录格式待样例验证后开安装 |
| codex | agent | user | 1 | 1 | 1 | 1 | 独立 TOML；内置 Agent 可被同名覆盖，预览须显示 |
| codex | agent | project | 1 | 1 | 1 | 0 | 首期用户级限定，项目级只扫描归档 |
| codex | skill | user | 1 | 1 | 1 | 1 | |
| codex | skill | project | 1 | 1 | 1 | 1 | 保留附属资源与 `agents/openai.yaml` |
| claude | agent | user | 1 | 1 | 1 | 1 | 递归 Markdown，frontmatter `name` 定身份 |
| claude | agent | project | 1 | 1 | 1 | 0 | 首期用户级限定 |
| claude | skill | user | 1 | 1 | 1 | 1 | |
| claude | skill | project | 1 | 1 | 1 | 1 | 保留 `tools`/`permissionMode`/`hooks` 字段并提示敏感配置 |
| kimi | agent | user | 1 | 1 | 1 | 1 | 当前版 Markdown |
| kimi | agent | user（`yaml-legacy`） | 1 | 1 | 1 | 0 | 旧 kimi-cli YAML：仅归档与原样导出，恢复按需验证 |
| kimi | agent | project | 1 | 1 | 1 | 0 | 首期用户级限定 |
| kimi | skill | user | 1 | 1 | 1 | 1 | `$KIMI_CODE_HOME/skills/` 与共享 `.agents/skills/` |
| kimi | skill | project | 1 | 1 | 1 | 1 | `.kimi-code/skills/` 与共享目录 |
| opencode | agent | user | 1 | 1 | 1 | 1 | Markdown 优先 |
| opencode | agent | user（`jsonc-inline`） | 1 | 1 | 1 | 0 | 内联条目仅提取归档，首期用户手动合并 |
| opencode | agent | project | 1 | 1 | 1 | 0 | 首期用户级限定；V1/V2 差异由 D 验证 |
| opencode | skill | user | 1 | 1 | 1 | 1 | |
| opencode | skill | project | 1 | 1 | 1 | 1 | |
| dsh | 全部 | 全部 | 0 | 0 | 0 | 0 | `pending`：Agent Preset 与 Skill 目录待发行版验证（E） |
| minimax | 全部 | 全部 | 0 | 0 | 0 | 0 | `pending`：Skill 可先行，子代理仅已确认样例归档（E） |

## 6. Adapter trait（Rust，`src-tauri/src/agent_assets/adapter.rs`）

```rust
pub trait Adapter: Send + Sync {
    /// §2.1 工具 ID。
    fn tool(&self) -> &'static str;
    /// 该工具的能力矩阵切片（来自 §5.2 静态表，不得运行时编造）。
    fn capabilities(&self) -> Vec<CapabilityEntry>;
    /// 只读扫描某作用域下的资源（project 作用域必须传 project_root）。
    fn discover(&self, scope: Scope, project_root: Option<&Path>)
        -> Result<Vec<DiscoveredAsset>, AssetError>;
    /// 解析单个资源元数据；frontmatter/配置解析失败只降级为 warnings，
    /// 绝不 Err（不阻断归档）。找不到资源返回 NOT_FOUND。
    fn inspect(&self, scope: Scope, native_id: &str, project_root: Option<&Path>)
        -> Result<InspectionReport, AssetError>;
    /// 生成导出计划：文件相对路径清单 + 敏感字段提示 + 警告 + 包外引用。
    /// 只列清单不读内容；真正读取、哈希、安全检查由共用层 bundle::pack_from_disk 完成。
    /// 未声明 archivable 的组合返回 CAPABILITY_UNSUPPORTED。
    fn plan_export(&self, scope: Scope, native_id: &str, project_root: Option<&Path>)
        -> Result<ExportPlan, AssetError>;
}
```

约定：

- `discover`/`inspect`/`plan_export` 一律**只读**，不写任何文件；
- `project_root` 为绝对路径，`scope=user` 时忽略（可传 None）；
- `ExportPlan` 含 `source_root`（资源根的绝对路径，供 pack 落盘收集；也用于前端展示位置）；
- 安装（install）**不在 trait 内**：首期由各适配器在本模块内实现独立安装函数并自行命令注册（E/F 阶段接线），共用层只提供路径安全与原子写工具。

`DiscoveredAsset` / `InspectionReport` / `ExportPlan` 字段与 §7 命令返回一致（serde 序列化即 JSON 形状）。

## 7. Tauri 命令清单（A 已在 `lib.rs` 注册）

统一 `async` + `spawn_blocking`（同 `lib.rs` 现有远端命令风格）。参数为空对象时省略。

| 命令 | 参数 | 返回 | 数据源 |
| --- | --- | --- | --- |
| `agent_assets_capabilities` | 无 | `Vec<CapabilityEntry>` | 本地矩阵 |
| `agent_assets_server_status` | 无 | `ServerAssetStatus`（下） | GET `/agent-assets/capabilities` |
| `agent_assets_discover` | `tool, kind, scope, project_root?` | `Vec<DiscoveredAsset>` | 适配器 |
| `agent_assets_inspect` | `tool, kind, scope, native_id, project_root?` | `InspectionReport` | 适配器 |
| `agent_assets_plan_export` | `tool, kind, scope, native_id, project_root?, source_override?` | `ExportPlan` | 适配器；`source_override` 仅用于手选导入（显式资源根） |
| `agent_assets_pack` | `tool, kind, scope, native_id, project_root?, source_override?` | `AssetBundle` | plan_export + 共用层打包；`source_override` 语义同上 |
| `agent_assets_import_files` | `tool, kind, paths` | `Vec<ImportCandidate>`（§7.3） | 本地：逐项 §9 安全检查 + 适配器解析；不识别项 `importable=false`，不阻断整批 |
| `agent_assets_list_remote` | `tool?, kind?, scope?, q?, page?, page_size?` | `RemoteAssetPage` | GET `/agent-assets` |
| `agent_assets_upload` | `bundle, client_request_id` | `RemoteAssetResult` | POST `/agent-assets` |
| `agent_assets_update_remote` | `asset_id, expected_version, bundle` | `RemoteAssetResult` | PUT `/agent-assets/<id>` |
| `agent_assets_list_revisions` | `asset_id` | `Vec<RemoteRevision>` | GET `/agent-assets/<id>/revisions` |
| `agent_assets_fetch_bundle` | `asset_id, version` | `AssetBundle` | GET `…/revisions/<rev>/bundle`；下载本身不复验，预览/安装前由调用方走指纹复验（§3.3） |
| `agent_assets_export_to_disk` | `bundle, dest_dir, overwrite` | `DiskExportResult`（§7.2） | 本地：`parse_bundle` 复验后写 `dest_dir/<tool>-<kind>-<native_id>/`；已存在默认跳过，`overwrite=true` 先备份 |
| `agent_assets_install` | `bundle, target_scope, project_root?, overwrite` | `InstallResult`（§7.2） | 本地：能力/格式预检 + 目标根解析 + 同卷暂存/原子发布；未声明 `installable` 返回 `CAPABILITY_UNSUPPORTED` |

`ServerAssetStatus`：

```json
{ "supported": true, "api_version": 1, "limits": { "…§8.1 同形…" }, "reason": null, "detail": null, "insecure_transport": false }
```

未注册设备：`supported=false, reason="CONFIG_MISSING"`；旧服务端（404/405/响应不可解析）：`supported=false, reason="OLD_SERVER_UNSUPPORTED"`，前端提示升级服务端，本地发现/打包仍可用；网络失败：`reason="NETWORK_ERROR"`；凭证无效/被撤销：`reason="UNAUTHORIZED"`。`detail` 为人读补充（可省略）。该命令**不抛错**——所有探测失败都折成 `supported=false` 的状态对象。

### 7.1 核心返回结构 JSON 示例

`DiscoveredAsset`：

```json
{
  "tool": "zcode", "kind": "skill", "scope": "user",
  "native_id": "pdf-report", "native_format": "skill-dir",
  "display_name": "PDF 报告", "description": "生成 PDF 报告",
  "entry_path": "SKILL.md",
  "warnings": [],
  "source_root": "C:\\Users\\me\\.zcode\\skills\\pdf-report"
}
```

`InspectionReport`（frontmatter 解析失败时 `parsed_ok=false` + `parse_error`，files 照常）：

```json
{
  "asset": { "…DiscoveredAsset…" },
  "metadata": { "name": "pdf-report", "description": "…", "model": null, "tools": [],
                "frontmatter_raw": "---\nname: …\n---", "parsed_ok": true, "parse_error": null },
  "files": [ { "path": "SKILL.md", "size": 612, "executable": false, "binary": false } ],
  "sensitive_fields": [ { "file": "SKILL.md", "field": "env", "reason": "frontmatter 含疑似环境变量注入字段" } ],
  "warnings": [], "external_references": []
}
```

`ExportPlan`：

```json
{
  "tool": "zcode", "kind": "skill", "scope": "user",
  "native_format": "skill-dir", "native_id": "pdf-report",
  "display_name": "PDF 报告", "description": "…", "target_versions": [],
  "entry_path": "SKILL.md",
  "source_root": "C:\\Users\\me\\.zcode\\skills\\pdf-report",
  "files": [ { "path": "SKILL.md", "executable": false, "binary": false },
             { "path": "scripts/run.sh", "executable": true, "binary": false } ],
  "sensitive_fields": [], "warnings": [], "external_references": []
}
```

`RemoteAsset` / `RemoteRevision` / `RemoteAssetResult`（服务端返回原样透传）：

```json
{
  "asset": {
    "asset_id": "a_1f2…", "tool": "zcode", "kind": "skill", "native_id": "pdf-report",
    "display_name": "PDF 报告", "description": null, "origin_scope": "user",
    "native_format": "skill-dir", "latest_version": 2,
    "latest_bundle_sha256": "…", "latest_manifest_sha256": "…",
    "total_size": 20480, "entry_count": 4,
    "created_at": 1760000000000, "updated_at": 1760100000000, "updated_by_device": "dev_…"
  },
  "revision": {
    "asset_id": "a_1f2…", "version": 2, "bundle_sha256": "…", "manifest_sha256": "…",
    "total_size": 20480, "entry_count": 4,
    "created_at": 1760100000000, "created_by_device": "dev_…"
  },
  "replayed": false, "deduplicated": false
}
```

时间戳一律 epoch 毫秒（i64）。`RemoteAsset`/`RemoteRevision` 的 `tool`/`kind`/`origin_scope` 按小写字符串透传（客户端不强转枚举，服务端将来新增类型不破老客户端的反序列化）。

### 7.2 安装与原生导出返回结构（工作包 C/D 追加）

`DiskExportResult`（`agent_assets_export_to_disk`）：

```json
{
  "written": ["D:\\exports\\codex-agent-reviewer\\reviewer.toml"],
  "skipped": [],
  "backup_dir": "D:\\exports\\.agenthud-bak-1760000000000",
  "warnings": ["内联配置片段已按单条提取导出，请手动合并回 OpenCode 配置（首期不改写整份配置）"]
}
```

`InstallResult`（`agent_assets_install`）：

```json
{
  "installed_paths": [],
  "skipped": [{ "path": "C:\\Users\\me\\.codex\\agents\\reviewer.toml", "reason": "目标已存在（overwrite=false，默认跳过）" }],
  "backup_dir": null,
  "tool_instructions": null,
  "warnings": ["目标目录 C:\\Users\\me\\.agents\\skills 为共享目录：安装将影响同样读取该目录的其他工具"]
}
```

- 两个命令都先走 `parse_bundle` 复验指纹（§3.3）与路径安全（§9）；失败分别返回 `BUNDLE_INVALID` / `PATH_UNSAFE`，不落任何文件。
- 目标已存在且 `overwrite=false` 时只返回 `skipped`，不写盘；`overwrite=true` 先备份到 `<目标根>/.agenthud-bak-<epoch_ms>/` 再原子发布，失败保留或恢复原内容。
- `tool_instructions` 当前四款工具均为 `null`（`jsonc-inline` / `yaml-legacy` 的 `installable=false`，安装会直接返回 `CAPABILITY_UNSUPPORTED`，引导用户走原生导出 + 手动合并/旧版 `--agent-file`）。
- `warnings` 会包含共享目录影响提示、内置 Agent 同名覆盖提示与内联/旧格式的手动步骤说明。

### 7.3 手选原生文件导入返回结构（C/D 收尾追加）

`agent_assets_import_files` 对 `paths` 逐项返回，成功与失败项共用同一数组，单项失败不影响整批：

```json
[
  {
    "tool": "claude", "kind": "agent",
    "path": "D:\\manual\\code-reviewer.md",
    "importable": true,
    "asset": { "tool": "claude", "kind": "agent", "scope": "user", "native_id": "code-reviewer",
               "native_format": "markdown-frontmatter", "display_name": "code-reviewer", "description": "…",
               "entry_path": "code-reviewer.md", "warnings": ["来源：手选原生文件导入"],
               "source_root": "D:\\manual" },
    "metadata": { "name": "code-reviewer", "description": "…", "model": null, "tools": [],
                  "frontmatter_raw": "---\nname: code-reviewer\n---", "parsed_ok": true, "parse_error": null },
    "files": [ { "path": "code-reviewer.md", "size": 612, "executable": false, "binary": false } ],
    "sensitive_fields": [], "warnings": ["来源：手选原生文件导入"],
    "external_references": [], "source_root": "D:\\manual", "error": null
  },
  {
    "tool": "claude", "kind": "agent",
    "path": "D:\\manual\\notes.txt",
    "importable": false,
    "asset": null,
    "metadata": { "name": null, "description": null, "model": null, "tools": [],
                  "frontmatter_raw": null, "parsed_ok": false,
                  "parse_error": "agent 文件扩展名不匹配（允许 .md）: D:\\manual\\notes.txt" },
    "files": [], "sensitive_fields": [],
    "warnings": ["agent 文件扩展名不匹配（允许 .md）: D:\\manual\\notes.txt"],
    "external_references": [], "source_root": null,
    "error": "agent 文件扩展名不匹配（允许 .md）: D:\\manual\\notes.txt"
  }
]
```

- `native_id` 推断与各适配器 `discover` 一致：Claude/Kimi 取 frontmatter `name`（Kimi 缺省回退文件名）、Codex 取 TOML `name`（缺省回退文件名）、ZCode 取文件名去扩展、OpenCode Markdown 取文件名去扩展（位于 `agents/`/`agent/`/`mode/`/`modes/` 目录下时保留相对路径，与 discover 的嵌套 ID 一致）、skill 统一取目录名。
- 手选导入的 `asset.scope` / `ImportCandidate` 清单默认按 `user` 记录；后续 `agent_assets_plan_export` / `agent_assets_pack` 传 `source_override`（前端 `sourceOverride`）时按显式资源根解析，并校验解析出的 `native_id` 与请求参数一致；不传时行为与普通 discover 流程完全相同。
- 目录型 skill 可传目录本身或其中的 `SKILL.md`；目录缺少 `SKILL.md`、扩展名与 kind 不匹配、路径非绝对、路径本体是符号链接/junction 等都折成 `importable=false` 的单项，不返回整批错误。

## 8. 服务端 API（B 实现）

鉴权复用现有设备模型：`Authorization: Bearer <device_token>`；已注册设备可读写同一资源库（自托管单用户）。

### 8.1 `GET /agent-assets/capabilities`

返回服务端能力与限制（也是**版本探测端点**，旧服务端无此路由）：

```json
{ "api_version": 1, "limits": {
  "max_request_bytes": 26214400, "max_file_bytes": 8388608,
  "max_bundle_bytes": 16777216, "max_entries_per_bundle": 200,
  "max_revisions_per_asset": 20, "library_quota_bytes": 268435456 } }
```

### 8.2 `GET /agent-assets`（列表，仅元数据）

Query：`tool`、`kind`、`scope`（精确匹配，可省）、`q`（`native_id`/`display_name` 子串，大小写不敏感，可省）、`page`（默认 1）、`page_size`（默认 20，≤100）。

```json
{ "items": [ RemoteAsset… ], "page": 1, "page_size": 20, "total": 42 }
```

### 8.3 `POST /agent-assets`（创建）

Body：`{ "client_request_id": "UUIDv4", "bundle": AssetBundle }`。服务端在一个事务内：

1. 结构校验（schema、条目与内容一一对应、逐文件 size/sha256、`manifest_sha256`/`bundle_sha256` 复验、§9 路径检查、容量限制）；
2. 幂等（见 8.6）；
3. 新建资源（服务端分配 `asset_id`）并写入版本 1。

成功返回 `RemoteAssetResult`（`replayed`/`deduplicated` 标志见 8.6）。

### 8.4 `PUT /agent-assets/<id>`（更新，产生新版本）

Body：`{ "expected_version": u64, "bundle": AssetBundle }`。`expected_version` 必须等于该资源当前 `latest_version`，否则 `409 VERSION_CONFLICT`（`details.current_version`），客户端重新预览后重试或改走 POST 另存。校验同 8.3。成功追加新版本（`latest_version + 1`）。

### 8.5 `GET /agent-assets/<id>/revisions` 与 `GET /agent-assets/<id>/revisions/<rev>/bundle`

前者返回 `{ "items": [ RemoteRevision… ] }`（按 `version` 降序）。后者返回该版本的 `AssetBundle` 原信封（含内容）。旧版本保留可下载；超 `max_revisions_per_asset` 时淘汰最旧（淘汰策略服务端可调，文档化即可）。

### 8.6 幂等与去重规则

- **客户端请求 ID 去重**：`(device_id, client_request_id)` 为幂等键，服务端保留结果至少 48 小时；重放返回**首次结果**（HTTP 200）且 `replayed=true`，不产生新写入。断网重试必须复用同一 `client_request_id`。
- **相同内容去重**：POST 时若已存在同 `(tool, kind, native_id, origin_scope)` 的资源，且其 `latest_version` 的 `bundle_sha256` 与 `manifest_sha256` 与本次**完全一致**，则不写新版本，返回该资源与该版本，`deduplicated=true`。内容不同则正常新建资源（同名资源允许共存，各持独立 `asset_id`；更新请走 PUT）。
- **PUT 相同内容去重**：`expected_version` 校验通过后，若本次 `bundle_sha256` + `manifest_sha256` 与该资源当前 `latest_version` 完全一致，同样不追加版本，返回现有资源与该版本（结构与 POST 一致）且 `deduplicated=true`；避免无意义版本膨胀挤掉历史版本。
- PUT 无客户端请求 ID（`expected_version` 即并发控制）；409 后客户端应重新 inspect/plan/pack 再重试。

### 8.7 容量与保留（建议默认值，服务端可配置，须在 8.1 如实返回）

| 项 | 默认值 |
| --- | --- |
| 单文件 | 8 MiB |
| 单包总内容 | 16 MiB |
| 单包条目数 | 200 |
| 请求体上限（base64 膨胀 ≈4/3 + 清单） | 25 MiB |
| 单资源保留版本数 | 20 |
| 资源库总容量 | 256 MiB |

服务端**不执行、不渲染**任何包内容；预览按纯文本（`binary=false` 条目）。解压类请求首期不存在（信封是 JSON，无 zip 炸弹面）；将来若接收压缩包，必须先全量校验条目再解压并限制解压后大小。

容量统计口径：`total_size` / `library_quota_bytes` 按**解码后的文件内容字节数**计（逐文件 `size` 之和），不含清单、base64 padding 与 JSON 信封开销；SQLite 实际落盘的 `bundle_blob` 是 base64 JSON 信封，体积约为内容字节的 1.37 倍（4/3 膨胀 + 清单），磁盘占用可能明显高于配额数字。

## 9. 路径安全规则（`safety.rs`，客户端与服务端双侧执行同一规则）

对每个包内相对路径（以 `/` 为分隔符）：

1. 非空且 ≤ 512 字节；仅 `/` 分隔（拒绝 `\`）；无前导 `/`；
2. 逐段检查：无空段（`//`、尾 `/`）、无 `.`/`..` 段、无盘符或任何 `:`（同时覆盖 NTFS 备用数据流）、无控制字符（`0x00`–`0x1F`、`0x7F`）、段不以点或空格结尾；
3. 拒绝 Windows 保留名（**取每段去掉首个 `.` 之前的部分**做大小写不敏感比较，即 `CON`、`CON.txt` 同拒）：`CON` `PRN` `AUX` `NUL` `COM1`–`COM9` `LPT1`–`LPT9`；
4. 拒绝 8.3 短名形态：把路径段按 `.` 切分，任一切分片匹配 `^.*~[0-9]+$`（仅 ASCII 数字、大小写不敏感）即拒绝。等价描述：stem（最后一个 `.` 之前的部分，无点则整段）以 `~` + 纯 ASCII 数字结尾（`^.+~[0-9]+$`）即拒绝；为覆盖多点形态，任一点分隔片命中同样拒绝。`NAME~1`、`name~1.md`、`a.b~1.md`、`name~1.tar.gz`、`~1` 一律拒绝，`wave~front.md` 放行。Python 使用 `[0-9]`（不得用 `\d`，后者匹配 Unicode 数字）；Rust 只认 ASCII 数字；
5. 拒绝 Windows 保留字符 `< > " | ? *`；
6. **同目录大小写碰撞**：同一包内两条不同路径按大小写折叠（lowercase）后相同即拒绝（保护 Windows/大小写不敏感文件系统上的覆盖）；
7. **符号链接与 junction**：落盘收集/安装时用 `symlink_metadata` 检查——文件本体是符号链接、或其任一父目录是符号链接/junction（Windows 上 std 的 `is_symlink()` 对二者均返回 true）即拒绝该资源归档；外部引用只报告（§3.5）不递归。

违反 1–6 返回 `PATH_UNSAFE`，`details.path` 指明违规条目。凭据文件/私钥类内容（如文件名 `credentials`、`.pem`、`id_rsa`、`.env`）由适配器在 `sensitive_fields`/`warnings` 提示，首期按「预览可见 + 用户确认」处理，不自动上传。

## 10. ZCode 参考实现（A 交付，`adapters/zcode.rs`，C/D 照此形态）

### 10.1 目录与发现

| 资源 | 作用域 | 根目录 | 规则 |
| --- | --- | --- | --- |
| agent | user | `~/.zcode/agents/` | 非递归扫 `*.md`；`native_id` = 文件名去扩展；首期无项目级 |
| skill | user | `~/.zcode/skills/` → `~/.agents/skills/` | 每个子目录含 `SKILL.md` 即资源；`native_id` = 目录名；双根同名按 `.zcode` 优先，被遮蔽的记入 `warnings` |
| skill | project | `<root>/.zcode/skills/` → `<root>/.agents/skills/` | 同上；仅发现与归档（installable=false） |

### 10.2 格式

- agent：`---` 围栏 YAML frontmatter（`name`/`description`/`model`/`tools` 等，未知字段原样保留）+ Markdown 正文；包根 = `agents/` 目录，`entry_path = <native_id>.md`。
- skill：包根 = skill 目录；`entry_path = SKILL.md`；目录内脚本按扩展（`.sh/.py/.js/.ps1/.bat/.cmd/.exe`）或 Unix 执行位标 `executable`，含 `0x00` 字节标 `binary`。
- inspect 的 frontmatter 解析**失败不阻断**：`parsed_ok=false` + `parse_error`，归档继续（原文件字节为真）。
- 敏感字段提示：frontmatter 键名含 `token`/`secret`/`key`/`api`/`env`/`credential`（大小写不敏感）时记入 `sensitive_fields`。

### 10.3 最小往返样例（同 `adapters/zcode.rs` 单测内置）

agent（`~/.zcode/agents/demo-agent.md`）：

```markdown
---
name: demo-agent
description: 演示用最小子代理
model: glm-4.7
tools:
  - read
  - grep
---
你是演示子代理。正文支持任意 Markdown，含未知 frontmatter 字段时原样保留。
```

skill（`~/.zcode/skills/demo-skill/`）：

```text
SKILL.md          # frontmatter（name/description）+ 正文，CRLF 与 UTF-8 均需往返不变
scripts/run.sh    # executable=true
assets/logo.png   # binary=true（含 0x00 字节）
references/tips.md
```

## 11. 版本与演进

- 清单 `schema_version` 与 API `api_version` 独立演进，均从 1 起；不识别的版本必须显式报错，禁止静默降级。
- 新增清单字段：可选字段向后兼容（旧端忽略）；破坏性变更升 `schema_version`。
- 旧服务端（无 `/agent-assets/capabilities`）：客户端固定显示「服务端待升级」，禁用上传/列表，本地发现、打包、原生文件导出不受影响。
- 旧客户端连新服务端：不影响（新端点独立，旧同步通道不动）。
