//
//  QuotaView.swift
//  agenthud
//
//  多 Agent 多账号额度监控：直连账号按 agent 分组，每账号一张卡
//  （套餐徽章 + 各窗口进度条 + 余额行）；codex/claude/cursor 为只读
//  同步型，卡片带「桌面同步」标注与更新时间。
//  直连数据走 AgentQuotaClients（官方接口），同步数据走 server 快照。
//

import SwiftUI

struct QuotaView: View {
    @EnvironmentObject var settings: AppSettings
    @EnvironmentObject var refresh: RefreshService

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(spacing: 16) {
                    if !settings.isQuotaConfigured && refresh.syncedQuotaGroups.isEmpty {
                        NotConfiguredBanner(
                            message: "尚未配置额度账号，请在设置中添加；Codex / Claude / Cursor 额度由桌面端同步，需先连接同步服务。")
                            .padding(.horizontal)
                    } else {
                        if refresh.quotaRefreshing && refresh.quotaEntries.isEmpty {
                            ProgressView().padding(.top, 40)
                        }

                        // 直连账号：按 agent 分组（保持 AgentKind 声明顺序）
                        ForEach(directAgentOrder, id: \.rawValue) { agent in
                            let entries = refresh.quotaEntries.filter { $0.agent == agent }
                            if !entries.isEmpty {
                                agentHeader(agent, count: entries.count)
                                ForEach(entries) { entry in
                                    AccountQuotaCard(entry: entry)
                                        .padding(.horizontal)
                                }
                            }
                        }

                        // 只读同步型：桌面已上传的快照
                        ForEach(refresh.syncedQuotaGroups) { group in
                            SyncedQuotaCard(group: group)
                                .padding(.horizontal)
                        }
                    }
                }
                .padding(.vertical)
            }
            .navigationTitle("额度监控")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    Button {
                        Task { await refresh.refreshQuotaOnly() }
                    } label: { Image(systemName: "arrow.clockwise") }
                    .disabled(refresh.quotaRefreshing)
                }
            }
        }
    }

    /// 出现了账号的直连 agent 顺序（对齐 AgentKind.allCases 声明序）。
    private var directAgentOrder: [AgentKind] {
        let present = Set(refresh.quotaEntries.map { $0.agent })
        return AgentKind.allCases.filter { present.contains($0) }
    }

    private func agentHeader(_ agent: AgentKind, count: Int) -> some View {
        HStack {
            Text(agent.displayName)
                .font(.subheadline.weight(.semibold))
                .foregroundColor(.secondary)
            if count > 1 {
                Tag("\(count) 个账号")
            }
            Spacer()
        }
        .padding(.horizontal)
    }
}

// MARK: - 直连账号卡片

private struct AccountQuotaCard: View {
    let entry: QuotaEntry

    var body: some View {
        Card(title: nil) {
            VStack(alignment: .leading, spacing: 10) {
                // 头部：账号名 + 状态徽章 + 套餐徽章
                HStack(spacing: 8) {
                    Text(entry.label.isEmpty ? entry.agent.displayName : entry.label)
                        .font(.subheadline.weight(.semibold))
                    StatusBadge(status: entry.status)
                    Spacer()
                    if let plan = entry.planName, !plan.isEmpty {
                        Text(plan.uppercased())
                            .font(.caption.weight(.semibold).monospaced())
                            .padding(.horizontal, 8).padding(.vertical, 3)
                            .background(Capsule().fill(Color.yellow.opacity(0.2)))
                    }
                }

                // 各窗口进度条
                if entry.windows.isEmpty && entry.balance == nil {
                    Text(entry.status == "ok" ? "暂无额度数据" : (entry.message ?? "暂无额度数据"))
                        .font(.caption)
                        .foregroundColor(.secondary)
                }
                ForEach(entry.windows) { w in
                    windowRow(w)
                }

                // 余额行（充值型 agent）
                if let balance = entry.balance {
                    balanceRow(balance)
                }

                // 附加说明（查询失败原因 / 平台提示），失败类标红
                if let message = entry.message, !message.isEmpty,
                   entry.status != "ok" || entry.windows.isEmpty {
                    Text(message)
                        .font(.caption)
                        .foregroundColor(entry.status == "ok" ? .secondary : .orange)
                }
            }
        }
    }

    @ViewBuilder
    private func windowRow(_ w: QuotaWindow) -> some View {
        if let pct = w.usedPercent {
            ProgressBar(pct: Int(pct.rounded()),
                        label: w.title,
                        resetMs: w.resetsAt)
        } else {
            // 无百分比（如仅 used/total 的窗口）：文本行展示
            HStack {
                Text(w.title).font(.subheadline.weight(.medium))
                Spacer()
                Text(usageText(w))
                    .font(.caption.monospacedDigit())
                    .foregroundColor(.secondary)
            }
        }
        if let usage = usageFullText(w) {
            Text(usage)
                .font(.caption.monospacedDigit())
                .foregroundColor(.secondary)
        }
    }

    /// used / total [unit] 组合（有一项才展示）。
    private func usageFullText(_ w: QuotaWindow) -> String? {
        var parts: [String] = []
        if let used = w.used { parts.append(formatCount(used)) }
        if let total = w.total { parts.append(formatCount(total)) }
        guard !parts.isEmpty else { return nil }
        let joined = parts.joined(separator: " / ")
        return w.unit.map { "\(joined) \($0)" } ?? joined
    }

    private func usageText(_ w: QuotaWindow) -> String {
        usageFullText(w) ?? "—"
    }

    /// 数量格式化（保留至多 2 位小数，去尾零）。
    private func formatCount(_ v: Double) -> String {
        let rounded = (v * 100).rounded() / 100
        if rounded == rounded.rounded() {
            return String(Int(rounded))
        }
        return String(format: "%.2f", rounded)
    }

    private func balanceRow(_ b: QuotaBalance) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Divider()
            HStack {
                Text("余额")
                    .font(.subheadline.weight(.medium))
                Spacer()
                Text("\(b.currency)\(String(format: "%.2f", b.amount))")
                    .font(.subheadline.monospacedDigit().weight(.semibold))
            }
            HStack(spacing: 12) {
                if let granted = b.granted {
                    Tag("赠送 \(String(format: "%.2f", granted))")
                }
                if let toppedUp = b.toppedUp {
                    Tag("充值 \(String(format: "%.2f", toppedUp))")
                }
            }
            .font(.caption2)
            .foregroundColor(.secondary)
        }
    }
}

// MARK: - 只读同步型卡片（桌面同步）

private struct SyncedQuotaCard: View {
    let group: SyncedAgentQuotaGroup

    private var updatedAtText: String {
        let f = DateFormatter()
        f.dateFormat = "MM-dd HH:mm"
        return f.string(from: Date(timeIntervalSince1970: TimeInterval(group.updatedAtMs) / 1000))
    }

    var body: some View {
        Card(title: nil) {
            VStack(alignment: .leading, spacing: 10) {
                HStack(spacing: 8) {
                    Text(group.agent.displayName)
                        .font(.subheadline.weight(.semibold))
                    // 「桌面同步」来源徽章：与直连型视觉区分
                    Label("桌面同步", systemImage: "arrow.triangle.2.circlepath.icloud")
                        .font(.caption2)
                        .padding(.horizontal, 6).padding(.vertical, 2)
                        .background(Capsule().fill(Color.blue.opacity(0.15)))
                        .foregroundColor(.blue)
                    Spacer()
                    if let plan = group.planType, !plan.isEmpty {
                        Text(plan.uppercased())
                            .font(.caption.weight(.semibold).monospaced())
                            .padding(.horizontal, 8).padding(.vertical, 3)
                            .background(Capsule().fill(Color.yellow.opacity(0.2)))
                    }
                }
                ForEach(group.windows) { w in
                    if let pct = w.usedPercent {
                        ProgressBar(pct: Int(pct.rounded()),
                                    label: w.title,
                                    resetMs: w.resetsAt)
                    }
                }
                Text("更新于 \(updatedAtText)")
                    .font(.caption2)
                    .foregroundColor(.secondary)
            }
        }
    }
}

// MARK: - 状态徽章

struct StatusBadge: View {
    let status: String

    var body: some View {
        let (text, color): (String, Color) = {
            switch status {
            case "ok": return ("正常", .green)
            case "expired": return ("已过期", .red)
            case "pending": return ("待确认", .gray)
            default: return ("查询失败", .orange)
            }
        }()
        Text(text)
            .font(.caption2.weight(.medium))
            .padding(.horizontal, 6).padding(.vertical, 2)
            .background(Capsule().fill(color.opacity(0.15)))
            .foregroundColor(color)
    }
}
