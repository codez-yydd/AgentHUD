//
//  WidgetViews.swift
//  agenthudWidget
//
//  Widget 视图：主屏（小/中/大尺寸，全部账号 HUD 圆环）+ 锁屏（rectangular/circular/inline）。
//  所有数据从 AgentHUDEntry.snapshot 读取，不发起网络请求。
//  主屏按 quotaItems 渲染全部账号/服务商：中尺寸每个账号一列，主环取
//  占用最高的订阅窗口，下面固定展示 5h / 周 / 月三档；月档只认真实月度
//  窗口（GLM 的 MCP 月度是次数额度，不属于订阅月额度，不参与月档/主环）。
//  大尺寸逐行展开并显示套餐 + 账号（凭证备注/尾号，避免无意义编号）；
//  小尺寸只展示占用最高账号的三档窗口。锁屏仍按选中账号的标量字段渲染。
//

import WidgetKit
import SwiftUI

// MARK: - 形态判断辅助

extension WidgetSnapshot {
    /// 余额型（DeepSeek / Kimi 开放平台）：显示余额数字而非百分比。
    var isBalanceStyle: Bool { balanceAmount != nil }
    /// 只读同步型（Codex / Claude / Cursor）。
    var isSyncedStyle: Bool { isSynced == true }
    /// 余额符号（"CNY"→¥，"USD"→$，其余原样）。
    var balanceSymbol: String {
        switch balanceCurrency {
        case "CNY", "¥": return "¥"
        case "USD", "$": return "$"
        default: return balanceCurrency ?? ""
        }
    }
    /// 余额展示文本（如 "¥110.00"）。
    var balanceText: String {
        guard let amount = balanceAmount else { return "—" }
        return balanceSymbol + String(format: "%.2f", amount)
    }
}

// MARK: - 主屏 Widget 视图

struct AgentHUDHomeWidgetView: View {
    let entry: AgentHUDEntry
    @Environment(\.widgetFamily) var family

    var body: some View {
        switch family {
        case .systemSmall:
            // 主屏展示全部账号，选中账号只影响锁屏，因此主屏不显示 stale 提示
            SmallHomeView(snap: entry.snapshot, currency: entry.currency, stale: false)
        case .systemMedium:
            MediumHomeView(snap: entry.snapshot, currency: entry.currency, stale: false)
        case .systemLarge:
            LargeHomeView(snap: entry.snapshot, currency: entry.currency, stale: false)
        default:
            SmallHomeView(snap: entry.snapshot, currency: entry.currency, stale: false)
        }
    }
}

// MARK: - 额度展示辅助

extension WidgetQuotaItem {
    var isBalance: Bool { balanceAmount != nil }
    var isError: Bool {
        message != nil || status == "error" || status == "expired"
    }

    var balanceSymbol: String {
        switch balanceCurrency {
        case "CNY", "¥": return "¥"
        case "USD", "$": return "$"
        default: return balanceCurrency ?? ""
        }
    }

    /// 小圆环里用的余额缩写（¥110 / ¥1.2K / $6.5）。
    var balanceCompact: String {
        guard let amount = balanceAmount else { return "—" }
        let symbol = balanceSymbol
        if abs(amount) >= 100_000 { return symbol + String(format: "%.0fK", amount / 1000) }
        if abs(amount) >= 10_000 { return symbol + String(format: "%.1fK", amount / 1000) }
        if amount == amount.rounded() { return symbol + String(Int(amount)) }
        return symbol + String(format: "%.1f", amount)
    }

    /// 一个账号取占用最高（最紧急）的订阅窗口作为主环。
    /// GLM 的 `mcp` 是 MCP 月度次数额度，不是订阅月额度（桌面 HUD 也不
    /// 展示），不能参与主环，否则会出现「圆环 2%、5h/周/月全 0 或 —」。
    var primaryWindow: WidgetQuotaWindow? {
        let slotKeys: Set<String> = [
            "hour5", "weekly", "monthly", "interval", "cursor_auto", "cursor_api",
        ]
        let preferred = windows.filter { slotKeys.contains($0.key) }
        // 只有非标准窗口（积分/Token 配额等）时才回退到全部窗口，但 MCP
        // 永远不参与主环。
        let fallback = windows.filter { $0.key != "mcp" }
        let candidates = preferred.isEmpty ? fallback : preferred
        return candidates.max { ($0.pct ?? -1) < ($1.pct ?? -1) }
    }

}

extension WidgetSnapshot {
    /// 新版快照直接用 quotaItems；旧快照回退到单账号字段，保证升级瞬间不空白。
    var allQuotaItems: [WidgetQuotaItem] {
        if let items = quotaItems, !items.isEmpty { return items }
        guard accountId != nil || hour5Pct != nil || weeklyPct != nil
                || balanceAmount != nil else { return [] }

        var windows: [WidgetQuotaWindow] = []
        if let w = weeklyPct {
            windows.append(WidgetQuotaWindow(
                key: "weekly",
                title: agentKind == "cursor" ? "Auto" : "周",
                pct: w, resetsAt: weeklyResetMs))
        }
        if let h = hour5Pct {
            windows.append(WidgetQuotaWindow(key: "hour5", title: "5h",
                                             pct: h, resetsAt: hour5ResetMs))
        }
        // mcpPct 是 GLM 的 MCP 月度次数额度，不是订阅月额度；旧快照也
        // 不补进 windows，避免它被误当成「月」或主环。
        return [WidgetQuotaItem(
            id: "legacy",
            label: accountLabel ?? (level.isEmpty ? "额度" : level),
            detail: level.isEmpty ? nil : level,
            agent: agentKind ?? "",
            isSynced: isSynced == true,
            status: "ok",
            windows: windows,
            balanceAmount: balanceAmount,
            balanceCurrency: balanceCurrency,
            balanceGranted: balanceGranted,
            balanceToppedUp: balanceToppedUp,
            updatedAt: updatedAt)]
    }

    var timeAgoText: String {
        guard updatedAt > 0 else { return "待同步" }
        let diff = Date().timeIntervalSince1970 - updatedAt
        if diff < 60 { return "刚刚" }
        if diff < 3600 { return "\(Int(diff / 60))分前" }
        return "\(Int(diff / 3600))时前"
    }
}

// MARK: - 主屏视觉组件

private extension WidgetQuotaItem {
    /// 余额完整文本（大尺寸行内用，如 "¥110.00"）。
    var balanceFullText: String {
        guard let amount = balanceAmount else { return "—" }
        return balanceSymbol + String(format: "%.2f", amount)
    }

    /// 主环颜色：错误统一橙色，其余按占用率。
    var ringColor: Color {
        if isError { return WidgetTheme.amber }
        return primaryWindow?.pct.map(widgetQuotaColor) ?? WidgetTheme.tertiary
    }

    /// 套餐展示名（"max" → "Max"）。
    var planText: String? {
        guard let p = planName?.trimmingCharacters(in: .whitespacesAndNewlines),
              !p.isEmpty else { return nil }
        if p == p.lowercased(), let first = p.first {
            return String(first).uppercased() + String(p.dropFirst())
        }
        return p
    }

    /// 原始账号/凭证备注名（空 = 默认账号）。
    var accountText: String? {
        guard let a = account?.trimmingCharacters(in: .whitespacesAndNewlines),
              !a.isEmpty else { return nil }
        return a
    }

    /// 中/小尺寸用：把长账号指纹缩成「尾号 7127」这类可区分文本。
    static func shortenAccount(_ raw: String) -> String {
        let text = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        if text.hasPrefix("账号 ") {
            let digits = text.dropFirst(3).trimmingCharacters(in: .whitespaces)
            if digits.count > 5 { return "尾号 " + String(digits.suffix(5)) }
        }
        if text.hasPrefix("凭证 ") { return text }
        if let at = text.firstIndex(of: "@") {
            let local = text[..<at]
            if local.count > 8 {
                return String(local.prefix(6)) + "…" + String(text[at...])
            }
        }
        if text.count > 12 {
            return String(text.prefix(4)) + "…" + String(text.suffix(4))
        }
        return text
    }

    /// 大尺寸用：数字指纹仍然只显示尾号，人类可读的备注名才完整展示。
    static func displayAccount(_ raw: String) -> String {
        let text = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        if text.hasPrefix("账号 ") {
            let digits = text.dropFirst(3).trimmingCharacters(in: .whitespaces)
            if digits.count > 5 { return "尾号 " + String(digits.suffix(5)) }
        }
        return text
    }

    /// 账号身份：套餐 + 账号（多账号时不再用无意义编号）。
    func identityText(compact: Bool, needsDefault: Bool = false) -> String {
        var parts: [String] = []
        if let plan = planText { parts.append(plan) }
        if let acct = accountText {
            parts.append(compact ? Self.shortenAccount(acct) : Self.displayAccount(acct))
        } else if needsDefault {
            parts.append("默认")
        }
        if parts.isEmpty {
            if let d = detail?.trimmingCharacters(in: .whitespacesAndNewlines),
               !d.isEmpty {
                return compact ? Self.shortenAccount(d) : d
            }
            return "默认账号"
        }
        return parts.joined(separator: " · ")
    }

    /// 服务名 + 套餐：GLM · Max / Codex · Plus。
    var serviceTitle: String {
        guard let plan = planText else { return label }
        return "\(label) · \(plan)"
    }

    /// 账号行：尾号 7127 / 凭证 1 / 默认账号（无账号且只有一个服务实例时返回 nil）。
    func accountLine(needsDefault: Bool) -> String? {
        if let acct = accountText {
            return Self.shortenAccount(acct)
        }
        return needsDefault ? "默认账号" : nil
    }

    /// 6 列紧凑模式用：只保留 5 位尾号，避免标题被截断。
    static func accountSuffix(_ raw: String) -> String {
        let text = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        if text.hasPrefix("账号 ") {
            let digits = text.dropFirst(3).trimmingCharacters(in: .whitespaces)
            if digits.count > 5 { return String(digits.suffix(5)) }
        }
        if text.hasPrefix("凭证 ") { return text }
        if text.count > 8 { return String(text.suffix(4)) }
        return text
    }

    /// 6 列紧凑模式：套餐·账号放在一行，标题只留服务名。
    func compactIdentity(needsDefault: Bool) -> String {
        var parts: [String] = []
        if let plan = planText { parts.append(plan) }
        if let acct = accountText {
            parts.append(Self.accountSuffix(acct))
        } else if needsDefault {
            parts.append("默认")
        }
        if parts.isEmpty {
            if let d = detail?.trimmingCharacters(in: .whitespacesAndNewlines),
               !d.isEmpty { return d }
            return "默认"
        }
        return parts.joined(separator: "·")
    }

}

/// 顶部花费 + Token / 请求 / 更新时间。三档尺寸共用，紧凑模式用于中/小尺寸。
private struct WidgetCostHeader: View {
    let snap: WidgetSnapshot
    let currency: Currency
    let stale: Bool
    var compact: Bool = false

    var body: some View {
        HStack(alignment: .bottom, spacing: 8) {
            VStack(alignment: .leading, spacing: 0) {
                Text(stale ? "待刷新" : "今日花费")
                    .font(.system(size: compact ? 9 : 9.5, weight: .medium))
                    .foregroundStyle(stale ? WidgetTheme.amber : WidgetTheme.secondary)
                Text(costText)
                    .font(.system(size: compact ? 23 : 26, weight: .bold, design: .rounded))
                    .monospacedDigit()
                    .foregroundStyle(WidgetTheme.mint)
                    .lineLimit(1)
                    .minimumScaleFactor(0.8)
            }

            Spacer(minLength: 8)

            HStack(alignment: .bottom, spacing: compact ? 12 : 16) {
                metric("Token", Format.tokens(snap.todayTokens))
                metric("请求", "\(snap.todayRequests)")
                metric("更新", snap.timeAgoText)
            }
        }
    }

    private func metric(_ label: String, _ value: String) -> some View {
        VStack(alignment: .trailing, spacing: 1) {
            Text(label)
                .font(.system(size: compact ? 8.5 : 9, weight: .medium))
                .foregroundStyle(WidgetTheme.tertiary)
            Text(value)
                .font(.system(size: compact ? 10.5 : 11,
                              weight: .semibold, design: .rounded))
                .monospacedDigit()
                .foregroundStyle(Color.white.opacity(0.80))
                .lineLimit(1)
        }
    }

    private var costText: String {
        let n = (currency == .cny) ? snap.todayCostCny : snap.todayCostUsd
        return Format.cost(n, currency)
    }
}

private struct WidgetDivider: View {
    var body: some View {
        Rectangle()
            .fill(Color.white.opacity(0.07))
            .frame(height: 0.5)
    }
}

/// HUD 风格圆环：中心百分比/余额，外圈角度渐变 + 轻微辉光。
private struct QuotaRingView: View {
    let pct: Int?
    var amount: String? = nil
    let size: CGFloat
    var color: Color = WidgetTheme.mint

    var body: some View {
        let line = max(3.5, size * 0.095)
        ZStack {
            Circle()
                .fill(Color.white.opacity(0.025))

            Circle()
                .stroke(WidgetTheme.track, lineWidth: line)

            if let pct {
                Circle()
                    .trim(from: 0, to: CGFloat(min(max(Double(pct) / 100, 0), 1)))
                    .stroke(
                        AngularGradient(colors: [color.opacity(0.40), color],
                                        center: .center,
                                        startAngle: .degrees(0),
                                        endAngle: .degrees(360)),
                        style: StrokeStyle(lineWidth: line, lineCap: .round))
                    .rotationEffect(.degrees(-90))
                    .shadow(color: color.opacity(0.45), radius: 3)

                VStack(spacing: -2) {
                    Text("\(pct)")
                        .font(.system(size: size * 0.30, weight: .bold, design: .rounded))
                        .monospacedDigit()
                        .foregroundStyle(.white.opacity(0.96))
                        .minimumScaleFactor(0.7)
                        .lineLimit(1)
                    Text("%")
                        .font(.system(size: max(7, size * 0.135),
                                      weight: .semibold, design: .rounded))
                        .foregroundStyle(WidgetTheme.tertiary)
                }
            } else if let amount {
                Circle()
                    .stroke(color.opacity(0.32), lineWidth: line)
                Text(amount)
                    .font(.system(size: size * 0.24, weight: .bold, design: .rounded))
                    .monospacedDigit()
                    .foregroundStyle(color)
                    .lineLimit(1)
                    .minimumScaleFactor(0.45)
                    .padding(size * 0.12)
            } else {
                Text("—")
                    .font(.system(size: size * 0.28, weight: .semibold))
                    .foregroundStyle(WidgetTheme.tertiary)
            }
        }
        .frame(width: size, height: size)
    }
}

/// 中尺寸超过 6 个账号时，第五列后折叠为「+N 更多」。
private struct MoreQuotaCell: View {
    let count: Int
    let ringSize: CGFloat
    let labelSize: CGFloat

    var body: some View {
        VStack(spacing: 6) {
            QuotaRingView(pct: nil, amount: "+\(count)",
                          size: ringSize, color: WidgetTheme.secondary)
            Text("更多")
                .font(.system(size: labelSize, weight: .semibold))
                .foregroundStyle(WidgetTheme.secondary)
                .lineLimit(1)
        }
        .frame(maxWidth: .infinity)
    }
}

/// 空数据 / 额度未同步提示。
private struct HomePlaceholder: View {
    let hasData: Bool

    var body: some View {
        HStack(spacing: 8) {
            Image(systemName: hasData ? "arrow.triangle.2.circlepath" : "iphone.gen3")
                .font(.system(size: 14, weight: .semibold))
                .foregroundStyle(hasData ? WidgetTheme.secondary : WidgetTheme.amber)
            Text(hasData ? "额度未同步 · 桌面端刷新后自动更新"
                         : "打开 AgentHUD 同步数据后显示额度")
                .font(.system(size: 10.5, weight: .medium))
                .foregroundStyle(hasData ? WidgetTheme.secondary : WidgetTheme.amber)
                .lineLimit(2)
            Spacer(minLength: 0)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .center)
    }
}

// MARK: - 小尺寸：最高占用账号 + 全部窗口

private struct SmallHomeView: View {
    let snap: WidgetSnapshot
    let currency: Currency
    let stale: Bool

    private var hasData: Bool { snap.updatedAt > 0 }
    private var items: [WidgetQuotaItem] { snap.allQuotaItems }

    /// 小尺寸只放得下一个账号，选占用最高的那个；中/大尺寸展示全部账号。
    private var hero: WidgetQuotaItem? {
        guard !items.isEmpty else { return nil }
        return items.max {
            ($0.primaryWindow?.pct ?? -1) < ($1.primaryWindow?.pct ?? -1)
        }
    }

    private var duplicateLabels: Set<String> {
        let grouped = Dictionary(grouping: items, by: { $0.label })
        return Set(grouped.filter { $0.value.count > 1 }.keys)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(alignment: .firstTextBaseline, spacing: 4) {
                Text(stale ? "待刷新" : "今日")
                    .font(.system(size: 9, weight: .medium))
                    .foregroundStyle(stale ? WidgetTheme.amber : WidgetTheme.secondary)
                Text(costText)
                    .font(.system(size: 21, weight: .bold, design: .rounded))
                    .monospacedDigit()
                    .foregroundStyle(WidgetTheme.mint)
                    .lineLimit(1)
                    .minimumScaleFactor(0.75)
                Spacer(minLength: 4)
                Text(snap.timeAgoText)
                    .font(.system(size: 8, weight: .medium))
                    .foregroundStyle(WidgetTheme.tertiary)
            }

            WidgetDivider()
                .padding(.top, 6)
                .padding(.bottom, 7)

            if !hasData {
                HomePlaceholder(hasData: false)
            } else if let hero {
                Text(hero.serviceTitle)
                    .font(.system(size: 12, weight: .semibold))
                    .foregroundStyle(.white.opacity(0.95))
                    .lineLimit(1)
                if let account = hero.accountLine(needsDefault: duplicateLabels.contains(hero.label)) {
                    Text(account)
                        .font(.system(size: 9, weight: .medium))
                        .foregroundStyle(WidgetTheme.tertiary)
                        .lineLimit(1)
                }

                if hero.isError && hero.windows.isEmpty {
                    VStack(alignment: .leading, spacing: 3) {
                        Text("查询失败")
                            .font(.system(size: 11, weight: .semibold))
                            .foregroundStyle(WidgetTheme.amber)
                        Text("打开 App 查看原因")
                            .font(.system(size: 8.5, weight: .medium))
                            .foregroundStyle(WidgetTheme.tertiary)
                            .lineLimit(2)
                    }
                    .padding(.top, 8)
                } else {
                    HStack(alignment: .top, spacing: 6) {
                        if hero.isBalance {
                            windowCell(title: "余额", pct: nil,
                                       amount: hero.balanceCompact,
                                       color: WidgetTheme.mint)
                        } else {
                            ForEach(QuotaSlot.allCases) { slot in
                                let w = hero.window(for: slot)
                                windowCell(title: slot.title,
                                           pct: w?.pct,
                                           amount: nil,
                                           color: (w?.pct).map(widgetQuotaColor) ?? WidgetTheme.tertiary)
                            }
                        }
                    }
                    .padding(.top, 6)
                }

                Spacer(minLength: 0)

                Text(footerText)
                    .font(.system(size: 8, weight: .medium))
                    .foregroundStyle(WidgetTheme.tertiary)
                    .lineLimit(1)
            } else {
                HomePlaceholder(hasData: true)
            }
        }
    }

    private func windowCell(title: String, pct: Int?, amount: String?,
                            color: Color) -> some View {
        VStack(spacing: 3) {
            if let pct {
                QuotaRingView(pct: pct, size: 42, color: color)
            } else {
                QuotaRingView(pct: nil, amount: amount, size: 42, color: color)
            }
            Text(title)
                .font(.system(size: 9, weight: .medium))
                .foregroundStyle(WidgetTheme.tertiary)
                .lineLimit(1)
                .minimumScaleFactor(0.8)
        }
        .frame(maxWidth: .infinity)
    }

    private var footerText: String {
        if items.count <= 1 { return "最高占用账号 · 全部窗口" }
        return "共 \(items.count) 个账号 · 显示占用最高"
    }

    private var costText: String {
        let n = (currency == .cny) ? snap.todayCostCny : snap.todayCostUsd
        return Format.cost(n, currency)
    }
}

// MARK: - 中尺寸：账号列 + 全部窗口

/// 固定三档窗口槽位：让每个账号的 5h / 周 / 月在同一行对齐，
/// 缺失的窗口显示「—」，而不是按数组顺序错位。
/// 月槽只认真实月度窗口：GLM 的 `mcp` 是 MCP 月度次数额度，不是订阅
/// 月额度；`credits` / `quota` / `fuel` 等是积分/Token/加油包，也不能
/// 冒充月额度（否则会再次出现「GLM 月有值」这种错误）。
private enum QuotaSlot: Int, CaseIterable, Identifiable {
    case hour5, weekly, monthly
    var id: Int { rawValue }
    var title: String {
        switch self {
        case .hour5: return "5h"
        case .weekly: return "周"
        case .monthly: return "月"
        }
    }
}

private extension WidgetQuotaItem {
    func window(for slot: QuotaSlot) -> WidgetQuotaWindow? {
        switch slot {
        case .hour5:
            // MiniMax 的 interval 就是 5 小时会话窗口（桌面 HUD 同口径）。
            return windows.first { $0.key == "hour5" || $0.key == "interval" }
        case .weekly:
            return windows.first { $0.key == "weekly" }
        case .monthly:
            // Cursor 的 Auto/API 是账单周期额度，可以放在月档；其余只认
            // 明确的 monthly 键。MCP / 积分 / Token 配额一律不冒充月。
            return windows.first {
                ["monthly", "cursor_auto", "cursor_api"].contains($0.key)
            }
        }
    }
}

/// 中尺寸顶部一行：花费在左，Token/请求/更新在右，给额度腾出高度。
private struct MediumCostBar: View {
    let snap: WidgetSnapshot
    let currency: Currency
    let stale: Bool

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 6) {
            Text(stale ? "待刷新" : "今日花费")
                .font(.system(size: 9, weight: .medium))
                .foregroundStyle(stale ? WidgetTheme.amber : WidgetTheme.secondary)

            Text(costText)
                .font(.system(size: 20, weight: .bold, design: .rounded))
                .monospacedDigit()
                .foregroundStyle(WidgetTheme.mint)
                .lineLimit(1)
                .minimumScaleFactor(0.8)

            Spacer(minLength: 6)

            Text(Format.tokens(snap.todayTokens))
                .font(.system(size: 8.5, weight: .semibold, design: .rounded))
                .monospacedDigit()
                .foregroundStyle(WidgetTheme.secondary)
            Text("·")
                .font(.system(size: 8, weight: .medium))
                .foregroundStyle(WidgetTheme.tertiary)
            Text("\(snap.todayRequests) 次")
                .font(.system(size: 8.5, weight: .medium))
                .foregroundStyle(WidgetTheme.secondary)
            Text("· \(snap.timeAgoText)")
                .font(.system(size: 8, weight: .medium))
                .foregroundStyle(WidgetTheme.tertiary)
        }
    }

    private var costText: String {
        let n = (currency == .cny) ? snap.todayCostCny : snap.todayCostUsd
        return Format.cost(n, currency)
    }
}

/// 一个账号一列：主环 + 服务/套餐 + 账号 + 全部窗口百分比。
private struct MediumAccountColumn: View {
    let item: WidgetQuotaItem
    let needsDefault: Bool
    let compact: Bool
    let ringSize: CGFloat
    let labelSize: CGFloat
    let identitySize: CGFloat
    let windowSize: CGFloat

    var body: some View {
        VStack(spacing: 3) {
            ring

            VStack(spacing: 1) {
                Text(compact ? item.label : item.serviceTitle)
                    .font(.system(size: labelSize, weight: .semibold))
                    .foregroundStyle(.white.opacity(0.94))
                    .lineLimit(1)
                    .minimumScaleFactor(0.75)
                if compact {
                    Text(item.compactIdentity(needsDefault: needsDefault))
                        .font(.system(size: identitySize, weight: .medium))
                        .foregroundStyle(WidgetTheme.tertiary)
                        .lineLimit(1)
                        .minimumScaleFactor(0.7)
                } else if let account = item.accountLine(needsDefault: needsDefault) {
                    Text(account)
                        .font(.system(size: identitySize, weight: .medium))
                        .foregroundStyle(WidgetTheme.tertiary)
                        .lineLimit(1)
                        .minimumScaleFactor(0.75)
                }
            }
            .frame(height: labelSize + identitySize + 4, alignment: .top)

            if item.isError && item.windows.isEmpty {
                Text("查询失败")
                    .font(.system(size: windowSize, weight: .medium))
                    .foregroundStyle(WidgetTheme.amber)
                    .lineLimit(1)
                    .frame(maxWidth: .infinity, alignment: .leading)
            } else if item.isBalance {
                Text("可用余额")
                    .font(.system(size: windowSize, weight: .medium))
                    .foregroundStyle(WidgetTheme.tertiary)
                    .frame(maxWidth: .infinity, alignment: .leading)
            } else {
                VStack(spacing: 2) {
                    ForEach(QuotaSlot.allCases) { slot in
                        let w = item.window(for: slot)
                        HStack(spacing: 2) {
                            Text(slot.title)
                                .font(.system(size: windowSize, weight: .medium))
                                .foregroundStyle(WidgetTheme.tertiary)
                                .lineLimit(1)
                            Spacer(minLength: 1)
                            Text(w?.pct.map { "\($0)%" } ?? "—")
                                .font(.system(size: windowSize + 0.5,
                                              weight: .semibold, design: .rounded))
                                .monospacedDigit()
                                .foregroundStyle((w?.pct).map(widgetQuotaColor) ?? WidgetTheme.tertiary)
                                .lineLimit(1)
                        }
                        .frame(height: windowSize + 2)
                    }
                }
            }
        }
        .padding(.horizontal, 5)
        .frame(maxWidth: .infinity)
    }

    @ViewBuilder
    private var ring: some View {
        if item.isBalance {
            QuotaRingView(pct: nil, amount: item.balanceCompact,
                          size: ringSize, color: WidgetTheme.mint)
        } else if let pct = item.primaryWindow?.pct {
            QuotaRingView(pct: pct, size: ringSize, color: item.ringColor)
        } else if item.isError {
            QuotaRingView(pct: nil, amount: "!", size: ringSize,
                          color: WidgetTheme.amber)
        } else {
            QuotaRingView(pct: nil, amount: "—", size: ringSize,
                          color: WidgetTheme.tertiary)
        }
    }
}

private struct MediumHomeView: View {
    let snap: WidgetSnapshot
    let currency: Currency
    let stale: Bool

    private var hasData: Bool { snap.updatedAt > 0 }
    private var items: [WidgetQuotaItem] { snap.allQuotaItems }

    private struct Layout {
        let shown: [WidgetQuotaItem]
        let more: Int
        let compact: Bool
        let ring: CGFloat
        let label: CGFloat
        let identity: CGFloat
        let window: CGFloat
    }

    private var layout: Layout {
        // 当前主流是 5 个账号；6 个时收窄字号，超过 6 个折叠为 +N。
        if items.count > 6 {
            return Layout(shown: Array(items.prefix(5)), more: items.count - 5,
                          compact: true,
                          ring: 38, label: 10.5, identity: 8.5, window: 9)
        }
        if items.count == 6 {
            return Layout(shown: items, more: 0,
                          compact: true,
                          ring: 34, label: 10, identity: 8, window: 8.5)
        }
        return Layout(shown: items, more: 0,
                      compact: false,
                      ring: 40, label: 11, identity: 8.5, window: 9)
    }

    private var duplicateLabels: Set<String> {
        let grouped = Dictionary(grouping: items, by: { $0.label })
        return Set(grouped.filter { $0.value.count > 1 }.keys)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            MediumCostBar(snap: snap, currency: currency, stale: stale)

            WidgetDivider()
                .padding(.top, 5)
                .padding(.bottom, 6)

            if !hasData {
                HomePlaceholder(hasData: false)
            } else if items.isEmpty {
                HomePlaceholder(hasData: true)
            } else {
                HStack(alignment: .top, spacing: 0) {
                    ForEach(layout.shown) { item in
                        MediumAccountColumn(
                            item: item,
                            needsDefault: duplicateLabels.contains(item.label),
                            compact: layout.compact,
                            ringSize: layout.ring,
                            labelSize: layout.label,
                            identitySize: layout.identity,
                            windowSize: layout.window)
                    }
                    if layout.more > 0 {
                        MoreQuotaCell(count: layout.more,
                                      ringSize: layout.ring,
                                      labelSize: layout.label)
                    }
                }
                Spacer(minLength: 0)
            }
        }
    }
}

// MARK: - 大尺寸：每个账号一张横卡，展开全部窗口

private struct LargeQuotaRow: View {
    let item: WidgetQuotaItem
    let needsDefault: Bool
    let ringSize: CGFloat
    let labelSize: CGFloat
    let rowHeight: CGFloat

    var body: some View {
        HStack(spacing: 10) {
            ring

            VStack(alignment: .leading, spacing: 2) {
                HStack(spacing: 5) {
                    Text(item.label)
                        .font(.system(size: labelSize, weight: .semibold))
                        .foregroundStyle(.white.opacity(0.94))
                        .lineLimit(1)
                        .minimumScaleFactor(0.8)

                    if item.isSynced {
                        Text("同步")
                            .font(.system(size: 7, weight: .semibold))
                            .padding(.horizontal, 4)
                            .padding(.vertical, 1)
                            .background(Capsule().fill(Color(red: 0.29, green: 0.78, blue: 1.0).opacity(0.16)))
                            .foregroundStyle(Color(red: 0.45, green: 0.84, blue: 1.0))
                    }
                    if item.isBalance {
                        Text("余额")
                            .font(.system(size: 7, weight: .semibold))
                            .padding(.horizontal, 4)
                            .padding(.vertical, 1)
                            .background(Capsule().fill(WidgetTheme.mint.opacity(0.16)))
                            .foregroundStyle(WidgetTheme.mint)
                    }

                    Spacer(minLength: 4)

                    Text(item.identityText(compact: false,
                                           needsDefault: needsDefault))
                        .font(.system(size: 9, weight: .medium))
                        .foregroundStyle(WidgetTheme.tertiary)
                        .lineLimit(1)
                        .minimumScaleFactor(0.8)
                }

                detailLine
            }

            Spacer(minLength: 0)
        }
        .padding(.horizontal, 9)
        .frame(height: rowHeight)
        .background(
            RoundedRectangle(cornerRadius: 11, style: .continuous)
                .fill(WidgetTheme.card))
        .overlay(
            RoundedRectangle(cornerRadius: 11, style: .continuous)
                .strokeBorder(Color.white.opacity(0.045), lineWidth: 0.5))
    }

    @ViewBuilder
    private var ring: some View {
        if item.isBalance {
            QuotaRingView(pct: nil, amount: item.balanceCompact,
                          size: ringSize, color: WidgetTheme.mint)
        } else if let pct = item.primaryWindow?.pct {
            QuotaRingView(pct: pct, size: ringSize, color: item.ringColor)
        } else if item.isError {
            QuotaRingView(pct: nil, amount: "!", size: ringSize,
                          color: WidgetTheme.amber)
        } else {
            QuotaRingView(pct: nil, amount: "—", size: ringSize,
                          color: WidgetTheme.tertiary)
        }
    }

    @ViewBuilder
    private var detailLine: some View {
        if item.isBalance {
            HStack(spacing: 7) {
                Text("可用 \(item.balanceFullText)")
                    .font(.system(size: 10.5, weight: .semibold, design: .rounded))
                    .monospacedDigit()
                    .foregroundStyle(WidgetTheme.mint)
                if let granted = item.balanceGranted, granted > 0 {
                    Text("赠送 \(item.balanceSymbol)\(String(format: "%.2f", granted))")
                        .font(.system(size: 8.5, weight: .medium))
                        .foregroundStyle(WidgetTheme.tertiary)
                        .lineLimit(1)
                }
                Spacer(minLength: 0)
            }
        } else if item.windows.isEmpty {
            Text(item.isError ? "查询失败 · 打开 App 查看原因" : "无窗口数据")
                .font(.system(size: 9, weight: .medium))
                .foregroundStyle(item.isError ? WidgetTheme.amber : WidgetTheme.tertiary)
                .lineLimit(1)
        } else {
            HStack(spacing: 12) {
                ForEach(QuotaSlot.allCases) { slot in
                    let w = item.window(for: slot)
                    HStack(spacing: 3) {
                        Text(slot.title)
                            .font(.system(size: 9, weight: .medium))
                            .foregroundStyle(WidgetTheme.tertiary)
                            .lineLimit(1)
                        Text(w?.pct.map { "\($0)%" } ?? "—")
                            .font(.system(size: 10, weight: .semibold, design: .rounded))
                            .monospacedDigit()
                            .foregroundStyle((w?.pct).map(widgetQuotaColor) ?? WidgetTheme.tertiary)
                    }
                }
                Spacer(minLength: 0)
            }
        }
    }
}

private struct LargeHomeView: View {
    let snap: WidgetSnapshot
    let currency: Currency
    let stale: Bool

    private var hasData: Bool { snap.updatedAt > 0 }
    private var items: [WidgetQuotaItem] { snap.allQuotaItems }

    private struct Layout {
        let shown: [WidgetQuotaItem]
        let ring: CGFloat
        let label: CGFloat
        let rowHeight: CGFloat
        let spacing: CGFloat
    }

    private var duplicateLabels: Set<String> {
        let grouped = Dictionary(grouping: items, by: { $0.label })
        return Set(grouped.filter { $0.value.count > 1 }.keys)
    }

    private var layout: Layout {
        let maxRows = items.count > 6 ? 6 : items.count
        let shown = Array(items.prefix(maxRows))
        if shown.count <= 5 {
            return Layout(shown: shown, ring: 42, label: 13,
                          rowHeight: 48, spacing: 5)
        }
        return Layout(shown: shown, ring: 36, label: 11,
                      rowHeight: 42, spacing: 4)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            WidgetCostHeader(snap: snap, currency: currency,
                             stale: stale, compact: false)

            WidgetDivider()
                .padding(.top, 8)
                .padding(.bottom, 10)

            if !hasData {
                HomePlaceholder(hasData: false)
            } else if items.isEmpty {
                HomePlaceholder(hasData: true)
            } else {
                VStack(spacing: layout.spacing) {
                    ForEach(layout.shown) { item in
                        LargeQuotaRow(item: item,
                                      needsDefault: duplicateLabels.contains(item.label),
                                      ringSize: layout.ring,
                                      labelSize: layout.label,
                                      rowHeight: layout.rowHeight)
                    }
                }

                Spacer(minLength: 0)

                HStack(spacing: 4) {
                    Text(items.count > layout.shown.count
                         ? "显示 \(layout.shown.count)/\(items.count) 个账号 · 其余请在 App 查看"
                         : "共 \(items.count) 个账号 · 5h / 周 / 月 全部窗口")
                        .font(.system(size: 9, weight: .medium))
                        .foregroundStyle(WidgetTheme.tertiary)
                        .lineLimit(1)
                    Spacer(minLength: 0)
                }
                .padding(.top, 6)
            }
        }
    }
}

// MARK: - 锁屏 Widget 视图

struct AgentHUDLockScreenView: View {
    let entry: AgentHUDEntry
    @Environment(\.widgetFamily) var family

    var body: some View {
        switch family {
        case .accessoryRectangular:
            RectangularView(snap: entry.snapshot, currency: entry.currency, stale: entry.stale)
        case .accessoryCircular:
            CircularView(snap: entry.snapshot)
        case .accessoryInline:
            InlineView(snap: entry.snapshot, currency: entry.currency)
        default:
            RectangularView(snap: entry.snapshot, currency: entry.currency, stale: entry.stale)
        }
    }
}

/// 锁屏长条：花费 + 额度（余额型显示余额与账号名，同步型带「桌面同步」标）
private struct RectangularView: View {
    let snap: WidgetSnapshot
    let currency: Currency
    let stale: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 1) {
            if stale {
                Text("已切换账号 · 打开 App 刷新")
                    .font(.caption2)
                    .foregroundColor(.orange)
            } else if snap.updatedAt <= 0 {
                Text("AgentHUD · 待同步")
                    .font(.caption2.weight(.semibold))
                    .foregroundColor(.orange)
            } else {
                Text("AgentHUD")
                    .font(.caption2.weight(.semibold))
            }
            Text(costLine)
                .font(.headline.monospacedDigit())
            if snap.isBalanceStyle {
                Text(snap.balanceText + balanceSuffix)
                    .font(.caption2.monospacedDigit())
            } else {
                if let w = snap.weeklyPct {
                    Text((snap.agentKind == "cursor" ? "Auto " : "周 ") + "\(w)%\(syncSuffix)")
                        .font(.caption2.monospacedDigit())
                }
                if let h = snap.hour5Pct {
                    Text("5h \(h)%")
                        .font(.caption2.monospacedDigit())
                }
            }
        }
    }

    private var balanceSuffix: String {
        if let label = snap.accountLabel, !label.isEmpty {
            return " · \(label)"
        }
        return ""
    }

    private var syncSuffix: String {
        snap.isSyncedStyle ? " 同步" : ""
    }

    private var costLine: String {
        let n = (currency == .cny) ? snap.todayCostCny : snap.todayCostUsd
        return Format.cost(n, currency)
    }
}

/// 锁屏圆形：weekly 百分比环（余额型显示余额数字）
private struct CircularView: View {
    let snap: WidgetSnapshot

    var body: some View {
        ZStack {
            if snap.isBalanceStyle {
                AccessoryWidgetBackground()
                VStack(spacing: 0) {
                    Text(snap.balanceSymbol)
                        .font(.system(size: 9))
                        .foregroundColor(.secondary)
                    Text(balanceNumber)
                        .font(.system(size: 13, weight: .bold, design: .rounded).monospacedDigit())
                }
            } else if let w = snap.weeklyPct {
                AccessoryWidgetBackground()
                ProgressView(value: Double(w), total: 100) {
                    Text(snap.agentKind == "cursor" ? "Auto" : "周")
                        .font(.system(size: 9))
                } currentValueLabel: {
                    Text("\(w)")
                        .font(.system(size: 14, weight: .bold, design: .rounded).monospacedDigit())
                }
                .progressViewStyle(.circular)
                .tint(widgetQuotaColor(w))
            } else {
                AccessoryWidgetBackground()
                Text("—")
                    .font(.headline)
            }
        }
    }

    /// 余额缩写（整数部分；过长的千位以上用 K 缩写）。
    private var balanceNumber: String {
        guard let amount = snap.balanceAmount else { return "—" }
        if abs(amount) >= 100_000 {
            return String(format: "%.1fK", amount / 1000)
        }
        if amount == amount.rounded() {
            return String(Int(amount))
        }
        return String(format: "%.0f", amount)
    }
}

/// 锁屏顶部一行：花费 + token（余额型追加余额，同步型追加周百分比）
private struct InlineView: View {
    let snap: WidgetSnapshot
    let currency: Currency

    var body: some View {
        Text(inlineText)
    }

    /// 行文本（余额型追加余额；窗口型追加主窗百分比；同步型的 cursor 显示 Auto）。
    private var inlineText: String {
        let n = (currency == .cny) ? snap.todayCostCny : snap.todayCostUsd
        var text = "AgentHUD \(Format.cost(n, currency)) · \(Format.tokens(snap.todayTokens))"
        let quota: String
        if snap.isBalanceStyle {
            quota = snap.balanceText
        } else if let w = snap.weeklyPct {
            quota = "\(snap.agentKind == "cursor" ? "Auto" : "周")\(w)%"
        } else {
            quota = ""
        }
        if !quota.isEmpty {
            text += " · \(quota)"
        }
        return text
    }
}
