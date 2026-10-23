/**
 * 统计域词典：统计面板（标签/工具栏/设备筛选）、Coding Plan 额度面板、
 * 各 Agent 用量面板（Codex / Claude / Cursor）的文案。
 */

export const stats = {
  // ===== 统计面板顶栏 =====
  "stats.tab.summary": "汇总",
  "stats.tab.speed": "速度",
  "stats.reports": "报表",
  "stats.syncOn": "设备同步",
  "stats.syncOff": "配置设备同步",
  "stats.settings": "设置",
  "stats.pin": "常驻置顶",
  "stats.unpin": "取消常驻",
  "stats.github": "GitHub 仓库",
  // HUD 停靠条开关按钮（点击托盘图标弹出的悬浮面板顶部）
  "stats.hudBar": "HUD 停靠条",
  "stats.hudBarOn": "HUD 停靠条已开启（点击关闭）",
  "stats.hudBarOff": "HUD 停靠条已关闭（点击开启）",
  "stats.priceSettings": "价格设置",
  // 底部「价格设置」按钮的 title/aria：窄窗二期减负后价格页移到主窗口
  "stats.priceSettingsHint": "价格设置（将在主窗口打开）",
  "stats.sourcesAria": "统计来源",

  // ===== 设备筛选 =====
  "stats.deviceFilter": "筛选设备",
  "stats.deviceAll": "全部",
  "stats.deviceLocal": "本机",
  "stats.deviceLocalName": "本机（{name}）",

  // ===== Coding Plan 额度面板 =====
  "quota.title": "Coding Plan 额度监控",
  "quota.configHint": "请在 ZCode 客户端登录 Coding Plan 订阅，登录后自动读取额度",
  "quota.failed": "额度查询失败：{msg}",
  "quota.refresh": "刷新额度",
  "quota.todayDelta": "↑今日 {pct}%",
  "quota.allAccounts": "全部账号",
  "quota.quotaFail": "额度查询失败",
  "quota.weekShort": "周剩",
  "quota.hour5Short": "5h",

  // ===== Agent 用量面板（Codex / Claude / Cursor 共用结构）=====
  "stats.rateLimits": "额度",
  "stats.noDataFor": "未获取到 {name} 数据",
  "stats.codexNotFound": "未检测到 Codex",
  "stats.codexNotFoundHint":
    "请安装并使用 OpenAI Codex CLI 产生本地会话记录\n（~/.codex/sessions）后再查看",
  "stats.claudeNotFound": "未检测到 Claude Code",
  "stats.claudeNotFoundHint":
    "请安装并使用 Anthropic Claude Code 产生本地会话记录\n（~/.claude/projects）后再查看",
  "stats.dshNotFound": "未找到 DSH 会话数据",
  "stats.dshNotFoundHint":
    "请确认 DeepSeek Harness 已安装并使用过；数据读取自 ~/.dsh 会话目录。",
  // OpenCode 面板：本地用量（opencode.db 导入库）+ Go 套餐额度板块
  "stats.opencodeQuota": "Go 套餐额度",
  "stats.opencodeNotFound": "未找到 OpenCode 会话数据",
  "stats.opencodeNotFoundHint":
    "请确认 OpenCode 已安装并使用过；数据读取自本地库\n（~/.local/share/opencode/opencode.db）。",
  // MiniMax Code 面板：本地用量（~/.minimax 本地库）+ 套餐额度板块
  "stats.minimaxQuota": "套餐额度",
  "stats.minimaxcodeNotFound": "未找到 MiniMax Code 会话数据",
  "stats.minimaxcodeNotFoundHint":
    "请确认 MiniMax Code 已安装并使用过；数据读取自本地库\n（~/.minimax）。",
  // Claude 订阅额度增量窗口（API 返回该窗口才有值，缺省不渲染）
  "stats.claudeOpusWeekly": "Opus 周额度",
  "stats.claudeSonnetWeekly": "Sonnet 周额度",
  "stats.claudeExtraUsage": "超额消费",
  "stats.claudeOtherAccounts": "其他账号",
  "stats.cursorOtherAccounts": "其他账号",
  "stats.kimiOtherAccounts": "其他账号",
  // OpenCode / MiniMax Code 底部的手动 API Key 管理区（ProviderAccountsSection）：
  // 额度卡由各面板顶部额度块渲染，本区只放凭证管理，故沿用「其他账号」口径
  "stats.opencodeOtherAccounts": "其他账号",
  "stats.minimaxOtherAccounts": "其他账号",
  // DeepSeek Harness 底部：手动 DeepSeek 开放平台 API Key → 账户余额。
  // 标题按「余额」而非「其他账号」——它是平台账户余额，不是另一个账号的用量
  "stats.dshPlatformBalance": "DeepSeek 平台余额",
  "stats.kimiNotFound": "未检测到 Kimi Code",
  "stats.kimiNotFoundHint":
    "请安装并使用 Kimi Code CLI 产生本地会话记录\n（~/.kimi-code/sessions）后再查看",
  "stats.boosterBalance": "加油包余额",
  "stats.boosterMonthlyUsed": "本月已用 ¥{amount}",
  "stats.kimiMonthlyQuota": "月总额度",

  // ===== Cursor 面板 =====
  "cursor.notLoggedIn": "未检测到 Cursor 登录",
  "cursor.loginHint": "请在 Cursor 应用中登录，登录后自动读取本地用量",
  "cursor.account": "账户",
  "cursor.unknown": "未知",
  "cursor.planQuota": "套餐额度",
  "cursor.noQuotaData": "暂无额度数据",
  "cursor.onDemand": "按需用量",
  "cursor.cycle": "周期 {date}",
  "cursor.resetDate": "重置 {date}",
  "cursor.tokenSpend": "Token 花费",
  "cursor.selectedRange": "所选时间范围",
  "cursor.byModel": "按模型",
  "cursor.eventsFailed": "Token 明细拉取失败：{msg}",
  "cursor.noEvents": "所选时间范围内暂无 Token 使用明细",
};

// ===== 模型速度统计面板（ModelSpeedPanel，"速度"tab）=====
// V5 行块骨架 + 明细平铺：常态每行 = 模型名 + 速度值 + 相对速度条 + 3 行 ×
// 2 列明细网格（速度/首字/耗时/输入/输出/请求，全部数值直接可见、无需悬停
// 依赖）。明细格 = 11px 灰标签 + 12px 黑值，统计口径直接写进值串。速度分位
// 在 UI 中使用“慢 / 平均 / 快”直白标签（p50 是中位数、非总体加权均速；
// 文案按用户习惯用“平均”）。键集与 en 保持
// 一致（satisfies 约束）。
export const speed = {
  "speed.title": "模型速度",
  "speed.subtitle": "按平均速度比较模型的流式输出表现",
  "speed.rangeToday": "今日",
  "speed.range7d": "7天",
  "speed.rangeLabel": "速度统计范围",
  // 数据口径切换（本机 / 全部设备，scope 参数）
  "speed.scopeLocal": "本机",
  "speed.scopeAll": "全部",
  "speed.scopeLabel": "数据范围",
  "speed.scopeAllTitle": "全部设备：并入同步服务端上其他设备的明细（近似口径）",
  // 全部设备口径下远端拉取失败的弱提示（本机数据照常）
  "speed.remoteUnavailable": "远端数据不可用，当前仅显示本机",
  "speed.fastest": "最快平均速度",
  "speed.fastestHint": "当前范围内",
  "speed.autoRefresh": "每 30 秒自动更新",
  "speed.overviewModels": "模型",
  "speed.overviewRequests": "请求",
  "speed.overviewMean": "平均速度",
  // 均值格 title：全部设备口径下各分组的 P50 可能混合精确（generation）
  // 与近似（request_average）两种口径，简单平均无单一口径归属，故只加
  // 说明不加 ≈ 前缀
  "speed.overviewMeanHint": "各组速度的简单平均，混合精确与近似口径（近似含首字延迟）",
  "speed.overviewSuccess": "成功率",
  "speed.ranking": "模型排行",
  "speed.modelCount": "{n} 个模型",
  "speed.speedSamples": "速度样本 {n}",
  "speed.samplesNone": "暂无速度样本",
  // 近似速度口径提示（speedQuality = request_average 时挂在数值 title 上：
  // 导入库缺事件时刻，按请求总耗时折算）
  "speed.approxHint": "请求平均近似速度：该来源缺事件时刻，按请求总耗时折算（含首字延迟）",
  // 未知来源兜底标签（后端新增来源未及更新 SOURCE_META 时显示）
  "speed.sourceUnknown": "未知来源",
  "speed.relativeSpeed": "相对最快模型 {pct}%",
  "speed.relativeLabel": "相对最快模型",
  "speed.averagePeak": "平均 {avg} · 峰值 {max}",
  "speed.averageSlower": "平均 {avg} · 较慢 {slow}",
  "speed.requestSummary": "{n} 次 · 成功率 {pct}",
  "speed.p10": "慢",
  "speed.p50": "平均",
  "speed.p90": "快",
  "speed.ttft": "首字",
  "speed.latency": "耗时",
  "speed.inputShort": "输入",
  "speed.outputShort": "输出",
  "speed.latencyShort": "耗时",
  "speed.samplesShort": "请求",
  "speed.waitingData": "等待本地请求数据",
  "speed.live": "实时",
  "speed.loadFailed": "速度数据加载失败",
  "speed.retry": "重试",
  "speed.empty": "所选时间范围内暂无模型请求",
  "speed.emptyHint": "在 ZCode、Codex、Claude 等 Agent 中发起对话后自动统计",
  "speed.fail": "模型速度统计查询失败：{msg}",
  // 数据截至标识（每次查询成功后更新，Ns 随时间自走）
  "speed.updatedAt": "数据截至 {time} · {ago}s 前",
  "speed.updatedHint": "最近一次查询成功时刻；每 30 秒自动刷新",
  "speed.refresh": "立即刷新",
};
