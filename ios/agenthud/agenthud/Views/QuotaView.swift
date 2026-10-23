//
//  QuotaView.swift
//  agenthud
//
//  多 Agent 多账号额度监控：直连账号按 agent 分组，每账号一张卡
//  （图标 + 套餐徽章 + 窗口进度环 / 余额 + 更新时间）；codex/claude/cursor/kimi
//  及全部 provider 为只读同步型（同一 provider 多凭证按 label 拆卡），
//  卡片带「桌面同步」标注；余额型同步卡展示余额，错误态展示失败原因。
//  直连数据走 AgentQuotaClients（官方接口），同步数据走 server 快照。
//

import SwiftUI

struct QuotaView: View {
    @EnvironmentObject var settings: AppSettings
    @EnvironmentObject var refresh: RefreshService
    @EnvironmentObject var tabRouter: TabRouter

    var body: some View {
        NavigationStack {
            ZStack {
                AppBackground()

                ScrollView {
                    VStack(alignment: .leading, spacing: 14) {
                        PageHeader(title: "额度",
                                   isLoading: refresh.quotaRefreshing,
                                   onRefresh: { Task { await refresh.refreshQuotaOnly() } })

                        if !settings.isQuotaConfigured && refresh.syncedQuotaGroups.isEmpty {
                            NotConfiguredBanner(
                                message: settings.isSyncConfigured
                                    ? "还没有收到额度快照。请确认桌面端已登录账号并保持运行，额度会定期同步过来。"
                                    : "尚未连接同步服务。用量与额度都由桌面端同步，请先在设置中注册设备。",
                                actionTitle: "去设置",
                                action: { tabRouter.selected = .settings })
                        } else {
                            headline

                            if refresh.quotaRefreshing && refresh.quotaEntries.isEmpty {
                                LoadingPlaceholder()
                            }

                            // 直连账号：按 agent 分组。单账号 → 独立卡片；
                            // 多账号 → 一张卡里排成紧凑行，避免同类账号
                            // 各占一张大卡把页面拉成好几屏。
                            ForEach(directAgentOrder, id: \.rawValue) { agent in
                                let entries = refresh.quotaEntries.filter { $0.agent == agent }
                                if entries.count == 1 {
                                    AccountQuotaCard(entry: entries[0])
                                } else if !entries.isEmpty {
                                    SectionLabel(title: agent.displayName,
                                                 detail: "\(entries.count) 个账号 · 直连")
                                    GlassCard(padding: 15) {
                                        VStack(spacing: 0) {
                                            ForEach(Array(entries.enumerated()), id: \.element.id) { idx, entry in
                                                QuotaCompactRow(
                                                    title: QuotaView.displayTitle(label: entry.label, agent: entry.agent),
                                                    plan: entry.planName,
                                                    windows: entry.windows,
                                                    balance: entry.balance,
                                                    message: entry.message,
                                                    isError: entry.status != "ok",
                                                    updatedText: "直连 · 更新于 " + Self.msText(entry.updatedAt))
                                                if idx < entries.count - 1 {
                                                    HUDDivider().padding(.vertical, 13)
                                                }
                                            }
                                        }
                                    }
                                }
                            }

                            // 只读同步型：桌面已上传的快照，按 agent 分组
                            ForEach(syncedGroups) { section in
                                if section.groups.count == 1, let group = section.groups.first {
                                    SyncedQuotaCard(group: group)
                                } else {
                                    SectionLabel(title: section.agent.displayName,
                                                 detail: "\(section.groups.count) 个账号 · 桌面同步")
                                    GlassCard(padding: 15) {
                                        VStack(spacing: 0) {
                                            ForEach(Array(section.groups.enumerated()), id: \.element.id) { idx, group in
                                                QuotaCompactRow(
                                                    title: QuotaView.displayTitle(label: group.label, agent: group.agent),
                                                    plan: group.planBadge,
                                                    windows: group.windows,
                                                    balance: group.balance,
                                                    message: group.message,
                                                    isError: group.isError,
                                                    updatedText: "桌面同步 · 更新于 " + Self.msText(group.updatedAtMs))
                                                if idx < section.groups.count - 1 {
                                                    HUDDivider().padding(.vertical, 13)
                                                }
                                            }
                                        }
                                    }
                                }
                            }

                            if refresh.quotaEntries.isEmpty
                                && refresh.syncedQuotaGroups.isEmpty
                                && !refresh.quotaRefreshing {
                                GlassCard {
                                    EmptyState(icon: "gauge.with.dots.needle.67percent",
                                               title: "暂无额度数据",
                                               message: "下拉刷新，或到设置中检查账号与服务端连接")
                                }
                            }
                        }
                    }
                    .padding(.horizontal, Theme.pagePadding)
                    .padding(.top, 4)
                    .padding(.bottom, 96)
                    .hudContentWidth()
                }
                .scrollIndicators(.hidden)
                .refreshable { await refresh.refreshQuotaOnly() }
            }
            .toolbar(.hidden, for: .navigationBar)
        }
    }

    // MARK: - 顶部摘要（账号数 / 需关注窗口 / 周对比入口）

    private var headline: some View {
        HStack(spacing: 10) {
            HStack(spacing: 6) {
                Text("\(accountCount)")
                    .font(.system(size: 15, weight: .bold, design: .rounded))
                    .monospacedDigit()
                    .foregroundStyle(.white.opacity(0.95))
                Text("个账号")
                    .font(.system(size: 11))
                    .foregroundStyle(Theme.textTertiary)
            }
            if alertCount > 0 {
                HStack(spacing: 4) {
                    Image(systemName: "exclamationmark.triangle.fill")
                        .font(.system(size: 9, weight: .semibold))
                    Text("\(alertCount) 个窗口需关注")
                        .font(.system(size: 11, weight: .medium))
                }
                .foregroundStyle(Theme.amber)
            } else {
                Text("额度正常")
                    .font(.system(size: 11))
                    .foregroundStyle(Theme.textTertiary)
            }
            Spacer(minLength: 6)
            if settings.isSyncConfigured {
                compareEntry
            }
        }
    }

    private var accountCount: Int {
        refresh.quotaEntries.count + refresh.syncedQuotaGroups.count
    }

    private var alertCount: Int {
        let all = refresh.quotaEntries.flatMap { $0.windows }
            + refresh.syncedQuotaGroups.flatMap { $0.windows }
        return all.filter { ($0.usedPercent ?? 0) >= 85 }.count
    }

    /// 出现了账号的直连 agent 顺序（对齐 AgentKind.allCases 声明序）。
    private var directAgentOrder: [AgentKind] {
        let present = Set(refresh.quotaEntries.map { $0.agent })
        return AgentKind.allCases.filter { present.contains($0) }
    }

    /// 同步分组按 agent 归并，保持服务端给出的 source 顺序。
    private var syncedGroups: [SyncedGroupSection] {
        var order: [AgentKind] = []
        var map: [AgentKind: [SyncedAgentQuotaGroup]] = [:]
        for group in refresh.syncedQuotaGroups {
            if map[group.agent] == nil { order.append(group.agent) }
            map[group.agent, default: []].append(group)
        }
        return order.map { SyncedGroupSection(agent: $0, groups: map[$0] ?? []) }
    }

    /// 账号标签收敛：17 位纯数字账号只留后 4 位，避免长数字把标题挤成两行。
    static func shortAccountLabel(_ raw: String) -> String {
        let trimmed = raw.trimmingCharacters(in: .whitespaces)
        let tail = String(trimmed.reversed().prefix { $0.isNumber }.reversed())
        guard tail.count >= 12 else { return trimmed }
        let head = String(trimmed.dropLast(tail.count))
        return head + "…" + tail.suffix(4)
    }

    /// 可读的账号名：GLM 这类由凭证/用户 id 派生的「账号 16361781344907127」
    /// 在自己的平台上根本看不到，只留服务名 + 套餐徽章，不再展示这串数字。
    static func readableAccountName(_ raw: String) -> String? {
        let trimmed = raw.trimmingCharacters(in: .whitespaces)
        guard !trimmed.isEmpty else { return nil }
        if trimmed.hasPrefix("账号 ") || trimmed.hasPrefix("凭证 ") { return nil }
        if trimmed.allSatisfy({ $0.isNumber }) { return nil }
        return shortAccountLabel(trimmed)
    }

    /// 账号卡标题：能读懂的账号名才拼上，否则只显示服务名。
    static func displayTitle(label: String, agent: AgentKind) -> String {
        guard let name = readableAccountName(label) else { return agent.displayName }
        return "\(agent.displayName) · \(name)"
    }

    /// 毫秒时间戳 → "MM-dd HH:mm"（额度卡的更新时间口径）。
    private static func msText(_ ms: Int) -> String {
        let f = DateFormatter()
        f.dateFormat = "MM-dd HH:mm"
        return f.string(from: Date(timeIntervalSince1970: TimeInterval(ms) / 1000))
    }

    // MARK: - 周额度对比入口（收敛成摘要行右侧的小胶囊）

    private var compareEntry: some View {
        NavigationLink {
            CompareView()
        } label: {
            HStack(spacing: 6) {
                Image(systemName: "chart.bar.xaxis")
                    .font(.system(size: 10, weight: .semibold))
                Text("周对比")
                    .font(.system(size: 11.5, weight: .medium))
                Image(systemName: "chevron.right")
                    .font(.system(size: 8, weight: .bold))
                    .opacity(0.6)
            }
            .foregroundStyle(Theme.violet)
            .padding(.horizontal, 11)
            .padding(.vertical, 6)
            .background(Capsule().fill(Theme.violet.opacity(0.13)))
            .overlay(Capsule().strokeBorder(Theme.violet.opacity(0.28), lineWidth: 1))
            .contentShape(Capsule())
        }
        .buttonStyle(.plain)
    }
}

/// 同步分组按 agent 归并后的展示段。
private struct SyncedGroupSection: Identifiable {
    let agent: AgentKind
    let groups: [SyncedAgentQuotaGroup]
    var id: String { agent.rawValue }
}

// MARK: - 账号卡（单账号 / 分组行共用一套紧凑排版）

/// 直连账号卡：头部 + 窗口行 +（可选）余额 / 错误原因。
/// 更新时间和来源并进副标题，省掉原来「分隔线 + 页脚」两层结构。
private struct AccountQuotaCard: View {
    let entry: QuotaEntry

    private var isHealthy: Bool { entry.status == "ok" }

    var body: some View {
        GlassCard(padding: 15, tint: isHealthy ? nil : Theme.coral) {
            VStack(alignment: .leading, spacing: 12) {
                QuotaAccountHeader(
                    visual: AgentVisual.forAgent(entry.agent),
                    title: title,
                    subtitle: subtitle,
                    subtitleIcon: "bolt.horizontal.circle",
                    status: entry.status,
                    plan: entry.planName)

                QuotaAccountBody(windows: entry.windows,
                                 balance: entry.balance,
                                 message: entry.message,
                                 isError: !isHealthy,
                                 emptyText: entry.message ?? "暂无额度数据")
            }
        }
    }

    private var title: String {
        QuotaView.displayTitle(label: entry.label, agent: entry.agent)
    }

    private var subtitle: String {
        let source = title == entry.agent.displayName
            ? "直连" : "\(entry.agent.displayName) · 直连"
        return source + " · 更新于 " + Self.timeText(entry.updatedAt)
    }

    private static func timeText(_ ms: Int) -> String {
        let f = DateFormatter()
        f.dateFormat = "MM-dd HH:mm"
        return f.string(from: Date(timeIntervalSince1970: TimeInterval(ms) / 1000))
    }
}

/// 只读同步（桌面同步）账号卡。
private struct SyncedQuotaCard: View {
    let group: SyncedAgentQuotaGroup

    var body: some View {
        GlassCard(padding: 15, tint: group.isError ? Theme.coral : nil) {
            VStack(alignment: .leading, spacing: 12) {
                QuotaAccountHeader(
                    visual: AgentVisual.forAgent(group.agent),
                    title: QuotaView.displayTitle(label: group.label, agent: group.agent),
                    subtitle: "桌面同步 · 更新于 " + Self.timeText(group.updatedAtMs),
                    subtitleIcon: "icloud.and.arrow.down",
                    status: group.isError ? "error" : nil,
                    plan: group.planBadge)

                QuotaAccountBody(windows: group.windows,
                                 balance: group.balance,
                                 message: group.message,
                                 isError: group.isError,
                                 emptyText: "暂无额度数据")
            }
        }
    }

    private static func timeText(_ ms: Int) -> String {
        let f = DateFormatter()
        f.dateFormat = "MM-dd HH:mm"
        return f.string(from: Date(timeIntervalSince1970: TimeInterval(ms) / 1000))
    }
}

/// 同 agent 多账号时，卡内的一行账号（无图标，靠分组标题定位）。
private struct QuotaCompactRow: View {
    let title: String
    var plan: String? = nil
    let windows: [QuotaWindow]
    var balance: QuotaBalance? = nil
    var message: String? = nil
    var isError: Bool = false
    var updatedText: String? = nil

    var body: some View {
        VStack(alignment: .leading, spacing: 11) {
            HStack(spacing: 6) {
                Text(title)
                    .font(.system(size: 13.5, weight: .semibold))
                    .foregroundStyle(.white.opacity(0.92))
                    .lineLimit(1)
                if isError { StatusBadge(status: "error") }
                Spacer(minLength: 6)
                if let plan, !plan.isEmpty { PlanBadge(text: plan) }
            }

            QuotaAccountBody(windows: windows,
                             balance: balance,
                             message: message,
                             isError: isError,
                             emptyText: "暂无额度数据")

            if let updatedText {
                Text(updatedText)
                    .font(.system(size: 9.5))
                    .foregroundStyle(Theme.textTertiary.opacity(0.9))
            }
        }
    }
}

/// 账号卡头部：图标 + 账号名 + 副标题 + 右侧套餐徽章。
private struct QuotaAccountHeader: View {
    let visual: AgentVisual
    let title: String
    var subtitle: String? = nil
    var subtitleIcon: String? = nil
    var status: String? = nil
    var plan: String? = nil

    var body: some View {
        HStack(spacing: 10) {
            AgentTile(visual: visual, size: 32)

            VStack(alignment: .leading, spacing: 3) {
                HStack(spacing: 6) {
                    Text(title)
                        .font(.system(size: 14, weight: .semibold))
                        .foregroundStyle(.white.opacity(0.93))
                        .lineLimit(1)
                    if let status, status != "ok" { StatusBadge(status: status) }
                }
                if let subtitle {
                    HStack(spacing: 4) {
                        if let subtitleIcon {
                            Image(systemName: subtitleIcon)
                                .font(.system(size: 8, weight: .semibold))
                                .foregroundStyle(Theme.accent.opacity(0.85))
                        }
                        Text(subtitle)
                            .font(.system(size: 10))
                            .foregroundStyle(Theme.textTertiary)
                            .lineLimit(1)
                    }
                }
            }

            Spacer(minLength: 6)

            if let plan, !plan.isEmpty { PlanBadge(text: plan) }
        }
    }
}

/// 账号正文：窗口紧凑行 / 余额 / 错误原因。
private struct QuotaAccountBody: View {
    let windows: [QuotaWindow]
    var balance: QuotaBalance? = nil
    var message: String? = nil
    var isError: Bool = false
    var emptyText: String = "暂无额度数据"

    var body: some View {
        VStack(alignment: .leading, spacing: 11) {
            if isError, let message {
                errorRow(message)
            } else {
                if !windows.isEmpty {
                    VStack(spacing: 10) {
                        ForEach(windows) { w in
                            QuotaWindowRow(window: w)
                        }
                    }
                }
                if let balance {
                    BalanceSection(balance: balance, insetTop: !windows.isEmpty)
                }
                if windows.isEmpty && balance == nil {
                    Text(message ?? emptyText)
                        .font(.system(size: 11))
                        .foregroundStyle(Theme.textTertiary)
                }
                if let message, !message.isEmpty, !windows.isEmpty {
                    Text(message)
                        .font(.system(size: 10))
                        .foregroundStyle(Theme.amber.opacity(0.9))
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
        }
    }

    private func errorRow(_ message: String) -> some View {
        HStack(alignment: .top, spacing: 6) {
            Image(systemName: "exclamationmark.circle.fill")
                .font(.system(size: 10, weight: .semibold))
                .foregroundStyle(Theme.coral)
            Text(message)
                .font(.system(size: 10))
                .foregroundStyle(Theme.coral.opacity(0.9))
                .fixedSize(horizontal: false, vertical: true)
            Spacer(minLength: 0)
        }
        .padding(9)
        .background {
            RoundedRectangle(cornerRadius: 10, style: .continuous)
                .fill(Theme.coral.opacity(0.08))
        }
    }
}

// MARK: - 余额行

private struct BalanceSection: View {
    let balance: QuotaBalance
    var insetTop: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            if insetTop { HUDDivider() }

            HStack(alignment: .firstTextBaseline) {
                Text("账户余额")
                    .font(.system(size: 11, weight: .medium))
                    .foregroundStyle(Theme.textTertiary)
                Spacer()
                Text("\(QuotaText.symbol(balance.currency))\(String(format: "%.2f", balance.amount))")
                    .font(.system(size: 20, weight: .bold, design: .rounded))
                    .monospacedDigit()
                    .foregroundStyle(Theme.mint)
                    .lineLimit(1)
                    .minimumScaleFactor(0.7)
            }

            if balance.granted != nil || balance.toppedUp != nil {
                HStack(spacing: 6) {
                    if let granted = balance.granted {
                        Tag("赠送 \(QuotaText.symbol(balance.currency))\(String(format: "%.2f", granted))")
                    }
                    if let toppedUp = balance.toppedUp {
                        Tag("充值 \(QuotaText.symbol(balance.currency))\(String(format: "%.2f", toppedUp))")
                    }
                }
            }
        }
    }
}

// MARK: - 额度文案工具

enum QuotaText {
    /// 币种 → 展示符号。
    static func symbol(_ currency: String) -> String {
        switch currency {
        case "CNY", "¥", "RMB": return "¥"
        case "USD", "$": return "$"
        default: return currency
        }
    }

    /// used / total + 单位（有一项才展示）。
    static func usage(_ w: QuotaWindow) -> String? {
        var parts: [String] = []
        if let used = w.used { parts.append(formatCount(used)) }
        if let total = w.total { parts.append(formatCount(total)) }
        guard !parts.isEmpty else { return nil }
        let joined = parts.joined(separator: " / ")
        return w.unit.map { "\(joined) \($0)" } ?? joined
    }

    /// 数量格式化（保留至多 2 位小数，去尾零）。
    static func formatCount(_ v: Double) -> String {
        let rounded = (v * 100).rounded() / 100
        if rounded == rounded.rounded() {
            return String(Int(rounded))
        }
        return String(format: "%.2f", rounded)
    }
}

// MARK: - 状态 / 套餐徽章

struct StatusBadge: View {
    let status: String

    private var style: (String, Color) {
        switch status {
        case "ok": return ("正常", Theme.mint)
        case "expired": return ("已过期", Theme.coral)
        case "pending": return ("待确认", Theme.textSecondary)
        default: return ("查询失败", Theme.amber)
        }
    }

    var body: some View {
        HStack(spacing: 4) {
            Circle()
                .fill(style.1)
                .frame(width: 4, height: 4)
            Text(style.0)
                .font(.system(size: 9, weight: .medium))
        }
        .padding(.horizontal, 6)
        .padding(.vertical, 2)
        .background(Capsule().fill(style.1.opacity(0.12)))
        .foregroundStyle(style.1)
    }
}

struct PlanBadge: View {
    let text: String

    var body: some View {
        Text(text.uppercased())
            .font(.system(size: 9, weight: .bold, design: .monospaced))
            .tracking(0.4)
            .padding(.horizontal, 7)
            .padding(.vertical, 3)
            .background(Capsule().fill(Theme.amber.opacity(0.13)))
            .overlay(Capsule().strokeBorder(Theme.amber.opacity(0.22), lineWidth: 1))
            .foregroundStyle(Theme.amber)
            .lineLimit(1)
    }
}
