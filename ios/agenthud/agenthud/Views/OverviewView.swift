//
//  OverviewView.swift
//  agenthud
//
//  总览页（首页）：跨 Agent 的用量 / 花费 / 趋势 / 热力图汇总，
//  信息结构对齐桌面 dashboard 的 OverviewPage。
//
//  数据 3 个请求：
//   1) GET /usage（范围/设备筛选/桶选择逻辑与 StatsView.loadRange 一致）
//      → 指标卡 + 趋势卡；
//   2) GET /usage_records（limit=0 只拿 by_model 汇总，带 source 维度）
//      → 按 Agent 用量 + 缓存命中率；
//   3) GET /usage（固定近 91 天 day 桶）→ 13 周 × 7 天活动热力图。
//

import SwiftUI

struct OverviewView: View {
    @EnvironmentObject var settings: AppSettings
    @EnvironmentObject var refresh: RefreshService
    @EnvironmentObject var tabRouter: TabRouter

    /// 默认今日（与小组件"今日花费/Token"口径一致）
    @State private var preset: RangePreset = .today
    @State private var customFrom: String = ""
    @State private var customTo: String = ""
    @State private var rangeUsage: RemoteUsage?
    @State private var rangeCost: CostResult?
    @State private var trendPoints: [TrendPoint] = []
    @State private var agentRows: [OverviewAgentRow] = []
    @State private var hitRate: Double?
    @State private var heatColumns: [[HeatCell]] = []
    @State private var loading = false
    @State private var error: String?

    /// 请求代际（竞态保护）：load 每次发起自增，异步回写前校验仍是
    /// 最新代际，旧的迟到响应整体丢弃。
    @State private var requestEpoch = 0

    var body: some View {
        NavigationStack {
            ZStack {
                AppBackground()

                ScrollView {
                    VStack(alignment: .leading, spacing: 16) {
                        PageHeader(title: "总览",
                                   subtitle: "\(preset.displayName) · \(filterLabel)",
                                   isLoading: loading,
                                   onRefresh: load)

                        if !settings.isSyncConfigured {
                            NotConfiguredBanner(
                                message: "尚未连接同步服务，总览需要先注册设备。",
                                actionTitle: "去设置",
                                action: { tabRouter.selected = .settings })
                        } else {
                            DeviceFilterBar()

                            RangePicker(preset: $preset,
                                        customFrom: $customFrom, customTo: $customTo) {
                                load()
                            }

                            LoadingBar(loading: loading && rangeUsage != nil, error: error)

                            if let usage = rangeUsage {
                                RangeSummaryCard(usage: usage,
                                                 cost: rangeCost,
                                                 hitRate: hitRate,
                                                 rangeLabel: preset.displayName,
                                                 deviceLabel: filterLabel)
                            } else if loading {
                                LoadingPlaceholder()
                            }

                            if !trendPoints.isEmpty {
                                TrendChartCard(points: trendPoints)
                            }

                            AgentUsageCard(rows: agentRows)

                            HeatmapCard(columns: heatColumns)
                        }
                    }
                    .padding(.horizontal, Theme.pagePadding)
                    .padding(.top, 4)
                    .padding(.bottom, 110)
                    .hudContentWidth()
                }
                .scrollIndicators(.hidden)
                .refreshable { await performLoad() }
            }
            .toolbar(.hidden, for: .navigationBar)
        }
        .onAppear { load() }
        .onChange(of: refresh.selectedFilter) { _, _ in load() }
    }

    // MARK: - 筛选文案

    private var filterLabel: String {
        switch refresh.selectedFilter {
        case .all:
            return "全部设备"
        case .local:
            return settings.sync.device_name.isEmpty ? "本机" : settings.sync.device_name
        case .specific(let id):
            return refresh.devices.first { $0.device_id == id }?.device_name ?? "指定设备"
        }
    }

    // MARK: - 加载（3 个请求并发）

    private func load() {
        Task { await performLoad() }
    }

    private func performLoad() async {
        guard settings.isSyncConfigured else { return }
        requestEpoch += 1
        let epoch = requestEpoch
        loading = true
        error = nil

        // 主线程取齐配置快照再进网络阶段（与 StatsView.loadRange 同款写法）
        let server = settings.sync.server_url
        let token = settings.sync.device_token
        let localId = settings.sync.device_id
        let filter = refresh.selectedFilter
        let pricing = settings.pricing
        let currency = settings.currency
        let (fromMs, toMs) = Format.rangeToMs(preset: preset, now: Date(),
                                              custom: (customFrom, customTo))
        let bucket: TrendBucket = (preset == .today || preset == .d1) ? .hour : .day
        let nowMs = Int(Date().timeIntervalSince1970 * 1000)
        // 热力图固定窗口：近 5 个整周（35 天）。从今天 0 点起算而非
        // now 往前推，避免裁掉当天前半段数据。
        let heatFromMs = Int(Calendar.current.startOfDay(for: Date())
            .timeIntervalSince1970 * 1000) - 34 * 86_400_000

        do {
            // 指标/趋势的主请求失败 → 整页报错；汇总与热力图请求失败 →
            // 降级为对应卡片空态，不阻塞主数据
            async let usageTask = APIClient.shared.usage(
                serverURL: server, deviceToken: token,
                fromMs: fromMs, toMs: toMs, bucket: bucket,
                filter: filter, localDeviceId: localId)
            async let recordsTask = try? APIClient.shared.usageRecords(
                serverURL: server, deviceToken: token,
                fromMs: fromMs, toMs: toMs,
                filter: filter, localDeviceId: localId,
                limit: 0, offset: 0)
            async let heatTask = try? APIClient.shared.usage(
                serverURL: server, deviceToken: token,
                fromMs: heatFromMs, toMs: nowMs, bucket: .day,
                filter: filter, localDeviceId: localId)

            let usage = try await usageTask
            let records = await recordsTask
            let heat = await heatTask

            let cost = Billing.computeRemoteCost(usage, pricing: pricing)
            let points = Billing.remoteTrendToLocal(usage,
                                                    pricing: pricing,
                                                    bucket: bucket)
            let rows = Self.buildAgentRows(from: records?.by_model ?? [],
                                            pricing: pricing,
                                            currency: currency)
            let hit = UsageAggregate.cacheHitRate(records?.by_model ?? [])
            let columns = HeatmapBuilder.build(from: heat?.trend ?? [],
                                               today: Date())

            await MainActor.run {
                // 竞态保护：范围/设备筛选快速切换时，旧请求迟到直接丢弃
                guard epoch == self.requestEpoch else { return }
                self.rangeUsage = usage
                self.rangeCost = cost
                self.trendPoints = points
                self.agentRows = rows
                self.hitRate = hit
                self.heatColumns = columns
                self.loading = false
            }
        } catch {
            await MainActor.run {
                guard epoch == self.requestEpoch else { return }
                self.error = error.localizedDescription
                self.loading = false
            }
        }
    }

    /// by_model（带 source 维度）按 source 聚合 → 按 Agent 行（tokens 降序）。
    private static func buildAgentRows(from stats: [UsageRecordsModelStat],
                                       pricing: PricingConfig,
                                       currency: Currency) -> [OverviewAgentRow] {
        let grouped = Dictionary(grouping: stats, by: { $0.source })
        var rows: [OverviewAgentRow] = []
        for (source, items) in grouped {
            let tokens = items.reduce(0) { $0 + $1.total_tokens }
            let requests = items.reduce(0) { $0 + $1.requests }
            // 花费按 Billing 的逐模型单价对每 (source, model) 行累加
            let cost = items.reduce(0.0) { acc, s in
                acc + Billing.modelCost(modelId: s.model_id,
                                        inputTokens: s.input_tokens,
                                        outputTokens: s.output_tokens,
                                        cacheReadTokens: s.cache_read_tokens,
                                        pricing: pricing,
                                        currency: currency)
            }
            rows.append(OverviewAgentRow(source: source,
                                         tokens: tokens,
                                         requests: requests,
                                         cost: cost))
        }
        return rows.sorted { $0.tokens > $1.tokens }
    }
}

// MARK: - 按 Agent 行

struct OverviewAgentRow: Identifiable {
    let source: String
    let tokens: Int
    let requests: Int
    let cost: Double
    var id: String { source }
}

// MARK: - 范围汇总主卡

private struct RangeSummaryCard: View {
    let usage: RemoteUsage
    let cost: CostResult?
    let hitRate: Double?
    let rangeLabel: String
    let deviceLabel: String
    @EnvironmentObject var settings: AppSettings

    private var costText: String {
        guard let c = cost else { return "—" }
        return settings.currency == .cny
            ? Format.cost(c.total_cny, .cny)
            : Format.cost(c.total_usd, .usd)
    }

    var body: some View {
        GlassCard(tint: Theme.accent, elevated: true) {
            VStack(alignment: .leading, spacing: 16) {
                HStack(alignment: .top, spacing: 14) {
                    VStack(alignment: .leading, spacing: 8) {
                        Text("范围内花费")
                            .font(.system(size: 11, weight: .medium))
                            .foregroundStyle(Theme.textTertiary)
                        Text(costText)
                            .font(.system(size: 40, weight: .bold, design: .rounded))
                            .monospacedDigit()
                            .foregroundStyle(
                                LinearGradient(colors: [Theme.mint, Theme.accent],
                                               startPoint: .leading, endPoint: .trailing))
                            .lineLimit(1)
                            .minimumScaleFactor(0.55)
                        HStack(spacing: 6) {
                            miniChip(rangeLabel, icon: "calendar")
                            miniChip(deviceLabel, icon: "desktopcomputer")
                        }
                    }
                    Spacer(minLength: 4)

                    if let rate = hitRate {
                        CacheRing(rate: rate)
                            .padding(.top, 2)
                    }
                }

                HUDDivider()

                HStack(spacing: 10) {
                    HeroMetric(label: "请求数",
                               value: String(usage.overall.requests))
                    HeroMetric(label: "总 Token",
                               value: Format.tokens(usage.overall.total_tokens))
                    HeroMetric(label: "缓存读",
                               value: Format.tokens(usage.overall.cache_read_tokens),
                               tint: Theme.violet)
                }
            }
        }
    }

    private func miniChip(_ text: String, icon: String) -> some View {
        HStack(spacing: 4) {
            Image(systemName: icon)
                .font(.system(size: 8, weight: .semibold))
            Text(text)
                .font(.system(size: 10, weight: .medium))
                .lineLimit(1)
        }
        .foregroundStyle(Theme.textSecondary)
        .padding(.horizontal, 8)
        .padding(.vertical, 4)
        .background(Capsule().fill(Color.white.opacity(0.055)))
        .overlay(Capsule().strokeBorder(Color.white.opacity(0.05), lineWidth: 1))
    }
}

/// 缓存命中率小环。
private struct CacheRing: View {
    let rate: Double

    var body: some View {
        ZStack {
            Circle()
                .stroke(Color.white.opacity(0.07), lineWidth: 6)
            Circle()
                .trim(from: 0, to: CGFloat(min(max(rate, 0), 1)))
                .stroke(
                    AngularGradient(colors: [Theme.violet.opacity(0.45), Theme.violet],
                                    center: .center),
                    style: StrokeStyle(lineWidth: 6, lineCap: .round))
                .rotationEffect(.degrees(-90))
            VStack(spacing: 0) {
                Text(Format.pct(rate))
                    .font(.system(size: 14, weight: .bold, design: .rounded))
                    .monospacedDigit()
                    .foregroundStyle(.white.opacity(0.94))
                    .minimumScaleFactor(0.7)
                Text("缓存")
                    .font(.system(size: 8, weight: .medium))
                    .foregroundStyle(Theme.textTertiary)
            }
        }
        .frame(width: 62, height: 62)
    }
}

// MARK: - 按 Agent 用量卡

private struct AgentUsageCard: View {
    let rows: [OverviewAgentRow]
    @EnvironmentObject var settings: AppSettings

    var body: some View {
        GlassCard {
            VStack(alignment: .leading, spacing: 14) {
                CardHeader(title: "按 Agent",
                           icon: "cpu",
                           tint: Theme.accent,
                           detail: rows.isEmpty ? nil : "\(rows.count) 个来源")

                if rows.isEmpty {
                    EmptyState(icon: "tray", title: "范围内无数据",
                               message: "换个时间范围或设备筛选试试")
                } else {
                    let totalTokens = max(rows.reduce(0) { $0 + $1.tokens }, 1)
                    VStack(spacing: 0) {
                        ForEach(Array(rows.enumerated()), id: \.element.id) { index, row in
                            agentRow(row, totalTokens: totalTokens)
                            if index < rows.count - 1 {
                                HUDDivider().padding(.vertical, 11)
                            }
                        }
                    }
                }
            }
        }
    }

    private func agentRow(_ row: OverviewAgentRow, totalTokens: Int) -> some View {
        let visual = AgentVisual.forSource(row.source)
        return HStack(alignment: .top, spacing: 11) {
            AgentTile(visual: visual, size: 32)
                .padding(.top, 1)

            VStack(alignment: .leading, spacing: 6) {
                HStack(alignment: .firstTextBaseline, spacing: 8) {
                    Text(UsageSourceLabels.label(row.source))
                        .font(.system(size: 13.5, weight: .semibold))
                        .foregroundStyle(.white.opacity(0.90))
                        .lineLimit(1)
                    Spacer(minLength: 6)
                    Text(Format.cost(row.cost, settings.currency))
                        .font(.system(size: 13, weight: .semibold, design: .rounded))
                        .monospacedDigit()
                        .foregroundStyle(Theme.mint)
                }

                HStack(spacing: 5) {
                    Text("\(row.requests) 次")
                    Text("·")
                    Text(Format.tokens(row.tokens) + " tok")
                    Spacer(minLength: 4)
                    Text(Format.pct(Double(row.tokens) / Double(totalTokens)))
                        .foregroundStyle(Theme.textTertiary)
                }
                .font(.system(size: 10))
                .foregroundStyle(Theme.textSecondary)

                GeometryReader { geo in
                    ZStack(alignment: .leading) {
                        Capsule().fill(Color.white.opacity(0.06))
                        Capsule()
                            .fill(LinearGradient(
                                colors: [visual.tint.opacity(0.55), visual.tint],
                                startPoint: .leading, endPoint: .trailing))
                            .frame(width: max(4, geo.size.width * CGFloat(
                                min(1, Double(row.tokens) / Double(totalTokens)))))
                    }
                }
                .frame(height: 4)
            }
        }
    }
}

// MARK: - 活动热力图（GitHub 贡献图风格：列 = 周、行 = 周一…周日）

/// 单个热力图格子。
struct HeatCell: Identifiable, Hashable {
    /// 周下标（0 = 最老的一周）
    let week: Int
    /// 星期下标（0 = 周一 ... 6 = 周日）
    let weekday: Int
    /// 当日 token（< 0 表示未来日期，渲染为占位空格）
    let tokens: Int
    let date: Date

    var id: String { "\(week)-\(weekday)" }
}

/// 热力图数据构造（纯计算，便于复用与排查）。
enum HeatmapBuilder {
    /// day 桶趋势 → 每列一周、列内周一…周日的格子矩阵（默认近 5 周）。
    static func build(from trend: [RemoteTrendBucket],
                      today: Date,
                      weeks: Int = 5) -> [[HeatCell]] {
        let cal = Calendar.current
        let todayStart = cal.startOfDay(for: today)
        // 手工对齐周一开头（不依赖 locale 的 firstWeekday 设置）：
        // weekday 1=周日 … 7=周六 → 距本周周一的天数
        let weekday = cal.component(.weekday, from: todayStart)
        let daysFromMonday = (weekday + 5) % 7
        guard let thisMonday = cal.date(byAdding: .day,
                                        value: -daysFromMonday, to: todayStart),
              let firstMonday = cal.date(byAdding: .day,
                                         value: -(weeks - 1) * 7, to: thisMonday)
        else { return [] }

        // 服务端 day 桶的 label 是桶起点毫秒 → 归并到本地自然日
        //（服务端按 UTC 分桶，跨时区边界日会有少量归属偏移，量级可接受）
        var byDay: [Date: Int] = [:]
        for bucket in trend {
            guard let ms = Int(bucket.label) else { continue }
            let day = cal.startOfDay(for: Date(timeIntervalSince1970: TimeInterval(ms) / 1000))
            byDay[day, default: 0] += bucket.total_tokens
        }

        var columns: [[HeatCell]] = []
        for w in 0..<weeks {
            var col: [HeatCell] = []
            for d in 0..<7 {
                guard let date = cal.date(byAdding: .day,
                                          value: w * 7 + d, to: firstMonday)
                else { continue }
                let isFuture = date > todayStart
                col.append(HeatCell(week: w,
                                    weekday: d,
                                    tokens: isFuture ? -1 : (byDay[date] ?? 0),
                                    date: date))
            }
            columns.append(col)
        }
        return columns
    }

    /// 强度分档（对齐桌面 heatLevel）：0 无活动；其余按当日值 / 最大值
    /// 分四档（≤25% / ≤50% / ≤75% / >75%）。
    static func level(_ value: Int, max: Int) -> Int {
        guard value > 0, max > 0 else { return 0 }
        let ratio = Double(value) / Double(max)
        if ratio <= 0.25 { return 1 }
        if ratio <= 0.5 { return 2 }
        if ratio <= 0.75 { return 3 }
        return 4
    }

    /// 青蓝色系五档色阶（与主色一致）。
    static func color(level: Int) -> Color {
        switch level {
        case 1: return Theme.accent.opacity(0.26)
        case 2: return Theme.accent.opacity(0.48)
        case 3: return Theme.accent.opacity(0.74)
        case 4: return Theme.accent
        default: return Color.white.opacity(0.055)
        }
    }
}

private struct HeatmapCard: View {
    let columns: [[HeatCell]]
    @State private var selectedDay: Date?

    private var cells: [HeatCell] {
        columns.flatMap { $0 }.sorted { $0.date < $1.date }
    }

    private var today: Date {
        Calendar.current.startOfDay(for: Date())
    }

    private var maxTokens: Int {
        cells.map { max($0.tokens, 0) }.max() ?? 0
    }

    private var activeDays: Int {
        cells.filter { $0.tokens > 0 }.count
    }

    private var selection: HeatCell? {
        if let selectedDay {
            return cells.first {
                Calendar.current.isDate($0.date, inSameDayAs: selectedDay)
            }
        }
        return cells.last { $0.tokens >= 0 && $0.date <= today }
    }

    /// 近 5 周的日期范围（首日 – 末日），跨月时也能看懂日历。
    private var rangeText: String {
        guard let first = cells.first, let last = cells.last(where: { $0.tokens >= 0 })
        else { return "" }
        let f = DateFormatter()
        f.locale = Locale(identifier: "zh_CN")
        f.dateFormat = "M/d"
        return f.string(from: first.date) + "–" + f.string(from: last.date)
    }

    var body: some View {
        GlassCard {
            VStack(alignment: .leading, spacing: 12) {
                CardHeader(title: "活跃度",
                           icon: "calendar",
                           tint: Theme.accent,
                           detail: cells.isEmpty
                               ? "近 5 周"
                               : "\(rangeText) · \(activeDays) 天")

                if cells.isEmpty {
                    EmptyState(icon: "calendar.badge.exclamationmark",
                               title: "暂无活动数据")
                } else {
                    weekdayHeader
                    LazyVGrid(columns: Array(repeating: GridItem(.flexible(), spacing: 6),
                                             count: 7),
                              spacing: 6) {
                        ForEach(cells) { cell in
                            Button {
                                guard cell.tokens >= 0 else { return }
                                withAnimation(.easeOut(duration: 0.15)) {
                                    selectedDay = cell.date
                                }
                            } label: {
                                HeatDayCell(
                                    cell: cell,
                                    maxTokens: maxTokens,
                                    isSelected: isSelected(cell),
                                    isToday: Calendar.current.isDateInToday(cell.date))
                            }
                            .buttonStyle(.plain)
                            .disabled(cell.tokens < 0)
                        }
                    }
                    .frame(maxWidth: 360)

                    HStack(spacing: 8) {
                        selectedSummary
                        Spacer(minLength: 6)
                        legend
                    }
                }
            }
        }
    }

    private var weekdayHeader: some View {
        HStack(spacing: 6) {
            ForEach(["一", "二", "三", "四", "五", "六", "日"], id: \.self) { d in
                Text(d)
                    .font(.system(size: 9, weight: .medium))
                    .foregroundStyle(Theme.textTertiary)
                    .frame(maxWidth: .infinity)
            }
        }
        .frame(maxWidth: 360)
        .padding(.top, 2)
    }

    @ViewBuilder
    private var selectedSummary: some View {
        if let cell = selection {
            HStack(spacing: 6) {
                Text(selectedDateText(cell.date))
                    .font(.system(size: 10, weight: .semibold))
                    .foregroundStyle(.white.opacity(0.82))
                Text(cell.tokens > 0 ? Format.tokens(cell.tokens) + " tok" : "无用量")
                    .font(.system(size: 10, weight: .medium, design: .rounded))
                    .monospacedDigit()
                    .foregroundStyle(cell.tokens > 0 ? Theme.accent : Theme.textTertiary)
            }
            .padding(.horizontal, 8)
            .padding(.vertical, 4)
            .background(Capsule().fill(Color.white.opacity(0.045)))
        }
    }

    private func selectedDateText(_ date: Date) -> String {
        if Calendar.current.isDateInToday(date) { return "今天" }
        let formatter = DateFormatter()
        formatter.locale = Locale(identifier: "zh_CN")
        formatter.dateFormat = "M月d日 EEE"
        return formatter.string(from: date)
    }

    private var legend: some View {
        HStack(spacing: 4) {
            Text("少")
                .font(.system(size: 9))
                .foregroundStyle(Theme.textTertiary)
            ForEach(1...4, id: \.self) { level in
                RoundedRectangle(cornerRadius: 2)
                    .fill(HeatmapBuilder.color(level: level))
                    .frame(width: 9, height: 9)
            }
            Text("多")
                .font(.system(size: 9))
                .foregroundStyle(Theme.textTertiary)
        }
    }

    private func isSelected(_ cell: HeatCell) -> Bool {
        guard let selected = selection else { return false }
        return Calendar.current.isDate(selected.date, inSameDayAs: cell.date)
    }
}

/// 单个日历日格：日期数字 + 强度底色；今天/选中用描边标出。
private struct HeatDayCell: View {
    let cell: HeatCell
    let maxTokens: Int
    let isSelected: Bool
    let isToday: Bool

    private var isFuture: Bool { cell.tokens < 0 }
    private var level: Int { HeatmapBuilder.level(cell.tokens, max: maxTokens) }
    private var dayNumber: Int {
        Calendar.current.component(.day, from: cell.date)
    }

    private var fill: Color {
        isFuture ? Color.white.opacity(0.012) : HeatmapBuilder.color(level: level)
    }

    var body: some View {
        ZStack {
            RoundedRectangle(cornerRadius: 8, style: .continuous)
                .fill(fill)
            if isFuture {
                RoundedRectangle(cornerRadius: 8, style: .continuous)
                    .strokeBorder(Color.white.opacity(0.045), lineWidth: 1)
                Text("\(dayNumber)")
                    .font(.system(size: 9, design: .rounded))
                    .foregroundStyle(Theme.textTertiary.opacity(0.45))
            } else {
                Text("\(dayNumber)")
                    .font(.system(size: 10,
                                  weight: level >= 3 ? .semibold : .regular,
                                  design: .rounded))
                    .foregroundStyle(level >= 3
                                     ? Color.white.opacity(0.95)
                                     : Theme.textSecondary)
            }
        }
        .aspectRatio(1, contentMode: .fit)
        .overlay {
            if isSelected {
                RoundedRectangle(cornerRadius: 8, style: .continuous)
                    .strokeBorder(Theme.accent, lineWidth: 1.6)
            } else if isToday {
                RoundedRectangle(cornerRadius: 8, style: .continuous)
                    .strokeBorder(Theme.accent.opacity(0.5), lineWidth: 1)
            }
        }
        .contentShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
        .accessibilityLabel(accessibilityText)
    }

    private var accessibilityText: String {
        let f = DateFormatter()
        f.locale = Locale(identifier: "zh_CN")
        f.dateFormat = "M月d日"
        let date = f.string(from: cell.date)
        if isFuture { return date + "，未来日期" }
        if cell.tokens <= 0 { return date + "，无用量" }
        return date + "，" + Format.tokens(cell.tokens) + " tokens"
    }
}
