//
//  StatsView.swift
//  agenthud
//
//  统计面板：今日 / 7天 / 30天 / 自定义 范围切换，
//  展示 Token 构成、明细指标与按模型花费。
//

import SwiftUI

struct StatsView: View {
    @EnvironmentObject var settings: AppSettings
    @EnvironmentObject var refresh: RefreshService

    @State private var preset: RangePreset = .today
    @State private var customFrom: String = ""
    @State private var customTo: String = ""
    @State private var rangeUsage: RemoteUsage?
    @State private var rangeCost: CostResult?
    @State private var rangeLoading = false
    @State private var rangeError: String?

    /// 请求代际（竞态保护）：快速切换范围/设备时，旧请求的迟到响应丢弃。
    @State private var requestEpoch = 0

    var body: some View {
        NavigationStack {
            ZStack {
                AppBackground()

                ScrollView {
                    VStack(alignment: .leading, spacing: 16) {
                        PageHeader(title: "统计",
                                   subtitle: "\(preset.displayName) · \(filterLabel)",
                                   isLoading: rangeLoading,
                                   onRefresh: loadRange)

                        if !settings.isSyncConfigured {
                            NotConfiguredBanner(
                                message: "尚未连接同步服务，统计需要先注册设备。")
                        } else {
                            DeviceFilterBar()

                            RangePicker(preset: $preset,
                                        customFrom: $customFrom,
                                        customTo: $customTo) {
                                loadRange()
                            }

                            LoadingBar(loading: rangeLoading && rangeUsage != nil,
                                       error: rangeError)

                            if let usage = rangeUsage {
                                TokenCompositionCard(usage: usage,
                                                     cost: rangeCost,
                                                     hitRate: cacheHitRate(usage))
                                OverallCard(usage: usage, cost: rangeCost)
                                ModelBreakdownCard(usage: usage, cost: rangeCost)
                            } else if rangeLoading {
                                LoadingPlaceholder()
                            } else if rangeError == nil {
                                GlassCard {
                                    EmptyState(icon: "chart.bar",
                                               title: "范围内暂无用量",
                                               message: "换个时间范围或设备筛选试试")
                                }
                            }
                        }
                    }
                    .padding(.horizontal, Theme.pagePadding)
                    .padding(.top, 4)
                    .padding(.bottom, 110)
                    .hudContentWidth()
                }
                .scrollIndicators(.hidden)
                .refreshable { await performRangeLoad() }
            }
            .toolbar(.hidden, for: .navigationBar)
        }
        .onAppear { loadRange() }
        .onChange(of: refresh.selectedFilter) { _, _ in loadRange() }
    }

    // MARK: - 辅助文案

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

    /// 总缓存命中率：缓存读 / 输入（输入已含缓存读，与桌面口径一致）。
    private func cacheHitRate(_ usage: RemoteUsage) -> Double? {
        let input = usage.overall.input_tokens
        guard input > 0 else { return nil }
        return Double(usage.overall.cache_read_tokens) / Double(input)
    }

    // MARK: - 加载指定范围

    private func loadRange() {
        Task { await performRangeLoad() }
    }

    /// 异步可等待版（refreshable 共用）。请求代际校验避免快速切范围时
    /// 旧响应覆盖新数据。
    private func performRangeLoad() async {
        requestEpoch += 1
        let epoch = requestEpoch
        let (fromMs, toMs) = Format.rangeToMs(preset: preset, now: Date(),
                                               custom: (customFrom, customTo))
        rangeLoading = true
        rangeError = nil
        let server = settings.sync.server_url
        let token = settings.sync.device_token
        let filter = refresh.selectedFilter
        let localId = settings.sync.device_id
        let pricing = settings.pricing
        let bucket: TrendBucket = (preset == .today || preset == .d1) ? .hour : .day
        do {
            let usage = try await APIClient.shared.usage(
                serverURL: server, deviceToken: token,
                fromMs: fromMs, toMs: toMs,
                bucket: bucket,
                filter: filter, localDeviceId: localId)
            let cost = Billing.computeRemoteCost(usage, pricing: pricing)
            await MainActor.run {
                guard epoch == self.requestEpoch else { return }
                self.rangeUsage = usage
                self.rangeCost = cost
                self.rangeLoading = false
            }
        } catch {
            await MainActor.run {
                guard epoch == self.requestEpoch else { return }
                self.rangeError = error.localizedDescription
                self.rangeLoading = false
            }
        }
    }
}

// MARK: - 范围选择器

/// internal：除 StatsView 外，总览页 / 日志页复用同一套范围选择交互。
struct RangePicker: View {
    @Binding var preset: RangePreset
    @Binding var customFrom: String
    @Binding var customTo: String
    let onApply: () -> Void

    @Namespace private var selection

    var body: some View {
        VStack(spacing: 10) {
            ScrollView(.horizontal, showsIndicators: false) {
                HStack(spacing: 2) {
                    ForEach(RangePreset.allCases) { p in
                        Button {
                            withAnimation(.spring(response: 0.32, dampingFraction: 0.86)) {
                                preset = p
                            }
                            if p != .custom { onApply() }
                        } label: {
                            Text(p.displayName)
                                .font(.system(size: 12.5,
                                              weight: preset == p ? .semibold : .regular))
                                .foregroundStyle(preset == p ? Theme.accent : Theme.textSecondary)
                                .padding(.horizontal, 14)
                                .padding(.vertical, 8)
                                .background {
                                    if preset == p {
                                        Capsule()
                                            .fill(Theme.accent.opacity(0.15))
                                            .overlay(
                                                Capsule()
                                                    .strokeBorder(Theme.accent.opacity(0.35),
                                                                  lineWidth: 1))
                                            .matchedGeometryEffect(id: "range", in: selection)
                                    }
                                }
                        }
                        .buttonStyle(.plain)
                    }
                }
                .padding(3)
                .background {
                    Capsule()
                        .fill(Color.white.opacity(0.035))
                        .overlay(Capsule().strokeBorder(Theme.stroke, lineWidth: 1))
                }
            }
            .scrollClipDisabled()

            if preset == .custom {
                HStack(spacing: 10) {
                    customDateField("起", text: $customFrom)
                    Text("—")
                        .font(.caption)
                        .foregroundStyle(Theme.textTertiary)
                    customDateField("止", text: $customTo)
                    Button("应用", action: onApply)
                        .font(.system(size: 12, weight: .semibold))
                        .foregroundStyle(Color.black.opacity(0.85))
                        .padding(.horizontal, 14)
                        .padding(.vertical, 8)
                        .background(
                            Capsule().fill(
                                LinearGradient(colors: [Theme.accent, Theme.accentDeep],
                                               startPoint: .leading, endPoint: .trailing)))
                        .buttonStyle(.plain)
                }
                .padding(10)
                .background {
                    RoundedRectangle(cornerRadius: 16, style: .continuous)
                        .fill(Color.white.opacity(0.04))
                }
                .overlay {
                    RoundedRectangle(cornerRadius: 16, style: .continuous)
                        .strokeBorder(Theme.stroke, lineWidth: 1)
                }
            }
        }
    }

    private func customDateField(_ title: String, text: Binding<String>) -> some View {
        HStack(spacing: 4) {
            Text(title)
                .font(.system(size: 10, weight: .medium))
                .foregroundStyle(Theme.textTertiary)
            DatePicker("", selection: Binding(
                get: { Format.dateFromYMD(text.wrappedValue) ?? Date() },
                set: { text.wrappedValue = Format.dateStr(Int($0.timeIntervalSince1970 * 1000)) }),
                displayedComponents: .date)
                .labelsHidden()
                .datePickerStyle(.compact)
                .environment(\.locale, Locale(identifier: "zh_CN"))
        }
    }
}

// MARK: - 设备筛选条

struct DeviceFilterBar: View {
    @EnvironmentObject var settings: AppSettings
    @EnvironmentObject var refresh: RefreshService

    var body: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 7) {
                FilterChip(title: "全部",
                           selected: refresh.selectedFilter == .all,
                           icon: "square.stack.3d.up") {
                    refresh.setFilter(.all)
                }
                FilterChip(title: "本机",
                           selected: refresh.selectedFilter == .local,
                           icon: "iphone") {
                    refresh.setFilter(.local)
                }
                // 排除本机（已有「本机」chip）+ 过滤零记录设备（注册后从未
                // 上报用量的设备在筛选条里是噪音；管理入口不在此处）
                ForEach(refresh.devices.filter {
                    $0.device_id != settings.sync.device_id
                        && ($0.record_count ?? 0) > 0
                }) { d in
                    FilterChip(title: d.device_name,
                               selected: refresh.selectedFilter == .specific(d.device_id),
                               icon: "desktopcomputer") {
                        refresh.setFilter(.specific(d.device_id))
                    }
                }
            }
            .padding(.vertical, 1)
        }
        .scrollClipDisabled()
    }
}

// MARK: - Token 构成（主卡）

private struct TokenCompositionCard: View {
    let usage: RemoteUsage
    let cost: CostResult?
    let hitRate: Double?
    @EnvironmentObject var settings: AppSettings

    private var inputNonCache: Int {
        max(0, usage.overall.input_tokens - usage.overall.cache_read_tokens)
    }
    /// 构成条的分母：每个 token 只计一次——非缓存输入 + 缓存读 + 输出
    ///（input_tokens 已包含 cache_read，不能再把 input 整段计入）。
    private var barTotal: Int {
        max(inputNonCache + usage.overall.cache_read_tokens
            + usage.overall.output_tokens, 1)
    }
    private var costText: String {
        guard let c = cost else { return "—" }
        return settings.currency == .cny
            ? Format.cost(c.total_cny, .cny)
            : Format.cost(c.total_usd, .usd)
    }

    var body: some View {
        GlassCard(tint: Theme.accent, elevated: true) {
            VStack(alignment: .leading, spacing: 16) {
                HStack(spacing: 8) {
                    CardHeader(title: "Token 构成",
                               icon: "circle.hexagongrid.fill",
                               tint: Theme.violet,
                               detail: "\(usage.overall.requests) 次请求")
                }

                HStack(alignment: .firstTextBaseline, spacing: 6) {
                    Text(Format.tokens(usage.overall.total_tokens))
                        .font(.system(size: 36, weight: .bold, design: .rounded))
                        .monospacedDigit()
                        .foregroundStyle(.white.opacity(0.96))
                        .lineLimit(1)
                        .minimumScaleFactor(0.6)
                    Text("tok")
                        .font(.system(size: 12, weight: .medium))
                        .foregroundStyle(Theme.textTertiary)
                    Spacer(minLength: 8)
                    VStack(alignment: .trailing, spacing: 2) {
                        Text("花费")
                            .font(.system(size: 9))
                            .foregroundStyle(Theme.textTertiary)
                        Text(costText)
                            .font(.system(size: 17, weight: .semibold, design: .rounded))
                            .monospacedDigit()
                            .foregroundStyle(Theme.mint)
                    }
                }

                SegmentedBar(segments: [
                    .init(value: Double(inputNonCache), color: Theme.accent),
                    .init(value: Double(usage.overall.cache_read_tokens), color: Theme.violet),
                    .init(value: Double(usage.overall.output_tokens), color: Theme.mint),
                ], height: 9)

                HStack(alignment: .top, spacing: 10) {
                    compositionStat(title: "输入（非缓存）",
                                    value: Format.tokens(inputNonCache),
                                    pct: Double(inputNonCache) / Double(barTotal),
                                    color: Theme.accent)
                    compositionStat(title: "缓存读",
                                    value: Format.tokens(usage.overall.cache_read_tokens),
                                    pct: Double(usage.overall.cache_read_tokens) / Double(barTotal),
                                    color: Theme.violet)
                    compositionStat(title: "输出",
                                    value: Format.tokens(usage.overall.output_tokens),
                                    pct: Double(usage.overall.output_tokens) / Double(barTotal),
                                    color: Theme.mint)
                }

                HUDDivider()

                HStack(spacing: 10) {
                    HeroMetric(label: "缓存命中率",
                               value: hitRate.map { Format.pct($0) } ?? "—",
                               tint: Theme.violet)
                    HeroMetric(label: "缓存写",
                               value: Format.tokens(usage.overall.cache_write_tokens))
                    HeroMetric(label: "推理",
                               value: Format.tokens(usage.overall.reasoning_tokens))
                }
            }
        }
    }

    private func compositionStat(title: String, value: String,
                                  pct: Double, color: Color) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 4) {
                Circle().fill(color).frame(width: 5, height: 5)
                Text(title)
                    .font(.system(size: 9, weight: .medium))
                    .foregroundStyle(Theme.textTertiary)
                    .lineLimit(1)
            }
            Text(value)
                .font(.system(size: 14, weight: .semibold, design: .rounded))
                .monospacedDigit()
                .foregroundStyle(.white.opacity(0.88))
                .lineLimit(1)
                .minimumScaleFactor(0.7)
            Text(Format.pct(min(max(pct, 0), 1)))
                .font(.system(size: 9))
                .foregroundStyle(Theme.textTertiary)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

// MARK: - 明细指标

private struct OverallCard: View {
    let usage: RemoteUsage
    let cost: CostResult?
    @EnvironmentObject var settings: AppSettings

    var body: some View {
        GlassCard {
            VStack(alignment: .leading, spacing: 15) {
                CardHeader(title: "明细",
                           icon: "list.bullet.rectangle.portrait",
                           tint: Theme.accent,
                           detail: "服务端聚合")

                LazyVGrid(columns: [GridItem(.flexible(), spacing: 14),
                                    GridItem(.flexible(), spacing: 14)],
                          alignment: .leading, spacing: 16) {
                    MetricCell(title: "请求数",
                               value: String(usage.overall.requests),
                               icon: "arrow.up.arrow.down")
                    MetricCell(title: "总 Token",
                               value: Format.tokens(usage.overall.total_tokens),
                               icon: "sum")
                    MetricCell(title: "输入",
                               value: Format.tokens(usage.overall.input_tokens),
                               subtitle: "含缓存读",
                               icon: "arrow.down.to.line")
                    MetricCell(title: "输出",
                               value: Format.tokens(usage.overall.output_tokens),
                               icon: "arrow.up.from.line")
                    MetricCell(title: "花费 ¥",
                               value: cost.map { Format.cost($0.total_cny, .cny) } ?? "—",
                               color: Theme.mint,
                               icon: "yensign.circle")
                    MetricCell(title: "花费 $",
                               value: cost.map { Format.cost($0.total_usd, .usd) } ?? "—",
                               color: Theme.mint,
                               icon: "dollarsign.circle")
                }
            }
        }
    }
}

// MARK: - 按模型

private struct ModelBreakdownCard: View {
    let usage: RemoteUsage
    let cost: CostResult?
    @EnvironmentObject var settings: AppSettings

    private var models: [ModelStat] {
        usage.by_model.sorted { $0.total_tokens > $1.total_tokens }
    }

    var body: some View {
        GlassCard {
            VStack(alignment: .leading, spacing: 14) {
                CardHeader(title: "按模型",
                           icon: "square.stack.3d.up",
                           tint: Theme.violet,
                           detail: models.isEmpty ? nil : "\(models.count) 个模型")

                if models.isEmpty {
                    EmptyState(icon: "tray", title: "范围内无模型数据")
                } else {
                    let maxTokens = models.map { $0.total_tokens }.max() ?? 1
                    let totalTokens = max(models.reduce(0) { $0 + $1.total_tokens }, 1)
                    VStack(spacing: 0) {
                        ForEach(Array(models.enumerated()), id: \.element.id) { index, m in
                            ModelRow(stat: m,
                                     cost: costFor(modelId: m.model_id),
                                     maxTokens: maxTokens,
                                     totalTokens: totalTokens)
                            if index < models.count - 1 {
                                HUDDivider().padding(.vertical, 12)
                            }
                        }
                    }
                }
            }
        }
    }

    private func costFor(modelId: String) -> Double? {
        guard let c = cost else { return nil }
        return settings.currency == .cny
            ? c.per_model_cny.first { $0.model_id == modelId }?.cost
            : c.per_model_usd.first { $0.model_id == modelId }?.cost
    }
}

private struct ModelRow: View {
    let stat: ModelStat
    let cost: Double?
    let maxTokens: Int
    let totalTokens: Int
    @EnvironmentObject var settings: AppSettings

    private var visual: AgentVisual {
        AgentVisual.forProvider(stat.provider_id, modelId: stat.model_id)
    }

    var body: some View {
        HStack(alignment: .top, spacing: 11) {
            AgentTile(visual: visual, size: 32)
                .padding(.top, 1)

            VStack(alignment: .leading, spacing: 7) {
                HStack(alignment: .firstTextBaseline, spacing: 8) {
                    Text(stat.model_id)
                        .font(.system(size: 13.5, weight: .semibold))
                        .foregroundStyle(.white.opacity(0.90))
                        .lineLimit(1)
                        .truncationMode(.middle)
                    Spacer(minLength: 6)
                    if let c = cost {
                        Text(Format.cost(c, settings.currency))
                            .font(.system(size: 13, weight: .semibold, design: .rounded))
                            .monospacedDigit()
                            .foregroundStyle(Theme.mint)
                    }
                }

                HStack(spacing: 6) {
                    Text("\(stat.requests) 次")
                    Text("·")
                    Text(Format.tokens(stat.total_tokens) + " tok")
                    Spacer(minLength: 4)
                    Text(Format.pct(Double(stat.total_tokens) / Double(totalTokens)))
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
                                min(1, Double(stat.total_tokens) / Double(max(maxTokens, 1))))))
                    }
                }
                .frame(height: 4)
            }
        }
    }
}

// MARK: - 加载占位

struct LoadingPlaceholder: View {
    var message: String = "正在加载…"

    var body: some View {
        GlassCard {
            VStack(spacing: 12) {
                ProgressView()
                    .tint(Theme.accent)
                Text(message)
                    .font(.caption)
                    .foregroundStyle(Theme.textSecondary)
                    .multilineTextAlignment(.center)
            }
            .frame(maxWidth: .infinity)
            .padding(.vertical, 34)
        }
    }
}
