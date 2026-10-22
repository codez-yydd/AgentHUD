//
//  CompareView.swift
//  agenthud
//
//  周额度对比：按智谱重置周期展示 weekly 百分比变化 + 实际 token + 折算消耗。
//  快照来自 server /snapshots，token/消耗来自 /period_detail + peak 折算。
//
//  多账号快照先按「当前账号」过滤再切周期（对齐桌面 quota_history.rs 的
//  filter_for_current：account 为 nil 的旧快照保留，带指纹的只保留当前账号
//  的——多账号快照按 ts 交错混采会把几乎每条边界都切成"周期"，90 天可切出
//  数万个，被服务端 /period_detail 的周期数上限拦截）。本页由额度页 toolbar
//  的 NavigationLink push 进入（无内层 NavigationStack），导航栏来自父级栈。
//

import SwiftUI
import Charts

struct CompareView: View {
    @EnvironmentObject var settings: AppSettings
    @EnvironmentObject var refresh: RefreshService
    @EnvironmentObject var tabRouter: TabRouter

    /// 按设备筛选拉回的全量快照（内存留存：切账号只重新过滤切分，不重拉）
    @State private var allSnaps: [RemoteSnapshot] = []
    /// distinct 账号指纹（≥2 时展示账号筛选 chips）
    @State private var accounts: [String] = []
    /// 当前选中账号指纹（nil = 数据里没有任何指纹，无需过滤）
    @State private var selectedAccount: String?
    @State private var periods: [WeeklyPeriod] = []
    @State private var tokenBuckets: [WeeklyTokenBucket] = []
    @State private var consumedBuckets: [ConsumedBucket] = []
    @State private var loading = false
    @State private var error: String?

    /// 请求代际（竞态保护）：load / reparse 每次发起自增，
    /// 异步回写前校验仍是最新代际，旧的迟到响应整体丢弃。
    @State private var requestEpoch = 0

    /// 服务端 /period_detail 的周期数上限（PERIOD_DETAIL_MAX_PERIODS=200），
    /// 过滤后仍超出时只保留最近 200 个再请求
    private static let maxPeriods = 200

    /// 柱状图最多绘制的周期数（超出时只画最近的，避免 x 轴标签叠死；
    /// 周期明细表仍显示全部）。internal：同文件的 WeeklyPctChart 也要用。
    static let maxChartPeriods = 16

    var body: some View {
        ZStack {
            AppBackground()

            ScrollView {
                VStack(alignment: .leading, spacing: 16) {
                    if !settings.isSyncConfigured {
                        NotConfiguredBanner(message: "尚未连接同步服务，对比需要先注册设备。",
                                            actionTitle: "去设置",
                                            action: { tabRouter.selected = .settings })
                    } else {
                        LoadingBar(loading: loading && !periods.isEmpty, error: error)

                        // 账号筛选：多账号（≥2 个指纹）时展示 chips；单账号不展示
                        // 但同样按该账号过滤（见 reparse 的过滤规则）
                        if accounts.count >= 2 {
                            ScrollView(.horizontal, showsIndicators: false) {
                                HStack(spacing: 7) {
                                    ForEach(accounts, id: \.self) { acc in
                                        FilterChip(title: Self.chipTitle(acc),
                                                   selected: selectedAccount == acc,
                                                   icon: "person.crop.circle") {
                                            selectedAccount = acc
                                            reparse()
                                        }
                                    }
                                }
                                .padding(.vertical, 1)
                            }
                            .scrollClipDisabled()
                        }

                        if periods.isEmpty {
                            // 空态：已配置同步、加载完成、无错误但无周期可切
                            if loading {
                                LoadingPlaceholder(
                                    message: "正在解析近 90 天额度快照…\n数据量较大时可能需要几十秒")
                            } else if error == nil {
                                GlassCard {
                                    EmptyState(icon: "calendar.badge.exclamationmark",
                                               title: "暂无额度快照",
                                               message: "请确认桌面端已在本设备 / 账号范围上报快照")
                                }
                            }
                        } else {
                            WeeklyPctChart(periods: periods)

                            PeriodsTable(periods: periods,
                                         tokens: tokenBuckets,
                                         consumed: consumedBuckets)
                        }
                    }
                }
                .padding(.horizontal, Theme.pagePadding)
                .padding(.top, 8)
                .padding(.bottom, 104)
                .hudContentWidth()
            }
            .scrollIndicators(.hidden)
            .refreshable { await performLoad() }
        }
        .navigationTitle("周额度对比")
        .navigationBarTitleDisplayMode(.inline)
        .toolbar(.visible, for: .navigationBar)
        .toolbarBackground(.ultraThinMaterial, for: .navigationBar)
        .toolbarBackground(.visible, for: .navigationBar)
        .toolbar {
            // navigationBarTrailing：与 top 系 placement 视觉等价（导航栏
            // 右上角），规避 iOS 27 模拟器 runtime 上该 placement 导致
            // 整棵 TabView 白屏的兼容性问题
            ToolbarItem(placement: .navigationBarTrailing) {
                Button { load() } label: { Image(systemName: "arrow.clockwise") }
                    .tint(Theme.accent)
            }
        }
        .onAppear { load() }
        .onChange(of: refresh.selectedFilter) { _, _ in load() }
    }

    /// chip 显示名：指纹太长不能整串展示，用「账号 + 前 8 位」
    ///（与额度页 GLM 同步卡的账号指纹同一批，可相互对应）。
    private static func chipTitle(_ account: String) -> String {
        let prefix = account.prefix(8)
        return "账号 \(prefix)"
    }

    private func load() {
        Task { await performLoad() }
    }

    /// 拉快照 + 收集账号 + 默认选中（异步可等待版，refreshable 共用）。
    private func performLoad() async {
        guard settings.isSyncConfigured else { return }
        requestEpoch += 1
        let epoch = requestEpoch
        loading = true
        error = nil
        let server = settings.sync.server_url
        let token = settings.sync.device_token
        let localId = settings.sync.device_id
        let filter = refresh.selectedFilter

        do {
            // 取近 90 天的快照来切周期
            let now = Date()
            let fromMs = Int(now.timeIntervalSince1970 * 1000) - 90 * 86_400_000
            let toMs = Int(now.timeIntervalSince1970 * 1000)
            let snaps = try await APIClient.shared.snapshots(
                serverURL: server, deviceToken: token,
                fromMs: fromMs, toMs: toMs,
                filter: filter, localDeviceId: localId)
            // splitPeriods 依赖升序序列，显式排序兜底；ts 相同补次级键
            // weekly_reset（对齐桌面稳定排序语义，避免快照交错时顺序抖动）
            let sorted = snaps.sorted {
                if $0.ts != $1.ts { return $0.ts < $1.ts }
                return ($0.weekly_reset ?? $0.ts) < ($1.weekly_reset ?? $1.ts)
            }
            // distinct 账号指纹（保持出现顺序，去重）
            var seen = Set<String>()
            let distinct = sorted.compactMap { $0.account }
                .filter { seen.insert($0).inserted }
            await MainActor.run {
                guard epoch == self.requestEpoch else { return }
                self.allSnaps = sorted
                self.accounts = distinct
                // 刷新/切设备后：新账号集合仍包含原选中则保留（用户偏好），
                // 否则回退默认——最新一条带指纹快照的账号（对齐桌面
                // "当前账号"视角）；全部无指纹时保持 nil（过滤为恒等）
                if let cur = self.selectedAccount, distinct.contains(cur) {
                    // 保留已选账号
                } else {
                    self.selectedAccount = sorted.reversed()
                        .first { $0.account != nil }?.account ?? distinct.first
                }
                self.reparse()
            }
        } catch {
            await MainActor.run {
                guard epoch == self.requestEpoch else { return }
                self.error = error.localizedDescription
                self.loading = false
            }
        }
    }

    /// 按当前选中账号过滤全量快照 → 切周期 → 请求明细。
    /// 账号切换只走本函数（快照已在内存）；设备筛选变化走 load() 全量重拉。
    private func reparse() {
        Task { await performReparse() }
    }

    private func performReparse() async {
        requestEpoch += 1
        let epoch = requestEpoch

        // 账号过滤规则（与桌面「当前账号视角」同口径，但无归属采样直接丢弃）：
        // account == nil 的采样不可归属：同期存在带指纹采样时整体丢弃
        // （否则会冒充选中账号的历史）；全部无指纹的纯旧版客户端才保留。
        let parsed: [WeeklyPeriod] = await MainActor.run {
            let hasFingerprint = self.allSnaps.contains { $0.account != nil }
            let filtered = self.allSnaps.filter { snap in
                guard let acc = snap.account else { return !hasFingerprint }
                return acc == self.selectedAccount
            }
            var result = PeriodParser.splitPeriods(filtered.map { $0.toLocal() })
            // 防御：异常混采仍可能切出超上限的周期数，只保留最近 200 个
            if result.count > Self.maxPeriods {
                result = Array(result.suffix(Self.maxPeriods))
            }
            if result.isEmpty {
                self.periods = []
                self.tokenBuckets = []
                self.consumedBuckets = []
                self.loading = false
            } else {
                self.loading = true
                self.error = nil
            }
            return result
        }
        guard !parsed.isEmpty else { return }

        let server = settings.sync.server_url
        let token = settings.sync.device_token
        let localId = settings.sync.device_id
        let filter = refresh.selectedFilter

        do {
            // 取每个周期的明细折算 token + 消耗
            let periodPairs = parsed.map { ($0.reset_at, $0.end_at) }
            let detail = try await APIClient.shared.periodDetail(
                serverURL: server, deviceToken: token,
                periods: periodPairs,
                filter: filter, localDeviceId: localId)

            let tokenB: [WeeklyTokenBucket] = parsed.indices.map { i in
                let rows = (i < detail.count) ? detail[i].rows : []
                let total = rows.reduce(0) { $0 + $1.total_tokens }
                // 聚合协议：一行 = 同 (小时桶, 模型) 的多次请求，请求数
                // 按行内 requests 求和（旧服务端逐行响应每行为 1，同样成立）
                let reqs = rows.reduce(0) { $0 + $1.requests }
                return WeeklyTokenBucket(reset_at: parsed[i].reset_at,
                                          end_at: parsed[i].end_at,
                                          total_tokens: total,
                                          requests: reqs)
            }
            let consumed = Peak.detailToConsumed(detail, cfg: settings.peak)
            let consumedB: [ConsumedBucket] = parsed.indices.map { i in
                let c = (i < consumed.count) ? consumed[i] : (consumed: 0.0, requests: 0)
                return ConsumedBucket(reset_at: parsed[i].reset_at,
                                      end_at: parsed[i].end_at,
                                      consumed: c.consumed,
                                      requests: c.requests)
            }
            await MainActor.run {
                // 竞态保护：快速切 A→B 账号时，旧请求迟到直接丢弃
                guard epoch == self.requestEpoch else { return }
                self.periods = parsed
                self.tokenBuckets = tokenB
                self.consumedBuckets = consumedB
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
}

// MARK: - 周期 weekly 百分比图

private struct WeeklyPctChart: View {
    let periods: [WeeklyPeriod]

    var body: some View {
        let visible = periods.count > CompareView.maxChartPeriods
            ? Array(periods.suffix(CompareView.maxChartPeriods))
            : periods

        GlassCard {
            VStack(alignment: .leading, spacing: 14) {
                CardHeader(title: "每周峰值",
                           icon: "chart.bar.xaxis",
                           tint: Theme.accent,
                           detail: "\(visible.count) 个周期")

                let data = Array(visible.enumerated()).map { index, p in
                    ChartDatum(index: index, label: periodLabel(p), pct: p.pct_peak)
                }
                Chart(data) { d in
                    BarMark(
                        x: .value("周期", d.index),
                        y: .value("百分比", d.pct)
                    )
                    .foregroundStyle(
                        LinearGradient(
                            colors: [quotaColor(d.pct).opacity(0.95),
                                     quotaColor(d.pct).opacity(0.35)],
                            startPoint: .top, endPoint: .bottom))
                    .cornerRadius(4)
                }
                .chartXScale(range: .plotDimension(startPadding: 8, endPadding: 8))
                .chartYScale(domain: 0...100)
                .chartXAxis {
                    AxisMarks(values: tickIndices(count: data.count)) { value in
                        AxisValueLabel {
                            if let i = value.as(Int.self), data.indices.contains(i) {
                                Text(data[i].label)
                                    .font(.system(size: 9))
                                    .foregroundStyle(Theme.textTertiary)
                            }
                        }
                    }
                }
                .chartYAxis {
                    AxisMarks(position: .trailing, values: [0, 25, 50, 75, 100]) { value in
                        AxisGridLine(stroke: StrokeStyle(lineWidth: 0.5, dash: [3, 4]))
                            .foregroundStyle(Color.white.opacity(0.06))
                        AxisValueLabel {
                            if let v = value.as(Int.self) {
                                Text("\(v)%")
                                    .font(.system(size: 9))
                                    .foregroundStyle(Theme.textTertiary)
                            }
                        }
                    }
                }
                .frame(height: 170)
            }
        }
    }

    private struct ChartDatum: Identifiable {
        let index: Int
        let label: String
        let pct: Int
        var id: Int { index }
    }

    /// 最多 5 个 X 轴刻度，始终包含最近一个周期。
    private func tickIndices(count: Int) -> [Int] {
        guard count > 0 else { return [] }
        let step = max(1, Int(ceil(Double(count) / 5.0)))
        var indices: [Int] = []
        var i = count - 1
        while i >= 0 {
            indices.append(i)
            i -= step
        }
        return indices.reversed()
    }

    private func periodLabel(_ p: WeeklyPeriod) -> String {
        let d = Date(timeIntervalSince1970: TimeInterval(p.reset_at) / 1000)
        let fmt = DateFormatter()
        fmt.dateFormat = "MM-dd"
        return fmt.string(from: d)
    }
}

// MARK: - 周期表

private struct PeriodsTable: View {
    let periods: [WeeklyPeriod]
    let tokens: [WeeklyTokenBucket]
    let consumed: [ConsumedBucket]
    @EnvironmentObject var settings: AppSettings

    var body: some View {
        GlassCard {
            VStack(alignment: .leading, spacing: 14) {
                CardHeader(title: "周期明细",
                           icon: "list.bullet.rectangle",
                           tint: Theme.violet,
                           detail: "\(periods.count) 个周期")

                VStack(spacing: 0) {
                    ForEach(Array(periods.enumerated()), id: \.element.id) { index, p in
                        let i = periods.firstIndex { $0.reset_at == p.reset_at } ?? 0
                        let tok = tokens[safe: i]
                        let con = consumed[safe: i]
                        periodRow(p, tok: tok, con: con)
                        if index < periods.count - 1 {
                            HUDDivider().padding(.vertical, 11)
                        }
                    }
                }
            }
        }
    }

    private func periodRow(_ p: WeeklyPeriod,
                           tok: WeeklyTokenBucket?,
                           con: ConsumedBucket?) -> some View {
        VStack(alignment: .leading, spacing: 7) {
            HStack(spacing: 6) {
                Text(periodHeader(p))
                    .font(.system(size: 13, weight: .semibold))
                    .foregroundStyle(.white.opacity(0.9))
                if p.is_current {
                    Text("当前")
                        .font(.system(size: 9, weight: .medium))
                        .padding(.horizontal, 6)
                        .padding(.vertical, 2)
                        .background(Capsule().fill(Theme.mint.opacity(0.13)))
                        .foregroundStyle(Theme.mint)
                }
                Spacer()
                Text("起 \(p.pct_start)% · 峰 \(p.pct_peak)%")
                    .font(.system(size: 9).monospacedDigit())
                    .foregroundStyle(Theme.textTertiary)
                Text("\(p.pct_end)%")
                    .font(.system(size: 17, weight: .bold, design: .rounded))
                    .monospacedDigit()
                    .foregroundStyle(quotaColor(p.pct_end))
            }

            HStack(spacing: 6) {
                if let t = tok {
                    Tag("Token " + Format.tokens(t.total_tokens))
                    Tag("\(t.requests) 次")
                }
                if settings.peak.plan_type != nil, let c = con {
                    Tag("消耗 " + Format.tokens(Int(c.consumed)))
                }
                Tag("采样 \(p.sample_count)")
                Spacer(minLength: 4)
            }

            GeometryReader { geo in
                ZStack(alignment: .leading) {
                    Capsule().fill(Color.white.opacity(0.06))
                    Capsule()
                        .fill(LinearGradient(
                            colors: [quotaColor(p.pct_end).opacity(0.5), quotaColor(p.pct_end)],
                            startPoint: .leading, endPoint: .trailing))
                        .frame(width: max(4, geo.size.width * CGFloat(min(1, Double(p.pct_end) / 100))))
                }
            }
            .frame(height: 4)
        }
    }

    private func periodHeader(_ p: WeeklyPeriod) -> String {
        let f = DateFormatter()
        f.dateFormat = "MM-dd HH:mm"
        let r = Date(timeIntervalSince1970: TimeInterval(p.reset_at) / 1000)
        return f.string(from: r)
    }
}

extension Array {
    subscript(safe i: Int) -> Element? {
        return (0..<count).contains(i) ? self[i] : nil
    }
}
