/**
 * 英文词典：typeof zh 约束保证与中文键集完全一致（缺键/多键编译报错）。
 * 译文追求自然简洁，窄面板处用短词（如 Custom 而非 Customize）；
 * 品牌名（ZBar、Claude、Codex、Gemini、Cursor 等）不翻译。
 */
import { common } from "./dicts/common";
import { layout } from "./dicts/layout";
import { stats, speed } from "./dicts/stats";
import { summary } from "./dicts/summary";
import { pricing } from "./dicts/pricing";
import { sync } from "./dicts/sync";
import { compare } from "./dicts/compare";
import { report } from "./dicts/report";
import { settings } from "./dicts/settings";
import { theme } from "./dicts/theme";
import { projects } from "./dicts/projects";
import { share } from "./dicts/share";
import { credentials } from "./dicts/credentials";
import { dashboard } from "./dicts/dashboard";
import { zh } from "./zh";

export const en: typeof zh = {
  ...({
    // 基础状态
    "common.loading": "Loading…",
    "common.loadingUsage": "Loading {name} usage…",
    "common.switchLanguage": "Switch language",
    "common.refresh": "Refresh",
    "common.cancel": "Cancel",
    "common.confirm": "Confirm",
    "common.delete": "Delete",
    "common.show": "Show",
    "common.hide": "Hide",

    // 保存动作
    "common.save": "Save",
    "common.saving": "Saving…",
    "common.saved": "Saved ✓",
    "common.deleting": "Deleting…",

    // 额度 / 指标通用
    "common.refreshIn": "Refreshes in {time}",
    // 剩余额度百分比：紧跟数值都有窗口标签（5h/Week/MCP），英文不再带
    // "left" 后缀——260px 窄窗固定列宽下 "88% left" 会溢出（zh「剩 88%」不变）
    "common.remaining": "{pct}%",
    "common.cacheHit": "Cache hit {pct}",
    "common.hour5": "5h",
    "common.weekly": "Week",
    "common.resetAt": "Resets at {time}",

    // 用量页通用指标
    "common.totalCost": "Total cost",
    "common.totalTokens": "Total tokens",
    "common.cost": "Cost",
    "common.requests": "Requests",
    "common.requestCount": "Requests",
    "common.output": "Output",
    "common.cacheRate": "Cache rate",
    "common.tokenComposition": "Token breakdown",
    "common.input": "Input",
    "common.cache": "Cache",
    "common.reasoning": "Reasoning",
    "common.modelUsage": "Models",
    "common.noPrice": "No price set",

    // Speed / current model
    "common.avgSpeed": "Avg speed",
    "common.avgSpeedLocal": "Local avg speed",
    "common.fastestRequest": "Fastest request",
    "common.ttft": "First token",
    "common.currentModel": "Current model",
    "common.justNow": "just now",
    "common.minutesAgo": "{n}m ago",
    "common.hoursAgo": "{n}h ago",
    "common.daysAgo": "{n}d ago",

    // 趋势图
    "common.usageTrend": "Usage trend",
    "common.trendFlat": "Flat",
    "common.trendNew": "New",
    "common.trendVsHour": "Last hour vs previous",
    "common.trendVsDay": "Last day vs previous",

    // 设备下拉选项
    "common.deviceOption": "{name} ({id})",

    // 时间范围预设
    "range.today": "Today",
    "range.24h": "24h",
    "range.7d": "7d",
    "range.30d": "30d",
    "range.custom": "Custom",

    // 日期选择器星期表头
    "date.weekdays": "Su Mo Tu We Th Fr Sa",
  } satisfies typeof common),

  ...({
    "layout.back": "← Back",
    "layout.themeLight": "Switch to light",
    "layout.themeDark": "Switch to dark",
    // 语言切换按钮的目标语言字形（en 界面下点击切回中文）
    "layout.langGlyph": "中",
  } satisfies typeof layout),

  ...({
    // 统计面板顶栏
    "stats.tab.summary": "Summary",
    "stats.tab.speed": "Speed",
    "stats.reports": "Reports",
    "stats.syncOn": "Device sync",
    "stats.syncOff": "Set up device sync",
    "stats.settings": "Settings",
    "stats.pin": "Always on top",
    "stats.unpin": "Disable always on top",
    "stats.github": "GitHub repository",
    "stats.priceSettings": "Pricing",
    "stats.sourcesAria": "Data sources",

    // 设备筛选
    "stats.deviceFilter": "Filter devices",
    "stats.deviceAll": "All",
    "stats.deviceLocal": "Local",
    "stats.deviceLocalName": "Local ({name})",

    // Coding Plan 额度面板
    "quota.title": "Coding Plan quota",
    "quota.configHint": "Sign in to a Coding Plan in the ZCode client; quota is read automatically",
    "quota.failed": "Failed to load quota: {msg}",
    "quota.refresh": "Refresh quota",
    "quota.todayDelta": "+{pct}% today",
    "quota.allAccounts": "All accounts",
    "quota.quotaFail": "Quota query failed",
    "quota.weekShort": "Wk",
    "quota.hour5Short": "5h",

    // Agent 用量面板
    "stats.rateLimits": "Quotas",
    "stats.noDataFor": "No {name} data",
    "stats.codexNotFound": "No Codex detected",
    "stats.codexNotFoundHint":
      "Install and run OpenAI Codex CLI to create local sessions\n(~/.codex/sessions), then check back",
    "stats.claudeNotFound": "No Claude Code detected",
    "stats.claudeNotFoundHint":
      "Install and run Claude Code to create local sessions\n(~/.claude/projects), then check back",
    "stats.dshNotFound": "No DSH session data found",
    "stats.dshNotFoundHint":
      "Make sure DeepSeek Harness is installed and has been used; data is read from the ~/.dsh session directory.",
    // OpenCode panel: local usage (opencode.db import) + Go plan quota block
    "stats.opencodeQuota": "Go plan quota",
    "stats.opencodeNotFound": "No OpenCode session data found",
    "stats.opencodeNotFoundHint":
      "Make sure OpenCode is installed and has been used; data is read from the local database\n(~/.local/share/opencode/opencode.db).",
    // MiniMax Code panel: local usage (~/.minimax) + plan quota block
    "stats.minimaxQuota": "Plan quota",
    "stats.minimaxcodeNotFound": "No MiniMax Code session data found",
    "stats.minimaxcodeNotFoundHint":
      "Make sure MiniMax Code is installed and has been used; data is read from the local database\n(~/.minimax).",
    // Claude subscription quota incremental windows (rendered only when the API returns them)
    "stats.claudeOpusWeekly": "Opus weekly",
    "stats.claudeSonnetWeekly": "Sonnet weekly",
    "stats.claudeExtraUsage": "Extra usage",
    "stats.claudeOtherAccounts": "Other accounts",
    "stats.cursorOtherAccounts": "Other accounts",
    "stats.kimiOtherAccounts": "Other accounts",
    // Manual API key section at the bottom of the OpenCode / MiniMax Code panels
    "stats.opencodeOtherAccounts": "Other accounts",
    "stats.minimaxOtherAccounts": "Other accounts",
    // DeepSeek Harness bottom section: manual DeepSeek platform API key → balance
    "stats.dshPlatformBalance": "DeepSeek platform balance",
    "stats.kimiNotFound": "No Kimi Code detected",
    "stats.kimiNotFoundHint":
      "Install and run Kimi Code CLI to create local sessions\n(~/.kimi-code/sessions), then check back",
    "stats.boosterBalance": "Booster balance",
    "stats.boosterMonthlyUsed": "Monthly used ¥{amount}",
    "stats.kimiMonthlyQuota": "Monthly quota",

    // Cursor 面板
    "cursor.notLoggedIn": "Cursor not signed in",
    "cursor.loginHint":
      "Sign in to the Cursor app; local usage is read automatically",
    "cursor.account": "Account",
    "cursor.unknown": "Unknown",
    "cursor.planQuota": "Plan quotas",
    "cursor.noQuotaData": "No quota data",
    "cursor.onDemand": "On-demand",
    "cursor.cycle": "Cycle {date}",
    "cursor.resetDate": "Resets {date}",
    "cursor.tokenSpend": "Token spend",
    "cursor.selectedRange": "Selected range",
    "cursor.byModel": "By model",
    "cursor.eventsFailed": "Failed to load token usage: {msg}",
    "cursor.noEvents": "No token usage in the selected range",
  } satisfies typeof stats),

  ...({
    // 模型速度统计面板
    "speed.title": "Model Speed",
    "speed.subtitle": "Compare average streaming output speed across models",
    "speed.rangeToday": "Today",
    "speed.range7d": "7 days",
    "speed.rangeLabel": "Speed range",
    "speed.fastest": "Fastest average speed",
    "speed.fastestHint": "in selected range",
    "speed.autoRefresh": "Auto-refreshes every 30s",
    "speed.overviewModels": "Models",
    "speed.overviewRequests": "Requests",
    "speed.overviewMean": "Average speed",
    "speed.overviewSuccess": "Success",
    "speed.ranking": "Model ranking",
    "speed.modelCount": "{n} models",
    "speed.speedSamples": "{n} speed samples",
    "speed.samplesNone": "No speed samples",
    "speed.providerUnknown": "Unknown source",
    "speed.relativeSpeed": "{pct}% of fastest model",
    "speed.relativeLabel": "Relative to fastest model",
    "speed.averagePeak": "Average {avg} · peak {max}",
    "speed.averageSlower": "Average {avg} · slower {slow}",
    "speed.requestSummary": "{n} · {pct} success",
    "speed.p10": "Slow",
    "speed.p50": "Average",
    "speed.p90": "Fast",
    "speed.ttft": "TTFT",
    "speed.latency": "Latency",
    "speed.inputShort": "Input",
    "speed.outputShort": "Output",
    "speed.latencyShort": "Latency",
    "speed.samplesShort": "Requests",
    "speed.waitingData": "Waiting for local request data",
    "speed.live": "Live",
    "speed.loadFailed": "Could not load speed data",
    "speed.retry": "Retry",
    "speed.empty": "No model requests in the selected range",
    "speed.emptyHint": "Stats appear automatically once you chat in ZCode",
    "speed.fail": "Failed to load model speed stats: {msg}",
    // 数据截至标识（每次查询成功后更新，Ns 随时间自走）
    "speed.updatedAt": "As of {time} · {ago}s ago",
    "speed.updatedHint": "Last successful query; auto-refreshes every 30s",
    "speed.refresh": "Refresh now",
  } satisfies typeof speed),

  ...({
    "summary.totalCost": "Total cost",
    "summary.sources": "{count} sources",
    "summary.totalTokens": "Total tokens",
    "summary.costDist": "Cost split",
    "summary.quotaMonitor": "Quotas",
    "summary.resetDate": "Resets {date}",
    "summary.noToken": "Coding Plan not signed in",
    "summary.quotaFailed": "Quota unavailable",
    "summary.noData": "No data",
    "summary.loadFailed": "Failed to load",
    "summary.notLoggedIn": "Not signed in",
    "summary.noQuotaData": "No quota data",
    // OpenCode Go quota provenance: the official API returns authoritative
    // values; the local DB is an estimate from fixed constants. The badge must
    // keep them apart so estimated numbers are never mistaken for official ones.
    "summary.opencodeOfficial": "Official",
    "summary.opencodeEstimated": "Local estimate",
    // DSH has no subscription cap, so its summary quota row shows today's usage
    "summary.dshTodayUsage": "Today {tokens} tokens · {cost}",
  } satisfies typeof summary),

  ...({
    "pricing.loading": "Loading pricing…",
    "pricing.title": "Pricing",
    "pricing.cny": "¥ CNY",
    "pricing.usd": "$ USD",
    "pricing.unitHint": "Unit: $/M tokens. CNY converted at {rate}.",
    "pricing.checkUpdates": "Check updates",
    "pricing.syncModelsdev": "Sync from models.dev",
    "pricing.syncing": "Syncing…",
    "pricing.syncFailHint": "configure a network proxy in Settings and retry",
    "pricing.missingCount": "{count} models unpriced",
    "pricing.upToDate": "Up to date ✓",

    "pricing.diffTitle": "Price updates · built-in reference",
    "pricing.diffTitleModelsdev": "Price updates · models.dev",
    "pricing.diffHint":
      "Compared offline against the built-in reference. Items marked ≈ inherit a base model's price by variant name — verify before applying.",
    "pricing.diffHintModelsdev":
      "Official vendor prices fetched online (whitelisted vendors only, no resellers). Items marked ≈ still inherit a base model's price by variant name — verify before applying.",
    "pricing.modelsdevFetchedAt": "Data as of {time}",
    "pricing.modelsdevFromCache": "cache (offline)",
    "pricing.missingWarn":
      "{count} models in use have no price (their cost counts as 0):",
    "pricing.addPriceBelow": "Add prices manually in the list below",
    "pricing.badgeNew": "New",
    "pricing.badgeChanged": "Changed",
    "pricing.refFrom":
      "Reference price from base model {model} (same family); verify before applying",
    "pricing.old": "Old",
    "pricing.unpriced": "unpriced",
    "pricing.noDiff": "No differences — prices are up to date",
    "pricing.collapse": "Collapse",
    "pricing.applying": "Applying…",
    "pricing.applySelected": "Apply selected",
    "pricing.applySelectedCount": "Apply selected ({count})",
    "pricing.noModels": "No models yet. Make sure Z.ai has usage records.",

    "pricing.removeModel": "Remove from list",
    "pricing.hiddenCount": "{count} model(s) hidden",
    "pricing.restore": "Restore",
  } satisfies typeof pricing),

  ...({
    "sync.loading": "Loading sync settings…",
    "sync.title": "Device Sync",

    "sync.never": "Never",
    "sync.justNow": "Just now",
    "sync.minAgo": "{n} min ago",
    "sync.hourAgo": "{n} h ago",
    "sync.dayAgo": "{n} d ago",

    "sync.connectTitle": "Connect to sync server",
    "sync.connectHint":
      "Deploy the zbar-sync server with Docker first, then copy the Master Token from its logs.",
    "sync.serverUrl": "Server URL",
    "sync.masterLabel": "Credential (Master Token)",
    "sync.masterPh": "MASTER_TOKEN from docker logs",
    "sync.deviceName": "Device name",
    "sync.namePh": "e.g. work / home",
    "sync.httpWarn":
      "⚠️ HTTP is unencrypted. Keep it on a LAN or add an HTTPS reverse proxy.",
    "sync.connecting": "Connecting…",
    "sync.connect": "Connect & register",
    "sync.needMaster": "Enter the Master Token first (from server logs)",
    "sync.needMasterAuto": "Auto-cleanup requires the Master Token",

    "sync.server": "Server",
    "sync.lastSync": "Last sync",
    "sync.pending": "Pending",
    "sync.recordsCount": "{count} records",
    "sync.uploadCursor": "Last upload",
    "sync.mode": "Sync mode",
    "sync.manual": "Manual",
    "sync.auto": "Auto",
    "sync.interval": "Interval (s)",
    "sync.syncing": "Syncing…",
    "sync.syncNow": "Sync now",
    "sync.saveMode": "Save mode",
    "sync.disconnect": "Disconnect",

    "sync.uploaded": "Uploaded {count} records",
    "sync.deletedBoth": "Deleted {records} records and {devices} devices",
    "sync.deleted": "Deleted {count} records",
    "sync.merged": "Merged {count} records into \"{name}\"",
    "sync.targetDevice": "target device",
    "sync.renamed": "Renamed to \"{name}\"",
    "sync.deviceMissing": "Device not found or already deleted",

    "sync.dataMgmt": "Data management",
    "sync.totalRecords": "{count} in total",
    "sync.masterForCleanup": "Master Token (for cleanup)",
    "sync.pasteMasterPh": "Paste Master Token",
    "sync.autoCleanup": "Auto cleanup",
    "sync.on": "On",
    "sync.off": "Off",
    "sync.keepDays": "Retention days",
    "sync.localBadge": "Local",
    "sync.rename": "Rename",
    "sync.merge": "Merge",
    "sync.mergeInto": "Merge into another device",
    "sync.deleteDeviceData": "Delete this device's data",
    "sync.cleanByTime": "Clean by time",
    "sync.clearAll": "Clear all",
    "sync.reset": "Reset",

    "sync.mergeTitle": "Merge devices",
    "sync.mergeDesc": "Merge {count} records from \"{name} ({id})\" into:",
    "sync.mergeLocalWarn":
      "After merging into this device, the merged history may not appear in this device's \"All devices\" view. Prefer merging into the instance that device currently uses.",
    "sync.mergeWarn":
      "Records move to the target device and the source device is deleted — this cannot be undone. If the source device still syncs on another machine, disconnect and re-register there.",
    "sync.mergeConfirm": "Merge",

    "sync.renameTitle": "Rename device",
    "sync.renameDesc": "Rename \"{name} ({id})\".",

    "sync.confirmTitleDevice": "Delete device data",
    "sync.confirmTitleReset": "Reset server",
    "sync.confirmDescDevice": "All of this device's records will be deleted. This cannot be undone.",
    "sync.confirmDescBefore":
      "Deletes all data older than {days} days and shortens trend history. Cannot be undone.",
    "sync.confirmDescAll": "Clears all usage data (device registrations kept). Cannot be undone.",
    "sync.confirmDescReset": "Clears all data and deletes all devices, returning to a clean slate. Cannot be undone.",
    "sync.confirmDelete": "Delete",
  } satisfies typeof sync),

  ...({
    "compare.title": "Quota compare",
    "compare.device": "Device",
    "compare.all": "All (combined)",
    "compare.remoteHistoryFailed": "Failed to load remote quota history: {msg}",
    "compare.emptyTitle": "No weekly quota data yet",
    "compare.emptyHint":
      "Weekly quotas are sampled automatically while the app runs.\nKeep it running to accumulate data.",
    "compare.chartTitle": "Weekly quota used by subscription",
    "compare.chartHint":
      "Bar height = peak used % of each subscription's weekly quota within a calendar week.",
    "compare.allPeriods": "All weeks",
    "compare.thisWeek": "This week",
    "compare.selectedWeek": "Selected week",
    "compare.weekRange": "{from} ~ {to}",
    "compare.seriesHint":
      "Peak used % of each subscription in this week; \"No data\" = no samples this week",
    "compare.peakShort": "Peak",
    "compare.noDataShort": "No data",
    "compare.peakUsedPct": "peak {pct}% in week",
    "compare.noSample": "no samples this week",
    "compare.legacyAccount": "legacy data",
    "compare.legacyAccountHint":
      "Snapshots recorded before per-account tracking; cannot be attributed to a specific account",
    "compare.samples": "{count} samples",
    "compare.tokensOfAgents": "Agent actual token count (not a quota percentage)",
    "compare.tokenShort": "tokens",
    "compare.requestsCount": "{count} requests",
    "compare.noUsage": "No usage from enabled agents in this week",
    "compare.footer":
      "Quota % = peak used percentage of each subscription's weekly quota window within a calendar week; tokens = actual usage from enabled agents in that week. They are different metrics.",
  } satisfies typeof compare),

  ...({
    "report.title": "Usage Report",
    "report.refresh": "Refresh report",
    // Range selection reuses the shared RangePicker (range.* dict)
    "report.allDevices": "All devices",
    "report.emptyTitle": "No usage in this range",
    "report.emptyHint":
      "Make sure agents are enabled and had requests within the selected range.",

    // Per-stage loading hints (local and remote load independently)
    "report.loadingLocal": "Loading local data…",
    "report.loadingRemote": "Syncing remote data…",
    "report.cachedHint": "Showing cached data, refreshing in background…",

    // Agent filter (pill segments, pure frontend filtering; only agents with data)
    "report.filter.all": "All",

    // KPI cards (value on top, label below)
    "report.kpi.tokens": "Total tokens",
    "report.kpi.cost": "Total cost",
    "report.kpi.requests": "Total requests",
    "report.kpi.activeDays": "Active days",

    // Activity summary strip (short labels to avoid truncation in the narrow panel)
    "report.activity": "Activity",
    "report.act.total": "Tokens",
    "report.act.peakDay": "Peak",
    // ⓘ tooltip: peak metric definition
    "report.act.peakTip": "Highest single-day token day in range",
    "report.act.dailyAvg": "Daily avg",
    "report.act.streakCurrent": "Streak",
    "report.act.streakLongest": "Best",
    "report.act.daysUnit": "d",

    // Token activity heatmap
    "report.tokenActivity": "Token activity",
    "report.heat.window": "Last 12 months",
    "report.heat.daily": "Daily",
    "report.heat.weekly": "Weekly",
    "report.heat.cum": "Cumulative",
    "report.heat.empty": "No token activity",
    "report.heat.less": "Less",
    "report.heat.more": "More",
    // Two tooltip lines: date (+ week range / cumulative) / tokens and requests
    "report.heat.tooltipWeek": "Week of {date}",
    "report.heat.tooltipCum": "Cumulative to {date}",
    "report.heat.tooltipLine": "{tokens} tokens · {n} requests",

    // Usage trend bar chart
    "report.trend.days7": "Last 7 days",
    "report.trend.days30": "Last 30 days",
    "report.trend.other": "Other",
    "report.trend.allHidden": "All series hidden — click a legend item to restore",
    "report.trend.noDaily":
      "Current sources have no per-model daily breakdown to plot.",

    "report.noTrend": "No trend data to plot",
    "report.agentDist": "Agent breakdown",

    "report.copy": "Copy",
    "report.copied": "Copied ✓",
    "report.savedOpened": "Saved and opened ✓",

    "report.noteCursorDaily":
      "Cursor's official usage is daily, so the hourly trend excludes it; agent totals still include it.",

    "report.file.custom": "Report-",

    "report.md.custom": "Usage Report",
    "report.md.noData": "(no data)",
    "report.md.summaryLine": "Total cost {cost} | Tokens {tokens} | Requests {requests}",
    "report.md.agentLine": "{label}: {cost} | {tokens} tokens | {requests} requests",
    "report.md.top5": "Top 5 models",
    "report.md.modelLine": "{agent} / {model}: {cost} | {tokens} tokens",
    "report.md.tokenPeak": "Token peak: {label}, {tokens} tokens",
    "report.md.notes": "Notes",
    "report.md.footer": "Generated by AgentHUD · {date}",
  } satisfies typeof report),

  ...({
    "settings.title": "Settings",

    "settings.panelOpacity": "Panel opacity",
    "settings.opacity": "Opacity",
    "settings.opacityHint":
      "Lower values make the frosted panel more translucent. On dark theme keep it above 60% so text stays readable.",
    "settings.language": "Language",
    "settings.langZh": "简体中文",
    "settings.langEn": "English",

    "settings.autostart": "Launch at login",
    "settings.enable": "Enable",
    "settings.applying": "Applying…",
    "settings.autostartHint":
      "Start AgentHUD automatically after signing in to Windows or macOS. The panel stays hidden by default; open it from the tray.",
    "settings.readingState": "Reading status…",
    "settings.autostartReadFail": "Failed to read autostart state: {msg}",
    "settings.autostartFailOn": "Failed to enable autostart: {msg}",
    "settings.autostartFailOff": "Failed to disable autostart: {msg}",

    "settings.sources": "Stats sources",
    "settings.instant": "Instant",
    "settings.sourcesHint":
      "Hiding a source only removes its tab and summary card; local collection and device sync are unaffected.",
    // Group subtitles; membership comes from AGENT_GROUP (data origin) in
    // agentVisibility: local collection = zai/codex/claude/cursor/kimi/dsh/minimax
    "settings.groupLocal": "Local collection",
    "settings.groupCredential": "Credential-based",
    "settings.agentZaiDesc": "Z.ai usage & Coding Plan quota",
    "settings.agentCodexDesc": "Codex CLI usage & quotas",
    "settings.agentClaudeDesc": "Claude Code usage & plan quotas",
    "settings.agentCursorDesc": "Cursor usage & plan quotas",
    "settings.agentKimiDesc": "Kimi Code CLI usage & subscription quota",
    // DeepSeek Harness: local-collection agent for usage (nothing to add for it),
    // but the panel bottom hosts the DeepSeek platform balance section (manual
    // DeepSeek open-platform API key); still hidden by default and controlled by
    // the settings toggle — the only way in for users who never ran DSH
    "settings.agentDshDesc":
      "DeepSeek Harness usage and cost (read from local ~/.dsh session data; add a DeepSeek platform API key to see the account balance)",
    // Credential-based providers: hidden by default; shown once enabled or a
    // credential is added (quota integrations rolling out progressively)
    "settings.agentGeminiDesc": "Gemini subscription quota (hidden by default; shown once enabled or a credential is added)",
    "settings.agentGrokDesc": "Grok subscription quota (hidden by default; shown once enabled or a credential is added)",
    "settings.agentQoderDesc": "Qoder subscription quota (hidden by default; shown once enabled or a credential is added)",
    // Dual-source: local CLI data or a manual API key, either one is enough; the
    // panel bottom provides the entry point for users without the CLI
    "settings.agentOpencodegoDesc":
      "OpenCode usage & plan quota (hidden by default; reads local OpenCode data, or add a Go plan API key to query the official API)",
    // MiniMax merged view: usage comes from local MiniMax Code data, the plan
    // quota from a manually added sk-cp- Coding Plan key (region must match)
    "settings.agentMinimaxDesc":
      "MiniMax Code usage & plan quota (hidden by default; reads local MiniMax Code data, or add a Coding Plan API key for the plan quota)",
    // Moonshot (月之暗面) open platform: reads an account balance
    // (recharge-based), not a subscription quota; Kimi Code credentials already
    // cover it, hence no credential entry
    "settings.agentMoonshotDesc": "Kimi open platform balance (hidden by default)",
    // Legacy key (no settings row any more — deepseek is not in
    // AGENT_VISIBILITY_OPTIONS): the DeepSeek platform balance entry now lives
    // inside the DeepSeek Harness panel (provider id stays deepseek, see
    // agentVisibility.CREDENTIAL_AGENT_ALIAS); worded like the zh entry
    "settings.agentDeepseekDesc": "DeepSeek balance (hidden by default)",
    "settings.agentLongcatDesc": "LongCat subscription quota (hidden by default; shown once enabled or a credential is added)",
    "settings.agentMimoDesc": "MiMo subscription quota (hidden by default; shown once enabled or a credential is added)",
    "settings.agentAlibabaDesc": "Alibaba Coding Plan (Tongyi Lingma) quota (hidden by default; shown once enabled or a credential is added)",
    "settings.agentAlibabatokenDesc": "Alibaba Token Plan balance (hidden by default; shown once enabled or a credential is added)",
    "settings.agentStepfunDesc": "StepFun subscription quota (hidden by default; shown once enabled or a credential is added)",
    "settings.agentDoubaoDesc": "Volcengine subscription quota (hidden by default; shown once enabled or a credential is added)",

    "settings.resetDisplay": "Reset time display",
    "settings.resetCountdown": "Countdown",
    "settings.resetDatetime": "Exact time",
    "settings.resetDisplayHint":
      "How quota reset time is shown; both can be on at once. Exact times use MM-DD HH:mm.",

    // Desktop pet size level names (shared by PetSizeLevelPicker in petStyles;
    // pet settings themselves now live in the skin page pet card only)
    "settings.petSizeLevel1": "Small",
    "settings.petSizeLevel2": "Medium",
    "settings.petSizeLevel3": "Default",
    "settings.petSizeLevel4": "Large",
    "settings.petSizeLevel5": "XL",

    "settings.fontSize": "Font size",
    "settings.fontSmall": "Small",
    "settings.fontStandard": "Default",
    "settings.fontLarge": "Large",
    "settings.fontXl": "Larger",
    "settings.fontSizeHint":
      "Scales all panel content proportionally. If larger fonts show less, drag the window bigger to see more.",
    "settings.winSize": "Window size",
    "settings.winSizeSmall": "Small",
    "settings.winSizeStandard": "Standard",
    "settings.winSizeLarge": "Large",
    "settings.winSizeXl": "XL",
    "settings.winSizeCustom": "Custom",
    "settings.winSizeHint":
      "Drag the panel edges to resize freely, or pick a preset; your size is remembered.",

    "settings.fxCard": "Exchange rate",
    "settings.fxAutoNote": "Auto-updating; uncheck to edit manually",
    "settings.fxNever": "Never fetched",
    "settings.fxUnknownSource": "unknown source",
    "settings.fxDailyTitle": "Refresh the rate online once a day",
    "settings.fxDaily": "Update rate daily",
    "settings.updateNow": "Update now",
    "settings.updating": "Updating…",
    "settings.updateNowTitle": "Fetch the latest rate online (multiple free sources with fallback)",
    "settings.fxNote":
      "Prices are stored in USD; CNY costs are converted at this rate (same source as the ¥ view in Pricing).",

    // Network proxy (shared by models.dev pricing sync and rate refresh)
    "settings.proxyCard": "Network proxy",
    "settings.proxyHint":
      "Outbound proxy for online model-pricing sync and rate refresh. Accepts http:// and socks5:// addresses; leave empty for direct connection.",
    "settings.proxyDirect": "Not configured — direct connection",

    "settings.shortcut": "Global shortcut",
    "settings.shortcutHint":
      "Show/hide the panel. Format like alt+shift+z (modifiers: ctrl/alt/shift/cmd; main key: a letter or number).",
    "settings.apply": "Apply",
    "settings.applied": "Applied ✓",

    "settings.aboutCard": "About & update",
    "settings.currentVersion": "Version",
    "settings.checkUpdate": "Check update",
    "settings.checking": "Checking…",
    "settings.upToDate": "Up to date",
    "settings.newVersion": "New version {v}",
    "settings.downloadInstall": "Download & install",
    "settings.downloading": "Downloading {pct}%",
    "settings.installing": "Installing…",
    "settings.restartUpdate": "Restart to update",
    "settings.updateReady": "Version {v} is downloaded — restart to apply",
    "settings.backgroundDownloading": "Downloading update in the background…",
    "settings.updateFailed": "Update failed: {msg}",
    "settings.openDownloadPage": "Open download page",
    "settings.updateHint":
      "Checks for updates every hour and downloads in the background; a red dot appears on Settings when ready. Updates are signature-verified. The Chinese interface prefers the Gitee source and the English interface prefers GitHub, falling back to the other automatically on failure.",

    "settings.accountsCard": "ZCode account switch",
    "settings.accountsHint":
      "Capture the current ZCode login as a snapshot, then switch between accounts with one click (ZCode quits and relaunches automatically; the current login is backed up first and restored on failure). Snapshots stay local only and are never synced.",
    "settings.accountsCurrent": "Current",
    "settings.accountsUnknown": "Unrecognized",
    "settings.accountsCapture": "Capture login",
    "settings.accountsCapturing": "Capturing…",
    "settings.accountsCapturedNew": "New account snapshot saved ✓",
    "settings.accountsCapturedUpdate": "Account snapshot updated ✓",
    "settings.accountsSwitch": "Switch",
    "settings.accountsSwitching": "Switching…",
    "settings.accountsSwitched": "Switched to \"{name}\" ✓",
    "settings.accountsRelaunchFail":
      " (ZCode failed to relaunch automatically, please open it manually)",
    "settings.accountsConfirmTitle": "Switch account",
    "settings.accountsConfirmDesc":
      "This quits and relaunches the ZCode desktop app and switches the login to \"{name}\". The current login is backed up first and rolled back automatically on failure.",
    "settings.accountsRemoveConfirm":
      "Only the snapshot saved by this app is deleted; the current ZCode login is not affected.",
    "settings.accountsRename": "Rename",
    "settings.accountsRenameDesc": "Set a recognizable name for this snapshot.",
    "settings.accountsEmpty":
      "No snapshots yet. Log in to an account in the ZCode client and click \"Capture login\", then log in to another account and capture again — after that you can switch with one click.",
    "settings.accountsLoadFail": "Failed to load accounts: {msg}",

    "settings.accountsAutoSwitch": "Smart",
    "settings.accountsAutoConfirmTitle": "Smart-switch to \"{name}\"",
    "settings.accountsAutoSummary": "5h {h5}% left · Wk {wk}% left",
    "settings.accountsAutoReasonReady":
      "Both 5h and weekly quotas available, with the best combined headroom",
    "settings.accountsAutoReasonWait":
      "All accounts' 5h quotas are exhausted; this account's 5h quota resets first (in ~{min} min)",
    "settings.accountsAutoReasonWaitNoTime":
      "All accounts' 5h quotas are exhausted; picked the account whose 5h quota resets sooner",
    "settings.accountsAutoNoTarget":
      "No suitable account to switch to: other accounts failed to query or their weekly quotas are exhausted",
    "settings.accountsAutoSingle": "Only one account — no other account to switch to",
    "settings.accountsAutoNoData": "Quota data not ready yet, please retry later",
    "settings.accountsAutoToggle": "Auto-switch when 5h quota is exhausted",
    "settings.accountsAutoToggleHint":
      "When the current account's 5h quota runs out, automatically switch to an available account and relaunch ZCode. Interrupted tasks must be resumed manually from the ZCode task list.",
    "settings.accountsAutoNotifyOk":
      "Auto-switched to \"{name}\" (5h quota exhausted). Resume interrupted tasks from the ZCode task list.",
    "settings.accountsAutoNotifyFail":
      "5h quota exhausted, but no available account to switch to",
    "settings.accountsAutoLogOk": "{time} auto-switched to \"{name}\"",
    "settings.accountsAutoLogFail": "{time} auto-switch failed: no available account",
    "settings.accountsAutoLinuxUnsupported":
      "Account switching is not supported on this platform",
  } satisfies typeof settings),

  ...({
    "theme.title": "Skin",
    "theme.toolbarEntry": "Skin",
    // Sub-page titles / hub entry card titles: no-injection (standalone
    // windows, no skin install needed) / requires skin injection into ZCode
    "theme.sectionStandalone": "No injection · standalone windows",
    "theme.sectionInjected": "Requires ZCode injection",
    // One-line descriptions of the two entry cards on the hub page
    "theme.hubStandaloneDesc":
      "Session HUD and desktop pet — usable without installing the skin",
    "theme.hubInjectedDesc":
      "Live wallpaper skin and injected usage bars — install and inject into ZCode first",
    "theme.loading": "Loading wallpaper state…",

    "theme.cardTitle": "ZCode Live Wallpaper",
    "theme.cardHint":
      "Inject a live video wallpaper into the ZCode desktop app as the chat background. The original app is backed up before installing and can be restored anytime.",
    "theme.version": "Version {v}",

    "theme.statusInstalled": "Installed",
    "theme.statusNotInstalled": "Not installed",
    "theme.statusNeedsReinstall": "Reinstall needed",

    "theme.install": "Install live wallpaper",
    "theme.reinstall": "Reinstall",
    "theme.uninstall": "Restore original",

    "theme.confirmInstallTitle": "Install live wallpaper",
    "theme.confirmInstallDesc":
      "About to inject a live wallpaper into the ZCode desktop app:\n· ZCode app files will be modified; the first time, allow AgentHUD to modify ZCode in the system prompt (an administrator password is usually not required; only certain install locations may ask for one)\n· The original app is backed up automatically first and can be restored anytime\n· ZCode updates invalidate the theme and require a reinstall\nMake sure you understand the risks before continuing.",
    "theme.confirmUninstallTitle": "Restore original",
    "theme.confirmUninstallDesc":
      "ZCode will be replaced with the backup taken before installation; the live wallpaper and its parameters will be removed, restoring the official original.\nThis action may also show the system confirmation prompt (allow AgentHUD to modify ZCode).",
    "theme.confirmQuitNote":
      "ZCode is running and will quit automatically — save your conversations first.",
    // macOS-only update impact note in the confirm dialog (not shown on Windows)
    "theme.confirmMacUpdateNote":
      "Note: On macOS, after modifying the app, ZCode's built-in updater will no longer work (restoring the original won't bring it back). To update ZCode, please re-download it from the official website:",
    "theme.zcodeOfficialSite": "ZCode official site (zcode.z.ai)",

    "theme.needsReinstallBanner":
      "The skin needs to be reinstalled to keep working (ZCode was upgraded, or the install predates the pet feature — see the note below); reinstalling restores everything.",
    "theme.nodeMissingBanner":
      "Node.js was not found — injection depends on it. Install Node.js first (run `brew install node`, or download from nodejs.org), then retry.",
    "theme.backupMissingBanner":
      "The original backup is missing, so restore is unavailable; reinstall ZCode to recover.",

    "theme.installing": "Installing live wallpaper",
    "theme.uninstalling": "Restoring original",
    "theme.stage.precheck": "Prechecking",
    "theme.stage.quit": "Quitting app",
    "theme.stage.extract": "Extracting",
    "theme.stage.inject": "Injecting theme",
    "theme.stage.pack": "Repacking",
    "theme.stage.verify": "Verifying",
    "theme.stage.backup": "Backing up original",
    "theme.stage.replace": "Replacing files",
    "theme.stage.sign": "Re-signing",
    "theme.stage.launch": "Launch check",
    "theme.stage.cleanup": "Cleaning up",
    "theme.stage.done": "Done",
    "theme.stage.error": "Failed",

    "theme.paramsTitle": "Effects",
    "theme.paramsHint":
      "Saved as you drag and applied live within ~1s — no restart needed. Chat, sidebar and right-panel sliders work independently; all other areas keep a fixed ambient opacity.",
    "theme.paramWpBrightness": "Wallpaper brightness",
    "theme.paramWpSaturate": "Saturation",
    "theme.paramWpBlur": "Background blur",
    "theme.paramBaseAlpha": "Ambient tint",
    "theme.paramBaseAlphaHint":
      "Adds a semi-transparent theme-colored layer over the wallpaper to improve overall text readability; 50%+ recommended for light wallpapers.",
    "theme.paramMaskStrength": "Mask strength",
    "theme.paramTextShadow": "Text outline",
    "theme.paramTextShadowHint":
      "Adds a soft outline around UI text for better contrast; raise it for dark themes on light wallpapers, 0 disables it.",
    "theme.paramPanelOpacity": "Chat opacity",
    "theme.paramSidebarOpacity": "Sidebar opacity",
    "theme.paramSidebarRightOpacity": "Right panel opacity",
    "theme.paramPlaybackRate": "Playback speed",
    // Usage bar section (standalone card: font size + opacity + per-turn /
    // session total toggles, live reload)
    "theme.usageTitle": "Usage Bar",
    "theme.usageHint":
      "Look of the per-turn usage line (↑ input ↓ output · × requests · speed) at the end of each ZCode chat turn. Sliders save as you drag and apply live within ~1s.",
    "theme.paramUsageFontSize": "Font size",
    "theme.paramUsageFontSizeHint": "Text size of the usage line (9–16px)",
    "theme.paramUsageOpacity": "Opacity",
    "theme.paramUsageOpacityHint":
      "Text opacity of the usage line; the waiting state of an in-progress turn is fainter",
    // Session total bar toggle (usage.js V5: pinned above the chat input)
    "theme.usageSessionBar": "Session total bar",
    "theme.usageSessionBarHint":
      "Pins the running session total (total tokens plus the input/output/cache read/request breakdown) above the chat input, with live output speed and estimated growth while streaming",
    // Per-turn usage bar toggle (usage.js V19: usage line under each turn)
    "theme.usageTurnBar": "Per-turn usage bar",
    "theme.usageTurnBarHint":
      "Shows each turn's usage at the end of every chat turn (↑ input ↓ output · ⟲ cache read · × requests · speed · TTFT); font size and opacity follow the sliders above",
    // Quota row toggle (usage.js V28/V29: standalone quota line below the input box)
    "theme.usageQuotaBar": "Input quota row",
    "theme.usageQuotaBarHint":
      "Shows the remaining quota under the input box (5-hour/weekly window percentages, small right-aligned text); font size and opacity follow the sliders above",
    // Model speed rows toggle (usage.js V29: per-model speed rows above the
    // input box, split out of the session total bar as an independent container)
    "theme.usageModelRows": "Model speed rows",
    "theme.usageModelRowsHint":
      "Shows per-model speed rows above the input box (latest value color-coded with avg/fast/slow), controlled independently of the session total bar; font size and opacity follow the sliders above",
    // Symbol legend (mirrors the usage.js line format)
    "theme.usageLegend":
      "↑ input (non-cached) · ↓ output · ⟲ cache read · × model requests · t/s output speed · TTFT time to first token · Σ session total tokens (input+output+cache read) · ≈ in-flight output estimate (excluded from totals)",

    // Desktop pet section (the single entry for pet settings: master toggle
    // + injected/floating mode + style/size, persisted in pet.json, applies
    // instantly; injected-pet parameters apply within ~1s via variables.css
    // hot reload)
    "theme.petTitle": "Desktop Pet",
    "theme.petHint":
      "Keeps a pixel pet that animates with ZCode's live work state: by default it lives inside the ZCode chat page (requires the skin installed, draggable), or switch it to a standalone floating window; style and size apply to both modes and take effect immediately.",
    "theme.petEnabled": "Enable desktop pet",
    "theme.petEnabledHint":
      "Master switch: shows the pet in the mode selected below; off hides it entirely",
    "theme.petModeLabel": "Mode",
    "theme.petModeInjected": "Built-in",
    "theme.petModeFloating": "Floating window",
    "theme.petModeHint":
      "Built-in: rendered inside the ZCode chat page, follows the skin's hot reload (~1s) and can be dragged anywhere; requires the live wallpaper skin to be installed. Floating window: a standalone transparent always-on-top pet, independent of the skin, also draggable.",
    "theme.petApplyFail": "Failed to apply pet settings: {msg}",
    "theme.petLoadFail": "Failed to load pet settings: {msg}",
    // Custom pets (phase 3: Petdex-format import, PetStyleSection in petStyles;
    // since V8 the built-in group is the bundled Zhipu girl sheet pet, whose
    // name comes from the list data's meta.name)
    "theme.petGroupBuiltin": "Built-in styles",
    "theme.petGroupCustom": "Custom pets (Petdex)",
    "theme.petBuiltinLoading": "Loading built-in pet…",
    "theme.petCustomEmpty":
      "No custom pets yet. Drop a Petdex pet package onto the window to import one.",
    // Standalone sub-page variant: pet import is this page's only drop
    // semantic — zip / pet.json and bare png/webp sheets are all accepted
    "theme.petImportHintSkin":
      "Import a custom pet: drop a Petdex package onto the window (zip / pet.json with its spritesheet alongside; bare png/webp sheets can be dropped directly too)",
    "theme.petImporting": "Importing pet…",
    "theme.petImportDone": "Pet imported ✓ Tap it in the pet card to use",
    "theme.petImportFail": "Failed to import pet: {msg}",
    "theme.petDelete": "Delete",
    "theme.petDeleteConfirm":
      "Delete custom pet \"{name}\"? If it is currently in use, the built-in style will be restored.",
    "theme.petDeleteFail": "Failed to delete pet: {msg}",
    "theme.paramPetSize": "Pet size",
    "theme.paramPetSizeHint": "Sized by screen-height ratio (about 5.5%–15%); adapts when you switch screens or change resolution",
    // Pet state legend (mirrors the nine states of the pet.js state machine;
    // V9 splits thinking/walking)
    "theme.petLegend":
      "States: sleeping (ZCode idle for over 1 minute or AgentHUD not running) · idle (recent turn activity) · thinking (model planning, no output yet) · typing (output growing; speed tiers by token rate) · tool running (a tool is executing, e.g. command/build) · walking (heading to a new turn / waiting for the next) · celebrating (turn finished, about 3s) · failed (turn failed or cancelled, about 3s)",

    // Session HUD card (standalone always-on-top overlay showing today's
    // active sessions grouped by agent (ZCode/Claude/Codex/Kimi/Cursor)
    // with per-agent today totals and a grand total at the bottom; config
    // in session-hud.json, no skin dependency; freely resizable by
    // dragging, height auto-fits until resized, up to 3 rows per agent
    // with in-group folding. This card only keeps the master switch, the
    // active window and the display toggles — font size and opacity live
    // in the overlay's own settings panel)
    "theme.hudTitle": "Session HUD",
    "theme.hudHint":
      "A standalone transparent always-on-top overlay: the button at its top right opens a settings panel for the font size and window opacity. It aggregates today's data across ZCode/Claude/Codex/Kimi/Cursor: per-agent sections show the agent's today token total (header \"Name · Today Σ\") and its active session rows (model, state, Σ/↑/↓/⟲/× totals with speed t/s · TTFT, same convention as the in-chat session bar), sorted by recent activity, up to 3 rows per agent with in-group folding; the bottom Today row is the grand total across all agents. No skin required, drag to move, drag an edge to resize freely.",
    "theme.hudEnabled": "Enable Session HUD",
    "theme.hudEnabledHint": "Master switch: off hides the overlay and stops polling",
    "theme.hudApplyFail": "Failed to apply Session HUD settings: {msg}",
    "theme.hudLoadFail": "Failed to load Session HUD settings: {msg}",
    "theme.hudWindowLabel": "Active window",
    "theme.hudWindow5m": "5 min",
    "theme.hudWindow10m": "10 min",
    "theme.hudWindow30m": "30 min",
    "theme.hudWindowAll": "All",
    "theme.hudWindowHint":
      "Only sessions active (model requests or sent messages) within this window are shown; \"All\" falls back to the last 24 hours; up to 3 visible rows per agent, extras folded within the group",
    "theme.hudShowTokens": "Data row",
    "theme.hudShowTokensHint":
      "Per-session totals in the session-bar convention: Σ total · ↑ input · ↓ output · ⟲ cache read · × requests · speed t/s · TTFT; also shows each group header's \"Today Σ\" subtotal and the per-model speed rows below the list; turning it off hides the Today subtotals in group headers and the grand total row at the bottom",
    "theme.hudShowModel": "Model name",
    "theme.hudShowModelHint": "Show the session's most recently used model",
    "theme.currentWallpaper": "Current wallpaper",
    "theme.noWallpaper": "No wallpaper selected",
    "theme.lightWallpaperPreset": "Light wallpaper fit",
    "theme.lightWallpaperPresetDesc":
      "One click to apply the recommended values for light wallpapers (ambient tint 55%, mask 35%, brightness 60%, text outline 60%).",
    "theme.lightWallpaperPresetFail": "Failed to apply light wallpaper preset: {msg}",
    "theme.resetParams": "Reset to defaults",
    "theme.paramsSavedFlash": "Saved ✓",

    "theme.libraryTitle": "Wallpaper Library",
    "theme.libraryHint":
      "Click a wallpaper to apply it instantly (live within ~1s). Videos and images supported.",
    "theme.libraryDirLabel": "Wallpaper folder",
    "theme.libraryDirEmpty":
      "Not set. Drop a folder onto the zone below to set it as the wallpaper folder.",
    "theme.libraryClearDir": "Clear folder",
    "theme.libraryEmpty":
      "No wallpapers yet. Drop video / image files to import, or set a wallpaper folder to scan automatically.",
    "theme.libraryLoading": "Loading wallpapers…",
    "theme.libraryKindVideo": "Video",
    "theme.libraryKindImage": "Image",
    "theme.defaultWallpaperName": "Default Aurora",
    "theme.selecting": "Switching…",
    // Preview card: placeholder when the media file fails to load
    "theme.previewUnavailable": "Preview unavailable",

    // Drop zone (wallpaper / folder entry: the native file dialog is invisible
    // on dockless Accessory apps)
    "theme.dropWallpaperHint":
      "Drop a video / image file to change the wallpaper (mp4 / webm / mov / jpg / png / webp), or drop a folder to set the wallpaper folder",
    "theme.dropWallpaperBusy": "Processing dropped item…",

    "theme.installDone": "Live wallpaper installed ✓",
    "theme.uninstallDone": "ZCode restored to original ✓",
    "theme.wallpaperSet": "Wallpaper set to {name} ✓",
    "theme.wallpaperDirSet": "Wallpaper folder set: {path} ✓",
    "theme.wallpaperDirCleared": "Wallpaper folder cleared ✓",

    "theme.loadStateFail": "Failed to load wallpaper state: {msg}",
    "theme.loadParamsFail": "Failed to load effects: {msg}",
    "theme.setParamsFail": "Failed to save parameters: {msg}",
    "theme.setWallpaperFail": "Failed to set wallpaper: {msg}",
    "theme.selectWallpaperFail": "Failed to switch wallpaper: {msg}",
    "theme.setWallpaperDirFail": "Failed to set wallpaper folder: {msg}",
    "theme.listWallpapersFail": "Failed to load wallpapers: {msg}",
    "theme.resetParamsFail": "Failed to reset parameters: {msg}",
    "theme.installFail": "Install failed: {msg}",
    "theme.uninstallFail": "Restore failed: {msg}",

    // Restart ZCode (full reload of injected theme assets)
    "theme.restartZcode": "Restart ZCode",
    "theme.restarting": "Restarting…",
    "theme.confirmRestartTitle": "Restart ZCode",
    "theme.confirmRestartDesc":
      "ZCode will quit and relaunch (takes a few seconds) so the injected live wallpaper assets fully reload. If ZCode isn't running, it will simply be launched.\nSave any conversations in progress first.",
    "theme.restartDone": "ZCode restarted — injected assets fully reloaded ✓",
    "theme.launchDone": "ZCode wasn't running — launched directly ✓",
    "theme.restartFail": "Failed to restart ZCode: {msg}",
  } satisfies typeof theme),

  ...({
    // Projects tab (project list + session drill-down)
    "projects.tab": "Projects",
    "projects.title": "Project usage",
    "projects.back": "Back to projects",

    "projects.unknown": "Unknown project",
    "projects.loadFail": "Failed to load projects: {msg}",
    "projects.empty": "No project data in this range",
    "projects.emptyHint":
      "Projects with sessions in the selected range will show up here.",
    "projects.agentMix": "Agent mix",
    "projects.sessionCount": "{count} sessions",

    "projects.sessionsOf": "Sessions of {name}",
    "projects.agentFilter": "Agent",
    "projects.all": "All",
    "projects.loadMore": "Load more ({loaded}/{total})",
    "projects.loadingMore": "Loading…",
    "projects.noMore": "All loaded",
    "projects.sessionsEmpty": "No sessions for this project in the selected range",
    "projects.sessionsLoadFail": "Failed to load sessions: {msg}",

    "projects.models": "Models",
    "projects.in": "In",
    "projects.out": "Out",
    "projects.cacheRead": "Cache read",
    "projects.cacheWrite": "Cache write",
    "projects.duration": "Duration",
    "projects.speed": "Speed",
    "projects.ttft": "First tok",
  } satisfies typeof projects),

  ...({
    "share.button": "Share card",
    "share.title": "Generate share card",
    "share.totalCost": "Total cost",
    "share.totalTokens": "Total tokens",
    "share.totalRequests": "Total requests",
    "share.topModels": "Top 5 models",
    "share.heatTitle": "Token activity",
    "share.footer": "Generated by AgentHUD",
    "share.save": "Save image",
    "share.saving": "Saving…",
    "share.saved": "Saved ✓ {path}",
    "share.saveFail": "Save failed: {msg}",
    "share.close": "Close",
  } satisfies typeof share),

  ...({
    // ===== Credential card =====
    "credentials.cardTitle": "Credentials",
    "credentials.emptyTitle": "No credentials yet",
    "credentials.add": "Add credential",
    "credentials.edit": "Edit credential",
    "credentials.countBadge": "{n}",

    // ===== Add / edit dialog =====
    "credentials.label": "Label",
    "credentials.labelPlaceholder": "e.g. Pro plan",
    "credentials.labelHint": "Up to 32 characters, to tell credentials apart",
    "credentials.secret": "Secret",
    "credentials.secretPlaceholderApiKey": "Paste the API key",
    "credentials.secretPlaceholderCookie": "Paste the cookie string",
    "credentials.secretPlaceholderToken": "Paste the access token",
    "credentials.secretKeepHint": "Leave empty to keep the saved secret unchanged",
    "credentials.secretHintCookie":
      "Paste the Cookie request header or the full cURL command copied from your browser",
    "credentials.secretShow": "Show secret",
    "credentials.secretHide": "Hide secret",
    "credentials.region": "Region",
    "credentials.regionNone": "Default",
    "credentials.regionCn": "China",
    "credentials.regionGlobal": "Global",
    "credentials.secretRequired": "Please enter the secret",
    "credentials.labelRequired": "Label is required",

    // ===== Kind badges =====
    "credentials.kindApiKey": "API Key",
    "credentials.kindCookie": "Cookie",
    "credentials.kindToken": "Token",

    // ===== Check status / time =====
    "credentials.checkOk": "Verified",
    "credentials.checkFail": "Check failed",
    "credentials.notChecked": "Not checked",
    "credentials.updatedAt": "Updated {time}",

    // ===== Delete confirm / feedback =====
    "credentials.deleteTitle": "Delete credential",
    "credentials.deleteConfirm": "Delete \"{name}\"? This cannot be undone.",
    "credentials.saved": "Saved ✓",
    "credentials.saveFail": "Save failed: {msg}",
    "credentials.loadFail": "Failed to load credentials: {msg}",
    "credentials.resetFile": "Reset credentials file",
    "credentials.resetTitle": "Reset credentials file",
    "credentials.resetConfirm":
      "This clears every stored credential for this service and rebuilds an empty file (to repair a corrupted credentials file). Deleted credentials cannot be recovered. Reset now?",
    "credentials.resetDone": "Reset ✓",
    "credentials.resetFail": "Reset failed: {msg}",

    // ===== Add-service entry (tab-bar "+" button / service picker / settings shortcut) =====
    "credentials.addService": "Add service",
    "credentials.addServiceTitle": "Add subscription service",
    "credentials.addServiceHint":
      "Pick a service and paste its credential; add as many subscriptions as you need",
    "credentials.comingSoon": "Quota coming soon",
    "credentials.entriesCount": "{n} credentials",

    // ===== Generic quota panel =====
    "credentials.quotaPending":
      "Quota queries are coming soon. Your credential is ready; usage and quota will appear here once connected.",
    "credentials.quotaRefreshing": "Querying quota, please wait…",
    "credentials.noCredentialEntry":
      "This service has no credential entry: it queries credentials already on this machine or local data, and none is available right now.",
    "credentials.entryError": "Query failed",
    "credentials.entryExpired": "Expired",
    "credentials.entryPending": "Pending",
    "credentials.balance": "Balance",
    "credentials.balanceGranted": "{amount} granted",
    "credentials.balanceToppedUp": "{amount} topped up",
    // Absolute line of a quota window (QuotaEntryCard): the percentage on the
    // right is always "% left" (matching Z.ai / Codex / Cursor), so the absolute
    // values must say "Used" explicitly to avoid being read as remaining.
    // {amount} already carries the unit when there is one (e.g. "700 calls")
    "credentials.windowUsedAbs": "Used {amount} / {total}",

    // ===== Quota window titles (QuotaEntryCard maps by window.key; the titles
    //      sent by the Rust provider modules are hardcoded Chinese, translated
    //      here for the English UI. Unknown keys fall back to the Rust title.)
    //      zh picks the most common wording across providers. =====
    "credentials.windowTitle.hour5": "5-hour",
    "credentials.windowTitle.weekly": "Weekly",
    "credentials.windowTitle.monthly": "Monthly",
    "credentials.windowTitle.interval": "Current window",
    "credentials.windowTitle.credits": "Credits",
    "credentials.windowTitle.quota": "Token quota",
    "credentials.windowTitle.fuel": "Fuel pack",
    "credentials.windowTitle.sub_credits": "Subscription credits",
    "credentials.windowTitle.topup_credits": "Top-up credits",
    "credentials.windowTitle.opus_weekly": "Opus weekly",
    "credentials.windowTitle.sonnet_weekly": "Sonnet weekly",
    "credentials.windowTitle.extra_usage": "Extra usage",
    "credentials.windowTitle.pro": "Pro model",
    "credentials.windowTitle.flash": "Flash model",
    "credentials.windowTitle.auto": "Auto",
    "credentials.windowTitle.api": "API",

    // ===== Optional-credentials hint for local-type providers (data read from
    //      local CLI login state / DB; credentials are not required) =====
    "credentials.localEmptyTitle": "Local data source, no credentials needed",
    "credentials.localEmptyHint":
      "This service reads the local CLI login state automatically; no credentials required. You can still add one manually.",
    // Dual-source (local DB + API key, either works): when no local data exists,
    // do not claim "no credentials needed" — the user needs the key entry point
    "credentials.localEmptyTitleDual": "No local data detected yet",
    "credentials.localEmptyHintDual":
      "Install the OpenCode CLI and sign in to the Go plan to read the local database automatically. Without the CLI it still works — add an API key to query the official endpoint directly.",
    // MiniMax Code is dual-source too, but the text above is OpenCode-specific;
    // override it via CredentialsCard's dualHintKey on the MiniMax panel
    "credentials.localEmptyHintDualMinimax":
      "Install MiniMax Code and sign in to read local usage automatically. Without it, adding a Coding Plan API key (starting with sk-cp-) still works — it queries the official endpoint for plan quota.",

    // ===== Per-provider guides (placeholders until each service is wired up) =====
    // guideBrief.<provider> is the first sentence shown in the empty state
    // (where to sign in / get the key); the full guide.<provider> (region
    // prerequisites, required cookie items, etc.) is revealed via "View setup steps"
    "credentials.guideMore": "View setup steps",
    "credentials.guideLess": "Hide steps",
    "credentials.guideBrief.claude":
      "Copy the sk-ant-oat OAuth access token from ~/.claude/.credentials.json on this machine and paste it here.",
    "credentials.guideBrief.cursor":
      "Sign in to cursor.com and paste the Cookie, or the local Cursor login state is read automatically.",
    "credentials.guideBrief.grok":
      "Install the Grok CLI and run grok login to read the local login state automatically; you can also copy the key from ~/.grok/auth.json and add it as a Token.",
    "credentials.guideBrief.opencodego":
      "Subscribe to Go / Go Plus in the OpenCode Console (opencode.ai/console) and paste the API key here to query usage straight from the official endpoint.",
    "credentials.guideBrief.qoder":
      "Sign in to Qoder (global: qoder.com / China: qoder.com.cn).",
    "credentials.guideBrief.minimax":
      "Create a Coding Plan API key under \"API Keys\" on the MiniMax platform.",
    "credentials.guideBrief.moonshot":
      "Create an API key under \"API Key Management\" on the Moonshot platform.",
    "credentials.guideBrief.deepseek":
      "Create an API key under \"API Keys\" on the DeepSeek platform (platform.deepseek.com).",
    "credentials.guideBrief.longcat":
      "Sign in to longcat.chat and open the platform/usage page.",
    "credentials.guideBrief.mimo":
      "Sign in to the Xiaomi MiMo platform (platform.xiaomimimo.com).",
    "credentials.guideBrief.alibaba":
      "Create a DashScope API key in the Alibaba Cloud Model Studio console and subscribe to a Coding Plan.",
    "credentials.guideBrief.alibabatoken":
      "Sign in to the Alibaba Cloud Bailian console subscription page (China / Global).",
    "credentials.guideBrief.stepfun":
      "Sign in to the StepFun platform (platform.stepfun.com).",
    "credentials.guideBrief.doubao":
      "Get an API key from Volcengine Ark and paste it here.",
    "credentials.guideBrief.kimi":
      "OAuth web sign-in is recommended (credentials are saved automatically); you can also paste an OAuth token (refresh_token) or an API key.",
    "credentials.guide.kimi":
      "Click \"Web sign-in\" and finish the Kimi authorization in your browser — the credential is saved automatically as a refresh_token (no Kimi Code CLI needed); or switch the credential type to API Key and paste it. The region must match the account's site (choose Global for global accounts, web sign-in included); 5-hour/weekly/monthly quota and fuel-package balance will appear automatically once added.",
    // ===== Kimi OAuth web sign-in (device code flow) =====
    "credentials.oauthEntryTitle": "Web sign-in (recommended)",
    "credentials.oauthEntryButton": "Web sign-in",
    "credentials.oauthEntryHint":
      "Finish the Kimi authorization in your browser; the credential is saved automatically — no manual token pasting. For global accounts, select Global below first",
    "credentials.oauthTitle": "Kimi web sign-in",
    "credentials.oauthStepHint":
      "1. Click \"Open authorization page\" and sign in / approve in your browser; 2. If the page asks for a code, paste the confirmation code below.",
    "credentials.oauthRegionCurrent": "Region: {region}",
    "credentials.oauthCopy": "Copy code",
    "credentials.oauthCopied": "Copied",
    "credentials.oauthCopyFail": "Copy failed — please copy manually",
    "credentials.oauthStarting": "Starting sign-in…",
    "credentials.oauthWaiting": "Waiting for authorization — please confirm in the newly opened page…",
    "credentials.oauthValidFor": "(code valid for {minutes} min)",
    "credentials.oauthSuccess": "Signed in, credential saved",
    "credentials.oauthDenied": "Authorization denied; start again if you want to sign in",
    "credentials.oauthExpired": "The authorization code has expired — please start again",
    "credentials.oauthBack": "Back to manual input",
    "credentials.oauthOpen": "Open authorization page",
    "credentials.oauthRetry": "Start again",
    "credentials.oauthDone": "Signed in via browser, credential saved",
    // ===== Kimi credential type switch (OAuth token / API key) =====
    "credentials.kindLabel": "Credential type",
    "credentials.kindOAuthToken": "OAuth token",
    "credentials.secretPlaceholderOauthToken":
      "Paste the OAuth token (refresh_token)",
    "credentials.guide.claude":
      "Copy claudeAiOauth.accessToken (an sk-ant-oat OAuth access token) from ~/.claude/.credentials.json on this machine — or from another machine where the account is logged in — and paste it here to show that account's subscription quota.",
    "credentials.guide.cursor":
      "Sign in to cursor.com → copy the Cookie request header of any request (starting with WorkosCursorSessionToken) from the F12 Network panel → paste it here; that account's plan quota will appear automatically. When the Cursor app is signed in on this machine, the main panel reads the local login state automatically — no credentials needed.",
    "credentials.guide.gemini":
      "Install the Gemini CLI and sign in with gemini in your terminal; usage and quota are read from the local login state automatically — no credentials needed.",
    "credentials.guide.grok":
      "Install the Grok CLI and run grok login to read your monthly quota from the local login state automatically; you can also copy the key from ~/.grok/auth.json and add it as a Token.",
    "credentials.guide.qoder":
      "Sign in to Qoder (global: qoder.com / China: qoder.com.cn — pick the region matching your credential) → open the console/account usage page → copy the Cookie request header of any request (or the whole \"Copy as cURL\") from the F12 Network panel → paste it here; big-model credit usage will appear automatically.",
    "credentials.guide.opencodego":
      "Two ways, either is enough: (1) Install the OpenCode CLI and sign in to the Go plan — usage is estimated automatically from the local database (~/.local/share/opencode/) with no credentials; (2) Subscribe to Go / Go Plus in the OpenCode Console (opencode.ai/console) and paste the API key — this queries the official endpoint (/zen/go/v1/usage) for authoritative 5-hour / weekly / monthly percentages and reset times, and does not consume plan quota. Use (2) if you do not have the CLI installed.",
    "credentials.guide.minimax":
      "Create a Coding Plan API key (an sk-cp- token) under \"API Keys\" on the MiniMax platform (platform.minimaxi.com; global: the minimax.io console) and paste it here; current-window and weekly usage will appear automatically. The region must match the site where the key was created (for a global-site key, select Global).",
    "credentials.guide.moonshot":
      "Create an API key under \"API Key Management\" on the Moonshot platform (platform.moonshot.cn; global: platform.moonshot.ai) and paste it here; your balance with granted/topped-up breakdown will appear automatically. The region must match the site where the key was created (for a global-site key, select Global).",
    "credentials.guide.deepseek":
      "Create an API key under \"API Keys\" on the DeepSeek platform (platform.deepseek.com) and paste it here; your balance with granted/topped-up breakdown will appear automatically.",
    "credentials.guide.longcat":
      "Sign in to longcat.chat → open the platform/usage page → copy the Cookie request header of any request (or the whole \"Copy as cURL\") from the F12 Network panel → paste it here; token quota and fuel-package balance will appear automatically.",
    "credentials.guide.mimo":
      "Sign in to the Xiaomi MiMo platform (platform.xiaomimimo.com) → copy the Cookie request header of any request (or the whole \"Copy as cURL\", which must include both api-platform_serviceToken and userId) from the F12 Network panel → paste it here; your account balance and current credit usage will appear automatically.",
    "credentials.guide.alibaba":
      "Create a DashScope API key under \"API-KEY Management\" in the Alibaba Cloud Model Studio console (China: bailian.console.aliyun.com / Global: modelstudio.console.alibabacloud.com — pick the region matching your credential), subscribe to a Coding Plan (Tongyi Lingma), and paste it here; 5-hour/weekly/monthly usage will appear automatically.",
    "credentials.guide.alibabatoken":
      "Sign in to the Alibaba Cloud Bailian console subscription page (China: bailian.console.aliyun.com / Global: modelstudio.console.alibabacloud.com — pick the region matching your credential) → filter \"tokenplan\" requests in the F12 Network panel → copy the Cookie request header of that request (or the whole \"Copy as cURL\") → paste it here; the Team credit pool or Personal/Solo 5-hour/7-day rolling usage will appear automatically (Personal/Solo needs the Cookie of a request under the quota host bailian-cs.console.aliyun.com / bailian-singapore-cs.alibabacloud.com).",
    "credentials.guide.stepfun":
      "Sign in to the StepFun platform (platform.stepfun.com) → find any Dashboard request in the F12 Network panel → copy the Oasis-Token value from the request Cookie and paste it here; 5-hour/weekly usage or subscription/top-up credit balances will appear automatically.",
    "credentials.guide.doubao":
      "Get an API key from Volcengine Ark and paste it here; usage and quota will appear automatically once connected.",
  } satisfies typeof credentials),

  ...({
    "dashboard.subtitle": "Usage & quota console",

    // Self-drawn title bar window controls (tooltips + aria-labels)
    "dashboard.titlebar.minimize": "Minimize",
    "dashboard.titlebar.maximize": "Maximize",
    "dashboard.titlebar.restore": "Restore",
    "dashboard.titlebar.close": "Close",

    "dashboard.overview": "Overview",
    "dashboard.logs": "Request logs",
    "dashboard.compare": "Weekly compare",
    "dashboard.pricing": "Pricing",

    "dashboard.overview.desc":
      "Aggregated usage, cost and trends across services.",
    "dashboard.logs.desc":
      "Per-request details: time, model, token breakdown, cost and duration; this machine only, filterable by source.",
    "dashboard.compare.desc":
      "Weekly quota left per subscription, used-% trends over the selected range, and local vs. other-device token comparison.",
    "dashboard.pricing.desc":
      "Model unit-price maintenance and pricing table updates.",

    "dashboard.mainWindow": "Main window",
    "dashboard.openMainWindow": "Open main window",
    // Inline hint when the dashboard window cannot be created
    "dashboard.openMainWindowFailed": "Open failed",

    // ===== Overview page: header & states =====
    "dashboard.overview.rangeLabel": "Time range",
    "dashboard.overview.range.today": "Today",
    "dashboard.overview.range.7d": "Last 7 days",
    "dashboard.overview.range.30d": "Last 30 days",
    "dashboard.overview.range.90d": "Last 90 days",
    // Data scope (this machine / all devices): defaults to All devices to match
    // the menu-bar title scope
    "dashboard.overview.scopeLabel": "Data scope",
    "dashboard.overview.scope.local": "This machine",
    "dashboard.overview.scope.all": "All devices",
    // Multi-device sync off: scope control is disabled, explain why
    "dashboard.overview.syncDisabledHint":
      "Multi-device sync is off, so only this machine's data is shown. Turn on sync to merge other devices (same scope as the menu-bar title).",
    // Remote fetch failed while in All-devices scope: degraded to local-only
    "dashboard.overview.remoteDegraded":
      "Other devices' data could not be fetched this time; showing this machine only",
    // Progressive render: local data is visible while remote devices still merge
    "dashboard.overview.remoteMerging": "Merging remote…",
    // Marker next to tokens-in-range when the row includes other devices' usage
    "dashboard.overview.remoteIncluded": "incl. remote",
    "dashboard.overview.remoteIncludedHint":
      "Includes usage uploaded from other devices",
    "dashboard.overview.lastUpdated": "Updated {time}",
    "dashboard.overview.errorBanner": "Failed to load data: {error}",
    "dashboard.overview.partialFailures":
      "Some agents failed to load and were skipped: {names}",
    "dashboard.overview.emptyTitle": "No usage data yet",
    "dashboard.overview.emptyHint":
      "No local usage collected from any agent yet.\nOpen the matching agent's stats in the panel once, or install and run a supported CLI, then reload.",
    "dashboard.overview.retry": "Reload",
    // Dedicated error state when every agent failed (no usable data at all)
    "dashboard.overview.errorTitle": "Failed to load",
    "dashboard.overview.errorReason": "Reason: {error}",

    // ===== Overview page: metric cards =====
    // Cards 1/2 have a fixed two-column sub line (in|out, avg|today); each
    // column truncates on its own, so no "A · B" joined single line here
    "dashboard.overview.rangeTokens": "Tokens in range",
    "dashboard.overview.subInput": "In {value}",
    "dashboard.overview.subOutput": "Out {value}",
    "dashboard.overview.estCost": "Estimated cost",
    "dashboard.overview.subAvg": "avg {value}/day",
    "dashboard.overview.subToday": "today {value}",
    "dashboard.overview.requestCount": "Requests",
    "dashboard.overview.dailyAvgRequests": "avg {n}/day",
    "dashboard.overview.todayTokens": "Tokens today",
    "dashboard.overview.todayRequests": "{n} requests today",
    // 4th card on the "today" range (the today-token card would duplicate
    // "Tokens in range"): shows the change vs yesterday plus yesterday's tokens
    "dashboard.overview.vsYesterday": "vs yesterday",
    "dashboard.overview.yesterdayTokens": "Yesterday {n}",

    // ===== Overview page: trend chart =====
    "dashboard.overview.trendTitle": "Usage trend",
    // Metric switch group (aria-label): cost / token / requests
    "dashboard.overview.metricLabel": "Metric",
    "dashboard.overview.trendSub": "{range} · {total} total",
    "dashboard.overview.seriesInput": "Input side",
    "dashboard.overview.seriesOutput": "Output",
    "dashboard.overview.peak": "Peak {value} ({date})",
    "dashboard.overview.ttTotal": "Total",
    "dashboard.overview.ttOutput": "Output",
    "dashboard.overview.ttInput": "Input side",
    "dashboard.overview.ttRequests": "Requests",
    "dashboard.overview.ttCost": "Cost",
    "dashboard.overview.splitNote":
      "* Cursor and legacy data have no per-day input/output split; their tokens count as input side",

    // ===== Overview page: heatmap (52 weeks, ZCode-style, three modes) =====
    "dashboard.overview.heatTitle": "Activity heatmap",
    "dashboard.overview.heatSub": "Last 52 weeks (fixed window)",
    // Total covers the 52 natural weeks shown in the grid
    "dashboard.overview.heatTotal": "52-week total",
    "dashboard.overview.heatActiveDays": "Active days",
    "dashboard.overview.heatActiveAvg": "Avg / active day",
    "dashboard.overview.heatLess": "Less",
    "dashboard.overview.heatMore": "More",
    // The legend keeps only levels 1..4; the no-data grey cell gets its own label
    "dashboard.overview.heatNoData": "No data",
    // Aggregation mode switch (pure client-side, no refetch)
    "dashboard.overview.heatModeLabel": "Aggregation",
    "dashboard.overview.heatMode.daily": "Daily",
    "dashboard.overview.heatMode.weekly": "Weekly",
    "dashboard.overview.heatMode.cumulative": "Cumulative",
    // Column tooltips in weekly / cumulative modes
    "dashboard.overview.heatWeekTotal": "This week",
    "dashboard.overview.heatCumTotal": "Cumulative to week",
    // Cursor and other devices only cover the last 90 days
    "dashboard.overview.heatFootnote":
      "The last 90 days include Cursor and other devices; earlier cells are local-only (excluding Cursor).",

    // ===== Overview page: agent table =====
    "dashboard.overview.agentTitle": "Agent usage",
    "dashboard.overview.agentSub": "Sorted by tokens in range",
    "dashboard.overview.colAgent": "Agent",
    "dashboard.overview.colToday": "Tokens today",
    "dashboard.overview.colRange": "Tokens in range",
    "dashboard.overview.colCost": "Cost in range",
    "dashboard.overview.colQuota": "Weekly quota left",
    // 5-hour window left (Z.ai / Codex / Claude / Kimi snapshots and the
    // same-named provider-quota window; agents without it show "--")
    "dashboard.overview.colHour5": "5h",
    "dashboard.overview.colHour5Hint":
      "Quota left in the rolling 5-hour window (only for subscriptions that have it, otherwise --)",
    "dashboard.overview.hour5ResetHint": "5h window · resets {time}",
    "dashboard.overview.unpriced": "Unpriced",
    "dashboard.overview.agentEmpty": "No agent usage data yet",
    "dashboard.overview.billingCycle": "billing cycle",
    // Short label for the weekly quota row in the agent table (pairs with "Mo" below)
    "dashboard.overview.weekShort": "Wk",
    // Monthly quota second-line label (provider monthly window, e.g. OpenCode Go)
    "dashboard.overview.monthlyShort": "Mo",
    "dashboard.overview.costFootnote":
      "Same source as the menu bar: this machine + other devices (when sync is on), estimated with this machine's price table (USD price × current FX rate); not a billable amount. Cursor details are not uploaded, so Cursor counts are local-only.",

    // ===== Compare page (wide window) =====
    "dashboard.compare.device": "Device",
    "dashboard.compare.deviceAll": "All (merged)",
    "dashboard.compare.rangeLabel": "Time range",
    "dashboard.compare.errorBanner": "Failed to load data: {error}",
    "dashboard.compare.remoteHistoryFailed":
      "Failed to load remote quota history: {msg}",
    // Summary mode fell back to local-only because remote history failed
    "dashboard.compare.remoteDegraded":
      "Other-device data was unavailable this time; local figures only",
    "dashboard.compare.emptyTitle": "No weekly quota data yet",
    "dashboard.compare.emptyHint":
      "Weekly quotas are sampled automatically while the app runs.\nKeep it running to collect data, or open the matching agent's stats once before refreshing.",
    "dashboard.compare.retry": "Reload",
    "dashboard.compare.matrixTitle": "Weekly quota left by agent",
    "dashboard.compare.matrixSub":
      "Latest sample per series (local + other devices, deduplicated)",
    // The card subtitle states the most recent sampling time directly
    // (was a native title tooltip on each quota block)
    "dashboard.compare.matrixSampledLatest": "Latest sample {time}",
    // Accessible name / tooltip for the ↻ reset time (the glyph alone says nothing)
    "dashboard.compare.matrixNextReset": "Next reset",
    "dashboard.compare.matrixEmpty": "No quota series to show",
    "dashboard.compare.matrixNoData": "No data",
    "dashboard.compare.trendTitle": "Weekly quota trend",
    "dashboard.compare.trendSub": "{range} · used ratio over time",
    "dashboard.compare.trendEmpty": "No quota samples in this range",
    "dashboard.compare.trendLegendHint":
      "Click a legend item to show / hide a series",
    "dashboard.compare.trendShowAll": "Show all",
    "dashboard.compare.trendFootnote":
      "Lines show each series' weekly quota used % (= 100 − remaining), connected by sample time; periods without samples are not connected.",
    "dashboard.compare.trendSnapshotNote":
      "Quota snapshots only cover the last {days} days; earlier dates cannot be selected.",
    "dashboard.compare.tokensTitle": "Agent token comparison",
    "dashboard.compare.tokensSub": "{range} · local + other devices merged",
    "dashboard.compare.tokensLocal": "Local",
    "dashboard.compare.tokensRemote": "Other devices",
    "dashboard.compare.tokensRequests": "{count} requests",
    "dashboard.compare.tokensEmpty": "No token usage to compare in this range",
    "dashboard.compare.tokensFootnote":
      "Tokens = actual usage per agent in the selected range: local aggregation plus uploaded data from synced devices. Cursor details are not uploaded, so it is local-only.",
    "dashboard.compare.footnote":
      "Quota % = used ratio of each subscription's weekly window (local and remote samples deduplicated by time + account, keeping the higher value); tokens = actual usage of enabled agents in the selected range. They are different metrics.",

    // ===== Pricing page (wide window) =====
    "dashboard.pricing.currencyLabel": "Currency",
    "dashboard.pricing.cny": "¥ CNY",
    "dashboard.pricing.usd": "$ USD",
    // View-only currency switch for this page (the persisted global currency
    // follows the interface language and lives in the panel)
    "dashboard.pricing.currencyViewOnly": "this page only",
    "dashboard.pricing.currencyViewOnlyHint":
      "View-only for this page; does not change the global currency (the global currency follows the interface language and can be switched in the floating panel).",
    // Unsaved-changes hint + Save button dirty label
    "dashboard.pricing.dirtyHint": "{count} unsaved",
    "dashboard.pricing.saveDirty": "Save *",
    // Light confirmation after applying price updates
    "dashboard.pricing.appliedCount": "Applied {count}",
    "dashboard.pricing.unitHint":
      "Unit: $/million tokens. CNY is converted for display at rate {rate}.",
    "dashboard.pricing.fxRefresh": "Refresh FX rate",
    "dashboard.pricing.fxRefreshing": "Updating…",
    "dashboard.pricing.checkUpdates": "Check price updates",
    "dashboard.pricing.syncModelsdev": "Sync from models.dev",
    "dashboard.pricing.syncing": "Syncing…",
    "dashboard.pricing.syncFailHint":
      "configure a network proxy in Settings and retry",
    "dashboard.pricing.upToDate": "Up to date ✓",
    "dashboard.pricing.missingCount": "{count} models unpriced",
    "dashboard.pricing.tableTitle": "Model prices",
    "dashboard.pricing.tableSub":
      "Input / output / cache read in $ per million tokens; click Save (top right) to persist edits",
    "dashboard.pricing.searchPlaceholder": "Search model ID / display name",
    "dashboard.pricing.filterEmpty": "No matching models",
    "dashboard.pricing.count": "{n} models",
    "dashboard.pricing.colModel": "Model",
    "dashboard.pricing.colInput": "Input",
    "dashboard.pricing.colOutput": "Output",
    "dashboard.pricing.colCacheRead": "Cache read",
    "dashboard.pricing.colSource": "Source",
    "dashboard.pricing.colActions": "Actions",
    "dashboard.pricing.sourceConfigured": "Priced",
    "dashboard.pricing.sourceUnpriced": "Unpriced",
    "dashboard.pricing.sourceManual": "Manual",
    "dashboard.pricing.removeModel": "Remove from list",
    "dashboard.pricing.addTitle": "Add model price",
    "dashboard.pricing.addIdLabel": "Model ID",
    "dashboard.pricing.addPlaceholder": "e.g. glm-5.4-flash",
    "dashboard.pricing.add": "Add",
    "dashboard.pricing.addEmptyId": "Enter a model ID",
    "dashboard.pricing.addExists":
      "This model is already listed (case / invisible-character variants count as the same model)",
    "dashboard.pricing.hiddenTitle": "Hidden models",
    "dashboard.pricing.hiddenCount": "{count} models hidden",
    "dashboard.pricing.restore": "Restore",
    "dashboard.pricing.noModels":
      "No model data yet. Make sure some usage has been recorded, or add a model price manually.",
    "dashboard.pricing.footnote":
      "Prices are a local unit-price configuration used only for this app's cost estimates (not a billable amount); applying reference prices never overwrites unselected entries.",

    // ===== Price diff panel (built-in reference / models.dev) =====
    "dashboard.pricing.diffTitle": "Price updates · built-in reference",
    "dashboard.pricing.diffTitleModelsdev": "Price updates · models.dev",
    "dashboard.pricing.diffHint":
      "Reference comparison is offline. Items marked ≈ matched a base model by variant name; please confirm before applying.",
    "dashboard.pricing.diffHintModelsdev":
      "Official vendor prices from models.dev (whitelisted vendors only, no proxy channels). Items marked ≈ still matched a base model by variant name; please confirm before applying.",
    "dashboard.pricing.modelsdevFetchedAt": "Data fetched: {time}",
    "dashboard.pricing.modelsdevFromCache": "cached (offline)",
    "dashboard.pricing.missingWarn":
      "{count} models are in use but have no price (cost counted as 0):",
    "dashboard.pricing.addPriceBelow":
      "Add prices manually in the model list below",
    "dashboard.pricing.badgeNew": "New",
    "dashboard.pricing.badgeChanged": "Changed",
    "dashboard.pricing.refFrom":
      "Reference price taken from base model {model} (same family variant); confirm before applying",
    "dashboard.pricing.old": "old",
    "dashboard.pricing.unpriced": "unpriced",
    "dashboard.pricing.noDiff": "No differences, prices are up to date",
    "dashboard.pricing.collapse": "Collapse",
    "dashboard.pricing.applying": "Applying…",
    "dashboard.pricing.applySelected": "Apply selected",
    "dashboard.pricing.applySelectedCount": "Apply selected ({count})",

    "dashboard.logs.rangeLabel": "Time range",
    "dashboard.logs.range.all": "All",
    // Header auto-refresh interval (Off / 5s / 10s / 30s / 60s, default 30s):
    // the trigger shows "Auto-refresh" + the active option, the full sentence
    // lives in title/aria-label to keep the header compact
    "dashboard.logs.autoRefresh": "Auto-refresh",
    "dashboard.logs.autoRefreshOff": "Off",
    "dashboard.logs.autoRefreshSec": "{n}s",
    "dashboard.logs.autoRefreshHint": "Auto-refresh interval: {value}",
    "dashboard.logs.sourceLabel": "Source",
    "dashboard.logs.sourceAll": "All sources",
    "dashboard.logs.modelLabel": "Model",
    "dashboard.logs.modelAll": "All models",
    "dashboard.logs.metricsRequests": "Total requests",
    "dashboard.logs.metricsCost": "Total cost",
    "dashboard.logs.metricsCostHint":
      "Nothing priced in this range (models here have no price configured)",
    "dashboard.logs.metricsCacheHit": "Cache hit rate",
    "dashboard.logs.metricsCacheHint":
      "Cache-read tokens ÷ input tokens (merged per source convention; shown as — when there is no input)",
    "dashboard.logs.tableTitle": "Request details",
    "dashboard.logs.localOnly": "This machine only",
    "dashboard.logs.colTime": "Time",
    "dashboard.logs.colSource": "Source",
    "dashboard.logs.colModel": "Model",
    "dashboard.logs.colInput": "Input",
    "dashboard.logs.colOutput": "Output",
    "dashboard.logs.colCache": "Cache",
    "dashboard.logs.cacheCellTitle":
      "Cache read {read} · cache write {write}",
    "dashboard.logs.colCost": "Cost",
    "dashboard.logs.colDuration": "Duration",
    "dashboard.logs.colTtft": "TTFT",
    "dashboard.logs.colSpeed": "Speed",
    "dashboard.logs.rowHint": "Click a row for details",
    "dashboard.logs.cellNoData": "No data",
    "dashboard.logs.noDurationTitle":
      "This source's session records contain no duration data (e.g. Codex session files have no duration; older Claude Code versions do not persist it)",
    "dashboard.logs.noTtftTitle":
      "This source's session records contain no TTFT data",
    "dashboard.logs.noSpeedTitle":
      "This source records no duration, so speed cannot be computed",
    "dashboard.logs.unpriced": "Unpriced",
    "dashboard.logs.unpricedHint":
      "This model has no price configured; cost counted as 0",
    "dashboard.logs.emptyTitle": "No requests in this time range",
    "dashboard.logs.emptyHint":
      "Try another time range or source.\nSources that are not installed or never used do not appear here.",
    "dashboard.logs.emptyModelTitle":
      "No requests for this model under the current filters",
    "dashboard.logs.emptyModelHint":
      "No requests for \"{model}\" in the current source / time range. Clear the model filter to see all models.",
    "dashboard.logs.clearModel": "Clear model filter",
    "dashboard.logs.pageEmptyHint":
      "No data on this page — it may be beyond the record range; go back a page.",
    "dashboard.logs.errorTitle": "Failed to load",
    "dashboard.logs.errorReason": "Reason: {error}",
    "dashboard.logs.retry": "Reload",
    "dashboard.logs.partialFailures":
      "Some sources could not be read this time, so data may be incomplete: {names}",
    "dashboard.logs.pagePrev": "Prev",
    "dashboard.logs.pageNext": "Next",
    "dashboard.logs.pageInfo": "Page {page} / {total}",
    "dashboard.logs.pageTotal": "{n} requests",
    "dashboard.logs.detailTitle": "Request details",
    "dashboard.logs.close": "Close",
    "dashboard.logs.detailBasic": "Basic",
    "dashboard.logs.detailSource": "Source",
    "dashboard.logs.detailModel": "Model",
    "dashboard.logs.detailProvider": "Provider",
    "dashboard.logs.detailSession": "Session ID",
    "dashboard.logs.detailProject": "Project",
    "dashboard.logs.detailTokens": "Token breakdown",
    "dashboard.logs.detailInput": "Input",
    "dashboard.logs.detailOutput": "Output",
    "dashboard.logs.detailCacheRead": "Cache read",
    "dashboard.logs.detailCacheWrite": "Cache write",
    "dashboard.logs.detailReasoning": "Reasoning",
    "dashboard.logs.detailTotal": "Total",
    "dashboard.logs.detailCost": "Cost",
    "dashboard.logs.detailPerf": "Performance",
    "dashboard.logs.detailDuration": "Duration",
    "dashboard.logs.detailTtft": "TTFT",
    "dashboard.logs.detailSpeed": "Speed",
    "dashboard.logs.speedApproxHint":
      "No TTFT data; speed approximated from total duration",

    // ===== Custom time range (shared range control of the three pages) =====
    // Trigger shows the active preset name, or the "MM-DD ~ MM-DD" summary when
    // a custom range is applied (with a calendar icon and an accent outline);
    // the panel's preset row mirrors each page's existing presets
    "dashboard.customRange": "Custom",
    "dashboard.customRange.title": "Time range",
    "dashboard.customRange.selectedHint": "Custom range {range} (click to edit)",
    "dashboard.customRange.presets": "Quick ranges",
    "dashboard.customRange.start": "Start date",
    "dashboard.customRange.end": "End date",
    // Placeholder while that end of the range has not been picked yet
    "dashboard.customRange.empty": "Not set",
    "dashboard.customRange.apply": "Apply",
    "dashboard.customRange.cancel": "Cancel",
    // Calendar month navigation (accessible name + tooltip)
    "dashboard.customRange.prevMonth": "Previous month",
    "dashboard.customRange.nextMonth": "Next month",
    "dashboard.customRange.thisMonth": "Back to current month",
    // Earliest selectable date (page specific: fetch window / snapshot retention)
    "dashboard.customRange.minHint": "Earliest date: {date}",
    "dashboard.customRange.errorIncomplete": "Pick both a start and an end date",
    "dashboard.customRange.errorOrder":
      "Start date cannot be later than the end date",
    "dashboard.customRange.errorOutOfRange":
      "Selected dates are outside the available range",
  } satisfies typeof dashboard),
};
