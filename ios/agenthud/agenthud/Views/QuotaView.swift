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
                    VStack(alignment: .leading, spacing: 16) {
                        PageHeader(title: "额度",
                                   subtitle: subtitle,
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
                            if settings.isSyncConfigured {
                                compareEntry
                            }

                            if refresh.quotaRefreshing && refresh.quotaEntries.isEmpty {
                                LoadingPlaceholder()
                            }

                            // 直连账号：按 agent 分组（保持 AgentKind 声明顺序）
                            ForEach(directAgentOrder, id: \.rawValue) { agent in
                                let entries = refresh.quotaEntries.filter { $0.agent == agent }
                                if !entries.isEmpty {
                                    if entries.count > 1 {
                                        GroupHeader(title: agent.displayName,
                                                    icon: AgentVisual.forAgent(agent).icon,
                                                    image: AgentVisual.forAgent(agent).image,
                                                    tint: AgentVisual.forAgent(agent).tint,
                                                    detail: "\(entries.count) 个账号 · 直连")
                                    }
                                    ForEach(entries) { entry in
                                        AccountQuotaCard(entry: entry)
                                    }
                                }
                            }

                            // 只读同步型：桌面已上传的快照，按 agent 分组
                            ForEach(syncedGroups) { section in
                                if section.groups.count > 1 {
                                    GroupHeader(title: section.agent.displayName,
                                                icon: AgentVisual.forAgent(section.agent).icon,
                                                image: AgentVisual.forAgent(section.agent).image,
                                                tint: AgentVisual.forAgent(section.agent).tint,
                                                detail: "\(section.groups.count) 个账号 · 桌面同步")
                                }
                                ForEach(section.groups) { group in
                                    SyncedQuotaCard(group: group)
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
                    .padding(.bottom, 110)
                    .hudContentWidth()
                }
                .scrollIndicators(.hidden)
                .refreshable { await refresh.refreshQuotaOnly() }
            }
            .toolbar(.hidden, for: .navigationBar)
        }
    }

    // MARK: - 页头摘要

    private var subtitle: String {
        let count = refresh.quotaEntries.count + refresh.syncedQuotaGroups.count
        guard count > 0 else { return "暂无额度账号" }

        let allWindows = refresh.quotaEntries.flatMap { $0.windows }
            + refresh.syncedQuotaGroups.flatMap { $0.windows }
        let alerts = allWindows.filter { ($0.usedPercent ?? 0) >= 85 }.count

        if alerts > 0 {
            return "\(count) 个账号 · \(alerts) 个窗口需关注"
        }
        if let at = refresh.lastRefreshedAt {
            return "\(count) 个账号 · 更新于 \(Self.timeText(at))"
        }
        return "\(count) 个账号 · 状态正常"
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

    private static func timeText(_ date: Date) -> String {
        let f = DateFormatter()
        f.dateFormat = "HH:mm"
        return f.string(from: date)
    }

    // MARK: - 周额度对比入口

    private var compareEntry: some View {
        NavigationLink {
            CompareView()
        } label: {
            GlassCard(padding: 14, radius: 18, tint: Theme.violet) {
                HStack(spacing: 12) {
                    AgentTile(icon: "chart.bar.xaxis", tint: Theme.violet, size: 36)
                    VStack(alignment: .leading, spacing: 3) {
                        Text("周额度对比")
                            .font(.system(size: 14, weight: .semibold))
                            .foregroundStyle(.white.opacity(0.92))
                        Text("按重置周期查看峰值与折算消耗")
                            .font(.system(size: 10))
                            .foregroundStyle(Theme.textTertiary)
                    }
                    Spacer()
                    Image(systemName: "chevron.right")
                        .font(.system(size: 11, weight: .semibold))
                        .foregroundStyle(Theme.textTertiary)
                }
            }
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

// MARK: - 直连账号卡片

private struct AccountQuotaCard: View {
    let entry: QuotaEntry

    private var visual: AgentVisual { AgentVisual.forAgent(entry.agent) }

    private var isHealthy: Bool { entry.status == "ok" }

    var body: some View {
        GlassCard(tint: isHealthy ? nil : Theme.coral) {
            VStack(alignment: .leading, spacing: 14) {
                header

                if entry.windows.isEmpty && entry.balance == nil {
                    Text(entry.message ?? "暂无额度数据")
                        .font(.system(size: 11))
                        .foregroundStyle(Theme.textTertiary)
                } else {
                    QuotaWindowsView(windows: entry.windows)
                }

                if let balance = entry.balance {
                    BalanceSection(balance: balance, insetTop: !entry.windows.isEmpty)
                }

                if let message = entry.message, !message.isEmpty,
                   entry.status != "ok" {
                    messageRow(message)
                }

                footer
            }
        }
    }

    private var header: some View {
        HStack(spacing: 11) {
            AgentTile(visual: visual, size: 38)

            VStack(alignment: .leading, spacing: 3) {
                HStack(spacing: 6) {
                    Text(entry.label.isEmpty ? entry.agent.displayName : entry.label)
                        .font(.system(size: 15, weight: .semibold))
                        .foregroundStyle(.white.opacity(0.92))
                        .lineLimit(1)
                    if entry.status != "ok" {
                        StatusBadge(status: entry.status)
                    }
                }
                HStack(spacing: 5) {
                    Circle()
                        .fill(isHealthy ? Theme.mint : Theme.quota(100))
                        .frame(width: 5, height: 5)
                    Text(entry.agent.displayName)
                        .font(.system(size: 10))
                        .foregroundStyle(Theme.textTertiary)
                }
            }

            Spacer(minLength: 6)

            if let plan = entry.planName, !plan.isEmpty {
                PlanBadge(text: plan)
            }
        }
    }

    private var footer: some View {
        HStack(spacing: 5) {
            Image(systemName: "clock")
                .font(.system(size: 8, weight: .medium))
            Text("更新于 " + Self.timeText(entry.updatedAt))
                .font(.system(size: 9))
            Spacer()
            if isHealthy {
                Text("直连查询")
                    .font(.system(size: 9))
                    .foregroundStyle(Theme.textTertiary)
            }
        }
        .foregroundStyle(Theme.textTertiary)
    }

    private func messageRow(_ message: String) -> some View {
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

    private static func timeText(_ ms: Int) -> String {
        let f = DateFormatter()
        f.dateFormat = "MM-dd HH:mm"
        return f.string(from: Date(timeIntervalSince1970: TimeInterval(ms) / 1000))
    }
}

// MARK: - 只读同步型卡片（桌面同步）

private struct SyncedQuotaCard: View {
    let group: SyncedAgentQuotaGroup

    private var visual: AgentVisual { AgentVisual.forAgent(group.agent) }

    var body: some View {
        GlassCard(tint: group.isError ? Theme.coral : nil) {
            VStack(alignment: .leading, spacing: 14) {
                header

                if group.isError {
                    HStack(alignment: .top, spacing: 6) {
                        Image(systemName: "exclamationmark.circle.fill")
                            .font(.system(size: 10, weight: .semibold))
                            .foregroundStyle(Theme.coral)
                        Text(group.message ?? "查询失败")
                            .font(.system(size: 10))
                            .foregroundStyle(Theme.coral.opacity(0.9))
                            .lineLimit(3)
                        Spacer(minLength: 0)
                    }
                    .padding(9)
                    .background {
                        RoundedRectangle(cornerRadius: 10, style: .continuous)
                            .fill(Theme.coral.opacity(0.08))
                    }
                } else {
                    if !group.windows.isEmpty {
                        QuotaWindowsView(windows: group.windows)
                    }
                    if let balance = group.balance {
                        BalanceSection(balance: balance, insetTop: !group.windows.isEmpty)
                    }
                    if group.windows.isEmpty && group.balance == nil {
                        Text("暂无额度数据")
                            .font(.system(size: 11))
                            .foregroundStyle(Theme.textTertiary)
                    }
                }

                footer
            }
        }
    }

    private var header: some View {
        HStack(spacing: 11) {
            AgentTile(visual: visual, size: 38)

            VStack(alignment: .leading, spacing: 3) {
                Text(group.displayTitle)
                    .font(.system(size: 15, weight: .semibold))
                    .foregroundStyle(.white.opacity(0.92))
                    .lineLimit(1)
                HStack(spacing: 5) {
                    Image(systemName: "icloud.and.arrow.down")
                        .font(.system(size: 8, weight: .semibold))
                        .foregroundStyle(Theme.accent)
                    Text("桌面同步")
                        .font(.system(size: 10))
                        .foregroundStyle(Theme.textTertiary)
                }
            }

            Spacer(minLength: 6)

            if let plan = group.planBadge {
                PlanBadge(text: plan)
            }
        }
    }

    private var footer: some View {
        HStack(spacing: 5) {
            Image(systemName: "clock")
                .font(.system(size: 8, weight: .medium))
            Text("更新于 " + Self.timeText(group.updatedAtMs))
                .font(.system(size: 9))
            Spacer()
        }
        .foregroundStyle(Theme.textTertiary)
    }

    private static func timeText(_ ms: Int) -> String {
        let f = DateFormatter()
        f.dateFormat = "MM-dd HH:mm"
        return f.string(from: Date(timeIntervalSince1970: TimeInterval(ms) / 1000))
    }
}

// MARK: - 窗口内容（环 / 条自适应）

private struct QuotaWindowsView: View {
    let windows: [QuotaWindow]

    private var canUseRings: Bool {
        !windows.isEmpty
            && windows.count <= 3
            && windows.allSatisfy { $0.usedPercent != nil }
    }

    var body: some View {
        if canUseRings {
            HStack(alignment: .top, spacing: 4) {
                ForEach(windows) { w in
                    QuotaRing(pct: Int((w.usedPercent ?? 0).rounded()),
                              title: w.title,
                              resetMs: w.resetsAt,
                              detail: QuotaText.usage(w))
                }
            }
            .padding(.vertical, 2)
        } else {
            VStack(spacing: 13) {
                ForEach(windows) { w in
                    if let pct = w.usedPercent {
                        ProgressBar(pct: Int(pct.rounded()),
                                    label: w.title,
                                    resetMs: w.resetsAt)
                        if let text = QuotaText.usage(w) {
                            Text(text)
                                .font(.system(size: 10).monospacedDigit())
                                .foregroundStyle(Theme.textTertiary)
                        }
                    } else {
                        HStack {
                            Text(w.title)
                                .font(.system(size: 13, weight: .medium))
                                .foregroundStyle(.white.opacity(0.88))
                            Spacer()
                            Text(QuotaText.usage(w) ?? "—")
                                .font(.system(size: 11).monospacedDigit())
                                .foregroundStyle(Theme.textSecondary)
                        }
                    }
                }
            }
        }
    }
}

// MARK: - 余额行

private struct BalanceSection: View {
    let balance: QuotaBalance
    var insetTop: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            if insetTop { HUDDivider() }

            HStack(alignment: .firstTextBaseline) {
                Text("账户余额")
                    .font(.system(size: 11, weight: .medium))
                    .foregroundStyle(Theme.textTertiary)
                Spacer()
                Text("\(QuotaText.symbol(balance.currency))\(String(format: "%.2f", balance.amount))")
                    .font(.system(size: 26, weight: .bold, design: .rounded))
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
