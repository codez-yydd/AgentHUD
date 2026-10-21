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

  // 侧栏导航项（同时用作页面标题）：用量总览 / 请求日志 / 周额度对比 / 价格设置
  "dashboard.overview": "用量总览",
  "dashboard.logs": "请求日志",
  "dashboard.compare": "周额度对比",
  "dashboard.pricing": "价格设置",

  // 页面说明
  "dashboard.overview.desc": "跨服务用量、花费与趋势的汇总视图。",
  "dashboard.logs.desc":
    "逐个请求的明细：时间、模型、Token 构成、花费与耗时；可按设备、来源与模型筛选。",
  "dashboard.compare.desc":
    "各订阅当前额度剩余、近 N 天已用比例趋势，以及本机与其他设备的 Token 对比。",
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
  // 时间范围前四项复用 dashboard.overview.range.*（同款分段控件文案必须一致），
  // 只有「全部」是本页独有的第五档
  "dashboard.logs.rangeLabel": "时间范围",
  "dashboard.logs.range.all": "全部",
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
