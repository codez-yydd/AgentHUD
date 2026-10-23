//
//  LogsView.swift
//  agenthud
//
//  请求日志页：逐条请求明细（GET /usage_records，proto 6 起）。
//  范围 / 设备 / 来源 / 模型四维筛选，汇总条 + 按日期分组的明细分页
//  （每页 50 条，上一页/下一页，不无限追加），行点击进入全字段详情 sheet。
//

import SwiftUI

struct LogsView: View {
    @EnvironmentObject var settings: AppSettings
    @EnvironmentObject var refresh: RefreshService
    @EnvironmentObject var tabRouter: TabRouter

    @State private var preset: RangePreset = .today
    @State private var customFrom: String = ""
    @State private var customTo: String = ""
    /// nil = 全部来源
    @State private var source: String?
    /// nil = 全部模型
    @State private var modelId: String?
    @State private var availableModels: [String] = []

    @State private var rows: [UsageRecordRow] = []
    @State private var total: Int = 0
    @State private var summaryTokens: Int = 0
    @State private var hitRate: Double?
    @State private var loading = false
    @State private var error: String?
    @State private var selectedRow: UsageRecordRow?
    @State private var showFilters = false

    /// 当前页码（1-based）。每次只保留本页 rows，不无限追加，
    /// 避免长时间翻页后 SwiftUI 需要维护成千上万个行视图。
    @State private var page = 1

    /// 请求代际（竞态保护）：reload / 翻页每次发起自增，回写前校验仍是
    /// 最新代际，旧筛选或旧页的迟到响应整体丢弃。
    @State private var requestEpoch = 0

    /// 每页条数。手机上一屏大约 6-8 行，50 条足够翻看，
    /// 同时把单页视图数量控制在不会明显卡顿的量级。
    private static let pageSize = 50

    var body: some View {
        NavigationStack {
            ZStack {
                AppBackground()

                ScrollViewReader { proxy in
                    ScrollView {
                        VStack(alignment: .leading, spacing: 14) {
                            PageHeader(title: "请求日志",
                                       isLoading: loading,
                                       onRefresh: reload)

                            if !settings.isSyncConfigured {
                                NotConfiguredBanner(
                                    message: "尚未连接同步服务，日志需要先注册设备。",
                                    actionTitle: "去设置",
                                    action: { tabRouter.selected = .settings })
                            } else {
                                HUDControlBar(preset: $preset,
                                              customFrom: $customFrom,
                                              customTo: $customTo,
                                              onApply: reload) {
                                    filterButton
                                }

                                LoadingBar(loading: loading, error: error)

                                summaryCard

                                if total > Self.pageSize && !rows.isEmpty {
                                    paginationBar(scrollProxy: proxy)
                                        .id("logs-pagebar")
                                }

                                if rows.isEmpty && !loading && error == nil {
                                    GlassCard {
                                        EmptyState(icon: "doc.text.magnifyingglass",
                                                   title: "范围内无请求记录",
                                                   message: "换个时间范围或筛选条件试试")
                                    }
                                }

                                ForEach(daySections) { section in
                                    SectionLabel(title: section.title,
                                                 detail: "\(section.rows.count) 条")
                                    dayCard(section)
                                }

                                if total > Self.pageSize && !rows.isEmpty {
                                    Text("第 \(page) / \(pageCount) 页 · 共 \(total) 条")
                                        .font(.system(size: 10))
                                        .foregroundStyle(Theme.textTertiary)
                                        .frame(maxWidth: .infinity)
                                        .padding(.top, 2)
                                }
                            }
                        }
                        .padding(.horizontal, Theme.pagePadding)
                        .padding(.top, 4)
                        .padding(.bottom, 96)
                        .hudContentWidth()
                    }
                    .scrollIndicators(.hidden)
                    .refreshable { await performReload() }
                }
            }
            .toolbar(.hidden, for: .navigationBar)
            .sheet(item: $selectedRow) { row in
                LogDetailSheet(row: row)
            }
        }
        .onAppear {
            loadModels()
            reload()
        }
        .onChange(of: refresh.selectedFilter) { _, _ in reload() }
    }

    // MARK: - 页头 / 分组

    private var daySections: [LogDaySection] {
        let cal = Calendar.current
        var order: [Date] = []
        var map: [Date: [UsageRecordRow]] = [:]
        for row in rows {
            let day = cal.startOfDay(
                for: Date(timeIntervalSince1970: TimeInterval(row.started_at) / 1000))
            if map[day] == nil { order.append(day) }
            map[day, default: []].append(row)
        }
        order.sort(by: >)
        return order.map { day in
            LogDaySection(day: day,
                          title: Self.dayTitle(day),
                          rows: map[day] ?? [])
        }
    }

    private static func dayTitle(_ day: Date) -> String {
        let cal = Calendar.current
        if cal.isDateInToday(day) { return "今天" }
        if cal.isDateInYesterday(day) { return "昨天" }
        let f = DateFormatter()
        f.locale = Locale(identifier: "zh_CN")
        f.dateFormat = "M月d日 EEEE"
        return f.string(from: day)
    }

    private func dayCard(_ section: LogDaySection) -> some View {
        GlassCard(padding: 14, radius: 20) {
            VStack(spacing: 0) {
                ForEach(Array(section.rows.enumerated()), id: \.element.id) { index, row in
                    Button {
                        selectedRow = row
                    } label: {
                        LogRowView(row: row)
                    }
                    .buttonStyle(.plain)
                    if index < section.rows.count - 1 {
                        HUDDivider().padding(.vertical, 10)
                    }
                }
            }
        }
    }

    // MARK: - 来源 / 模型筛选（收进一个筛选按钮 + sheet）

    private var activeFilterCount: Int {
        (source == nil ? 0 : 1) + (modelId == nil ? 0 : 1)
    }

    private var filterButton: some View {
        Button {
            showFilters = true
        } label: {
            ZStack(alignment: .topTrailing) {
                Image(systemName: "line.3.horizontal.decrease")
                    .font(.system(size: 12, weight: .semibold))
                    .foregroundStyle(activeFilterCount > 0 ? Theme.accent : Theme.textSecondary)
                    .frame(width: 34, height: 30)
                    .background(Circle().fill(activeFilterCount > 0
                                              ? Theme.accent.opacity(0.16)
                                              : Color.white.opacity(0.05)))
                    .overlay(Circle().strokeBorder(activeFilterCount > 0
                                                   ? Theme.accent.opacity(0.35)
                                                   : Theme.stroke,
                                                   lineWidth: 1))
                if activeFilterCount > 0 {
                    Text("\(activeFilterCount)")
                        .font(.system(size: 8, weight: .bold, design: .rounded))
                        .foregroundStyle(Color.black.opacity(0.85))
                        .frame(width: 13, height: 13)
                        .background(Circle().fill(Theme.accent))
                        .offset(x: 4, y: -3)
                }
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel("筛选")
        .sheet(isPresented: $showFilters) { filterSheet }
    }

    private var filterSheet: some View {
        NavigationStack {
            ZStack {
                AppBackground()

                ScrollView {
                    VStack(alignment: .leading, spacing: 14) {
                        SectionLabel(title: "来源")
                        GlassCard(padding: 6, radius: 16) {
                            VStack(spacing: 0) {
                                filterOption(title: "全部来源", selected: source == nil,
                                             tint: Theme.accent) {
                                    source = nil
                                }
                                HUDDivider().padding(.horizontal, 10)
                                ForEach(UsageSourceLabels.all, id: \.self) { src in
                                    filterOption(title: UsageSourceLabels.label(src),
                                                 selected: source == src,
                                                 tint: AgentVisual.forSource(src).tint) {
                                        source = src
                                    }
                                }
                            }
                        }

                        SectionLabel(title: "模型")
                        GlassCard(padding: 6, radius: 16) {
                            VStack(spacing: 0) {
                                filterOption(title: "全部模型", selected: modelId == nil,
                                             tint: Theme.accent) {
                                    modelId = nil
                                }
                                ForEach(availableModels, id: \.self) { model in
                                    HUDDivider().padding(.horizontal, 10)
                                    filterOption(title: model, selected: modelId == model,
                                                 tint: Theme.violet) {
                                        modelId = model
                                    }
                                }
                            }
                        }
                    }
                    .padding(.horizontal, Theme.pagePadding)
                    .padding(.vertical, 12)
                }
                .scrollIndicators(.hidden)
            }
            .navigationTitle("筛选")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("重置") {
                        source = nil
                        modelId = nil
                        showFilters = false
                        reload()
                    }
                    .disabled(activeFilterCount == 0)
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button("完成") {
                        showFilters = false
                        reload()
                    }
                    .fontWeight(.semibold)
                }
            }
        }
        .presentationDetents([.medium, .large])
        .presentationBackground(Theme.bgTop)
        .tint(Theme.accent)
    }

    private func filterOption(title: String, selected: Bool,
                              tint: Color, action: @escaping () -> Void) -> some View {
        Button {
            action()
            showFilters = false
            reload()
        } label: {
            HStack(spacing: 10) {
                if selected {
                    Image(systemName: "checkmark")
                        .font(.system(size: 11, weight: .bold))
                        .foregroundStyle(tint)
                        .frame(width: 16)
                } else {
                    Color.clear.frame(width: 16, height: 1)
                }
                Text(title)
                    .font(.system(size: 13, weight: selected ? .semibold : .regular))
                    .foregroundStyle(selected ? .white.opacity(0.95) : Theme.textSecondary)
                    .lineLimit(1)
                Spacer(minLength: 0)
            }
            .padding(.horizontal, 10)
            .padding(.vertical, 11)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }

    // MARK: - 汇总卡

    private var summaryCard: some View {
        GlassCard(padding: 14, radius: 18) {
            HStack(spacing: 10) {
                HeroMetric(label: "记录条数", value: String(total))
                HeroMetric(label: "总 Token",
                           value: Format.tokens(summaryTokens))
                HeroMetric(label: "缓存命中率",
                           value: hitRate.map { Format.pct($0) } ?? "—",
                           tint: Theme.violet)
            }
        }
    }

    // MARK: - 分页

    private var pageCount: Int {
        max(1, Int(ceil(Double(total) / Double(Self.pageSize))))
    }

    /// 分页条：左边一行说明，右边 ‹ 页码 › 胶囊。
    /// 比整条胶囊更轻，和摘要卡之间不再有两个同等重量的横条。
    private func paginationBar(scrollProxy: ScrollViewProxy) -> some View {
        HStack(spacing: 8) {
            Text("共 \(total) 条 · 每页 \(Self.pageSize)")
                .font(.system(size: 10))
                .foregroundStyle(Theme.textTertiary)
                .monospacedDigit()

            Spacer(minLength: 6)

            HStack(spacing: 2) {
                pageArrow(systemImage: "chevron.left",
                          enabled: page > 1 && !loading) {
                    changePage(page - 1, proxy: scrollProxy)
                }
                HStack(spacing: 4) {
                    Text("\(page) / \(pageCount)")
                        .font(.system(size: 11, weight: .semibold, design: .rounded))
                        .monospacedDigit()
                        .foregroundStyle(.white.opacity(0.88))
                    if loading {
                        ProgressView()
                            .controlSize(.mini)
                            .tint(Theme.accent)
                    }
                }
                .frame(minWidth: 46)
                pageArrow(systemImage: "chevron.right",
                          enabled: page < pageCount && !loading) {
                    changePage(page + 1, proxy: scrollProxy)
                }
            }
            .padding(.horizontal, 4)
            .padding(.vertical, 2)
            .background(Capsule().fill(Color.white.opacity(0.045)))
            .overlay(Capsule().strokeBorder(Theme.stroke, lineWidth: 1))
        }
        .padding(.top, 2)
    }

    private func pageArrow(systemImage: String,
                           enabled: Bool,
                           action: @escaping () -> Void) -> some View {
        Button(action: action) {
            ZStack {
                Circle().fill(Color.white.opacity(enabled ? 0.07 : 0.02))
                Image(systemName: systemImage)
                    .font(.system(size: 11, weight: .semibold))
                    .foregroundStyle(enabled ? Theme.accent : Theme.textTertiary.opacity(0.45))
            }
            .frame(width: 30, height: 30)
            .contentShape(Circle())
        }
        .buttonStyle(.plain)
        .disabled(!enabled)
    }

    private func changePage(_ newPage: Int, proxy: ScrollViewProxy) {
        guard !loading, newPage >= 1, newPage <= pageCount else { return }
        page = newPage
        withAnimation(.easeOut(duration: 0.22)) {
            proxy.scrollTo("logs-pagebar", anchor: .top)
        }
        Task { await performLoad(page: newPage) }
    }

    // MARK: - 加载

    /// 筛选条件变化 → 回到第一页重新加载。
    private func reload() {
        Task { await performReload() }
    }

    private func performReload() async {
        guard settings.isSyncConfigured else { return }
        await MainActor.run { self.page = 1 }
        await performLoad(page: 1)
    }

    /// 单页加载：只替换当前页 rows，不追加，避免无限增长。
    private func performLoad(page requestedPage: Int) async {
        requestEpoch += 1
        let epoch = requestEpoch
        let offset = max(0, (requestedPage - 1) * Self.pageSize)

        await MainActor.run {
            self.loading = true
            self.error = nil
        }

        let server = settings.sync.server_url
        let token = settings.sync.device_token
        let localId = settings.sync.device_id
        let filter = refresh.selectedFilter
        let (fromMs, toMs) = Format.rangeToMs(preset: preset, now: Date(),
                                              custom: (customFrom, customTo))
        let src = source
        let model = modelId

        do {
            let result = try await APIClient.shared.usageRecords(
                serverURL: server, deviceToken: token,
                fromMs: fromMs, toMs: toMs,
                source: src, modelId: model,
                filter: filter, localDeviceId: localId,
                limit: Self.pageSize, offset: offset)
            let hit = UsageAggregate.cacheHitRate(result.by_model)
            let tokens = UsageAggregate.totalTokens(result.by_model)
            await MainActor.run {
                guard epoch == self.requestEpoch else { return }
                self.total = result.total
                self.summaryTokens = tokens
                self.hitRate = hit
                self.rows = result.rows
                self.page = requestedPage
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

    /// 模型清单（全部设备/来源出现过的 model_id；加载失败静默降级为仅「全部」）。
    private func loadModels() {
        guard settings.isSyncConfigured else { return }
        let server = settings.sync.server_url
        let token = settings.sync.device_token
        Task {
            if let models = try? await APIClient.shared.models(
                serverURL: server, deviceToken: token) {
                await MainActor.run {
                    self.availableModels = models
                }
            }
        }
    }
}

// MARK: - 日期分组

private struct LogDaySection: Identifiable {
    let day: Date
    let title: String
    let rows: [UsageRecordRow]
    var id: Date { day }
}

// MARK: - 耗时 / 首字 / 输出速度

/// 明细行的性能指标口径，与桌面 dashboard/logs/model.ts 的本地回退公式一致：
/// 速度 = 输出 token / 生成窗口；生成窗口 = 耗时 − 首字（首字无效时退回总耗时，
/// 并用 ≈ 标记是按整次请求平均估算的）。
enum LogMetrics {
    /// 耗时：< 100s 一位小数秒，否则取整秒（避免长数字挤爆窄格）。
    static func durationText(_ ms: Int?) -> String? {
        guard let ms, ms > 0 else { return nil }
        if ms < 100_000 { return String(format: "%.1fs", Double(ms) / 1000) }
        return String(format: "%.0fs", Double(ms) / 1000)
    }

    /// 首字延迟固定两位小数秒（0.35s / 0.42s 需要区分）。
    static func ttftText(_ ms: Int?) -> String? {
        guard let ms, ms > 0 else { return nil }
        return String(format: "%.2fs", Double(ms) / 1000)
    }

    static func speed(outputTokens: Int,
                      durationMs: Int?,
                      ttftMs: Int?) -> (tps: Double, approximate: Bool)? {
        guard let duration = durationMs, duration > 0 else { return nil }
        let generation: Int?
        if let ttft = ttftMs, ttft > 0, ttft < duration {
            generation = duration - ttft
        } else {
            generation = nil
        }
        let window = generation ?? duration
        guard window > 0 else { return nil }
        return (Double(outputTokens) / (Double(window) / 1000),
                generation == nil)
    }

    /// 速度文本：≥100 取整，否则一位小数；近似口径前置 ≈。
    static func tpsText(_ speed: (tps: Double, approximate: Bool)?) -> String? {
        guard let speed, speed.tps.isFinite else { return nil }
        let value = speed.tps >= 100
            ? String(Int(speed.tps.rounded()))
            : String(format: "%.1f", speed.tps)
        return (speed.approximate ? "≈" : "") + value + " t/s"
    }
}

// MARK: - 明细行（模型 + 来源 / 设备 / token / 耗时 / 速度 / 首字）

private struct LogRowView: View {
    let row: UsageRecordRow
    @EnvironmentObject var refresh: RefreshService

    private var visual: AgentVisual { AgentVisual.forSource(row.source) }

    var body: some View {
        HStack(alignment: .center, spacing: 10) {
            AgentTile(visual: visual, size: 28)

            VStack(alignment: .leading, spacing: 4) {
                // 第一行：模型 + 时间。模型名过长时中间截断，保证时间永远
                // 靠右对齐成一列。
                HStack(spacing: 8) {
                    Text(row.model_id.isEmpty ? "未知模型" : row.model_id)
                        .font(.system(size: 13, weight: .semibold))
                        .foregroundStyle(.white.opacity(0.90))
                        .lineLimit(1)
                        .truncationMode(.middle)
                    Spacer(minLength: 6)
                    Text(Self.timeText(row.started_at))
                        .font(.system(size: 10, weight: .medium, design: .rounded))
                        .monospacedDigit()
                        .foregroundStyle(Theme.textTertiary)
                        .layoutPriority(1)
                }

                // 第二行：来源 · 设备 · 输入/输出 · 耗时。全部单行，
                // 来源与设备允许收缩省略，token / 耗时保持完整。
                HStack(spacing: 5) {
                    Text(UsageSourceLabels.label(row.source))
                        .font(.system(size: 10, weight: .semibold))
                        .foregroundStyle(visual.tint)
                        .lineLimit(1)
                        .fixedSize(horizontal: true, vertical: false)
                    Text("·")
                        .font(.system(size: 10))
                        .foregroundStyle(Theme.textTertiary)
                    Text(deviceName)
                        .font(.system(size: 10))
                        .foregroundStyle(Theme.textSecondary)
                        .lineLimit(1)
                        .truncationMode(.tail)
                        .layoutPriority(-1)
                    Spacer(minLength: 4)
                    // token / 耗时固定整块，优先压缩来源与设备名，
                    // 避免 "126.0K" 被折成两行。
                    HStack(spacing: 4) {
                        Text("↓\(Format.tokens(row.input_tokens))")
                            .font(.system(size: 10).monospacedDigit())
                            .foregroundStyle(Theme.textSecondary)
                        Text("↑\(Format.tokens(row.output_tokens))")
                            .font(.system(size: 10).monospacedDigit())
                            .foregroundStyle(Theme.textSecondary)
                        if let duration = LogMetrics.durationText(row.duration_ms) {
                            Text("·")
                                .font(.system(size: 10))
                                .foregroundStyle(Theme.textTertiary)
                            Text(duration)
                                .font(.system(size: 10).monospacedDigit())
                                .foregroundStyle(Theme.textTertiary)
                        }
                    }
                    .lineLimit(1)
                    .fixedSize(horizontal: true, vertical: false)
                    .layoutPriority(1)
                }
            }

            Image(systemName: "chevron.right")
                .font(.system(size: 9, weight: .semibold))
                .foregroundStyle(Theme.textTertiary.opacity(0.7))
        }
        .contentShape(Rectangle())
    }

    /// 设备显示名（注册名优先，缺失退回指纹前 8 位）。
    private var deviceName: String {
        if let d = refresh.devices.first(where: { $0.device_id == row.device_id }) {
            return d.device_name
        }
        return String(row.device_id.prefix(8))
    }

    /// 行内时间：HH:mm:ss（完整日期在详情 sheet 里）。
    private static func timeText(_ ms: Int) -> String {
        let f = DateFormatter()
        f.dateFormat = "HH:mm:ss"
        return f.string(from: Date(timeIntervalSince1970: TimeInterval(ms) / 1000))
    }

}

// MARK: - 详情 sheet（全字段）

private struct LogDetailSheet: View {
    let row: UsageRecordRow
    @EnvironmentObject var refresh: RefreshService
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            ZStack {
                AppBackground()

                List {
                    Section("请求") {
                        detailRow("时间", Self.fullTime(row.started_at))
                        detailRow("模型", row.model_id)
                        detailRow("来源", UsageSourceLabels.label(row.source))
                        detailRow("设备", deviceText)
                        detailRow("Provider", row.provider_id, selectable: true)
                    }
                    Section("Token") {
                        detailRow("输入", Format.tokens(row.input_tokens))
                        detailRow("输出", Format.tokens(row.output_tokens))
                        detailRow("缓存读", Format.tokens(row.cache_read_input_tokens))
                        detailRow("缓存写", Format.tokens(row.cache_creation_input_tokens))
                        detailRow("推理", Format.tokens(row.reasoning_tokens))
                        detailRow("总 Token", Format.tokens(row.computed_total_tokens))
                    }
                    Section {
                        detailRow("耗时", LogMetrics.durationText(row.duration_ms) ?? "—")
                        detailRow("首字延迟", LogMetrics.ttftText(row.time_to_first_token_ms) ?? "—")
                        detailRow("输出速度", LogMetrics.tpsText(speed) ?? "—")
                    } header: {
                        Text("性能")
                    } footer: {
                        if let speed, speed.approximate {
                            Text("≈ 表示缺少首字数据，速度按整次请求总耗时估算。")
                                .font(.system(size: 10))
                                .foregroundStyle(Theme.textTertiary)
                        }
                    }
                    Section("上下文") {
                        detailRow("会话 ID", row.session_id, selectable: true)
                        detailRow("项目", projectText)
                    }
                }
                .scrollContentBackground(.hidden)
                .listRowBackground(Color.white.opacity(0.045))
                .listSectionSpacing(14)
            }
            .navigationTitle("请求详情")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("完成") { dismiss() }
                }
            }
        }
        .presentationDetents([.medium, .large])
        .presentationBackground(Theme.bgTop)
        .tint(Theme.accent)
    }

    private var speed: (tps: Double, approximate: Bool)? {
        LogMetrics.speed(outputTokens: row.output_tokens,
                         durationMs: row.duration_ms,
                         ttftMs: row.time_to_first_token_ms)
    }

    /// 项目显示名优先，缺失回退 project_key，两者皆无 → 「—」。
    private var projectText: String {
        if let display = row.project_display, !display.isEmpty { return display }
        if let key = row.project_key, !key.isEmpty { return key }
        return "—"
    }

    private var deviceText: String {
        if let d = refresh.devices.first(where: { $0.device_id == row.device_id }) {
            return d.device_name
        }
        return row.device_id
    }

    /// 详情行（selectable 的值支持长按复制，如会话 ID / Provider）。
    private func detailRow(_ title: String, _ value: String?,
                           selectable: Bool = false) -> some View {
        HStack(alignment: .top) {
            Text(title)
                .font(.system(size: 13))
                .foregroundStyle(Theme.textSecondary)
            Spacer()
            Self.valueText(value, selectable: selectable)
        }
    }

    /// 值文本（selectable 时挂 textSelection；.enabled/.disabled 是两个
    /// 不同类型，不能用三元表达式切换，走 @ViewBuilder 分支）。
    @ViewBuilder
    private static func valueText(_ value: String?, selectable: Bool) -> some View {
        let text = Text(value.flatMap { $0.isEmpty ? nil : $0 } ?? "—")
            .font(.system(size: 11, design: .monospaced))
            .multilineTextAlignment(.trailing)
            .foregroundStyle(.white.opacity(0.88))
        if selectable {
            text.textSelection(.enabled)
        } else {
            text
        }
    }

    private static func fullTime(_ ms: Int) -> String {
        let f = DateFormatter()
        f.dateFormat = "yyyy-MM-dd HH:mm:ss"
        return f.string(from: Date(timeIntervalSince1970: TimeInterval(ms) / 1000))
    }

}
