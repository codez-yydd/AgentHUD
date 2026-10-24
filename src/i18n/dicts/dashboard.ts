/**
 * 主窗口（Dashboard）词典：产品副标题、侧栏导航（同时作为页面标题）、
 * 页面说明与用量总览页（首页）/请求日志页/周额度对比页/价格设置页的完整
 * 文案，以及悬浮窗统计页的主窗口入口文案。
 *
 * 用词口径：金额一律称「花费」（与面板既有文案一致，不用「成本」）。
 */

export const dashboard = {
  // 产品副标题（自绘标题栏品牌区：logo + AgentHUD + 本副标题）
  "dashboard.subtitle": "用量与额度管理",

  // 自绘标题栏的窗口控制按钮（图标按钮的 title 提示与 aria-label，
  // 措辞沿用 Windows 中文版：最小化 / 最大化 / 还原 / 关闭）
  "dashboard.titlebar.minimize": "最小化",
  "dashboard.titlebar.maximize": "最大化",
  "dashboard.titlebar.restore": "还原",
  "dashboard.titlebar.close": "关闭",

  // 侧栏导航项（同时用作页面标题）：用量总览 / 请求日志 / 模型速度 / 周额度对比 / 同步与设备 / 价格设置
  "dashboard.overview": "用量总览",
  "dashboard.logs": "请求日志",
  "dashboard.speed": "模型速度",
  "dashboard.compare": "周额度对比",
  "dashboard.sync": "同步与设备",
  "dashboard.pricing": "价格设置",

  // 页面说明
  "dashboard.overview.desc": "跨服务用量、花费与趋势的汇总视图。",
  "dashboard.logs.desc":
    "逐个请求的明细：时间、模型、Token 构成、花费与耗时；可按设备、来源与模型筛选。",
  "dashboard.speed.desc":
    "跨来源、跨渠道对比各模型的流式输出速度；同名模型在不同来源或渠道独立成行，近似口径带 ≈ 标记。",
  "dashboard.compare.desc":
    "各订阅当前额度剩余、近 N 天已用比例趋势，以及本机与其他设备的 Token 对比。",
  "dashboard.sync.desc": "本机同步状态、服务端设备管理与数据清理。",
  "dashboard.pricing.desc": "模型单价维护与价格表更新。",

  // 悬浮窗统计页的主窗口入口（头部工具条图标按钮 + 完整提示/aria）
  "dashboard.mainWindow": "主窗口",
  "dashboard.openMainWindow": "打开主窗口",
  // 建窗失败时的行内轻提示（底部信息行，短暂展示后自动消失）
  "dashboard.openMainWindowFailed": "打开失败",

  // ===== 用量总览页：页头与状态 =====
  "dashboard.overview.rangeLabel": "时间范围",
  "dashboard.overview.range.today": "今日",
  "dashboard.overview.range.7d": "近 7 天",
  "dashboard.overview.range.30d": "近 30 天",
  "dashboard.overview.range.90d": "近 90 天",
  // 数据范围切换（本机 / 全部设备）：默认「全部设备」，与菜单栏标题口径对齐
  "dashboard.overview.scopeLabel": "数据范围",
  "dashboard.overview.scope.local": "本机",
  "dashboard.overview.scope.all": "全部设备",
  // Agent 筛选下拉（全部 + 各 Agent 品牌名）：指标卡 / 趋势 / 热力图 / 模型表
  // 随之切片到所选 Agent，Agent 统计表保留跨 Agent 对比
  "dashboard.overview.agentLabel": "Agent",
  "dashboard.overview.agent.all": "全部 Agent",
  // 未启用多设备同步：范围控件禁用，说明为何只能看本机口径
  "dashboard.overview.syncDisabledHint":
    "未启用多设备同步，当前仅显示本机数据。开启同步后可合并其他设备（与菜单栏标题同口径）。",
  // 「全部设备」下远端取数失败：降级为本机口径的弱提示（不清空已有数据）
  "dashboard.overview.remoteDegraded":
    "其他设备数据本次未能获取，当前仅本机口径",
  // 渐进渲染：本机数据已可见、远端（其他设备）仍在后台合并的页头弱标记
  "dashboard.overview.remoteMerging": "远端合并中…",
  // Agent 表范围内 Token 旁的标记：该行数值含其他设备贡献
  "dashboard.overview.remoteIncluded": "含远端",
  "dashboard.overview.remoteIncludedHint": "该值包含其他设备同步上传的用量",
  "dashboard.overview.lastUpdated": "上次刷新 {time}",
  "dashboard.overview.errorBanner": "数据加载失败：{error}",
  "dashboard.overview.partialFailures": "部分 Agent 取数失败，已跳过：{names}",
  "dashboard.overview.emptyTitle": "暂无用量数据",
  "dashboard.overview.emptyHint":
    "没有采集到任何 Agent 的本地用量。\n先在主面板查看一次对应 Agent 的统计，或安装并运行受支持的 CLI 后再刷新。",
  "dashboard.overview.retry": "重新加载",
  // 全部 Agent 取数失败（无任何可用数据）时的专用错误态：
  // 与上面的采集引导空态区分——空态是「没有数据」，这里是「取数失败」
  "dashboard.overview.errorTitle": "加载失败",
  "dashboard.overview.errorReason": "原因：{error}",

  // ===== 用量总览页：指标卡 =====
  // 第 1/2 卡副行是固定双列（输入|输出、日均|今日），每列独立截断：
  // 不再提供「A · B」拼接整行（窄窗 + 大字号档下长串必然折行）
  "dashboard.overview.rangeTokens": "范围内 Token",
  "dashboard.overview.subInput": "输入 {value}",
  "dashboard.overview.subOutput": "输出 {value}",
  "dashboard.overview.estCost": "估算花费",
  "dashboard.overview.subAvg": "日均 {value}",
  "dashboard.overview.subToday": "今日 {value}",
  "dashboard.overview.requestCount": "请求数",
  "dashboard.overview.dailyAvgRequests": "日均 {n} 次",
  "dashboard.overview.todayTokens": "今日 Token",
  "dashboard.overview.todayRequests": "今日请求 {n}",
  // 今日范围下第 4 卡（今日 Token 卡）与「范围内 Token」重复，换成昨日环比：
  // 值 = 环比（↑/↓x%，与面板 TrendChart 同款措辞），副文 = 昨日 token 量
  "dashboard.overview.vsYesterday": "较昨日",
  "dashboard.overview.yesterdayTokens": "昨日 {n}",

  // ===== 用量总览页：趋势图 =====
  "dashboard.overview.trendTitle": "用量趋势",
  // 指标切换分组（aria-label）：花费 / Token / 请求（与面板 TrendChart 同款语言）
  "dashboard.overview.metricLabel": "指标",
  "dashboard.overview.trendSub": "{range} · 合计 {total}",
  // Agent 筛选生效时的趋势卡副标题：Agent 名插在范围与合计之间（标注数据
  // 归属——合计已不是跨 Agent 口径）；未筛选用上面的 trendSub
  "dashboard.overview.trendSubAgent": "{range} · {agent} · 合计 {total}",
  "dashboard.overview.seriesInput": "输入侧",
  "dashboard.overview.seriesOutput": "输出",
  "dashboard.overview.peak": "峰值 {value}（{date}）",
  "dashboard.overview.ttTotal": "合计",
  "dashboard.overview.ttOutput": "输出",
  "dashboard.overview.ttInput": "输入侧",
  "dashboard.overview.ttRequests": "请求",
  "dashboard.overview.ttCost": "花费",
  "dashboard.overview.splitNote":
    "* Cursor 与旧数据无逐日输入/输出拆分，其用量计入输入侧",

  // ===== 用量总览页：热力图（52 周 ZCode 风格，三种聚合模式）=====
  "dashboard.overview.heatTitle": "活跃热力图",
  "dashboard.overview.heatSub": "近一年（52 周固定窗口）",
  // 合计口径 = 网格展示的 52 个自然周（自然周对齐，最早最多 6 天不在网格内）
  "dashboard.overview.heatTotal": "52 周合计",
  "dashboard.overview.heatActiveDays": "活跃天数",
  "dashboard.overview.heatActiveAvg": "活跃日均",
  "dashboard.overview.heatLess": "少",
  "dashboard.overview.heatMore": "多",
  // 图例里的第 5 种格子语义：色阶只放 1..4 四档，无数据的灰格单独用文字标注
  "dashboard.overview.heatNoData": "无数据",
  // 聚合模式分段（纯客户端计算，不重新取数）
  "dashboard.overview.heatModeLabel": "聚合方式",
  "dashboard.overview.heatMode.daily": "每日",
  "dashboard.overview.heatMode.weekly": "每周",
  "dashboard.overview.heatMode.cumulative": "累计",
  // 每周 / 累计模式的列级 tooltip
  "dashboard.overview.heatWeekTotal": "本周合计",
  "dashboard.overview.heatCumTotal": "截至本周累计",
  // 口径脚注：Cursor 与其他设备只覆盖近 90 天（更早的格子仅本机来源）
  "dashboard.overview.heatFootnote":
    "近 90 天含 Cursor 与其他设备，更早仅本机（不含 Cursor）。",

  // ===== 用量总览页：Agent 统计表 =====
  "dashboard.overview.agentTitle": "Agent 用量",
  "dashboard.overview.agentSub": "按范围内 Token 降序",
  "dashboard.overview.colAgent": "Agent",
  "dashboard.overview.colToday": "今日 Token",
  "dashboard.overview.colRange": "范围内 Token",
  "dashboard.overview.colCost": "范围内花费",
  "dashboard.overview.colQuota": "周额度剩余",
  // 5 小时窗口剩余列（Z.ai / Codex / Claude / Kimi 快照，OpenCode Go 等
  // provider quota 通道的同名窗口；无该窗口的 Agent 显示 "--"）
  "dashboard.overview.colHour5": "5 小时",
  "dashboard.overview.colHour5Hint":
    "近 5 小时窗口的额度剩余（有该窗口的订阅才显示，其余为 --）",
  // 5 小时列的容器悬停提示：↻ 重置时刻已恢复行内小字展示（与周额度列同款），
  // title 仅作补充（无重置时刻或已过期时不显示）
  "dashboard.overview.hour5ResetHint": "5 小时窗口 · 下次重置 {time}",
  "dashboard.overview.unpriced": "未定价",
  "dashboard.overview.agentEmpty": "暂无 Agent 用量数据",
  "dashboard.overview.billingCycle": "账单周期",
  // 周额度主行短标签：Agent 表列宽有限，用「周」；与下方「月」短标签对仗，
  // 不改通用的 common.weekly（窄窗多处复用）
  "dashboard.overview.weekShort": "周",
  // 月额度第二行标签（provider 通道 monthly 窗口，如 OpenCode Go；短词）
  "dashboard.overview.monthlyShort": "月",
  "dashboard.overview.costFootnote":
    "与菜单栏同源：本机 + 其他设备（同步开启时）按本机价格表估算（美元价 × 当前汇率），不代表账单金额。Cursor 明细不上传，仅本机口径。",

  // ===== 用量总览页：模型用量表 =====
  "dashboard.overview.modelTitle": "模型用量",
  "dashboard.overview.modelSub": "按范围内 Token 降序",
  "dashboard.overview.colModel": "模型",
  "dashboard.overview.colRequests": "请求数",
  "dashboard.overview.colModelInput": "输入 Token",
  // 输入列表头悬停：口径与指标卡「输入侧」一致（总量 − 输出，缓存读写都算输入侧）
  "dashboard.overview.colModelInputHint":
    "输入侧口径：总量 − 输出，含缓存读与缓存写",
  "dashboard.overview.colModelOutput": "输出 Token",
  "dashboard.overview.colModelTotal": "总 Token",
  "dashboard.overview.colShare": "占比",
  "dashboard.overview.modelEmpty": "该范围内暂无模型用量",
  // 折叠：默认前 10 行，超出给「展开全部 N 个模型」文本按钮
  "dashboard.overview.modelExpandAll": "展开全部 {count} 个模型",
  "dashboard.overview.modelCollapse": "收起",
  // by_model 缺失的旧数据 / 无逐模型拆分的来源汇总行（不伪装成某个模型）
  "dashboard.overview.modelUnsplit": "（未拆分）",
  "dashboard.overview.modelUnsplitHint":
    "旧数据或无逐模型拆分的来源：用量计入输入侧，不参与模型成本折算",
  // 同一模型的多种写法折叠为一行时，模型名 tooltip 追加的说明：列出各原始写法
  // 与请求数（分隔符 " · "，见 ModelTable）。写法含带 provider 前缀的完整 id
  // （如 zhipuai-coding-plan/glm-5.3-flash）与裸名——它们在本表按展示名折叠
  "dashboard.overview.modelVariantsHint": "已合并的写法：{list}",
  "dashboard.overview.modelVariantItem": "{model}（{count} 次请求）",
  "dashboard.overview.modelFootnote":
    "模型花费按本机价格表估算（美元价 × 当前汇率），未覆盖的模型显示「未定价」。Cursor 明细无逐日模型拆分，其行取该来源固定 90 天窗口的全量。「未拆分」为旧数据或无逐模型拆分的来源，用量计入输入侧且不计花费。",

  // ===== 周额度对比页（大窗）=====
  "dashboard.compare.device": "设备",
  "dashboard.compare.deviceAll": "全部（汇总）",
  "dashboard.compare.rangeLabel": "时间范围",
  "dashboard.compare.errorBanner": "数据加载失败：{error}",
  "dashboard.compare.remoteHistoryFailed": "远端额度历史获取失败：{msg}",
  // 汇总模式下远端历史取数失败：降级为仅本机口径，用弱提示条说明（不清空已有数据）
  "dashboard.compare.remoteDegraded":
    "其他设备数据本次未能获取，当前仅本机口径",
  "dashboard.compare.emptyTitle": "暂无周额度数据",
  "dashboard.compare.emptyHint":
    "应用开启后会自动采样各订阅的周额度。\n保持运行以积累数据，或在本机查看一次对应 Agent 的统计后再刷新。",
  "dashboard.compare.retry": "重新加载",
  "dashboard.compare.matrixTitle": "各 Agent 周额度剩余",
  "dashboard.compare.matrixSub": "每个系列取最近一次采样（本机 + 其他设备合并去重）",
  // 卡片副标题直接写出最近一次采样时刻（原先是额度块上的原生 title 悬停，
  // 悬停前看不到；矩阵里各系列取的都是这一时刻各自的最新采样）
  "dashboard.compare.matrixSampledLatest": "最近采样 {time}",
  // 额度块里 ↻ 重置时刻的可访问名与悬停说明（↻ 符号本身不表意）
  "dashboard.compare.matrixNextReset": "下次重置",
  "dashboard.compare.matrixEmpty": "暂无可展示的额度系列",
  "dashboard.compare.matrixNoData": "无数据",
  "dashboard.compare.trendTitle": "周额度趋势",
  "dashboard.compare.trendSub": "{range} · 各系列已用比例随时间变化",
  "dashboard.compare.trendEmpty": "该范围内没有额度采样",
  "dashboard.compare.trendLegendHint": "点击图例可显示 / 隐藏系列",
  "dashboard.compare.trendShowAll": "全部显示",
  "dashboard.compare.trendFootnote":
    "折线为各系列周额度的「已用 %」（= 100 − 剩余），按采样时刻连线；无采样时段不连线。",
  // 自定义范围下趋势卡的补充说明：额度快照只读到 COMPARE_HISTORY_DAYS 天前
  "dashboard.compare.trendSnapshotNote":
    "额度快照仅保留近 {days} 天，更早的日期不可选。",
  "dashboard.compare.tokensTitle": "Agent Token 对比",
  "dashboard.compare.tokensSub": "{range} · 本机与其他设备合并",
  "dashboard.compare.tokensLocal": "本机",
  "dashboard.compare.tokensRemote": "其他设备",
  "dashboard.compare.tokensRequests": "{count} 次请求",
  "dashboard.compare.tokensEmpty": "该范围内没有可对比的 Token 用量",
  "dashboard.compare.tokensFootnote":
    "Token = 各 Agent 在所选范围内的实际用量：本机按来源聚合 + 已同步设备的上传数据求和。Cursor 明细不上传，仅本机口径。",
  "dashboard.compare.footnote":
    "额度 % = 各订阅周额度窗口的已用比例（本机与远端采样按「时间 + 账号」去重取较高值）；Token = 所选范围内已启用 Agent 的实际用量。两者不是同一指标。",

  // ===== 同步与设备页（大窗）：本机状态卡 / 设备表 / 数据管理 =====
  // 相对时间（沿面板 sync.* 同款措辞，键独立以兼顾本页语境）
  "dashboard.sync.never": "从未",
  "dashboard.sync.justNow": "刚刚",
  "dashboard.sync.minAgo": "{n} 分钟前",
  "dashboard.sync.hourAgo": "{n} 小时前",
  "dashboard.sync.dayAgo": "{n} 天前",
  // 无数据占位（last_seen_at 缺失 / null：旧服务端或只传过快照的设备）
  "dashboard.sync.noData": "—",

  // ---- 区块①：本机同步状态卡 ----
  "dashboard.sync.statusTitle": "本机同步",
  "dashboard.sync.stateEnabled": "运行中",
  "dashboard.sync.statePaused": "已暂停",
  "dashboard.sync.fieldServer": "服务端地址",
  "dashboard.sync.fieldIdentity": "本机身份",
  "dashboard.sync.fieldPing": "连通状态",
  "dashboard.sync.fieldLastSync": "上次同步",
  "dashboard.sync.fieldPending": "待上传",
  "dashboard.sync.pendingCount": "{count} 条",
  "dashboard.sync.pingOk": "正常",
  "dashboard.sync.pingUnreachable": "不可达",
  "dashboard.sync.pingDisabled": "未启用",
  "dashboard.sync.pingDisabledHint": "未注册设备，无服务端可探测",
  "dashboard.sync.mode": "同步模式",
  "dashboard.sync.manual": "手动",
  "dashboard.sync.auto": "自动",
  "dashboard.sync.interval": "间隔（秒）",
  // 模式/间隔即改即存成功后的轻提示
  "dashboard.sync.modeSaved": "已保存",
  "dashboard.sync.syncNow": "立即同步",
  "dashboard.sync.syncing": "同步中…",
  // 暂停态点「立即同步」的禁用说明（title）
  "dashboard.sync.syncPausedHint": "同步已暂停，开启同步后可手动上传",
  "dashboard.sync.pause": "暂停同步",
  "dashboard.sync.resume": "开启同步",
  "dashboard.sync.disconnect": "断开连接",
  "dashboard.sync.uploaded": "已上传 {count} 条",
  // 注册引导态（未注册时的表单卡）
  "dashboard.sync.registerTitle": "连接到同步服务器",
  "dashboard.sync.registerDesc":
    "填入自建同步服务端地址与 Master Token 注册本机；注册完成后自动开启同步并全量上传历史明细。",
  "dashboard.sync.serverUrl": "服务器地址",
  "dashboard.sync.masterToken": "Master Token",
  "dashboard.sync.deviceName": "设备名称",
  "dashboard.sync.register": "连接并注册",
  "dashboard.sync.registering": "连接中…",
  // 未使用 https:// 的地址（含裸地址——后端会默认补 http://）均按明文警示
  "dashboard.sync.httpWarn":
    "明文传输：该地址未使用 HTTPS，建议内网使用或配置 HTTPS 反向代理。",

  // ---- 区块②：服务端设备表（master token 解锁管理操作）----
  "dashboard.sync.devicesTitle": "服务端设备",
  "dashboard.sync.devicesSub": "{count} 台设备 · {records} 条明细",
  "dashboard.sync.colDevice": "设备",
  "dashboard.sync.colDeviceId": "设备 ID",
  "dashboard.sync.colRegistered": "注册时间",
  "dashboard.sync.colLastSeen": "最后活跃",
  "dashboard.sync.lastSeenHint":
    "该设备最后一条请求明细的数据时刻（旧服务端或只传过快照的设备无此值）",
  "dashboard.sync.colRecords": "明细记录",
  "dashboard.sync.recordsHint": "仅统计请求明细，不含额度快照",
  "dashboard.sync.colOps": "操作",
  "dashboard.sync.localBadge": "本机",
  "dashboard.sync.rename": "改名",
  "dashboard.sync.merge": "合并",
  "dashboard.sync.deleteData": "删数据",
  "dashboard.sync.devicesEmpty":
    "未获取到任何设备，可尝试刷新；若刚重置过服务端，请在本机重新注册。",
  "dashboard.sync.devicesError": "设备列表获取失败：{error}",
  "dashboard.sync.masterLabel": "Master Token",
  "dashboard.sync.masterHint":
    "设备管理与数据清理需要 Master Token（服务端启动日志可查）。仅保存在本页内存，离开本页即失效。",
  "dashboard.sync.masterPlaceholder": "粘贴 Master Token 解锁管理操作",
  "dashboard.sync.masterNeeded": "请先填写 Master Token",
  // 卡③未解锁时的顶部弱提示（配合按钮禁用，不只靠 title）
  "dashboard.sync.masterLockedHint": "填写上方 Master Token 后可执行数据清理",
  // 设备改名弹层
  "dashboard.sync.renameTitle": "设备改名",
  "dashboard.sync.renameDesc": "修改「{name}（{id}）」的名称（1-32 字）。",
  // 设备合并弹层
  "dashboard.sync.mergeTitle": "合并设备",
  "dashboard.sync.mergeDesc": "将「{name}（{id}）」的 {count} 条明细合并到：",
  "dashboard.sync.mergeTarget": "目标设备",
  "dashboard.sync.mergeWarn":
    "来源设备的全部数据（明细与额度快照）将并入目标设备，随后来源设备被删除，不可恢复。若来源设备仍注册在某台机器上，请到那台机器断开并重新注册。",
  "dashboard.sync.mergeConfirm": "确认合并",
  "dashboard.sync.merged": "已合并 {count} 条明细到「{name}」",

  // ---- 区块③：服务端数据管理 ----
  "dashboard.sync.dataTitle": "服务端数据管理",
  // 卡③顶部范围说明：清理对象是服务端上的汇总数据，本机数据不受影响
  "dashboard.sync.dataHint":
    "以下操作清理的是同步服务端上的汇总数据（全部设备上传的明细与额度快照），本机数据不受影响，执行前均有二次确认。",
  "dashboard.sync.totalRecords": "共 {count} 条明细",
  "dashboard.sync.autoCleanup": "自动清理",
  // 自动清理开关的副说明（天数框常显：明确天数可自定义）
  "dashboard.sync.autoCleanupHint":
    "开启后服务端每天自动删除超过保留天数的旧数据（保留天数可自行设定，如填 90 即只保留最近 90 天），防止服务端数据库无限膨胀；默认关闭。",
  "dashboard.sync.on": "已开启",
  "dashboard.sync.off": "已关闭",
  "dashboard.sync.keepDays": "保留天数",
  "dashboard.sync.cleanByTime": "按时间清理",
  // 三个危险操作的副说明（各跟在按钮旁）
  "dashboard.sync.cleanByTimeHint":
    "立刻删除全部设备指定天数之前的远端数据，只执行这一次。",
  "dashboard.sync.clearAll": "全部清空",
  "dashboard.sync.clearAllHint":
    "删除服务端上的全部用量数据，设备注册保留。注意：已上传的历史不会重新上传，主窗口「全部设备」口径将不再有这些远端历史。",
  "dashboard.sync.reset": "重置服务端",
  "dashboard.sync.resetHint":
    "删除全部数据并移除所有设备注册（恢复出厂）。重置后所有设备需重新注册才能同步。",
  "dashboard.sync.cleanupError": "清理状态获取失败：{error}",
  // 自动清理开关 / 保留天数保存成功的轻提示
  "dashboard.sync.autoCleanupSaved": "自动清理已更新",
  "dashboard.sync.deleted": "已删除 {count} 条记录",
  "dashboard.sync.deletedBoth": "已删除 {records} 条记录、{devices} 个设备",

  // ---- 确认弹层（断开 / 删设备数据 / 按时间清理 / 全部清空 / 重置）----
  // 确认键文案按动作命名（破坏性递进：断开不删数据用普通键，其余红色）
  "dashboard.sync.confirmDisconnectTitle": "断开连接",
  "dashboard.sync.confirmDisconnectDesc":
    "将清除本机的同步凭证与上传游标（服务端数据保留）。下次注册后会重新全量上传历史明细。",
  "dashboard.sync.confirmDisconnect": "断开连接",
  "dashboard.sync.confirmDeviceTitle": "删除设备数据",
  "dashboard.sync.confirmDeviceDesc":
    "只删除该设备在服务端上的数据（全部明细与额度快照），不可恢复；设备注册保留。",
  "dashboard.sync.confirmDelete": "删除",
  "dashboard.sync.confirmBeforeTitle": "按时间清理",
  "dashboard.sync.confirmBeforeDesc":
    "将删除 {days} 天前的所有数据（明细与快照），不可恢复。本机数据不受影响，但已上传的历史不会重新上传。",
  "dashboard.sync.confirmClean": "清理",
  "dashboard.sync.confirmAllTitle": "全部清空",
  "dashboard.sync.confirmAllDesc":
    "将清空所有设备的用量数据（保留设备注册），不可恢复。本机数据不受影响，但已上传的历史不会重新上传。",
  "dashboard.sync.confirmClear": "清空",
  "dashboard.sync.confirmResetTitle": "重置服务端",
  "dashboard.sync.confirmResetDesc":
    "将清空全部数据并移除所有设备的注册（含本机），服务端回到初始状态；每台设备（包括本机）需重新注册才能同步。不可恢复。",
  "dashboard.sync.confirmReset": "重置",

  // ---- 配置读取失败的错误重试态（防已注册用户被误导重注册）----
  "dashboard.sync.errorTitle": "加载失败",
  "dashboard.sync.errorReason": "原因：{error}",
  "dashboard.sync.retry": "重新加载",

  // ===== 价格设置页（大窗）=====
  "dashboard.pricing.currencyLabel": "币种",
  "dashboard.pricing.cny": "¥ 人民币",
  "dashboard.pricing.usd": "$ 美元",
  // 币种切换的「仅本页查看」标注（全局币种由界面语言推导，无手动切换入口）
  "dashboard.pricing.currencyViewOnly": "仅本页",
  "dashboard.pricing.currencyViewOnlyHint":
    "仅本页查看口径，不修改全局币种（全局币种跟随界面语言：中文 ¥ / 英文 $）",
  // 未保存改动提示与保存按钮 dirty 文案
  "dashboard.pricing.dirtyHint": "{count} 处未保存",
  "dashboard.pricing.remoteUpdated":
    "服务端价格已被其他设备更新；不同模型会自动合并，同一模型以最后写入为准。",
  "dashboard.pricing.saveDirty": "保存 *",
  // 应用价格更新成功后的轻提示
  "dashboard.pricing.appliedCount": "已应用 {count} 项",
  "dashboard.pricing.unitHint": "单位：$/百万 token。人民币按汇率 {rate} 折算展示。",
  "dashboard.pricing.fxRefresh": "刷新汇率",
  "dashboard.pricing.fxRefreshing": "更新中…",
  "dashboard.pricing.checkUpdates": "检查价格更新",
  "dashboard.pricing.syncModelsdev": "从 models.dev 同步",
  "dashboard.pricing.syncing": "同步中…",
  "dashboard.pricing.syncFailHint": "可在设置页配置网络代理后重试",
  "dashboard.pricing.upToDate": "已是最新 ✓",
  "dashboard.pricing.missingCount": "{count} 个模型未配价",
  "dashboard.pricing.tableTitle": "模型价格表",
  "dashboard.pricing.tableSub":
    "输入 / 输出 / 缓存读，单位 $/百万 token；编辑后点右上「保存」落盘",
  "dashboard.pricing.searchPlaceholder": "搜索模型 ID / 显示名",
  "dashboard.pricing.filterEmpty": "没有匹配的模型",
  "dashboard.pricing.count": "{n} 个模型",
  "dashboard.pricing.colModel": "模型",
  "dashboard.pricing.colInput": "输入",
  "dashboard.pricing.colOutput": "输出",
  "dashboard.pricing.colCacheRead": "缓存读",
  "dashboard.pricing.colSource": "来源",
  "dashboard.pricing.colActions": "操作",
  "dashboard.pricing.sourceConfigured": "已配价",
  "dashboard.pricing.sourceUnpriced": "未配价",
  "dashboard.pricing.sourceManual": "手动",
  "dashboard.pricing.removeModel": "从列表移除",
  "dashboard.pricing.removeConfirmDesc":
    "将从列表移除并记入隐藏名单，可在下方「已隐藏模型」中恢复。",
  "dashboard.pricing.addTitle": "新增模型定价",
  "dashboard.pricing.addIdLabel": "模型 ID",
  "dashboard.pricing.addPlaceholder": "如 glm-5.4-flash",
  "dashboard.pricing.add": "添加",
  "dashboard.pricing.addEmptyId": "请输入模型 ID",
  "dashboard.pricing.addExists":
    "该模型已在列表中（大小写 / 隐藏字符变体视为同一模型）",
  "dashboard.pricing.hiddenTitle": "已隐藏模型",
  "dashboard.pricing.hiddenCount": "已隐藏 {count} 个模型",
  "dashboard.pricing.restore": "恢复",
  "dashboard.pricing.noModels":
    "暂无模型数据。请确认已产生使用记录，或手动新增模型定价。",
  "dashboard.pricing.footnote":
    "价格为模型单价的本地配置，仅用于本应用的花费估算（不代表账单金额）；应用参考价不会自动覆盖未勾选的条目。",

  // ===== 价格差异面板（内置参考表 / models.dev）=====
  "dashboard.pricing.diffTitle": "价格更新 · 内置参考表",
  "dashboard.pricing.diffTitleModelsdev": "价格更新 · models.dev",
  "dashboard.pricing.diffHint":
    "参考价离线对比不联网。带 ≈ 标记的是变体名匹配的基础模型参考价，应用前请确认。",
  "dashboard.pricing.diffHintModelsdev":
    "官方厂商在线价格（仅白名单厂商，不含中转渠道）。带 ≈ 标记的仍是变体名匹配的基础模型参考价，应用前请确认。",
  "dashboard.pricing.modelsdevFetchedAt": "数据时间：{time}",
  "dashboard.pricing.modelsdevFromCache": "缓存（离线）",
  "dashboard.pricing.missingWarn":
    "以下 {count} 个模型实际在用但未配置价格（花费按 0 计）：",
  "dashboard.pricing.addPriceBelow": "请在下方模型列表中手动补价",
  "dashboard.pricing.badgeNew": "新增",
  "dashboard.pricing.badgeChanged": "变动",
  "dashboard.pricing.refFrom":
    "参考价取自基础模型 {model}（同系变体），应用前请确认价格",
  "dashboard.pricing.old": "旧",
  "dashboard.pricing.unpriced": "未配价",
  "dashboard.pricing.noDiff": "无差异，价格已是最新",
  "dashboard.pricing.collapse": "收起",
  "dashboard.pricing.applying": "应用中…",
  "dashboard.pricing.applySelected": "应用选中",
  "dashboard.pricing.applySelectedCount": "应用选中 ({count})",

  // ===== 请求日志页（大窗）：筛选行 / 明细表 / 分页条 / 详情抽屉 =====
  // 时间范围四档与其他主窗口页统一，全部复用 dashboard.overview.range.*
  "dashboard.logs.rangeLabel": "时间范围",
  // 页头自动刷新间隔（关闭 / 5 / 10 / 30 / 60 秒，默认 30 秒）：触发按钮显示
  // 「自动刷新 + 当前档」，完整语义放 title 与 aria-label，页头保持紧凑
  "dashboard.logs.autoRefresh": "自动刷新",
  "dashboard.logs.autoRefreshOff": "关闭",
  "dashboard.logs.autoRefreshSec": "{n} 秒",
  "dashboard.logs.autoRefreshHint": "自动刷新间隔：{value}",
  "dashboard.logs.sourceLabel": "来源",
  "dashboard.logs.sourceAll": "全部来源",
  "dashboard.logs.modelLabel": "模型",
  "dashboard.logs.modelAll": "全部模型",
  // 页头设备下拉（全部设备 / 本机 / 各远端设备；独立偏好 zbar-logs-device，
  // 不读写总览页的 zbar-dashboard-scope）。全部 / 本机文案复用
  // dashboard.overview.scope.*，本机带设备名时用 deviceLocalName
  "dashboard.logs.deviceLabel": "设备",
  "dashboard.logs.deviceLocalName": "{name}（本机）",
  // 设备下拉 title（启用态）：完整语义「设备筛选 + 当前选中项」，触发器
  // 可能截断长设备名（与自动刷新下拉「完整语义放 title」同一先例）
  "dashboard.logs.deviceHint": "设备筛选：{value}",
  // 汇总指标行（当前筛选条件下的全范围聚合，不是当前页）
  "dashboard.logs.metricsRequests": "总请求数",
  "dashboard.logs.metricsCost": "总成本",
  "dashboard.logs.metricsCostHint": "当前范围内没有可计价数据（请求未配置价格）",
  "dashboard.logs.metricsCacheHit": "缓存命中率",
  "dashboard.logs.metricsCacheHint":
    "缓存读 token ÷ 输入 token（按来源口径合并；无输入数据时显示为 —）",
  // Token 三格（总量 / 输入 / 输出）：数值格紧凑格式；hint 词条是纯口径说明
  // （不含数值），组件把精确值（exactCount）拼在前面组成 title
  // （「精确值 · 口径说明」，「—」态只显示 hint），与表格缓存列
  // cacheCellTitle「精确值在 title」同一先例。输入的跨源口径差异必须写进
  // title（详见 src-tauri/src/request_logs.rs 文件头「汇总指标」注释：
  // included 源 input 已含缓存读，separate 源不含）
  "dashboard.logs.metricsTokensTotal": "Token 总量",
  "dashboard.logs.metricsTokensTotalHint":
    "输入 + 输出 + 缓存等全部口径的总量（按各来源的记录口径汇总）",
  "dashboard.logs.metricsInputTokens": "输入",
  "dashboard.logs.metricsInputTokensHint":
    "输入 token（口径随来源不同：ZCode / Codex / DSH / OpenCode / MiniMax 已含缓存读，Claude / Kimi 不含缓存读写）",
  "dashboard.logs.metricsOutputTokens": "输出",
  "dashboard.logs.metricsOutputTokensHint":
    "输出 token 合计（按各来源的记录口径）",
  "dashboard.logs.tableTitle": "请求明细",
  // 卡片副标题的本机口径文案：「全部设备」时改用 dashboard.overview.scope.all
  //（数据范围分段与总览页共用同一套词条）
  "dashboard.logs.localOnly": "仅本机记录",
  "dashboard.logs.colTime": "时间",
  "dashboard.logs.colSource": "来源",
  "dashboard.logs.colModel": "模型",
  "dashboard.logs.colInput": "输入",
  "dashboard.logs.colOutput": "输出",
  // 缓存读 / 缓存写合并列（值形如「12.3万 / 4.5万」，精确值在 title）
  "dashboard.logs.colCache": "缓存",
  "dashboard.logs.cacheCellTitle": "缓存读 {read} · 缓存写 {write}",
  "dashboard.logs.colCost": "花费",
  "dashboard.logs.colDuration": "耗时",
  "dashboard.logs.colTtft": "首字",
  // 速度列：数值 + 小号 t/s 单位；≈ 表示按总耗时估算的近似速度
  "dashboard.logs.colSpeed": "速度",
  // 模型速度页的表列（速度三列 / 首字 / 耗时 / 输入输出 / 成功率等复用
  // stats dict 的 speed.* 键，见 SpeedPage 表头）
  "dashboard.speed.colRank": "排名",
  "dashboard.speed.colProvider": "渠道",
  "dashboard.speed.colSamples": "速度样本",
  // 排名列恒显速度名次（与当前排序列无关）
  "dashboard.speed.rankHint":
    "按 P50 输出速度的名次（当前来源筛选范围内，与当前排序列无关；无速度样本的行无名次）",
  // 速度分位列头的百分位语义（title 挂在可排序按钮上）
  "dashboard.speed.p10Hint": "输出速度 P10 分位（慢速端 10% 请求的界值），tok/s",
  "dashboard.speed.p50Hint": "输出速度 P50 中位数（一半请求快于此值），tok/s",
  "dashboard.speed.p90Hint": "输出速度 P90 分位（90% 请求快于此值），tok/s",
  // 汇总指标行的口径说明（title）
  "dashboard.speed.metricRequestsHint":
    "窗口内全部分组的请求总数（全部设备口径含其他设备的请求）",
  "dashboard.speed.metricSuccessHint":
    "按本机请求加权的各组成功率（远端行无状态、不参与）",
  // 来源筛选后无数据的专属空态（LogsPage 模型空态同款先例）
  "dashboard.speed.emptySourceTitle": "该来源在此范围内无模型请求",
  "dashboard.speed.emptySourceHint": "来源「{source}」在所选时间范围内没有请求数据",
  "dashboard.speed.clearSource": "清除来源筛选",
  // 输入 / 输出列的总量口径（列头 title）与均值说明（数值格 title 拼接用）
  "dashboard.speed.inTotalHint": "窗口内累计输入 token（输入含缓存读）",
  "dashboard.speed.outTotalHint": "窗口内累计输出 token",
  "dashboard.speed.avgPerRequest": "均值 {v}/请求",
  "dashboard.logs.rowHint": "点击行查看详情",
  "dashboard.logs.cellNoData": "无数据",
  // 耗时 / 首字 / 速度缺失说明：讲清是数据源不落盘（而不是加载失败），
  // 避免被当成页面故障；Remote 后缀词条给远端行（同步明细不含这些列），
  // 措辞不指向「会话记录」避免误导
  "dashboard.logs.noDurationTitle":
    "该来源的会话记录不含耗时数据（如 Codex 会话文件无耗时、旧版 Claude Code 不落盘耗时）",
  "dashboard.logs.noDurationRemoteTitle": "远端设备的同步明细不含耗时数据",
  "dashboard.logs.noTtftTitle": "该来源的会话记录不含首字数据",
  "dashboard.logs.noTtftRemoteTitle": "远端设备的同步明细不含首字数据",
  "dashboard.logs.noSpeedTitle":
    "无耗时数据，或该行样本无效（时长越界 / 时序异常被拒收），无法计算速度",
  "dashboard.logs.unpriced": "未定价",
  "dashboard.logs.unpricedHint": "该模型未配置价格，花费按 0 计",
  "dashboard.logs.emptyTitle": "该时间范围内没有请求记录",
  "dashboard.logs.emptyHint":
    "换个时间范围或来源试试。\n未安装 / 未使用过的来源不会出现在这里。",
  // 模型筛选无结果：保留选中值，给说明 + 一键清除（不静默清空）
  "dashboard.logs.emptyModelTitle": "该模型在当前条件下没有请求",
  "dashboard.logs.emptyModelHint":
    "当前来源 / 时间范围下没有「{model}」的请求记录；可清除模型筛选查看全部模型。",
  "dashboard.logs.clearModel": "清除模型筛选",
  // 设备筛选无结果（具体设备口径下空表）：与模型空态同款——专属标题点明
  // 设备名 + 一键切回全部设备；与远端不可达的提示条（remoteUnreachableDevice）
  // 互不覆盖：不可达的后果走提示条，这里只说「该设备无记录」
  "dashboard.logs.emptyDeviceTitle": "{name} 在此范围内暂无请求记录",
  "dashboard.logs.emptyDeviceHint":
    "当前来源 / 时间范围下没有来自该设备的请求记录；可查看全部设备。",
  "dashboard.logs.viewAllDevices": "查看全部设备",
  // total > 0 但当前页没有行（深页 / 页码越界回收帧）
  "dashboard.logs.pageEmptyHint":
    "当前页没有数据，可能已超出记录范围，请返回上一页。",
  "dashboard.logs.errorTitle": "加载失败",
  "dashboard.logs.errorReason": "原因：{error}",
  "dashboard.logs.retry": "重新加载",
  // 单源取数失败（有数据时的弱提示，对齐总览页 partialFailures 惯例）；
  // "remote"（远端一路失败 / 未启用同步 / 旧服务端 404）不再混进来源名清单，
  // 单独成条说清后果（下方两条：合并口径降级为纯本机展示 / 具体设备口径下
  // 唯一来源失败、空表的解释）
  "dashboard.logs.partialFailures":
    "部分来源本次未取到，数据可能不完整：{names}",
  "dashboard.logs.remoteDegraded": "远端数据不可达，当前仅显示本机记录",
  "dashboard.logs.remoteUnreachableDevice":
    "远端设备数据不可达，未获取到该设备的记录",
  "dashboard.logs.pagePrev": "上一页",
  "dashboard.logs.pageNext": "下一页",
  "dashboard.logs.pageInfo": "第 {page} / {total} 页",
  "dashboard.logs.pageTotal": "共 {n} 条",
  "dashboard.logs.detailTitle": "请求详情",
  "dashboard.logs.close": "关闭",
  "dashboard.logs.detailBasic": "基本信息",
  "dashboard.logs.detailSource": "来源",
  // 远端行的设备归属（「全部设备」口径从服务端明细带回的 device_id）
  "dashboard.logs.detailDevice": "设备",
  "dashboard.logs.detailModel": "模型",
  "dashboard.logs.detailProvider": "Provider",
  "dashboard.logs.detailSession": "会话 ID",
  "dashboard.logs.detailProject": "项目",
  "dashboard.logs.detailTokens": "Token 明细",
  "dashboard.logs.detailInput": "输入",
  "dashboard.logs.detailOutput": "输出",
  "dashboard.logs.detailCacheRead": "缓存读",
  "dashboard.logs.detailCacheWrite": "缓存写",
  "dashboard.logs.detailReasoning": "思考",
  "dashboard.logs.detailTotal": "合计",
  "dashboard.logs.detailCost": "花费",
  "dashboard.logs.detailPerf": "性能",
  "dashboard.logs.detailDuration": "耗时",
  "dashboard.logs.detailTtft": "首字",
  "dashboard.logs.detailSpeed": "速度",
  "dashboard.logs.speedApproxHint":
    "缺首字到完成的事件时刻，按请求总耗时估算的近似速度（思考型请求由后端按含思考的全程窗口计算，不带此标记）",

  // ===== 自定义时间范围（总览 / 日志 / 对比三页共用的页头时间范围控件）=====
  // 触发按钮显示当前范围标签：生效的预设名，或自定义区间的「MM-DD ~ MM-DD」
  // 摘要（自定义时带日历图标 + 高亮描边）；面板顶部预设行为各页现有档位
  "dashboard.customRange": "自定义",
  "dashboard.customRange.title": "时间范围",
  "dashboard.customRange.selectedHint": "自定义范围 {range}（点开可修改）",
  "dashboard.customRange.presets": "快捷范围",
  "dashboard.customRange.start": "开始日期",
  "dashboard.customRange.end": "结束日期",
  // 字段卡尚未选到该端时的占位
  "dashboard.customRange.empty": "未选择",
  "dashboard.customRange.apply": "应用",
  "dashboard.customRange.cancel": "取消",
  // 日历月份导航（‹ / › 与「回本月」按钮的可访问名与悬停说明）
  "dashboard.customRange.prevMonth": "上个月",
  "dashboard.customRange.nextMonth": "下个月",
  "dashboard.customRange.thisMonth": "回到本月",
  // 面板内说明可选范围下限（各页限制不同：取数窗口 / 额度快照保留期）
  "dashboard.customRange.minHint": "可选最早日期：{date}",
  "dashboard.customRange.errorIncomplete": "请选择起始与结束日期",
  "dashboard.customRange.errorOrder": "起始日期不能晚于结束日期",
  "dashboard.customRange.errorOutOfRange": "所选日期超出可选范围",
};
