# AgentHUD 子代理与 Skill 管理实施方案

调研日期：2026-11-09

## 首期交付

在桌面仪表盘增加「Agent 与 Skill」页，覆盖 ZCode、Codex、Claude Code、Kimi Code、DeepSeek Harness、MiniMax Code、OpenCode。用户选择工具和作用域，从本地配置或手选原生文件导入，预览后存入现有自托管服务器。另一台设备可以浏览资源、下载原生文件或文件包，再按目标工具支持的方式恢复。用户级和项目级分别展示，项目级导出时重新选择目标项目。

把「下载原生文件」与「安装到本机」做成两个操作。经实测支持安全写入的格式可提供显式安装。需要工具注册的格式导出到中立目录，并给出该版本验证过的注册步骤。首期限定同工具、同格式往返。跨工具转换、后台自动同步、自动执行、插件市场、配置编辑器及移动端入口放在后续。

## 官方格式与适配范围

下表中的作用域和写入方式是首期建议。实现者须以目标发行版本复核目录、格式、优先级和重载方式，记录测试的确切版本、平台与官方链接。

| 工具 | 子代理 | Skill | 实施要点与依据 |
| --- | --- | --- | --- |
| ZCode | 用户级 `~/.zcode/agents/<name>.md`，YAML frontmatter 和 Markdown 提示词 | 用户级 `~/.zcode/skills/<name>/SKILL.md`，官方还提供项目级导入 | 子代理首期只做用户级。项目 Skill 先用实际版本生成一个样例验证目录，再开放安装。核对内置保留名及设置页刷新行为。 [子代理](https://zcode.z.ai/en/docs/subagents) · [Skill](https://zcode.z.ai/en/docs/skill) |
| Codex | 用户 `~/.codex/agents/`、项目 `.codex/agents/`，独立 TOML，必填 `name`、`description`、`developer_instructions` | 用户和项目的 `.agents/skills/`。项目发现范围从工作目录到仓库根 | 确认 `CODEX_HOME` 下的 Agent 解析。Skill 保留附属资源及 `agents/openai.yaml`。内置 Agent 可被同名自定义 Agent 覆盖，安装预览须显示。 [子代理](https://learn.chatgpt.com/docs/agent-configuration/subagents#custom-agents) · [Skill](https://learn.chatgpt.com/docs/build-skills#where-codex-loads-local-skills) |
| Claude Code | 用户 `~/.claude/agents/`、项目 `.claude/agents/`，递归 Markdown，frontmatter 的 `name` 决定身份 | 用户 `~/.claude/skills/`、项目 `.claude/skills/` | 保留 `tools`、`permissionMode`、`hooks` 等字段，安装前展示敏感配置。保留子目录，并检测不同文件声明同一个 Agent 名的情况。 [子代理](https://code.claude.com/docs/en/sub-agents) · [Skill](https://code.claude.com/docs/en/skills) |
| Kimi Code CLI | 当前版为 Markdown，用户 `$KIMI_CODE_HOME/agents/`、项目 `.kimi-code/agents/`，也有共享 `.agents/agents/` | 用户 `$KIMI_CODE_HOME/skills/`、项目 `.kimi-code/skills/`，也有共享 `.agents/skills/` | 项目现有接入读取 `.kimi-code`，服务端 ID 用 `kimi`，界面显示 Kimi Code。旧 kimi-cli 的 YAML 和引用文件单独标为旧格式，先做归档，按需验证旧版恢复。保留 `override`、`whenToUse` 和 Skill 类型。 [当前子代理](https://www.kimi.com/code/docs/en/kimi-code-cli/customization/agents.html) · [当前 Skill](https://www.kimi.com/code/docs/en/kimi-code-cli/customization/skills.html) · [旧版 Agent](https://moonshotai.github.io/kimi-cli/en/customization/agents.html) |
| DeepSeek Harness | 按 **Agent Preset** 适配。上游当前代码用 Cordis YAML 的 `@deepseek-ai/dsh-agent-preset` 声明和 bundle/profile 安装，spawn 用 `agentPreset` 选它 | 用户 `<dshHome>/skills`、项目 `.dsh/skills`，也读共享 `.agents/skills`。原生支持目录包及平铺 Markdown | 用发布版验证 preset 的导入入口。导出原生声明和已选依赖，展示未打包的插件依赖。将旧目录版与声明版分别标识。预览禁止求值 `!!js`，AgentHUD 只导出供用户按官方流程注册。 [Preset 声明](https://github.com/deepseek-ai/deepseek-harness/blob/fcc0ad4/packages/preset/agent-preset/README.zh.md) · [注册与安装](https://github.com/deepseek-ai/deepseek-harness/blob/fcc0ad4/packages/preset/agent-preset-registry/README.zh.md) · [Skill](https://github.com/deepseek-ai/deepseek-harness/blob/master/docs/subsystems/skills.md) |
| MiniMax Code | 官方源码预览存在原生 `agent.md` 配置文档，也有从 Claude/Codex 导入并预览转换的代码。目标发行版的原生导出、重新注册入口待验证 | 官方 CLI 确认项目 `.minimax/skills/`、用户 `~/.agents/skills/`，另读兼容根 | Skill 可先交付。子代理先做已确认样例的原样归档与下载，界面标出「安装待验证」。以官方发行版实测后再开安装。Desktop 和 CLI 分别记录，不从会话库推导配置路径。 [官方 CLI 与源码范围](https://github.com/MiniMax-AI/minimax-code/blob/3c21163/README.md) · [Agent 导入实现](https://github.com/MiniMax-AI/minimax-code/blob/3c21163/packages/local-runtime-v2/src/service/agent/application/agent-import.ts) · [Skill 目录](https://github.com/MiniMax-AI/mcode-stack) |
| OpenCode | Markdown 与 JSON/JSONC 内联定义。V2 文档用 `agents`，旧版常见 `agent`，按版本识别。Markdown 支持用户配置目录和项目 `.opencode/agents/` | 用户配置目录、项目 `.opencode/skills/`，也读 Claude 和共享目录。V2 另有平铺 Markdown | Markdown 优先。内联定义只提取选中的条目，原生导出为独立 JSONC 片段。首期由用户合并内联片段，避免改写整个配置。测试嵌套 ID 与 V1/V2 权限差异。 [Agent](https://opencode.ai/docs/agents/) · [Skill](https://opencode.ai/docs/skills/) · [V1 迁移](https://opencode.ai/docs/migrate-v1/) |

## 共用接口与存储

- **适配器在 Rust/Tauri。** 独立放在 `src-tauri/src/agent_assets/`。复用项目现有标识，避免把配置导入加入 `local_agents.rs` 的用量来源接口。每个适配器提供 `capabilities`、`discover`、`inspect`、`plan_export`。共用层处理预览、打包、路径检查和安装。能力按工具、资源类型、版本、作用域分别声明，区分「可扫描」「可归档」「可原生导出」「可安装」。前端按能力显示动作。
- **资源包以原文件为准。** 用版本化清单与普通文件内容传输。清单含 `schema_version`、`tool`、`kind`、`native_format`、`native_id`、显示名、描述、原作用域、适用版本、入口相对路径、文件大小和 SHA-256。每个资源独立打包。保留未知字段、换行、二进制附件和脚本的可执行标记。仅记录包内相对路径，跨设备恢复时由适配器解析新根目录。解析结果用于索引和预览，原文件不经重新序列化。内联配置以提取出的条目为资源包，明确标记这一例外。外部引用只报告，禁止递归读取包外路径。
- **SQLite 保存目录与版本内容。** 复用 Flask、`server/db.py` 的迁移方式和设备鉴权，增加独立的资源及版本表。小规模自托管首版可直接存 BLOB，每次写入用一个事务。按服务器分配的资源 ID 管理，同名项目和作用域可共存。文件清单和内容按确定顺序计算摘要，由服务端复验。同一资源重复提交相同内容返回已有结果。更新已有资源要携带预期版本，冲突返回 409，允许重新预览或另存。首次创建用客户端请求 ID 去重。保留旧版本供下载。
- **API 单独版本化。** 建议 `GET /agent-assets/capabilities`、`GET /agent-assets`、`POST /agent-assets`、`PUT /agent-assets/<id>`、`GET /agent-assets/<id>/revisions`、`GET /agent-assets/<id>/revisions/<rev>/bundle`。列表只返元数据、支持筛选和分页。设备身份以服务端鉴权结果为准。延用自托管单用户模型，已注册设备可读写同一资源库。客户端复用 `load_sync_config` 的地址和凭证，旧服务器缺少能力接口时提示升级，仍可下载本地原生文件。单独定义保留与备份行为，验收用量清理和设备撤销对资源库的影响。
- **操作边界。** 上传预览展示实际文件清单及敏感字段提示，只采集选中的 Agent 定义和 Skill 包。拒绝凭据文件、私钥、穿越路径、绝对路径、Windows 设备名及路径别名、大小写碰撞、符号链接与 junction。服务端限制请求体、条目数、单包及总库容量。若接收压缩包，另限制解压后大小，并在提取前验证所有条目。远端内容按纯文本预览，服务端不执行。跨设备传输使用 HTTPS，允许回环本地开发。安装重新校验路径和预览时的文件指纹，默认跳过冲突，显式覆盖才备份并写入。用同卷暂存与原子发布处理单资源，失败时保留或恢复原内容。共享目录安装须提示影响其他工具。

## 分派和合并

| 工作包 | 负责人交付 | 依赖 |
| --- | --- | --- |
| A · 公共契约与桌面骨架 | 资源包和错误码、能力表、适配器签名、共用导出及安装边界、一份可往返的样例 | 先提交契约，适配器各用独立模块 |
| B · 服务器 | 数据表迁移、版本化 API、鉴权、幂等与并发测试、容量与备份说明 | A 的契约，可与适配器并行 |
| C · 第一批适配器 | Codex、Claude、ZCode 各一个独立 PR，覆盖 TOML、Markdown 和用户级限定 | A 的契约 |
| D · 第二批适配器 | Kimi、OpenCode 各一个独立 PR，分别提交当前与旧版本样例，覆盖共享目录与内联配置 | A 的契约 |
| E · 发行版验证与适配 | DSH、MiniMax 分别提交验证报告及独立 PR，优先完成 Skill，明确哪些子代理格式可归档、可导出、可恢复 | A 的契约。原生恢复入口验证后才启用安装 |
| F · 界面与联调 | 仪表盘独立入口、中英文字串、作用域切换、预览、上传、下载、冲突提示和升级状态 | 先对契约开发，再接 B 与适配器 |

由 A/F 的集成负责人统一接入 `src-tauri/src/lib.rs`、`src/api.ts`、`src/dashboard/DashboardApp.tsx`，避免各适配器同时改公共入口。先合并 A，再合并 B/C/D/E，最后接通 F 并发布。每个适配器 PR 附官方链接、目标工具和平台版本、发现根和优先级、支持矩阵、最小样例、恢复步骤与工具实际识别的证据。

## 最终审查清单

1. **两设备往返。** A 从用户级和受支持的项目级各导入一个子代理和一个带脚本、引用及二进制附件的 Skill。服务器重启后 B 仍可下载。原生文件和包内相对路径一致，支持安装的资源能被目标工具列出并调用。用只读或无副作用样例验证。
2. **格式与版本。** 覆盖 CRLF、UTF-8、未知字段、同名资源、嵌套项目、共享目录、版本不兼容和缺失依赖。重点抽查 Kimi 新旧区分、OpenCode JSONC 单条提取、DSH preset 注册、MiniMax Desktop 与 CLI 的能力区分。
3. **失败与并发。** 断网重试、同一请求重放、两个设备同时改同一版本、预览后目标被修改、安装中断、超限包、恶意路径和软链接。确认原有文件可恢复，鉴权失败及被撤销设备无法访问资源。
4. **现有功能回归。** 旧数据库自动升级、旧客户端正常同步、新客户端连接旧服务器有明确状态。运行服务器现有用例、Rust 适配器用例和桌面构建，复核用量清理及价格同步。

发布口径分别报告七款工具的归档、原生导出和安装能力。DSH 或 MiniMax 若缺少目标发行版的恢复证据，相关子代理仅标为已验证的归档或文件导出能力。
