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
    static let cardRadius: CGFloat = 22
    static let tileRadius: CGFloat = 14
    static let pagePadding: CGFloat = 18

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

/// 各 Tab 首页统一页头：品牌点 + 大标题 + 副标题 + 右侧刷新按钮。
/// 所有页面隐藏系统导航栏后用它承担标题层级。
struct PageHeader: View {
    let title: String
    var subtitle: String? = nil
    var isLoading: Bool = false
    var onRefresh: (() -> Void)? = nil

    var body: some View {
        HStack(alignment: .top, spacing: 12) {
            VStack(alignment: .leading, spacing: 6) {
                HStack(spacing: 6) {
                    Circle()
                        .fill(Theme.accent)
                        .frame(width: 5, height: 5)
                    Text("AGENTHUD")
                        .font(.system(size: 10, weight: .bold))
                        .tracking(1.8)
                        .foregroundStyle(Theme.textTertiary)
                }
                Text(title)
                    .font(.system(size: 31, weight: .bold, design: .rounded))
                    .foregroundStyle(.white)
                    .lineLimit(1)
                    .minimumScaleFactor(0.8)
                if let subtitle {
                    Text(subtitle)
                        .font(.footnote)
                        .foregroundStyle(Theme.textSecondary)
                        .lineLimit(2)
                }
            }
            Spacer(minLength: 8)
            if let onRefresh {
                RefreshButton(isLoading: isLoading, action: onRefresh)
            }
        }
        .padding(.top, 8)
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
                        .font(.system(size: 15, weight: .semibold))
                        .foregroundStyle(Theme.accent)
                }
            }
            .frame(width: 44, height: 44)
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

/// 页内分组标题（额度页按 agent 分组、日志页按日期分组等）。
struct GroupHeader: View {
    let title: String
    var icon: String? = nil
    var image: String? = nil
    var tint: Color = Theme.accent
    var detail: String? = nil

    var body: some View {
        HStack(spacing: 8) {
            if let image {
                AgentTile(icon: icon ?? "cpu", image: image, tint: tint, size: 24)
            } else if let icon {
                AgentTile(icon: icon, tint: tint, size: 24)
            }
            Text(title)
                .font(.system(size: 13, weight: .semibold, design: .rounded))
                .foregroundStyle(Theme.textSecondary)
            Spacer(minLength: 8)
            if let detail {
                Text(detail)
                    .font(.caption2)
                    .foregroundStyle(Theme.textTertiary)
            }
        }
        .padding(.top, 2)
    }
}

// MARK: - 指标块

/// 指标格：上方标签、下方大数字，可选图标与副标题。
struct MetricCell: View {
    let title: String
    let value: String
    var subtitle: String? = nil
    var color: Color = .white.opacity(0.92)
    var icon: String? = nil
    var tint: Color = Theme.accent

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 5) {
                if let icon {
                    Image(systemName: icon)
                        .font(.system(size: 9, weight: .semibold))
                        .foregroundStyle(tint)
                }
                Text(title)
                    .font(.caption2)
                    .foregroundStyle(Theme.textTertiary)
            }
            Text(value)
                .font(.system(size: 20, weight: .semibold, design: .rounded))
                .monospacedDigit()
                .foregroundStyle(color)
                .lineLimit(1)
                .minimumScaleFactor(0.65)
            if let subtitle {
                Text(subtitle)
                    .font(.system(size: 10))
                    .foregroundStyle(Theme.textTertiary)
                    .lineLimit(1)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

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

// MARK: - 小标签

/// 胶囊小标签（统计 / 日志 / 额度卡片内复用）。
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

/// 通用细进度条（额度窗口 / 占比条）。
struct ProgressBar: View {
    let pct: Int
    let label: String
    let resetMs: Int?
    var tint: Color? = nil

    var body: some View {
        VStack(alignment: .leading, spacing: 7) {
            HStack(spacing: 6) {
                Text(label)
                    .font(.system(size: 13, weight: .medium))
                    .foregroundStyle(.white.opacity(0.88))
                Spacer()
                Text("\(pct)%")
                    .font(.system(size: 13, weight: .semibold, design: .rounded))
                    .monospacedDigit()
                    .foregroundStyle(tint ?? Theme.quota(pct))
            }
            GeometryReader { geo in
                ZStack(alignment: .leading) {
                    Capsule().fill(Color.white.opacity(0.07))
                    Capsule()
                        .fill(LinearGradient(
                            colors: [(tint ?? Theme.quota(pct)).opacity(0.65),
                                     (tint ?? Theme.quota(pct))],
                            startPoint: .leading, endPoint: .trailing))
                        .frame(width: max(6, geo.size.width * CGFloat(min(max(Double(pct) / 100, 0), 1))))
                }
            }
            .frame(height: 7)
            if let r = resetMs {
                Text("重置 · " + Format.countdown(to: r))
                    .font(.system(size: 10))
                    .foregroundStyle(Theme.textTertiary)
            }
        }
    }
}

/// 额度进度环（账号卡主窗口用）。
struct QuotaRing: View {
    let pct: Int
    let title: String
    var resetMs: Int? = nil
    var detail: String? = nil
    var size: CGFloat = 76

    var body: some View {
        VStack(spacing: 8) {
            ZStack {
                Circle()
                    .stroke(Color.white.opacity(0.07), lineWidth: 7)
                Circle()
                    .trim(from: 0, to: CGFloat(min(max(Double(pct) / 100, 0), 1)))
                    .stroke(
                        AngularGradient(
                            colors: [Theme.quota(pct).opacity(0.45), Theme.quota(pct)],
                            center: .center,
                            startAngle: .degrees(0),
                            endAngle: .degrees(360)),
                        style: StrokeStyle(lineWidth: 7, lineCap: .round))
                    .rotationEffect(.degrees(-90))
                VStack(spacing: -1) {
                    Text("\(pct)")
                        .font(.system(size: size * 0.27, weight: .bold, design: .rounded))
                        .monospacedDigit()
                        .foregroundStyle(.white.opacity(0.95))
                    Text("%")
                        .font(.system(size: size * 0.12, weight: .semibold))
                        .foregroundStyle(Theme.textTertiary)
                }
            }
            .frame(width: size, height: size)

            Text(title)
                .font(.system(size: 11, weight: .medium))
                .foregroundStyle(Theme.textSecondary)
                .lineLimit(1)
            if let detail {
                Text(detail)
                    .font(.system(size: 9))
                    .monospacedDigit()
                    .foregroundStyle(Theme.textTertiary)
                    .lineLimit(1)
            }
            if let r = resetMs {
                Text(Format.countdown(to: r))
                    .font(.system(size: 9))
                    .foregroundStyle(Theme.textTertiary)
                    .lineLimit(1)
            }
        }
        .frame(maxWidth: .infinity)
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
