/**
 * 用量报告页词典：页内展示（ZCode 深色玻璃体系）、活跃度、热力图与趋势
 * 图表，以及导出 Markdown 与文件名（报告语言跟随 UI 语言，无独立选项）。
 */

export const report = {
  // ===== 页面框架 =====
  "report.title": "用量报告",
  "report.refresh": "刷新报告",
  // 时间范围选择复用共享 RangePicker（range.* 词典）
  "report.allDevices": "全部设备",
  "report.emptyTitle": "当前范围暂无用量",
  "report.emptyHint": "请确认 Agent 已开启，并在所选时间范围内产生过请求。",

  // ===== 分项加载提示（本地与远端解耦后分别提示慢在哪个环节） =====
  "report.loadingLocal": "正在加载本地数据…",
  "report.loadingRemote": "正在同步远端数据…",
  "report.cachedHint": "展示缓存数据，正在后台刷新…",

  // ===== Agent 筛选（胶囊分段，纯前端过滤不发请求；仅列出实际有数据的 agent） =====
  "report.filter.all": "全部",

  // ===== KPI 指标卡（数值上、标签下） =====
  "report.kpi.tokens": "总消耗 Token",
  "report.kpi.cost": "总花费",
  "report.kpi.requests": "总请求数",
  "report.kpi.activeDays": "活跃天数",

  // ===== 活跃度汇总条（五项；英文用短文案避免窄窗截断） =====
  "report.activity": "活跃度",
  "report.act.total": "累计 Token",
  "report.act.peakDay": "峰值 Token",
  // ⓘ 悬浮说明：峰值口径
  "report.act.peakTip": "范围内单日 Token 最高的一天",
  "report.act.dailyAvg": "日均 Token",
  "report.act.streakCurrent": "连续天数",
  "report.act.streakLongest": "最长连续",
  // 连续天数的单位后缀（zh "5天"、en "5d"）
  "report.act.daysUnit": "天",

  // ===== Token 活动热力图 =====
  "report.tokenActivity": "Token 活动",
  // 热力图固定一年视图，与报告范围无关（标题旁小字明示）
  "report.heat.window": "近 1 年",
  "report.heat.daily": "每日",
  "report.heat.weekly": "每周",
  "report.heat.cum": "累计",
  "report.heat.empty": "暂无 Token 活动",
  "report.heat.less": "少",
  "report.heat.more": "多",
  // 悬浮提示两行：日期（+周范围 / 累计口径）/ token 与请求数
  "report.heat.tooltipWeek": "{date} 当周",
  "report.heat.tooltipCum": "截至 {date} 累计",
  "report.heat.tooltipLine": "{tokens} Token · {n} 次请求",

  // ===== 用量趋势柱状图 =====
  "report.trend.days7": "近 7 天",
  "report.trend.days30": "近 30 天",
  "report.trend.other": "其他",
  "report.trend.allHidden": "已隐藏全部系列，点击图例恢复",
  "report.trend.noDaily": "当前数据源不含按模型的每日明细，无法绘制分组柱状图。",

  // ===== 区块 =====
  "report.noTrend": "当前来源没有可绘制的趋势数据",
  "report.agentDist": "Agent 分布",

  // ===== 底部操作 =====
  "report.copy": "复制",
  "report.copied": "已复制到剪贴板 ✓",
  "report.savedOpened": "已保存并在文件夹打开 ✓",

  // ===== 数据说明 =====
  "report.noteCursorDaily":
    "Cursor 官方明细按日返回，小时趋势未混入 Cursor，Agent 汇总仍包含它。",

  // ===== 导出文件名 =====
  "report.file.custom": "用量报告-",

  // ===== 导出 Markdown 全文 =====
  "report.md.custom": "用量报告",
  "report.md.noData": "（暂无数据）",
  "report.md.summaryLine": "总花费 {cost}｜Token {tokens}｜请求 {requests} 次",
  "report.md.agentLine": "{label}：{cost}｜{tokens} Token｜{requests} 次请求",
  "report.md.top5": "模型 TOP5",
  "report.md.modelLine": "{agent} / {model}：{cost}｜{tokens} Token",
  "report.md.tokenPeak": "Token 峰值：{label}，{tokens} Token",
  "report.md.notes": "说明",
  "report.md.footer": "由 AgentHUD 自动生成 · {date}",
};
