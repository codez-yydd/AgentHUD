//
//  WidgetViews.swift
//  agenthudWidget
//
//  Widget 视图：主屏（小/中尺寸）+ 锁屏（rectangular/circular/inline）。
//  所有数据从 AgentHUDEntry.snapshot 读取，不发起网络请求。
//  批2：按快照的账号形态自适应——GLM 型显示三档百分比、余额型
//  （DeepSeek/Kimi）显示余额数字与账号名、只读同步型（Codex/Claude/
//  Cursor）显示百分比并带「桌面同步」小标；选中账号与快照不一致时
//  （entry.stale）提示打开 App 刷新。
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
            SmallHomeView(snap: entry.snapshot, currency: entry.currency, stale: entry.stale)
        case .systemMedium:
            MediumHomeView(snap: entry.snapshot, currency: entry.currency, stale: entry.stale)
        default:
            SmallHomeView(snap: entry.snapshot, currency: entry.currency, stale: entry.stale)
        }
    }
}

/// 小尺寸：今日花费 + 今日 Token + 额度区（按账号形态自适应）
private struct SmallHomeView: View {
    let snap: WidgetSnapshot
    let currency: Currency
    let stale: Bool

    private var hasData: Bool { snap.updatedAt > 0 }

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack {
                Text("AgentHUD")
                    .font(.caption2.weight(.bold))
                    .foregroundColor(.secondary)
                Spacer()
                if stale {
                    Text("待刷新")
                        .font(.system(size: 8))
                        .foregroundColor(.orange)
                } else if !hasData {
                    Text("待同步")
                        .font(.system(size: 8))
                        .foregroundColor(.orange)
                } else {
                    Text(timeAgo)
                        .font(.system(size: 8))
                        .foregroundColor(.secondary)
                }
            }
            Text(costText)
                .font(.system(size: 26, weight: .bold, design: .rounded))
                .foregroundColor(.green)
            Text("\(tokenText) · \(snap.todayRequests) 次")
                .font(.caption2.monospacedDigit())
                .foregroundColor(.secondary)

            Divider()
            if !hasData {
                Text("打开 App 同步数据")
                    .font(.system(size: 9, weight: .medium))
                    .foregroundColor(.orange)
            } else if snap.isBalanceStyle {
                smallBalanceArea
            } else if snap.hour5Pct == nil && snap.weeklyPct == nil
                        && snap.mcpPct == nil {
                Text("额度未同步")
                    .font(.system(size: 9))
                    .foregroundColor(.secondary)
            } else {
                smallQuotaArea
            }
            Spacer(minLength: 0)
        }
        .padding(12)
    }

    /// 余额型：账号名 + 余额数字 + 赠送/充值拆分。
    private var smallBalanceArea: some View {
        VStack(alignment: .leading, spacing: 3) {
            if let label = snap.accountLabel, !label.isEmpty {
                Text(label)
                    .font(.system(size: 9))
                    .foregroundColor(.secondary)
                    .lineLimit(1)
            }
            Text(snap.balanceText)
                .font(.system(size: 15, weight: .bold, design: .rounded).monospacedDigit())
                .foregroundColor(.green)
            if let granted = snap.balanceGranted, granted > 0 {
                Text("赠送 \(snap.balanceSymbol)\(String(format: "%.2f", granted))")
                    .font(.system(size: 8))
                    .foregroundColor(.secondary)
            }
        }
    }

    /// 窗口型（GLM 三档 / 同步型百分比）。
    private var smallQuotaArea: some View {
        VStack(alignment: .leading, spacing: 3) {
            if snap.isSyncedStyle {
                syncedBadge
            }
            if let w = snap.weeklyPct {
                miniBar(label: syncedLabel("周"), pct: w)
            }
            if let h = snap.hour5Pct {
                miniBar(label: "5h", pct: h)
            }
        }
    }

    /// 同步型的周窗实为 cursor 的 auto 用量（主 App 映射进 weekly 槽）。
    private func syncedLabel(_ base: String) -> String {
        (snap.agentKind == "cursor") ? "Auto" : base
    }

    private var costText: String {
        let n = (currency == .cny) ? snap.todayCostCny : snap.todayCostUsd
        return Format.cost(n, currency)
    }
    private var tokenText: String { Format.tokens(snap.todayTokens) }
    private var timeAgo: String {
        let diff = Date().timeIntervalSince1970 - snap.updatedAt
        if diff < 60 { return "刚刚" }
        if diff < 3600 { return "\(Int(diff/60))分前" }
        return "\(Int(diff/3600))时前"
    }

    private var syncedBadge: some View {
        Text("桌面同步")
            .font(.system(size: 8))
            .padding(.horizontal, 4).padding(.vertical, 1)
            .background(Capsule().fill(Color.blue.opacity(0.2)))
            .foregroundColor(.blue)
    }

    private func miniBar(label: String, pct: Int) -> some View {
        HStack(spacing: 6) {
            Text(label).font(.system(size: 9)).foregroundColor(.secondary)
            GeometryReader { geo in
                ZStack(alignment: .leading) {
                    RoundedRectangle(cornerRadius: 2)
                        .fill(Color.white.opacity(0.1))
                    RoundedRectangle(cornerRadius: 2)
                        .fill(widgetQuotaColor(pct))
                        .frame(width: geo.size.width * CGFloat(min(max(Double(pct)/100,0),1)))
                }
            }
            .frame(height: 4)
            Text("\(pct)%")
                .font(.system(size: 9, design: .rounded))
                .foregroundColor(widgetQuotaColor(pct))
        }
    }
}

/// 中尺寸：左侧大数字 + 右侧额度区（按账号形态自适应）
private struct MediumHomeView: View {
    let snap: WidgetSnapshot
    let currency: Currency
    let stale: Bool

    private var hasData: Bool { snap.updatedAt > 0 }

    var body: some View {
        HStack(spacing: 12) {
            VStack(alignment: .leading, spacing: 4) {
                if stale {
                    Text("已切换账号，打开 App 刷新")
                        .font(.system(size: 9))
                        .foregroundColor(.orange)
                } else if !hasData {
                    Text("打开 App 同步数据")
                        .font(.system(size: 9))
                        .foregroundColor(.orange)
                }
                Text("今日花费")
                    .font(.caption2).foregroundColor(.secondary)
                Text(costText)
                    .font(.system(size: 28, weight: .bold, design: .rounded))
                    .foregroundColor(.green)
                Text("\(tokenText) Token")
                    .font(.caption.monospacedDigit())
                    .foregroundColor(.secondary)
                Text("\(snap.todayRequests) 次请求")
                    .font(.caption2).foregroundColor(.secondary)
                Spacer()
            }
            Divider()
            if !hasData {
                VStack(alignment: .leading, spacing: 4) {
                    Text("额度待同步")
                        .font(.caption)
                        .foregroundColor(.secondary)
                    Text("打开 AgentHUD")
                        .font(.system(size: 10))
                        .foregroundColor(.orange)
                    Text("刷新一次数据")
                        .font(.system(size: 10))
                        .foregroundColor(.orange)
                    Spacer()
                }
                .frame(maxWidth: .infinity, alignment: .leading)
            } else if snap.isBalanceStyle {
                mediumBalanceArea
            } else if snap.hour5Pct == nil && snap.weeklyPct == nil
                        && snap.mcpPct == nil {
                VStack(alignment: .leading, spacing: 4) {
                    Text("额度未同步")
                        .font(.caption)
                        .foregroundColor(.secondary)
                    Text("桌面端刷新后自动更新")
                        .font(.system(size: 9))
                        .foregroundColor(.secondary)
                    Spacer()
                }
                .frame(maxWidth: .infinity, alignment: .leading)
            } else {
                mediumQuotaArea
            }
        }
        .padding(12)
    }

    /// 余额型右栏：账号名 + 余额大字 + 拆分。
    private var mediumBalanceArea: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(snap.accountLabel ?? "余额")
                .font(.caption)
                .foregroundColor(.secondary)
                .lineLimit(1)
            Text(snap.balanceText)
                .font(.system(size: 22, weight: .bold, design: .rounded).monospacedDigit())
                .foregroundColor(.green)
            if let granted = snap.balanceGranted, granted > 0 {
                Text("赠送 \(snap.balanceSymbol)\(String(format: "%.2f", granted))")
                    .font(.system(size: 9))
                    .foregroundColor(.secondary)
            }
            if let toppedUp = snap.balanceToppedUp, toppedUp > 0 {
                Text("充值 \(snap.balanceSymbol)\(String(format: "%.2f", toppedUp))")
                    .font(.system(size: 9))
                    .foregroundColor(.secondary)
            }
            Spacer()
        }
    }

    /// 窗口型右栏：三档百分比 + 周重置倒计时（同步型带小标与更新时间）。
    private var mediumQuotaArea: some View {
        VStack(alignment: .leading, spacing: 6) {
            if snap.isSyncedStyle {
                HStack(spacing: 4) {
                    Text("桌面同步")
                        .font(.system(size: 8))
                        .padding(.horizontal, 4).padding(.vertical, 1)
                        .background(Capsule().fill(Color.blue.opacity(0.2)))
                        .foregroundColor(.blue)
                    if let label = snap.accountLabel, !label.isEmpty {
                        Text(label)
                            .font(.system(size: 8))
                            .foregroundColor(.secondary)
                    }
                }
            }
            if let h = snap.hour5Pct {
                quotaRow(label: "5h 窗口", pct: h)
            }
            if let w = snap.weeklyPct {
                quotaRow(label: (snap.agentKind == "cursor") ? "Auto" : "每周", pct: w)
            }
            if let m = snap.mcpPct {
                quotaRow(label: "MCP", pct: m)
            }
            if let r = snap.weeklyResetMs {
                Text("重置：" + Format.countdown(to: r))
                    .font(.system(size: 9))
                    .foregroundColor(.secondary)
            }
            Spacer()
        }
    }

    private var costText: String {
        let n = (currency == .cny) ? snap.todayCostCny : snap.todayCostUsd
        return Format.cost(n, currency)
    }
    private var tokenText: String { Format.tokens(snap.todayTokens) }

    private func quotaRow(label: String, pct: Int) -> some View {
        HStack(spacing: 6) {
            Text(label)
                .font(.system(size: 10))
                .foregroundColor(.secondary)
                .frame(width: 38, alignment: .leading)
            GeometryReader { geo in
                ZStack(alignment: .leading) {
                    RoundedRectangle(cornerRadius: 3)
                        .fill(Color.white.opacity(0.1))
                    RoundedRectangle(cornerRadius: 3)
                        .fill(widgetQuotaColor(pct))
                        .frame(width: geo.size.width * CGFloat(min(max(Double(pct)/100,0),1)))
                }
            }
            .frame(height: 6)
            Text("\(pct)%")
                .font(.system(size: 10, design: .rounded).monospacedDigit())
                .foregroundColor(widgetQuotaColor(pct))
                .frame(width: 30, alignment: .trailing)
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
