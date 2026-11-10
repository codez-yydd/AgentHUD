/**
 * 主窗口「Agent 与 Skill」页词典：服务端状态 / 工具与作用域选择 / 本地资源
 * 检查与预览 / 上传与版本冲突 / 远端资源库 / 下载与安装。
 *
 * 键名统一 `dashboard.agentAssets.*`；中英同文件导出（en.ts 以 mapped type
 * 保证键集一致）。错误文案里 `{error}` / `{code}` 为插值变量，后端返回的
 * message 原样放在插值位，不翻译。
 */

export const agentAssets = {
  // ===== 页面标题与说明 =====
  "dashboard.agentAssets": "Agent 与 Skill",
  "dashboard.agentAssets.desc":
    "在本机发现子代理与 Skill，预览文件清单后上传到同步服务；也可浏览其他设备的资源，下载原生文件或安装到本机。",

  // ===== 服务端状态卡 =====
  "dashboard.agentAssets.server.title": "服务端资源库",
  "dashboard.agentAssets.server.checking": "正在检测服务端状态…",
  "dashboard.agentAssets.server.supported":
    "服务端已支持 Agent 资源接口（API v{version}）",
  "dashboard.agentAssets.server.limits":
    "限制：单文件 {file} · 单包 {bundle} · 请求体 {request} · 单包条目 {entries} · 单资源保留 {revisions} 版 · 资源库总容量 {quota}",
  "dashboard.agentAssets.server.configMissing":
    "本机尚未注册同步设备，无法访问资源库。请先到「同步与设备」完成设备注册，再回来重试。",
  "dashboard.agentAssets.server.oldServer":
    "服务端待升级：当前服务端没有 Agent 资源接口，上传与远端列表已禁用；本地发现、打包与原生导出仍可用。",
  "dashboard.agentAssets.server.unauthorized":
    "设备凭证无效或已被撤销，请到「同步与设备」重新注册设备。",
  "dashboard.agentAssets.server.network":
    "无法连接服务端，请检查同步服务配置与网络后重试。",
  "dashboard.agentAssets.server.unknown": "服务端状态未知，远端功能暂不可用。",
  "dashboard.agentAssets.server.detail": "详情：{detail}",
  "dashboard.agentAssets.server.localOnly": "本地功能可用",
  "dashboard.agentAssets.server.retry": "重试",
  "dashboard.agentAssets.server.insecureTransport":
    "当前同步地址使用非回环的明文 HTTP，设备凭证与资源内容可能被窃听或篡改；建议改用 HTTPS。",

  // ===== 工具与作用域选择 =====
  "dashboard.agentAssets.select.title": "工具与作用域",
  "dashboard.agentAssets.select.tool": "工具",
  "dashboard.agentAssets.select.kind": "类型",
  "dashboard.agentAssets.select.scope": "作用域",
  "dashboard.agentAssets.select.kindAgent": "子代理",
  "dashboard.agentAssets.select.kindSkill": "Skill",
  "dashboard.agentAssets.select.scopeUser": "用户级",
  "dashboard.agentAssets.select.scopeProject": "项目级",
  "dashboard.agentAssets.select.projectRoot": "项目根路径",
  "dashboard.agentAssets.select.projectRootPlaceholder":
    "输入项目根目录的绝对路径（可从已有项目中选择）",
  "dashboard.agentAssets.select.chooseDir": "选择目录",
  "dashboard.agentAssets.select.chooseDirFailed":
    "目录选择器不可用，请直接输入绝对路径。",
  "dashboard.agentAssets.select.scan": "扫描",
  "dashboard.agentAssets.select.scanning": "扫描中…",
  "dashboard.agentAssets.select.pending": "待验证",
  "dashboard.agentAssets.select.pendingHint":
    "该工具首期仅占位，待发行版验证后开放。",
  "dashboard.agentAssets.select.unsupported":
    "该 工具 × 类型 × 作用域 组合首期不支持。",
  "dashboard.agentAssets.select.notScannable": "该组合不支持扫描。",
  "dashboard.agentAssets.select.projectRootRequired":
    "项目级资源需要项目根路径，请先填写后再扫描。",
  "dashboard.agentAssets.select.notes": "说明",
  "dashboard.agentAssets.select.capabilities": "能力",
  "dashboard.agentAssets.select.capScannable": "可扫描",
  "dashboard.agentAssets.select.capArchivable": "可归档",
  "dashboard.agentAssets.select.capNativeExportable": "可原生导出",
  "dashboard.agentAssets.select.capInstallable": "可安装",
  "dashboard.agentAssets.select.capOffHint": "该能力当前不可用",

  // ===== 本机资源 =====
  "dashboard.agentAssets.local.title": "本机资源",
  "dashboard.agentAssets.local.sub": "共 {n} 项",
  "dashboard.agentAssets.local.empty": "未发现资源。",
  "dashboard.agentAssets.local.emptyHint":
    "确认工具与作用域后重试；也可能该工具尚未安装或未配置资源。",
  "dashboard.agentAssets.local.loading": "正在扫描本机资源…",
  "dashboard.agentAssets.local.error": "扫描失败：{error}",
  "dashboard.agentAssets.local.adapterMissing":
    "该工具的适配器尚未合并（C/D/E 工作包进行中），当前仅 ZCode 可用。",
  "dashboard.agentAssets.local.format": "格式：{format}",
  "dashboard.agentAssets.local.sourceRoot": "位置",
  "dashboard.agentAssets.local.warnings": "{n} 条警告",
  "dashboard.agentAssets.local.expand": "查看详情",
  "dashboard.agentAssets.local.collapse": "收起详情",
  "dashboard.agentAssets.local.inspectLoading": "正在读取详情…",
  "dashboard.agentAssets.local.inspectError": "读取详情失败：{error}",
  "dashboard.agentAssets.local.metadata": "元数据",
  "dashboard.agentAssets.local.metaName": "名称",
  "dashboard.agentAssets.local.metaDescription": "描述",
  "dashboard.agentAssets.local.metaModel": "模型",
  "dashboard.agentAssets.local.metaTools": "工具",
  "dashboard.agentAssets.local.parseFailed":
    "frontmatter 解析失败：{error}（归档仍按原文件字节进行）",
  "dashboard.agentAssets.local.files": "文件清单（{n}）",
  "dashboard.agentAssets.local.binary": "二进制文件",
  "dashboard.agentAssets.local.executable": "可执行",
  "dashboard.agentAssets.local.frontmatter": "frontmatter 原文",
  "dashboard.agentAssets.local.noFrontmatter": "无 frontmatter",
  "dashboard.agentAssets.local.sensitive": "敏感字段（{n}）",
  "dashboard.agentAssets.local.sensitiveHint":
    "以下字段可能包含凭据或敏感配置，上传前请确认。",
  "dashboard.agentAssets.local.externalRefs": "包外引用（{n}，不会打包）",
  "dashboard.agentAssets.local.warningsTitle": "警告",
  "dashboard.agentAssets.local.actionPlan": "打包预览",
  "dashboard.agentAssets.local.actionUpload": "上传",
  "dashboard.agentAssets.local.actionExport": "导出到本机目录",
  "dashboard.agentAssets.local.actionPlanBusy": "生成预览…",
  "dashboard.agentAssets.local.actionUploadBusy": "准备上传…",
  "dashboard.agentAssets.local.actionExportBusy": "准备导出…",

  // ===== 手选原生文件/目录导入（契约 §7.3）=====
  "dashboard.agentAssets.import.action": "手选文件/目录导入",
  "dashboard.agentAssets.import.pickAgentTitle": "选择子代理文件",
  "dashboard.agentAssets.import.pickSkillTitle": "选择 Skill 目录",
  "dashboard.agentAssets.import.agentFilter": "子代理文件",
  "dashboard.agentAssets.import.pickFailed": "无法打开文件选择器：{error}",
  "dashboard.agentAssets.import.blockedPending":
    "该工具/类型首期仅占位，暂不支持手选导入。",
  "dashboard.agentAssets.import.blockedUnsupported":
    "该工具/类型首期不支持手选导入。",
  "dashboard.agentAssets.import.loading": "正在解析手选文件…",
  "dashboard.agentAssets.import.busy": "解析中…",
  "dashboard.agentAssets.import.title": "手选导入结果",
  "dashboard.agentAssets.import.sub": "可导入 {ok} / 共 {total}",
  "dashboard.agentAssets.import.scopeHint":
    "手选导入固定按用户级记录；可导入项可继续打包预览、上传或导出。",
  "dashboard.agentAssets.import.repick": "重新选择",
  "dashboard.agentAssets.import.clear": "清空结果",
  "dashboard.agentAssets.import.notImportable": "不可导入",
  "dashboard.agentAssets.import.path": "手选路径",
  "dashboard.agentAssets.import.empty": "未返回任何候选。",

  // ===== 导出计划 / 上传确认 / 导出弹层 =====
  "dashboard.agentAssets.plan.title": "打包预览",
  "dashboard.agentAssets.plan.uploadTitle": "上传确认",
  "dashboard.agentAssets.plan.exportTitle": "导出到本机目录",
  "dashboard.agentAssets.plan.subtitle": "{tool} · {kind} · {name}",
  "dashboard.agentAssets.plan.entry": "入口文件",
  "dashboard.agentAssets.plan.sourceRoot": "来源目录",
  "dashboard.agentAssets.plan.files": "将打包的文件（{n}）",
  "dashboard.agentAssets.plan.fileSize": "大小",
  "dashboard.agentAssets.plan.colPath": "路径",
  "dashboard.agentAssets.plan.colFlags": "标记",
  "dashboard.agentAssets.plan.targetVersions": "适用版本",
  "dashboard.agentAssets.plan.sensitive":
    "检测到 {n} 个敏感字段，可能包含凭据或私密配置：",
  "dashboard.agentAssets.plan.external":
    "包含 {n} 个包外引用，不会被打包，仅在此提示：",
  "dashboard.agentAssets.plan.warnings": "警告：",
  "dashboard.agentAssets.plan.target": "上传目标",
  "dashboard.agentAssets.plan.createNew": "新建资源（版本 1）",
  "dashboard.agentAssets.plan.updateTo":
    "更新「{name}」到 v{next}（当前 v{current}）",
  "dashboard.agentAssets.plan.matchError":
    "无法检查同名远端资源：{error}",
  "dashboard.agentAssets.plan.uploadHint":
    "确认后会重新打包当前文件并上传；断网重试将复用同一请求 ID。",
  "dashboard.agentAssets.plan.confirmUpload": "确认上传",
  "dashboard.agentAssets.plan.uploading": "打包并上传中…",
  "dashboard.agentAssets.plan.retryUpload": "重试上传",
  "dashboard.agentAssets.plan.exportHint":
    "确认后会打包当前文件并原样写入目标目录；默认跳过已存在的文件。",
  "dashboard.agentAssets.plan.destDir": "目标目录",
  "dashboard.agentAssets.plan.destDirPlaceholder": "输入绝对路径",
  "dashboard.agentAssets.plan.destDirRequired": "请填写目标目录。",
  "dashboard.agentAssets.plan.overwrite":
    "覆盖已存在的文件（覆盖前会备份）",
  "dashboard.agentAssets.plan.confirmExport": "开始导出",
  "dashboard.agentAssets.plan.exporting": "打包并导出中…",
  "dashboard.agentAssets.plan.close": "关闭",
  "dashboard.agentAssets.plan.uploadSuccess":
    "上传成功：{name} v{version}",
  "dashboard.agentAssets.plan.replayed": "幂等重放",
  "dashboard.agentAssets.plan.deduplicated": "内容去重",
  "dashboard.agentAssets.plan.exportSuccess": "已写入 {n} 个文件",
  "dashboard.agentAssets.plan.skipped": "跳过 {n} 个文件",
  "dashboard.agentAssets.plan.backupDir": "备份目录：{dir}",
  "dashboard.agentAssets.plan.assetId": "资源 ID：{id}",
  "dashboard.agentAssets.plan.conflictTitle": "版本冲突",
  "dashboard.agentAssets.plan.conflictDesc":
    "服务端当前版本为 v{current}，与预览时的 v{expected} 不一致。",
  "dashboard.agentAssets.plan.rePreview": "重新预览后再试",
  "dashboard.agentAssets.plan.saveAsNew": "另存为新资源",
  "dashboard.agentAssets.plan.contentChanged":
    "资源内容已变化（文件清单或大小与预览时不一致），请重新预览后再上传。",

  // ===== 远端资源库 =====
  "dashboard.agentAssets.remote.title": "远端资源库",
  "dashboard.agentAssets.remote.sub": "共 {n} 项",
  "dashboard.agentAssets.remote.disabled":
    "服务端未就绪，远端资源库暂不可用。",
  "dashboard.agentAssets.remote.allTools": "全部工具",
  "dashboard.agentAssets.remote.allKinds": "全部类型",
  "dashboard.agentAssets.remote.allScopes": "全部作用域",
  "dashboard.agentAssets.remote.searchPlaceholder":
    "搜索 native_id 或显示名",
  "dashboard.agentAssets.remote.refresh": "刷新列表",
  "dashboard.agentAssets.remote.updateHint":
    "更新已有资源：在上方选择同工具/类型/作用域的本机资源后点「上传」，在预览中选择要更新的远端资源。",
  "dashboard.agentAssets.remote.loading": "正在加载远端资源…",
  "dashboard.agentAssets.remote.empty": "没有匹配的资源。",
  "dashboard.agentAssets.remote.error": "加载失败：{error}",
  "dashboard.agentAssets.remote.version": "v{version}",
  "dashboard.agentAssets.remote.entries": "{n} 个文件",
  "dashboard.agentAssets.remote.updated": "更新于 {time}",
  "dashboard.agentAssets.remote.history": "版本历史",
  "dashboard.agentAssets.remote.historyLoading": "正在加载版本历史…",
  "dashboard.agentAssets.remote.historyEmpty": "暂无版本记录。",
  "dashboard.agentAssets.remote.historyError":
    "版本历史加载失败：{error}",
  "dashboard.agentAssets.remote.colVersion": "版本",
  "dashboard.agentAssets.remote.colSha": "指纹（SHA-256）",
  "dashboard.agentAssets.remote.colFiles": "文件",
  "dashboard.agentAssets.remote.colCreated": "创建时间",
  "dashboard.agentAssets.remote.colDevice": "设备",
  "dashboard.agentAssets.remote.downloadThis": "下载此版本",
  "dashboard.agentAssets.remote.download": "下载",
  "dashboard.agentAssets.remote.install": "安装到本机",
  "dashboard.agentAssets.remote.installDisabled":
    "该工具/格式首期不支持自动安装，请下载原生文件后手动安装。",
  "dashboard.agentAssets.remote.installFormatDisabled":
    "该原生格式（内联配置 / 旧版 YAML）不支持自动安装：请下载原生文件后手动合并回配置，或按旧版方式显式加载。",
  "dashboard.agentAssets.remote.installBadge": "安装待验证",
  "dashboard.agentAssets.remote.exportDisabled":
    "该工具/格式首期不支持原生导出。",
  "dashboard.agentAssets.remote.pagePrev": "上一页",
  "dashboard.agentAssets.remote.pageNext": "下一页",
  "dashboard.agentAssets.remote.pageInfo": "第 {page} / {total} 页",
  "dashboard.agentAssets.remote.pageTotal": "共 {n} 条",

  // ===== 下载 / 安装弹层 =====
  "dashboard.agentAssets.transfer.downloadTitle": "下载资源",
  "dashboard.agentAssets.transfer.installTitle": "安装到本机",
  "dashboard.agentAssets.transfer.subtitle": "{name} v{version}",
  "dashboard.agentAssets.transfer.fetching": "正在取回资源包…",
  "dashboard.agentAssets.transfer.fetchError":
    "取回资源包失败：{error}",
  "dashboard.agentAssets.transfer.fingerprint": "资源包指纹",
  "dashboard.agentAssets.transfer.manifestSha": "清单 SHA-256",
  "dashboard.agentAssets.transfer.bundleSha": "内容 SHA-256",
  "dashboard.agentAssets.transfer.files": "资源包文件（{n}）",
  "dashboard.agentAssets.transfer.downloadHint":
    "下载会把资源包原样还原为原生文件/文件包，不执行任何内容。",
  "dashboard.agentAssets.transfer.installHint":
    "安装会校验路径与文件指纹后写入目标作用域；默认跳过冲突，覆盖前会备份。共享目录安装可能影响其他工具。",
  "dashboard.agentAssets.transfer.unverifiedHint":
    "该工具能力按官方文档实现，未在本机实测目标工具版本，安装后请在目标工具中确认识别。",
  "dashboard.agentAssets.transfer.installScope": "安装作用域",
  "dashboard.agentAssets.transfer.projectRoot": "目标项目根路径",
  "dashboard.agentAssets.transfer.projectRootRequired":
    "项目级安装需要目标项目根路径。",
  "dashboard.agentAssets.transfer.confirmDownload": "下载并导出",
  "dashboard.agentAssets.transfer.downloading": "下载并导出中…",
  "dashboard.agentAssets.transfer.confirmInstall": "确认安装",
  "dashboard.agentAssets.transfer.installing": "安装中…",
  "dashboard.agentAssets.transfer.installSuccess": "已安装 {n} 个文件",
  "dashboard.agentAssets.transfer.toolInstructions": "目标工具注册步骤",
  "dashboard.agentAssets.transfer.skipped": "跳过 {n} 个文件",
  "dashboard.agentAssets.transfer.backupDir": "备份目录：{dir}",
  "dashboard.agentAssets.transfer.exportSuccess": "已写入 {n} 个文件",
  "dashboard.agentAssets.transfer.commandUnavailable":
    "该操作所需的命令尚未提供（安装/导出适配器开发中）：{error}",
  "dashboard.agentAssets.transfer.close": "关闭",

  // ===== 统一错误提示（按 e.code 分支）=====
  "dashboard.agentAssets.error.unauthorized":
    "设备凭证无效或已被撤销，请到「同步与设备」重新注册。",
  "dashboard.agentAssets.error.oldServer":
    "服务端待升级，上传与远端列表不可用。",
  "dashboard.agentAssets.error.network":
    "网络连接失败，请检查同步服务地址与网络后重试。",
  "dashboard.agentAssets.error.adapter":
    "该工具的适配器尚未合并（C/D/E 工作包进行中），当前仅 ZCode 可用。",
  "dashboard.agentAssets.error.retry": "重试",
  "dashboard.agentAssets.error.code": "错误码：{code}",

  // ===== 页脚安全说明 =====
  "dashboard.agentAssets.footnote":
    "安全说明：上传前会展示实际文件清单与敏感字段；凭据/私钥、路径穿越、软链接等风险在预览与打包时拦截；安装默认跳过冲突，覆盖前会备份。",
};

/** 英文词典：键集由 mapped type 与中文强制一致。 */
export const agentAssetsEn: { [K in keyof typeof agentAssets]: string } = {
  "dashboard.agentAssets": "Agents & Skills",
  "dashboard.agentAssets.desc":
    "Discover subagents and skills on this device, review the file list, then upload them to the sync server. You can also browse assets from other devices and download native files or install them locally.",

  "dashboard.agentAssets.server.title": "Server library",
  "dashboard.agentAssets.server.checking": "Checking server status…",
  "dashboard.agentAssets.server.supported":
    "Server supports the agent-assets API (API v{version})",
  "dashboard.agentAssets.server.limits":
    "Limits: per file {file} · per bundle {bundle} · request {request} · {entries} entries per bundle · {revisions} revisions per asset · {quota} library quota",
  "dashboard.agentAssets.server.configMissing":
    "No sync device is registered on this machine, so the library is unavailable. Register a device on the Sync & Devices page, then try again.",
  "dashboard.agentAssets.server.oldServer":
    "Server upgrade required: this server has no agent-assets API, so upload and remote listing are disabled. Local discovery, packing and native export still work.",
  "dashboard.agentAssets.server.unauthorized":
    "The device credential is invalid or revoked. Re-register the device on the Sync & Devices page.",
  "dashboard.agentAssets.server.network":
    "Cannot reach the server. Check the sync server settings and network, then retry.",
  "dashboard.agentAssets.server.unknown":
    "Server status is unknown; remote features are unavailable.",
  "dashboard.agentAssets.server.detail": "Details: {detail}",
  "dashboard.agentAssets.server.localOnly": "Local features available",
  "dashboard.agentAssets.server.retry": "Retry",
  "dashboard.agentAssets.server.insecureTransport":
    "The sync server uses plain HTTP on a non-loopback host. Device credentials and asset contents may be intercepted or tampered with; switching to HTTPS is recommended.",

  "dashboard.agentAssets.select.title": "Tool and scope",
  "dashboard.agentAssets.select.tool": "Tool",
  "dashboard.agentAssets.select.kind": "Type",
  "dashboard.agentAssets.select.scope": "Scope",
  "dashboard.agentAssets.select.kindAgent": "Subagent",
  "dashboard.agentAssets.select.kindSkill": "Skill",
  "dashboard.agentAssets.select.scopeUser": "User",
  "dashboard.agentAssets.select.scopeProject": "Project",
  "dashboard.agentAssets.select.projectRoot": "Project root",
  "dashboard.agentAssets.select.projectRootPlaceholder":
    "Absolute path of the project root (or pick a known project)",
  "dashboard.agentAssets.select.chooseDir": "Choose folder",
  "dashboard.agentAssets.select.chooseDirFailed":
    "The folder picker is unavailable; enter an absolute path directly.",
  "dashboard.agentAssets.select.scan": "Scan",
  "dashboard.agentAssets.select.scanning": "Scanning…",
  "dashboard.agentAssets.select.pending": "Pending",
  "dashboard.agentAssets.select.pendingHint":
    "This tool is a placeholder until its release build is verified.",
  "dashboard.agentAssets.select.unsupported":
    "This tool × type × scope combination is not supported yet.",
  "dashboard.agentAssets.select.notScannable":
    "This combination cannot be scanned.",
  "dashboard.agentAssets.select.projectRootRequired":
    "Project-scope assets require a project root. Enter it before scanning.",
  "dashboard.agentAssets.select.notes": "Notes",
  "dashboard.agentAssets.select.capabilities": "Capabilities",
  "dashboard.agentAssets.select.capScannable": "Scan",
  "dashboard.agentAssets.select.capArchivable": "Archive",
  "dashboard.agentAssets.select.capNativeExportable": "Native export",
  "dashboard.agentAssets.select.capInstallable": "Install",
  "dashboard.agentAssets.select.capOffHint":
    "This capability is currently unavailable",

  "dashboard.agentAssets.local.title": "Local assets",
  "dashboard.agentAssets.local.sub": "{n} found",
  "dashboard.agentAssets.local.empty": "No assets found.",
  "dashboard.agentAssets.local.emptyHint":
    "Check the tool and scope, then retry. The tool may not be installed or may have no assets configured.",
  "dashboard.agentAssets.local.loading": "Scanning local assets…",
  "dashboard.agentAssets.local.error": "Scan failed: {error}",
  "dashboard.agentAssets.local.adapterMissing":
    "The adapter for this tool is not merged yet (work packages C/D/E). Only ZCode is available now.",
  "dashboard.agentAssets.local.format": "Format: {format}",
  "dashboard.agentAssets.local.sourceRoot": "Location",
  "dashboard.agentAssets.local.warnings": "{n} warnings",
  "dashboard.agentAssets.local.expand": "View details",
  "dashboard.agentAssets.local.collapse": "Hide details",
  "dashboard.agentAssets.local.inspectLoading": "Loading details…",
  "dashboard.agentAssets.local.inspectError":
    "Failed to load details: {error}",
  "dashboard.agentAssets.local.metadata": "Metadata",
  "dashboard.agentAssets.local.metaName": "Name",
  "dashboard.agentAssets.local.metaDescription": "Description",
  "dashboard.agentAssets.local.metaModel": "Model",
  "dashboard.agentAssets.local.metaTools": "Tools",
  "dashboard.agentAssets.local.parseFailed":
    "Frontmatter parsing failed: {error} (archiving still uses the raw file bytes)",
  "dashboard.agentAssets.local.files": "Files ({n})",
  "dashboard.agentAssets.local.binary": "Binary file",
  "dashboard.agentAssets.local.executable": "Executable",
  "dashboard.agentAssets.local.frontmatter": "Raw frontmatter",
  "dashboard.agentAssets.local.noFrontmatter": "No frontmatter",
  "dashboard.agentAssets.local.sensitive": "Sensitive fields ({n})",
  "dashboard.agentAssets.local.sensitiveHint":
    "These fields may contain credentials or sensitive configuration. Review them before uploading.",
  "dashboard.agentAssets.local.externalRefs":
    "External references ({n}, not packaged)",
  "dashboard.agentAssets.local.warningsTitle": "Warnings",
  "dashboard.agentAssets.local.actionPlan": "Preview package",
  "dashboard.agentAssets.local.actionUpload": "Upload",
  "dashboard.agentAssets.local.actionExport": "Export to folder",
  "dashboard.agentAssets.local.actionPlanBusy": "Building preview…",
  "dashboard.agentAssets.local.actionUploadBusy": "Preparing upload…",
  "dashboard.agentAssets.local.actionExportBusy": "Preparing export…",

  "dashboard.agentAssets.import.action": "Import files/folders",
  "dashboard.agentAssets.import.pickAgentTitle": "Select subagent files",
  "dashboard.agentAssets.import.pickSkillTitle": "Select skill folders",
  "dashboard.agentAssets.import.agentFilter": "Subagent files",
  "dashboard.agentAssets.import.pickFailed": "Cannot open the picker: {error}",
  "dashboard.agentAssets.import.blockedPending":
    "This tool/type is a placeholder; hand-picked import is unavailable.",
  "dashboard.agentAssets.import.blockedUnsupported":
    "Hand-picked import is not supported for this tool/type yet.",
  "dashboard.agentAssets.import.loading": "Parsing selected files…",
  "dashboard.agentAssets.import.busy": "Parsing…",
  "dashboard.agentAssets.import.title": "Import results",
  "dashboard.agentAssets.import.sub": "{ok} importable of {total}",
  "dashboard.agentAssets.import.scopeHint":
    "Hand-picked imports are always recorded at user scope. Importable items can be previewed, uploaded or exported next.",
  "dashboard.agentAssets.import.repick": "Select again",
  "dashboard.agentAssets.import.clear": "Clear results",
  "dashboard.agentAssets.import.notImportable": "Not importable",
  "dashboard.agentAssets.import.path": "Selected path",
  "dashboard.agentAssets.import.empty": "No candidates returned.",

  "dashboard.agentAssets.plan.title": "Package preview",
  "dashboard.agentAssets.plan.uploadTitle": "Confirm upload",
  "dashboard.agentAssets.plan.exportTitle": "Export to folder",
  "dashboard.agentAssets.plan.subtitle": "{tool} · {kind} · {name}",
  "dashboard.agentAssets.plan.entry": "Entry file",
  "dashboard.agentAssets.plan.sourceRoot": "Source folder",
  "dashboard.agentAssets.plan.files": "Files to package ({n})",
  "dashboard.agentAssets.plan.fileSize": "Size",
  "dashboard.agentAssets.plan.colPath": "Path",
  "dashboard.agentAssets.plan.colFlags": "Flags",
  "dashboard.agentAssets.plan.targetVersions": "Target versions",
  "dashboard.agentAssets.plan.sensitive":
    "{n} sensitive fields detected; they may contain credentials or private configuration:",
  "dashboard.agentAssets.plan.external":
    "{n} external references detected; they are not packaged and are listed for information only:",
  "dashboard.agentAssets.plan.warnings": "Warnings:",
  "dashboard.agentAssets.plan.target": "Upload target",
  "dashboard.agentAssets.plan.createNew": "Create new asset (version 1)",
  "dashboard.agentAssets.plan.updateTo":
    "Update \"{name}\" to v{next} (current v{current})",
  "dashboard.agentAssets.plan.matchError":
    "Cannot check remote assets with the same name: {error}",
  "dashboard.agentAssets.plan.uploadHint":
    "Confirming repacks the current files and uploads them. Network retries reuse the same request ID.",
  "dashboard.agentAssets.plan.confirmUpload": "Confirm upload",
  "dashboard.agentAssets.plan.uploading": "Packing and uploading…",
  "dashboard.agentAssets.plan.retryUpload": "Retry upload",
  "dashboard.agentAssets.plan.exportHint":
    "Confirming packs the current files and writes them unchanged to the target folder. Existing files are skipped by default.",
  "dashboard.agentAssets.plan.destDir": "Target folder",
  "dashboard.agentAssets.plan.destDirPlaceholder": "Enter an absolute path",
  "dashboard.agentAssets.plan.destDirRequired":
    "Enter a target folder first.",
  "dashboard.agentAssets.plan.overwrite":
    "Overwrite existing files (backed up first)",
  "dashboard.agentAssets.plan.confirmExport": "Start export",
  "dashboard.agentAssets.plan.exporting": "Packing and exporting…",
  "dashboard.agentAssets.plan.close": "Close",
  "dashboard.agentAssets.plan.uploadSuccess":
    "Uploaded {name} v{version}",
  "dashboard.agentAssets.plan.replayed": "Idempotent replay",
  "dashboard.agentAssets.plan.deduplicated": "Content deduplicated",
  "dashboard.agentAssets.plan.exportSuccess": "{n} files written",
  "dashboard.agentAssets.plan.skipped": "{n} files skipped",
  "dashboard.agentAssets.plan.backupDir": "Backup folder: {dir}",
  "dashboard.agentAssets.plan.assetId": "Asset ID: {id}",
  "dashboard.agentAssets.plan.conflictTitle": "Version conflict",
  "dashboard.agentAssets.plan.conflictDesc":
    "The server is at v{current}, but this preview was based on v{expected}.",
  "dashboard.agentAssets.plan.rePreview": "Preview again and retry",
  "dashboard.agentAssets.plan.saveAsNew": "Save as new asset",
  "dashboard.agentAssets.plan.contentChanged":
    "The asset content changed (file list or sizes differ from the preview). Preview again before uploading.",

  "dashboard.agentAssets.remote.title": "Remote library",
  "dashboard.agentAssets.remote.sub": "{n} total",
  "dashboard.agentAssets.remote.disabled":
    "The server is not ready; the remote library is unavailable.",
  "dashboard.agentAssets.remote.allTools": "All tools",
  "dashboard.agentAssets.remote.allKinds": "All types",
  "dashboard.agentAssets.remote.allScopes": "All scopes",
  "dashboard.agentAssets.remote.searchPlaceholder":
    "Search native_id or display name",
  "dashboard.agentAssets.remote.refresh": "Refresh list",
  "dashboard.agentAssets.remote.updateHint":
    "To update an existing asset, select the local asset with the same tool/type/scope above and click Upload, then choose the remote target in the preview.",
  "dashboard.agentAssets.remote.loading": "Loading remote assets…",
  "dashboard.agentAssets.remote.empty": "No matching assets.",
  "dashboard.agentAssets.remote.error": "Failed to load: {error}",
  "dashboard.agentAssets.remote.version": "v{version}",
  "dashboard.agentAssets.remote.entries": "{n} files",
  "dashboard.agentAssets.remote.updated": "Updated {time}",
  "dashboard.agentAssets.remote.history": "Version history",
  "dashboard.agentAssets.remote.historyLoading":
    "Loading version history…",
  "dashboard.agentAssets.remote.historyEmpty": "No revisions yet.",
  "dashboard.agentAssets.remote.historyError":
    "Failed to load version history: {error}",
  "dashboard.agentAssets.remote.colVersion": "Version",
  "dashboard.agentAssets.remote.colSha": "Fingerprint (SHA-256)",
  "dashboard.agentAssets.remote.colFiles": "Files",
  "dashboard.agentAssets.remote.colCreated": "Created",
  "dashboard.agentAssets.remote.colDevice": "Device",
  "dashboard.agentAssets.remote.downloadThis": "Download this version",
  "dashboard.agentAssets.remote.download": "Download",
  "dashboard.agentAssets.remote.install": "Install locally",
  "dashboard.agentAssets.remote.installDisabled":
    "Automatic install is not supported for this tool/format yet; download the native files and install manually.",
  "dashboard.agentAssets.remote.installFormatDisabled":
    "Automatic install is not supported for this native format (inline config / legacy YAML). Download the native files and merge them into the config manually, or load them explicitly the legacy way.",
  "dashboard.agentAssets.remote.installBadge": "Install unavailable",
  "dashboard.agentAssets.remote.exportDisabled":
    "Native export is not supported for this tool/format yet.",
  "dashboard.agentAssets.remote.pagePrev": "Previous",
  "dashboard.agentAssets.remote.pageNext": "Next",
  "dashboard.agentAssets.remote.pageInfo": "Page {page} / {total}",
  "dashboard.agentAssets.remote.pageTotal": "{n} items",

  "dashboard.agentAssets.transfer.downloadTitle": "Download asset",
  "dashboard.agentAssets.transfer.installTitle": "Install locally",
  "dashboard.agentAssets.transfer.subtitle": "{name} v{version}",
  "dashboard.agentAssets.transfer.fetching": "Fetching the bundle…",
  "dashboard.agentAssets.transfer.fetchError":
    "Failed to fetch the bundle: {error}",
  "dashboard.agentAssets.transfer.fingerprint": "Bundle fingerprints",
  "dashboard.agentAssets.transfer.manifestSha": "Manifest SHA-256",
  "dashboard.agentAssets.transfer.bundleSha": "Content SHA-256",
  "dashboard.agentAssets.transfer.files": "Bundle files ({n})",
  "dashboard.agentAssets.transfer.downloadHint":
    "Downloading restores the bundle to native files/folders unchanged. Nothing is executed.",
  "dashboard.agentAssets.transfer.installHint":
    "Installing verifies paths and file fingerprints, then writes to the target scope. Conflicts are skipped by default and backed up before overwrite. Installing into a shared folder may affect other tools.",
  "dashboard.agentAssets.transfer.unverifiedHint":
    "This tool's support is implemented from official documentation and has not been verified against a locally installed version. Please confirm that the target tool recognizes the installed asset.",
  "dashboard.agentAssets.transfer.installScope": "Install scope",
  "dashboard.agentAssets.transfer.projectRoot": "Target project root",
  "dashboard.agentAssets.transfer.projectRootRequired":
    "Project-scope install requires a target project root.",
  "dashboard.agentAssets.transfer.confirmDownload": "Download and export",
  "dashboard.agentAssets.transfer.downloading": "Downloading and exporting…",
  "dashboard.agentAssets.transfer.confirmInstall": "Confirm install",
  "dashboard.agentAssets.transfer.installing": "Installing…",
  "dashboard.agentAssets.transfer.installSuccess": "{n} files installed",
  "dashboard.agentAssets.transfer.toolInstructions":
    "Registration steps for the target tool",
  "dashboard.agentAssets.transfer.skipped": "{n} files skipped",
  "dashboard.agentAssets.transfer.backupDir": "Backup folder: {dir}",
  "dashboard.agentAssets.transfer.exportSuccess": "{n} files written",
  "dashboard.agentAssets.transfer.commandUnavailable":
    "The command required for this action is not available yet (install/export adapters are in development): {error}",
  "dashboard.agentAssets.transfer.close": "Close",

  "dashboard.agentAssets.error.unauthorized":
    "The device credential is invalid or revoked. Re-register on the Sync & Devices page.",
  "dashboard.agentAssets.error.oldServer":
    "Server upgrade required; upload and remote listing are unavailable.",
  "dashboard.agentAssets.error.network":
    "Network request failed. Check the sync server address and network, then retry.",
  "dashboard.agentAssets.error.adapter":
    "The adapter for this tool is not merged yet (work packages C/D/E). Only ZCode is available now.",
  "dashboard.agentAssets.error.retry": "Retry",
  "dashboard.agentAssets.error.code": "Error code: {code}",

  "dashboard.agentAssets.footnote":
    "Safety: the actual file list and sensitive fields are shown before upload. Credentials/private keys, path traversal and symlink risks are checked during preview and packing. Install skips conflicts by default and backs up before overwrite.",
};
