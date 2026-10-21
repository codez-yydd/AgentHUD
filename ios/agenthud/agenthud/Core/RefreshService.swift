//
//  RefreshService.swift
//  agenthud
//
//  数据加载中枢：拉用量 + 多 Agent 额度，合并计费，写 Widget 缓存。
//  UI 订阅它的 @Published，每次刷新都自动更新。
//
//  额度体系（多 agent 多账号）：
//  - 直连账号（zai/deepseek/moonshot/minimax/opencodego/alibaba）用
//    TaskGroup 并发查询，单账号失败收敛为 error/expired 条目，互不阻塞；
//    完成后回写 AccountEntry.lastCheck（expired 归入 error，对齐桌面）。
//  - 只读同步型（codex/claude/cursor）从 server 拉 24h 内快照合并；
//    服务端不支持（404/超时）时降级为隐藏该分组，不报错。
//  - Widget 快照按「选中账号」写入（选中同步分组 → 选中直连账号 → 主账号
//    回退，见 writeWidgetSnapshot）；选中值存 App Group，设置页 Picker 与
//    Widget 编辑界面的 AppIntent 双向联动（批2）。
//

import Foundation
import Combine

@MainActor
public final class RefreshService: ObservableObject {
    public static let shared = RefreshService()

    // 状态
    @Published public var loading = false
    @Published public var lastError: String?
    /// 额度查询进行中（额度页 spinner / 按钮禁用用）
    @Published public var quotaRefreshing = false

    // 数据
    @Published public var todayUsage: RemoteUsage?
    @Published public var todayCost: CostResult?
    /// 直连账号的额度条目（与 AccountStore.accounts 同序）
    @Published public var quotaEntries: [QuotaEntry] = []
    /// 只读同步型分组（codex/claude/cursor；服务端不可用时为空）
    @Published public var syncedQuotaGroups: [SyncedAgentQuotaGroup] = []
    @Published public var devices: [DeviceInfo] = []
    @Published public var selectedFilter: DeviceFilter = .all
    @Published public var rangePreset: RangePreset = .today

    private let settings = AppSettings.shared
    private var refreshTask: Task<Void, Never>?

    public init() {}

    // MARK: - 公共刷新

    /// 主刷新：用量 + 额度 + 设备列表，刷新完写 Widget 缓存。
    public func refreshAll() {
        refreshTask?.cancel()
        refreshTask = Task { await doRefreshAll() }
    }

    /// 仅刷新额度（额度页主动刷新 / 设置页「查询全部」用）。
    public func refreshQuotaOnly() async {
        quotaRefreshing = true
        defer { quotaRefreshing = false }
        await refreshDirectQuota()
        await refreshSyncedQuota()
        // 合并各账号 planName 进 Widget Intent 的选项摘要（副标题展示用）
        AccountStore.shared.refreshOptionPlanNames(directEntries: quotaEntries,
                                                   syncedGroups: syncedQuotaGroups)
        writeWidgetSnapshot()
    }

    // MARK: - 直连账号查询（TaskGroup 并发，互不阻塞）

    private func refreshDirectQuota() async {
        let store = AccountStore.shared
        let accounts = store.accounts.filter { $0.agent.isDirect }
        guard !accounts.isEmpty else {
            self.quotaEntries = []
            return
        }
        // 先在主线程取齐（account, secret）快照，再并发查询——网络阶段
        // 不触碰 @MainActor 状态；secret 只进请求头，不落任何日志。
        // Keychain 读不到 secret 的账号不静默跳过：产出可见的 error 条目
        // （额度页显示卡片 + lastCheck 回写），引导用户到设置页重新保存。
        var jobs: [(AccountEntry, String)] = []
        var keychainFailed: [AccountEntry] = []
        for account in accounts {
            if let secret = store.secret(for: account.id), !secret.isEmpty {
                jobs.append((account, secret))
            } else {
                keychainFailed.append(account)
            }
        }
        let failureEntries = keychainFailed.map { account in
            QuotaEntry(accountId: account.id.uuidString,
                       label: account.label,
                       agent: account.agent,
                       status: "error",
                       message: "钥匙串读取失败，请在设置中重新保存凭证")
        }
        guard !jobs.isEmpty else {
            self.quotaEntries = failureEntries
            for entry in failureEntries {
                if let uuid = UUID(uuidString: entry.accountId) {
                    store.recordCheck(id: uuid, status: "error",
                                      message: entry.message)
                }
            }
            return
        }
        let collected = await withTaskGroup(of: QuotaEntry.self) { group in
            for (account, secret) in jobs {
                group.addTask {
                    await AgentQuotaClients.shared.query(account, secret: secret)
                }
            }
            var out: [QuotaEntry] = []
            for await entry in group {
                out.append(entry)
            }
            return out
        }
        // 保持账号列表顺序（与设置页一致）；Keychain 失败条目一并入列
        let order = accounts.map { $0.id.uuidString }
        let sorted = (collected + failureEntries).sorted {
            (order.firstIndex(of: $0.accountId) ?? .max)
                < (order.firstIndex(of: $1.accountId) ?? .max)
        }
        self.quotaEntries = sorted
        // 回写最近校验状态：expired 对账号卡语义就是校验失败，归入 error
        for entry in sorted {
            guard let uuid = UUID(uuidString: entry.accountId) else { continue }
            if entry.status == "ok" {
                store.recordCheck(id: uuid, status: "ok", message: nil)
            } else {
                store.recordCheck(id: uuid, status: "error", message: entry.message)
            }
        }
    }

    // MARK: - 只读同步型（server 快照，失败降级隐藏）

    private func refreshSyncedQuota() async {
        guard settings.isSyncConfigured else {
            self.syncedQuotaGroups = []
            return
        }
        let nowMs = Int(Date().timeIntervalSince1970 * 1000)
        let fromMs = nowMs - 24 * 3_600_000
        let server = settings.sync.server_url
        let token = settings.sync.device_token

        // 1) Codex / Claude / Cursor / Kimi + 全部 provider 的额度快照
        //    （服务端不支持 / 不可达：该组降级隐藏，不报错）
        var groups: [SyncedAgentQuotaGroup] = []
        if let snaps = try? await APIClient.shared.agentQuotaSnapshots(
            serverURL: server, deviceToken: token,
            fromMs: fromMs, toMs: nowMs) {
            groups = AgentQuotaSync.buildGroups(
                from: AgentQuotaSync.mergeSnapshots(snaps))
        }

        // 2) GLM 多账号快照（/snapshots 通道，桌面端 account 维度已上传；
        //    每个 account 一张同步卡，label 用账号指纹对齐桌面端展示）
        if let zaiSnaps = try? await APIClient.shared.snapshots(
            serverURL: server, deviceToken: token,
            fromMs: fromMs, toMs: nowMs) {
            groups.append(contentsOf: Self.buildGlmSyncGroups(from: zaiSnaps))
        }

        self.syncedQuotaGroups = groups
    }

    /// GLM（Z.ai）额度快照 → 同步分组：按 account 指纹分组、同秒去重取高、
    /// 每账号取最新一条（窗口映射对齐桌面 quota_history 的三档口径）。
    /// account 为 nil（旧客户端数据）归入无 label 的单卡。
    static func buildGlmSyncGroups(from snaps: [RemoteSnapshot]) -> [SyncedAgentQuotaGroup] {
        struct Key: Hashable { let account: String; let second: Int }

        var byKey: [Key: (snap: RemoteSnapshot, windows: [QuotaWindow])] = [:]
        for snap in snaps {
            let account = snap.account?.trimmingCharacters(in: .whitespaces) ?? ""
            var windows: [QuotaWindow] = []
            if snap.hour5_pct > 0 || snap.weekly_pct > 0 || snap.mcp_pct > 0 {
                windows.append(QuotaWindow(key: "hour5", title: "5h",
                                           usedPercent: Double(snap.hour5_pct)))
                windows.append(QuotaWindow(key: "weekly", title: "本周",
                                           usedPercent: Double(snap.weekly_pct),
                                           resetsAt: snap.weekly_reset))
                if snap.mcp_pct > 0 {
                    windows.append(QuotaWindow(key: "mcp", title: "MCP 月度",
                                               usedPercent: Double(snap.mcp_pct),
                                               used: snap.mcp_used.map(Double.init),
                                               total: snap.mcp_total.map(Double.init),
                                               unit: "次"))
                }
            }
            let key = Key(account: account, second: snap.ts / 1000)
            if var previous = byKey[key] {
                // 同秒多设备：各窗口百分比取高
                var map: [String: QuotaWindow] = [:]
                for w in previous.windows { map[w.key] = w }
                for w in windows {
                    if (w.usedPercent ?? 0) > (map[w.key]?.usedPercent ?? -1) {
                        map[w.key] = w
                    }
                }
                previous.windows = map.values.sorted { $0.key < $1.key }
                previous.snap = previous.snap.ts <= snap.ts ? previous.snap : snap
                byKey[key] = previous
            } else {
                byKey[key] = (snap, windows)
            }
        }

        // 每 account 取最新一条
        var latest: [String: (ts: Int, windows: [QuotaWindow], level: String)] = [:]
        for (key, value) in byKey {
            let label = key.account
            if let old = latest[label], old.ts >= value.snap.ts { continue }
            let level = value.snap.level.trimmingCharacters(in: .whitespaces)
            latest[label] = (value.snap.ts, value.windows, level)
        }
        // 账号指纹排序输出（无 label 的主形态排最前）
        return latest
            .sorted { $0.key < $1.key }
            .map { label, value in
                SyncedAgentQuotaGroup(agent: .zai,
                                      label: label.isEmpty ? "" : "账号 \(label)",
                                      planType: value.level.isEmpty ? nil : value.level,
                                      planName: nil,
                                      updatedAtMs: value.ts,
                                      windows: value.windows,
                                      balance: nil,
                                      message: nil)
            }
    }

    // MARK: - 内部

    private func doRefreshAll() async {
        loading = true
        lastError = nil
        defer { loading = false }

        // 1. 没配置同步 → 只拉额度
        guard settings.isSyncConfigured else {
            await refreshQuotaOnly()
            return
        }

        let server = settings.sync.server_url
        let token = settings.sync.device_token
        let localId = settings.sync.device_id

        async let devicesResult: [DeviceInfo]? = try? await APIClient.shared.devices(
            serverURL: server, deviceToken: token)

        // 2. 今日用量
        let now = Date()
        let cal = Calendar.current
        let startOfToday = cal.startOfDay(for: now)
        let fromMs = Int(startOfToday.timeIntervalSince1970 * 1000)
        let toMs = Int(now.timeIntervalSince1970 * 1000)

        do {
            let usage = try await APIClient.shared.usage(
                serverURL: server, deviceToken: token,
                fromMs: fromMs, toMs: toMs,
                bucket: .hour, filter: selectedFilter, localDeviceId: localId)
            self.todayUsage = usage
            self.todayCost = Billing.computeRemoteCost(usage, pricing: settings.pricing)
        } catch {
            self.lastError = error.localizedDescription
        }

        // 3. 设备列表（失败不阻塞）
        if let devs = await devicesResult {
            self.devices = devs
        }

        // 4. 额度（直连 + 同步）
        await refreshQuotaOnly()
    }

    // MARK: - Widget 缓存

    /// 把当前内存数据打包写入 App Group，供 Widget 渲染。
    /// currency 由 settings.saveCurrency() 单独写共享容器，不放进 snapshot。
    /// 额度数据来源按「选中账号」解析（批2）：选中同步分组 → 选中直连账号 →
    /// 主账号（第一个 zai，无则第一个直连）回退；今日统计恒为设备级数据。
    /// WidgetSnapshot 新字段（accountId/accountLabel/agentKind/balance 等）
    /// 决定 Widget 端渲染形态（三档百分比 / 余额 / 桌面同步）。
    public func writeWidgetSnapshot() {
        let usage = todayUsage
        let cost = todayCost
        let store = AccountStore.shared
        let selectedId = store.selectedWidgetAccountId

        // —— 解析额度来源（同步 → 直连 → 主账号回退）——
        var group: SyncedAgentQuotaGroup?
        var entry: QuotaEntry?
        var accountId = "primary"
        if let sel = selectedId {
            if sel.hasPrefix("sync:"), let agent = AgentKind(rawValue: String(sel.dropFirst(5))),
               let matched = syncedQuotaGroups.first(where: { $0.agent == agent }) {
                group = matched
                accountId = sel
            } else if let matched = quotaEntries.first(where: { $0.accountId == sel }) {
                entry = matched
                accountId = sel
            }
        }
        if group == nil && entry == nil,
           let primaryId = store.primaryAccount()?.id.uuidString,
           let matched = quotaEntries.first(where: { $0.accountId == primaryId }) {
            entry = matched
            accountId = primaryId
        }

        var hour5Pct: Int?
        var weeklyPct: Int?
        var mcpPct: Int?
        var mcpUsed: Int?
        var mcpTotal: Int?
        var weeklyResetMs: Int?
        var hour5ResetMs: Int?
        var planName: String?
        var accountLabel: String?
        var agentKind: String?
        var isSynced: Bool?
        var balance: QuotaBalance?
        var updatedAt = Date().timeIntervalSince1970

        func pct(_ windows: [QuotaWindow], _ key: String) -> Int? {
            windows.first { $0.key == key }?.usedPercent.map { Int($0.rounded()) }
        }

        if let group {
            // 同步型：窗口映射 hour5/weekly/mcp；cursor 的 auto 窗并入 weekly 槽。
            // 余额型同步分组（deepseek 等）透传 balance，Widget 走余额卡渲染。
            let windows = group.windows
            hour5Pct = pct(windows, "hour5")
            weeklyPct = pct(windows, "weekly") ?? pct(windows, "cursor_auto")
            mcpPct = pct(windows, "mcp")
            hour5ResetMs = windows.first { $0.key == "hour5" }?.resetsAt
            weeklyResetMs = windows.first {
                $0.key == "weekly" || $0.key == "cursor_auto"
            }?.resetsAt
            planName = group.planBadge
            accountLabel = group.displayTitle
            agentKind = group.agent.rawValue
            isSynced = true
            balance = group.balance
            updatedAt = TimeInterval(group.updatedAtMs) / 1000
        } else if let q = entry {
            let windows = q.windows
            hour5Pct = pct(windows, "hour5")
            weeklyPct = pct(windows, "weekly")
            mcpPct = pct(windows, "mcp")
            mcpUsed = windows.first { $0.key == "mcp" }?.used.map { Int($0.rounded()) }
            mcpTotal = windows.first { $0.key == "mcp" }?.total.map { Int($0.rounded()) }
            hour5ResetMs = windows.first { $0.key == "hour5" }?.resetsAt
            weeklyResetMs = windows.first { $0.key == "weekly" }?.resetsAt
            planName = q.planName
            accountLabel = q.label.isEmpty ? q.agent.displayName : q.label
            agentKind = q.agent.rawValue
            balance = q.balance
            updatedAt = TimeInterval(q.updatedAt) / 1000
        }

        let snap = WidgetSnapshot(
            todayCostCny: cost?.total_cny ?? 0,
            todayCostUsd: cost?.total_usd ?? 0,
            todayTokens: usage?.overall.total_tokens ?? 0,
            todayRequests: usage?.overall.requests ?? 0,
            hour5Pct: hour5Pct,
            weeklyPct: weeklyPct,
            mcpPct: mcpPct,
            mcpUsed: mcpUsed,
            mcpTotal: mcpTotal,
            weeklyResetMs: weeklyResetMs,
            hour5ResetMs: hour5ResetMs,
            level: planName ?? "",
            deviceName: settings.sync.device_name,
            updatedAt: updatedAt,
            accountId: accountId,
            accountLabel: accountLabel,
            agentKind: agentKind,
            isSynced: isSynced,
            balanceAmount: balance?.amount,
            balanceCurrency: balance?.currency,
            balanceGranted: balance?.granted,
            balanceToppedUp: balance?.toppedUp)

        settings.writeWidgetSnapshot(snap)
    }

    // MARK: - 设备筛选切换

    public func setFilter(_ f: DeviceFilter) {
        selectedFilter = f
        refreshAll()
    }
}
