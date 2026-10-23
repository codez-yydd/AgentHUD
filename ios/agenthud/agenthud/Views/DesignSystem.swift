//
//  DesignSystem.swift
//  agenthud
//
//  iOS 端统一视觉体系：配色、背景、玻璃卡片、页头、指标块、进度环与
//  各类小组件。所有页面只用这里的 token / 组件，避免各页各自写死颜色
//  与圆角导致观感不一致。
//
//  设计口径：
//   - 深空底色 + 顶部青蓝 / 底部紫色极光，营造 HUD 的纵深感；
//   - 卡片用 ultraThinMaterial + 渐变描边，不再是一整块纯黑；
//   - 主色 sky（选择态 / 图表），金额 mint，警告 amber，超限 coral；
//   - 数字统一 rounded + monospacedDigit，标题用 rounded 字体。
//

import SwiftUI

// MARK: - 颜色与尺寸 token

enum Theme {
    // 背景
    static let bgTop = Color(red: 0.035, green: 0.055, blue: 0.086)
    static let bgBottom = Color(red: 0.016, green: 0.024, blue: 0.040)

    // 主色 / 语义色
    static let accent = Color(red: 0.29, green: 0.78, blue: 1.00)      // #4AC7FF
    static let accentDeep = Color(red: 0.04, green: 0.48, blue: 0.90)  // #0B7BE6
    static let mint = Color(red: 0.30, green: 0.88, blue: 0.68)        // #4DE0AD
    static let violet = Color(red: 0.66, green: 0.58, blue: 1.00)      // #A895FF
    static let indigo = Color(red: 0.56, green: 0.66, blue: 1.00)      // #8FA8FF
    static let deepseek = Color(red: 0.30, green: 0.42, blue: 1.00)    // #4D6BFE
    static let amber = Color(red: 1.00, green: 0.76, blue: 0.35)       // #FFC25A
    static let coral = Color(red: 1.00, green: 0.45, blue: 0.51)       // #FF7383
    static let pink = Color(red: 1.00, green: 0.58, blue: 0.80)        // #FF94CC

    // 表面
    static let surface = Color.white.opacity(0.038)
    static let surfaceRaised = Color.white.opacity(0.062)
    static let surfaceSunken = Color.black.opacity(0.18)
    static let stroke = Color.white.opacity(0.09)
    static let strokeStrong = Color.white.opacity(0.16)

    // 文本
    static let textSecondary = Color.white.opacity(0.62)
    static let textTertiary = Color.white.opacity(0.44)

    // 圆角 / 间距
    static let cardRadius: CGFloat = 20
    static let tileRadius: CGFloat = 14
    static let pagePadding: CGFloat = 16

    /// 额度百分比语义色：越高越危险。
    static func quota(_ pct: Int) -> Color {
        if pct >= 85 { return coral }
        if pct >= 60 { return amber }
        return mint
    }
}

// MARK: - 全屏背景

/// 深空底 + 两团极光。静态绘制，滚动时无额外开销。
struct AppBackground: View {
    var body: some View {
        GeometryReader { geo in
            ZStack {
                LinearGradient(colors: [Theme.bgTop, Theme.bgBottom],
                               startPoint: .top, endPoint: .bottom)

                Circle()
                    .fill(RadialGradient(
                        colors: [Theme.accent.opacity(0.20), Theme.accent.opacity(0.0)],
                        center: .center, startRadius: 8, endRadius: 340))
                    .frame(width: 680, height: 680)
                    .position(x: geo.size.width * 0.92,
                              y: geo.size.height * 0.02)

                Circle()
                    .fill(RadialGradient(
                        colors: [Theme.violet.opacity(0.13), Theme.violet.opacity(0.0)],
                        center: .center, startRadius: 8, endRadius: 300))
                    .frame(width: 600, height: 600)
                    .position(x: geo.size.width * 0.02,
                              y: geo.size.height * 0.62)
            }
            .frame(width: geo.size.width, height: geo.size.height)
        }
        .ignoresSafeArea()
        .allowsHitTesting(false)
    }
}

// MARK: - 卡片

/// 全局玻璃卡片：material 底 + 从上到下的高光渐变 + 细描边。
struct GlassCard<Content: View>: View {
    var padding: CGFloat = 18
    var radius: CGFloat = Theme.cardRadius
    var tint: Color? = nil
    var elevated: Bool = false
    let content: Content

    init(padding: CGFloat = 18,
         radius: CGFloat = Theme.cardRadius,
         tint: Color? = nil,
         elevated: Bool = false,
         @ViewBuilder content: () -> Content) {
        self.padding = padding
        self.radius = radius
        self.tint = tint
        self.elevated = elevated
        self.content = content()
    }

    var body: some View {
        content
            .padding(padding)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background {
                RoundedRectangle(cornerRadius: radius, style: .continuous)
                    .fill(.ultraThinMaterial)
                    .overlay {
                        RoundedRectangle(cornerRadius: radius, style: .continuous)
                            .fill(LinearGradient(
                                colors: [
                                    (tint ?? .white).opacity(tint == nil ? 0.050 : 0.150),
                                    (tint ?? .white).opacity(0.010),
                                ],
                                startPoint: .topLeading,
                                endPoint: .bottomTrailing))
                    }
                    .overlay {
                        RoundedRectangle(cornerRadius: radius, style: .continuous)
                            .strokeBorder(LinearGradient(
                                colors: [.white.opacity(0.16), .white.opacity(0.035)],
                                startPoint: .topLeading,
                                endPoint: .bottomTrailing),
                                lineWidth: 1)
                    }
            }
            .shadow(color: .black.opacity(elevated ? 0.30 : 0.14),
                    radius: elevated ? 22 : 12,
                    x: 0, y: elevated ? 12 : 6)
    }
}

// MARK: - 页头

/// 各 Tab 首页统一页头：品牌小字 + 标题 + 右侧刷新按钮。
/// 所有页面隐藏系统导航栏后用它承担标题层级。
///
/// 收敛口径（2026-10 改版）：
///  - 标题从 31pt 降到 25pt，去掉页头副标题 —— 时间范围与设备筛选
///    已经在下方控制条里常驻，标题区再复述一遍只会让顶部堆积三行文字；
///  - 品牌行压到 9pt，只做身份标识，不再抢标题层级。
struct PageHeader: View {
    let title: String
    var eyebrow: String? = "AGENTHUD"
    var isLoading: Bool = false
    var onRefresh: (() -> Void)? = nil

    var body: some View {
        HStack(alignment: .center, spacing: 12) {
            VStack(alignment: .leading, spacing: 2) {
                if let eyebrow {
                    HStack(spacing: 5) {
                        Circle()
                            .fill(Theme.accent)
                            .frame(width: 4, height: 4)
                        Text(eyebrow)
                            .font(.system(size: 9, weight: .bold))
                            .tracking(1.7)
                            .foregroundStyle(Theme.textTertiary.opacity(0.9))
                    }
                }
                Text(title)
                    .font(.system(size: 25, weight: .bold, design: .rounded))
                    .foregroundStyle(.white)
                    .lineLimit(1)
                    .minimumScaleFactor(0.8)
            }
            Spacer(minLength: 8)
            if let onRefresh {
                RefreshButton(isLoading: isLoading, action: onRefresh)
            }
        }
        .padding(.top, 4)
    }
}

/// 圆形刷新按钮：加载中显示系统小菊花，不再用旋转图标（避免动画残留）。
struct RefreshButton: View {
    var isLoading: Bool
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            ZStack {
                Circle().fill(Theme.surfaceRaised)
                Circle().strokeBorder(Theme.stroke, lineWidth: 1)
                if isLoading {
                    ProgressView()
                        .controlSize(.small)
                        .tint(Theme.accent)
                } else {
                    Image(systemName: "arrow.clockwise")
                        .font(.system(size: 14, weight: .semibold))
                        .foregroundStyle(Theme.accent)
                }
            }
            .frame(width: 38, height: 38)
            .contentShape(Circle())
        }
        .buttonStyle(.plain)
        .disabled(isLoading)
        .accessibilityLabel("刷新")
    }
}

// MARK: - 卡片标题

/// 卡片内的标题块（图标 + 英文小标 + 中文标题 + 右侧说明）。
struct CardHeader: View {
    let title: String
    var icon: String? = nil
    var tint: Color = Theme.accent
    var detail: String? = nil

    var body: some View {
        HStack(spacing: 8) {
            if let icon {
                Image(systemName: icon)
                    .font(.system(size: 12, weight: .semibold))
                    .foregroundStyle(tint)
            }
            Text(title)
                .font(.system(size: 16, weight: .semibold, design: .rounded))
                .foregroundStyle(.white.opacity(0.92))
            Spacer(minLength: 8)
            if let detail {
                Text(detail)
                    .font(.caption2)
                    .foregroundStyle(Theme.textTertiary)
            }
        }
    }
}

// MARK: - 指标块

/// 小号指标（页面顶部三栏：请求数 / Token / 命中率）。
struct HeroMetric: View {
    let label: String
    let value: String
    var tint: Color = .white.opacity(0.92)

    var body: some View {
        VStack(alignment: .leading, spacing: 3) {
            Text(label)
                .font(.system(size: 10, weight: .medium))
                .foregroundStyle(Theme.textTertiary)
            Text(value)
                .font(.system(size: 15, weight: .semibold, design: .rounded))
                .monospacedDigit()
                .foregroundStyle(tint)
                .lineLimit(1)
                .minimumScaleFactor(0.7)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

// MARK: - 胶囊筛选 chip

struct FilterChip: View {
    let title: String
    let selected: Bool
    var icon: String? = nil
    var tint: Color = Theme.accent
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            HStack(spacing: 5) {
                if let icon {
                    Image(systemName: icon)
                        .font(.system(size: 9, weight: .semibold))
                }
                Text(title)
                    .font(.system(size: 12, weight: selected ? .semibold : .regular))
                    .lineLimit(1)
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 7)
            .background {
                Capsule()
                    .fill(selected ? tint.opacity(0.16) : Color.white.opacity(0.05))
            }
            .overlay {
                Capsule()
                    .strokeBorder(selected ? tint.opacity(0.42) : Color.white.opacity(0.06),
                                  lineWidth: 1)
            }
            .foregroundStyle(selected ? tint : Color.white.opacity(0.78))
        }
        .buttonStyle(.plain)
    }
}

// MARK: - 控制条（设备 / 时间范围 / 附加筛选）

/// 页面控制条：一屏只留一行筛选，替代原先「设备胶囊一整行 + 时间范围
/// 一整行」的堆叠布局。左侧设备菜单、右侧范围分段，附加筛选（日志的
/// 来源 / 模型）作为 extra 挂在右边。
struct HUDControlBar<Extra: View>: View {
    @Binding var preset: RangePreset
    @Binding var customFrom: String
    @Binding var customTo: String
    let onApply: () -> Void
    @ViewBuilder var extra: () -> Extra

    var body: some View {
        HStack(spacing: 8) {
            DeviceMenu()
            Spacer(minLength: 4)
            RangeControl(preset: $preset,
                         customFrom: $customFrom,
                         customTo: $customTo,
                         onApply: onApply)
            extra()
        }
    }
}

extension HUDControlBar where Extra == EmptyView {
    init(preset: Binding<RangePreset>,
         customFrom: Binding<String>,
         customTo: Binding<String>,
         onApply: @escaping () -> Void) {
        self.init(preset: preset,
                  customFrom: customFrom,
                  customTo: customTo,
                  onApply: onApply,
                  extra: { EmptyView() })
    }
}

/// 设备筛选：收成单个下拉菜单，不再横向铺一排胶囊。
/// 设备多时菜单可滚动，页面宽度不再被设备数量撑长。
struct DeviceMenu: View {
    @EnvironmentObject var settings: AppSettings
    @EnvironmentObject var refresh: RefreshService

    private var currentName: String {
        switch refresh.selectedFilter {
        case .all:
            return "全部设备"
        case .local:
            return settings.sync.device_name.isEmpty ? "本机" : settings.sync.device_name
        case .specific(let id):
            return refresh.devices.first { $0.device_id == id }?.device_name ?? "指定设备"
        }
    }

    private var otherDevices: [DeviceInfo] {
        refresh.devices.filter {
            $0.device_id != settings.sync.device_id && ($0.record_count ?? 0) > 0
        }
    }

    var body: some View {
        Menu {
            Button {
                refresh.setFilter(.all)
            } label: {
                Label("全部设备",
                      systemImage: refresh.selectedFilter == .all
                        ? "checkmark" : "square.stack.3d.up")
            }
            Button {
                refresh.setFilter(.local)
            } label: {
                Label(settings.sync.device_name.isEmpty ? "本机" : settings.sync.device_name,
                      systemImage: refresh.selectedFilter == .local
                        ? "checkmark" : "iphone")
            }
            if !otherDevices.isEmpty {
                Divider()
                ForEach(otherDevices) { d in
                    Button {
                        refresh.setFilter(.specific(d.device_id))
                    } label: {
                        Label(d.device_name,
                              systemImage: refresh.selectedFilter == .specific(d.device_id)
                                ? "checkmark" : "desktopcomputer")
                    }
                }
            }
        } label: {
            HStack(spacing: 5) {
                Image(systemName: "server.rack")
                    .font(.system(size: 9, weight: .semibold))
                Text(currentName)
                    .font(.system(size: 12, weight: .medium))
                    .lineLimit(1)
                    .truncationMode(.tail)
                Image(systemName: "chevron.down")
                    .font(.system(size: 8, weight: .bold))
                    .opacity(0.55)
            }
            .foregroundStyle(Theme.textSecondary)
            .padding(.horizontal, 10)
            .padding(.vertical, 7)
            .background(Capsule().fill(Color.white.opacity(0.05)))
            .overlay(Capsule().strokeBorder(Theme.stroke, lineWidth: 1))
            .contentShape(Capsule())
        }
        .fixedSize(horizontal: true, vertical: false)
    }
}

/// 时间范围：紧凑分段控件 + 日历按钮（自定义区间走 sheet，
/// 不再把两个 DatePicker 常驻在页面顶部）。
struct RangeControl: View {
    @Binding var preset: RangePreset
    @Binding var customFrom: String
    @Binding var customTo: String
    let onApply: () -> Void

    @State private var showCustom = false

    private let quick: [RangePreset] = [.today, .d1, .d7, .d30]

    private static func shortLabel(_ p: RangePreset) -> String {
        switch p {
        case .today: return "今日"
        case .d1: return "24时"
        case .d7: return "7天"
        case .d30: return "30天"
        case .custom: return "自定义"
        }
    }

    var body: some View {
        HStack(spacing: 1) {
            ForEach(quick, id: \.self) { p in
                Button {
                    guard preset != p else { return }
                    preset = p
                    onApply()
                } label: {
                    Text(Self.shortLabel(p))
                        .font(.system(size: 11.5,
                                      weight: preset == p ? .semibold : .regular))
                        .foregroundStyle(preset == p ? Theme.accent : Theme.textSecondary)
                        .padding(.horizontal, 7)
                        .padding(.vertical, 6)
                        .background {
                            if preset == p {
                                Capsule().fill(Theme.accent.opacity(0.16))
                            }
                        }
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
            }
            Button {
                showCustom = true
            } label: {
                Image(systemName: "calendar")
                    .font(.system(size: 11, weight: .semibold))
                    .foregroundStyle(preset == .custom ? Theme.accent : Theme.textSecondary)
                    .padding(.horizontal, 6)
                    .padding(.vertical, 6)
                    .background {
                        if preset == .custom {
                            Capsule().fill(Theme.accent.opacity(0.16))
                        }
                    }
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
        }
        .padding(3)
        .background(Capsule().fill(Color.white.opacity(0.04)))
        .overlay(Capsule().strokeBorder(Theme.stroke, lineWidth: 1))
        .fixedSize(horizontal: true, vertical: false)
        .sheet(isPresented: $showCustom) { customSheet }
    }

    private var customSheet: some View {
        VStack(spacing: 18) {
            HStack {
                Text("自定义区间")
                    .font(.system(size: 16, weight: .semibold, design: .rounded))
                Spacer()
                Button("取消") { showCustom = false }
                    .font(.system(size: 13))
                    .foregroundStyle(Theme.textSecondary)
            }

            HStack(spacing: 10) {
                dateField("起", text: $customFrom)
                Text("—")
                    .font(.caption)
                    .foregroundStyle(Theme.textTertiary)
                dateField("止", text: $customTo)
            }

            Button {
                preset = .custom
                showCustom = false
                onApply()
            } label: {
                Text("应用")
                    .font(.system(size: 14, weight: .semibold))
                    .foregroundStyle(Color.black.opacity(0.85))
                    .frame(maxWidth: .infinity)
                    .padding(.vertical, 11)
                    .background(
                        Capsule().fill(LinearGradient(
                            colors: [Theme.accent, Theme.accentDeep],
                            startPoint: .leading, endPoint: .trailing)))
            }
            .buttonStyle(.plain)
            .disabled(!customReady)
            .opacity(customReady ? 1 : 0.45)
        }
        .padding(18)
        .frame(maxHeight: .infinity, alignment: .top)
        .background(AppBackground())
        .presentationDetents([.height(250)])
        .presentationDragIndicator(.visible)
        .presentationBackground(Theme.bgTop)
        .tint(Theme.accent)
        .onAppear(perform: prefillCustom)
    }

    private var customReady: Bool {
        Format.dateFromYMD(customFrom) != nil && Format.dateFromYMD(customTo) != nil
    }

    /// 首次打开自定义区间时给一个合理的默认值（近 7 天），
    /// 避免空值直接应用后静默退化成「最近 24 小时」。
    private func prefillCustom() {
        guard customFrom.isEmpty || customTo.isEmpty else { return }
        let cal = Calendar.current
        let now = Date()
        let start = cal.date(byAdding: .day, value: -6, to: cal.startOfDay(for: now)) ?? now
        customFrom = Format.dateStr(Int(start.timeIntervalSince1970 * 1000))
        customTo = Format.dateStr(Int(now.timeIntervalSince1970 * 1000))
    }

    private func dateField(_ title: String, text: Binding<String>) -> some View {
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

/// 页内分组小标题（替代原先「每个分组一张卡片头」的写法）：
/// 只留一行 11pt 灰字，让内容本身承担视觉重量。
struct SectionLabel: View {
    let title: String
    var detail: String? = nil

    var body: some View {
        HStack(spacing: 6) {
            Text(title)
                .font(.system(size: 11, weight: .semibold))
                .tracking(0.8)
                .foregroundStyle(Theme.textTertiary)
            Spacer(minLength: 6)
            if let detail {
                Text(detail)
                    .font(.system(size: 10))
                    .foregroundStyle(Theme.textTertiary.opacity(0.75))
            }
        }
        .padding(.horizontal, 2)
    }
}

// MARK: - 小标签

struct Tag: View {
    let text: String
    init(_ text: String) { self.text = text }

    var body: some View {
        Text(text)
            .font(.system(size: 10, weight: .medium))
            .foregroundStyle(Theme.textSecondary)
            .padding(.horizontal, 7)
            .padding(.vertical, 3)
            .background(Capsule().fill(Color.white.opacity(0.05)))
            .overlay(Capsule().strokeBorder(Color.white.opacity(0.04), lineWidth: 1))
    }
}

// MARK: - Agent 图标

/// 各 Agent 的图标与配色。source（用量明细）与 AgentKind（额度）两套
/// 命名都在这里归一，页面不各自写死。
///
/// image 优先：品牌 mark 放在 Assets.xcassets 里（模板渲染，跟随 tint 上色），
/// 没有品牌资源的 provider 继续用 SF Symbols 兜底。
struct AgentVisual {
    let icon: String
    var image: String? = nil
    let tint: Color

    init(icon: String, image: String? = nil, tint: Color) {
        self.icon = icon
        self.image = image
        self.tint = tint
    }

    static func forSource(_ source: String) -> AgentVisual {
        switch source {
        case "zcode":
            return AgentVisual(icon: "sparkles", image: "BrandZai", tint: Theme.accent)
        case "codex":
            return AgentVisual(icon: "chevron.left.forwardslash.chevron.right",
                               image: "BrandCodex", tint: Theme.indigo)
        case "claude":
            return AgentVisual(icon: "asterisk", image: "BrandClaude", tint: Theme.amber)
        case "kimi":
            return AgentVisual(icon: "moon.stars.fill", image: "BrandKimi", tint: Theme.violet)
        case "dsh":
            return AgentVisual(icon: "water.waves", image: "BrandDeepSeek",
                               tint: Theme.deepseek)
        case "opencode":
            return AgentVisual(icon: "curlybraces", image: "BrandOpenCode", tint: Theme.mint)
        case "minimaxcode":
            return AgentVisual(icon: "waveform", image: "BrandMiniMax", tint: Theme.pink)
        default:
            return AgentVisual(icon: "cpu", tint: Theme.textSecondary)
        }
    }

    /// 用量 by_model 行只有 provider_id / model_id，没有 source 维度；
    /// 按常见 provider 命名做一次启发式映射，未知 provider 用中性 cpu 图标。
    static func forProvider(_ providerId: String, modelId: String = "") -> AgentVisual {
        let key = (providerId + " " + modelId).lowercased()
        if key.contains("codex") || key.contains("gpt") || key.contains("o3")
            || key.contains("o4") {
            return forSource("codex")
        }
        if key.contains("claude") || key.contains("anthropic") || key.contains("sonnet")
            || key.contains("opus") || key.contains("haiku") {
            return forSource("claude")
        }
        if key.contains("bigmodel") || key.contains("zai") || key.contains("glm") {
            return forSource("zcode")
        }
        if key.contains("deepseek") || key.contains("dsh") {
            return forSource("dsh")
        }
        if key.contains("opencode") {
            return forSource("opencode")
        }
        if key.contains("moonshot") {
            return AgentVisual(icon: "moon.stars.fill", image: "BrandMoonshot",
                               tint: Theme.violet)
        }
        if key.contains("kimi") {
            return forSource("kimi")
        }
        if key.contains("minimax") {
            return forSource("minimaxcode")
        }
        return AgentVisual(icon: "cube.fill", tint: Theme.violet)
    }

    static func forAgent(_ agent: AgentKind) -> AgentVisual {
        switch agent {
        case .zai:
            return AgentVisual(icon: "sparkles", image: "BrandZai", tint: Theme.accent)
        case .deepseek:
            return AgentVisual(icon: "water.waves", image: "BrandDeepSeek",
                               tint: Theme.deepseek)
        case .moonshot:
            return AgentVisual(icon: "moon.stars.fill", image: "BrandMoonshot",
                               tint: Theme.violet)
        case .minimax:
            return AgentVisual(icon: "waveform", image: "BrandMiniMax", tint: Theme.pink)
        case .opencodego:
            return AgentVisual(icon: "curlybraces", image: "BrandOpenCode", tint: Theme.mint)
        case .alibaba:
            return AgentVisual(icon: "cloud.fill", image: "BrandAlibaba",
                               tint: Theme.amber)
        case .codex:
            return AgentVisual(icon: "chevron.left.forwardslash.chevron.right",
                               image: "BrandCodex", tint: Theme.indigo)
        case .claude:
            return AgentVisual(icon: "asterisk", image: "BrandClaude", tint: Theme.amber)
        case .cursor:
            return AgentVisual(icon: "cursorarrow.rays", image: "BrandCursor",
                               tint: Theme.mint)
        case .kimi:
            return AgentVisual(icon: "moon.fill", image: "BrandKimi", tint: Theme.violet)
        case .qoder:
            return AgentVisual(icon: "square.stack.3d.up.fill", image: "BrandQoder",
                               tint: Theme.accent)
        case .longcat:
            return AgentVisual(icon: "cat.fill", image: "BrandLongCat",
                               tint: Theme.amber)
        case .mimo:
            return AgentVisual(icon: "circle.hexagongrid.fill", image: "BrandMiMo",
                               tint: Theme.pink)
        case .stepfun:
            return AgentVisual(icon: "chart.line.uptrend.xyaxis", image: "BrandStepFun",
                               tint: Theme.mint)
        case .grok:
            return AgentVisual(icon: "bolt.fill", image: "BrandGrok",
                               tint: Theme.coral)
        case .alibabatoken:
            return AgentVisual(icon: "shippingbox.fill", image: "BrandAlibabaToken",
                               tint: Theme.amber)
        }
    }
}

/// 圆角方形 Agent 图标：优先品牌 mark（模板着色），没有资源时回退 SF Symbol。
struct AgentTile: View {
    let icon: String
    var image: String? = nil
    var tint: Color = Theme.accent
    var size: CGFloat = 38

    init(icon: String, image: String? = nil,
         tint: Color = Theme.accent, size: CGFloat = 38) {
        self.icon = icon
        self.image = image
        self.tint = tint
        self.size = size
    }

    init(visual: AgentVisual, size: CGFloat = 38) {
        self.icon = visual.icon
        self.image = visual.image
        self.tint = visual.tint
        self.size = size
    }

    var body: some View {
        RoundedRectangle(cornerRadius: size * 0.32, style: .continuous)
            .fill(tint.opacity(0.13))
            .overlay {
                RoundedRectangle(cornerRadius: size * 0.32, style: .continuous)
                    .strokeBorder(tint.opacity(0.22), lineWidth: 1)
            }
            .overlay {
                Group {
                    if let image {
                        Image(image)
                            .renderingMode(.template)
                            .resizable()
                            .scaledToFit()
                            .frame(width: size * 0.52, height: size * 0.52)
                    } else {
                        Image(systemName: icon)
                            .font(.system(size: size * 0.40, weight: .semibold))
                    }
                }
                .foregroundStyle(tint)
            }
            .frame(width: size, height: size)
    }
}

// MARK: - 进度

/// 额度窗口紧凑行（账号卡内使用）：标题 + 重置倒计时 + 百分比 + 细进度条。
/// 相比 76pt 进度环，一行只占 30pt，多账号多窗口时不必翻好几屏。
struct QuotaWindowRow: View {
    let window: QuotaWindow

    private var pct: Int? {
        window.usedPercent.map { Int($0.rounded()) }
    }

    var body: some View {
        VStack(spacing: 6) {
            HStack(spacing: 8) {
                Text(window.title)
                    .font(.system(size: 12, weight: .medium))
                    .foregroundStyle(.white.opacity(0.86))
                    .lineLimit(1)
                Spacer(minLength: 6)
                if let r = window.resetsAt {
                    Text(Format.countdown(to: r))
                        .font(.system(size: 9.5))
                        .foregroundStyle(Theme.textTertiary)
                        .lineLimit(1)
                }
                if let p = pct {
                    Text("\(p)%")
                        .font(.system(size: 12, weight: .semibold, design: .rounded))
                        .monospacedDigit()
                        .foregroundStyle(Theme.quota(p))
                        .frame(minWidth: 34, alignment: .trailing)
                } else if let usage = QuotaText.usage(window) {
                    Text(usage)
                        .font(.system(size: 11, weight: .medium, design: .rounded))
                        .monospacedDigit()
                        .foregroundStyle(Theme.textSecondary)
                }
            }

            if let p = pct {
                GeometryReader { geo in
                    ZStack(alignment: .leading) {
                        Capsule().fill(Color.white.opacity(0.07))
                        if p > 0 {
                            Capsule()
                                .fill(LinearGradient(
                                    colors: [Theme.quota(p).opacity(0.62), Theme.quota(p)],
                                    startPoint: .leading, endPoint: .trailing))
                                .frame(width: max(5, geo.size.width * CGFloat(min(Double(p) / 100, 1))))
                        }
                    }
                }
                .frame(height: 5)
            }
        }
    }
}


/// 分段占比条（深浅一致的彩色分段，统计页 token 构成用）。
struct SegmentedBar: View {
    struct Segment: Identifiable {
        let id = UUID()
        let value: Double
        let color: Color
    }

    let segments: [Segment]
    var height: CGFloat = 10

    var body: some View {
        GeometryReader { geo in
            let total = max(segments.reduce(0) { $0 + $1.value }, 0.0001)
            HStack(spacing: 2) {
                ForEach(segments) { seg in
                    RoundedRectangle(cornerRadius: height / 2, style: .continuous)
                        .fill(seg.color)
                        .frame(width: max(0, geo.size.width * CGFloat(seg.value / total)))
                }
            }
            .clipShape(Capsule())
        }
        .frame(height: height)
        .background(Capsule().fill(Color.white.opacity(0.06)))
    }
}

// MARK: - 状态与空态

/// 空态：图标 + 标题 + 说明。
struct EmptyState: View {
    let icon: String
    let title: String
    var message: String? = nil

    var body: some View {
        VStack(spacing: 8) {
            Image(systemName: icon)
                .font(.system(size: 26, weight: .light))
                .foregroundStyle(Theme.textTertiary)
            Text(title)
                .font(.system(size: 13, weight: .medium))
                .foregroundStyle(Theme.textSecondary)
            if let message {
                Text(message)
                    .font(.system(size: 11))
                    .foregroundStyle(Theme.textTertiary)
                    .multilineTextAlignment(.center)
            }
        }
        .frame(maxWidth: .infinity)
        .padding(.vertical, 26)
    }
}

/// 未配置同步服务的引导卡。
struct NotConfiguredBanner: View {
    let message: String
    var actionTitle: String? = nil
    var action: (() -> Void)? = nil

    var body: some View {
        GlassCard(tint: Theme.accent, elevated: true) {
            VStack(alignment: .leading, spacing: 14) {
                ZStack {
                    RoundedRectangle(cornerRadius: 16, style: .continuous)
                        .fill(LinearGradient(
                            colors: [Theme.accent.opacity(0.24), Theme.violet.opacity(0.16)],
                            startPoint: .topLeading, endPoint: .bottomTrailing))
                    Image(systemName: "antenna.radiowaves.left.and.right")
                        .font(.system(size: 22, weight: .medium))
                        .foregroundStyle(Theme.accent)
                }
                .frame(width: 52, height: 52)

                Text("还没有连接数据源")
                    .font(.system(size: 18, weight: .semibold, design: .rounded))
                Text(message)
                    .font(.footnote)
                    .foregroundStyle(Theme.textSecondary)
                    .fixedSize(horizontal: false, vertical: true)

                if let act = action, let title = actionTitle {
                    Button(action: act) {
                        HStack(spacing: 6) {
                            Text(title)
                            Image(systemName: "arrow.right")
                                .font(.system(size: 11, weight: .semibold))
                        }
                        .font(.system(size: 13, weight: .semibold))
                        .foregroundStyle(Color.black.opacity(0.85))
                        .padding(.horizontal, 16)
                        .padding(.vertical, 10)
                        .background(
                            Capsule().fill(LinearGradient(
                                colors: [Theme.accent, Theme.accentDeep],
                                startPoint: .leading, endPoint: .trailing)))
                    }
                    .buttonStyle(.plain)
                }
            }
        }
    }
}

/// 加载 / 错误提示条。loading 用细进度条，error 用红色提示卡。
struct LoadingBar: View {
    let loading: Bool
    let error: String?

    var body: some View {
        VStack(spacing: 8) {
            if loading {
                ProgressView()
                    .controlSize(.small)
                    .tint(Theme.accent)
                    .frame(maxWidth: .infinity)
                    .padding(.vertical, 2)
            }
            if let err = error, !err.isEmpty {
                HStack(alignment: .top, spacing: 8) {
                    Image(systemName: "exclamationmark.triangle.fill")
                        .font(.system(size: 12, weight: .semibold))
                        .foregroundStyle(Theme.coral)
                    Text(err)
                        .font(.caption2)
                        .foregroundStyle(Theme.coral.opacity(0.92))
                        .lineLimit(3)
                        .fixedSize(horizontal: false, vertical: true)
                    Spacer(minLength: 0)
                }
                .padding(12)
                .background {
                    RoundedRectangle(cornerRadius: 14, style: .continuous)
                        .fill(Theme.coral.opacity(0.09))
                }
                .overlay {
                    RoundedRectangle(cornerRadius: 14, style: .continuous)
                        .strokeBorder(Theme.coral.opacity(0.22), lineWidth: 1)
                }
            }
        }
    }
}

// MARK: - 分隔线

struct HUDDivider: View {
    var body: some View {
        Rectangle()
            .fill(Color.white.opacity(0.055))
            .frame(height: 1)
    }
}

// MARK: - 共用修饰

extension View {
    /// 页面主体统一宽度（iPad / 横屏下不至于把卡片拉得过宽）。
    func hudContentWidth() -> some View {
        frame(maxWidth: 620)
            .frame(maxWidth: .infinity)
    }

    /// 列表 / 表单的深色底与强调色。
    func hudFormStyle() -> some View {
        scrollContentBackground(.hidden)
            .background(AppBackground())
            .tint(Theme.accent)
    }
}
