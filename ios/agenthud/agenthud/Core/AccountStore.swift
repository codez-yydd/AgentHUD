//
//  AccountStore.swift
//  agenthud
//
//  额度账号仓库：元数据（AccountEntry，不含 secret）存 UserDefaults JSON，
//  secret 存 Keychain（KeychainHelper）。对齐桌面 provider_credentials 的
//  "secret 永不回显、永不写日志" 契约——本类型不提供任何打印 secret 的途径。
//
//  同时承担旧版单 GLM 账号（QuotaConfig{token,endpoint}）的一次性迁移。
//

import Foundation
import Combine

@MainActor
public final class AccountStore: ObservableObject {
    public static let shared = AccountStore()

    /// 元数据在 UserDefaults 的存储 key（JSON 数组，不含 secret）。
    private static let storageKey = "agentQuotaAccounts"
    /// 旧版 QuotaConfig 的 UserDefaults key（迁移源）。
    private static let legacyQuotaKey = "quotaConfig"
    /// 备注名长度上限。
    private static let labelLimit = 32

    @Published public private(set) var accounts: [AccountEntry] = []

    /// App Group 共享容器（Widget Intent 读账号选项 / 写选中账号）。
    private let sharedDefaults: UserDefaults?

    /// secret 掩码缓存（账号 id → 掩码）：账号列表每帧渲染会读 maskedSecret，
    /// 逐帧查 Keychain 开销大且可能卡滚动；增删改时失效重取。
    /// 掩码本身是设计上有意暴露的展示形态（前 6 后 4），缓存无安全影响。
    private var maskCache: [UUID: String] = [:]

    private init() {
        accounts = Self.loadAccounts()
        self.sharedDefaults = UserDefaults(suiteName: APP_GROUP)
        migrateLegacyQuotaConfigIfNeeded()
        migrateSelectedWidgetAccountToAppGroup()
        writeWidgetAccountOptions()
    }

    // MARK: - 持久化

    private static func loadAccounts() -> [AccountEntry] {
        guard let data = UserDefaults.standard.data(forKey: storageKey),
              let list = try? JSONDecoder().decode([AccountEntry].self, from: data) else {
            return []
        }
        return list
    }

    private func persist() {
        if let data = try? JSONEncoder().encode(accounts) {
            UserDefaults.standard.set(data, forKey: Self.storageKey)
        }
        // 账号清单变化同步刷新 Widget Intent 的动态选项
        writeWidgetAccountOptions()
    }

    // MARK: - CRUD

    /// 新增账号（secret 写 Keychain）。label 超长自动截断。
    @discardableResult
    public func add(agent: AgentKind, label: String, region: AccountRegion?,
                    secret: String) -> AccountEntry {
        let entry = AccountEntry(agent: agent,
                                 label: Self.clampLabel(label, agent: agent),
                                 region: agent.supportsRegion ? region : nil)
        KeychainHelper.saveSecret(secret, account: entry.id.uuidString)
        // 预填掩码缓存（新账号卡片立即可见掩码，无需再查 Keychain）
        let trimmed = secret.trimmingCharacters(in: .whitespacesAndNewlines)
        if !trimmed.isEmpty {
            maskCache[entry.id] = KeychainHelper.maskSecret(trimmed)
        }
        accounts.append(entry)
        persist()
        return entry
    }

    /// 编辑账号。secret 传 nil 表示不修改（表单留空=保持原值，契约见设置页）。
    public func update(id: UUID, label: String, region: AccountRegion?,
                       secret: String?) {
        guard let idx = accounts.firstIndex(where: { $0.id == id }) else { return }
        accounts[idx].label = Self.clampLabel(label, agent: accounts[idx].agent)
        accounts[idx].region = accounts[idx].agent.supportsRegion ? region : nil
        accounts[idx].updatedAt = Int(Date().timeIntervalSince1970 * 1000)
        if let newSecret = secret {
            KeychainHelper.saveSecret(newSecret, account: id.uuidString)
            // secret 变化后失效掩码缓存（下次展示重取/按新值重建）
            let trimmed = newSecret.trimmingCharacters(in: .whitespacesAndNewlines)
            if trimmed.isEmpty {
                maskCache[id] = nil
            } else {
                maskCache[id] = KeychainHelper.maskSecret(trimmed)
            }
        }
        persist()
    }

    /// 删除账号（含 Keychain secret）。
    public func remove(id: UUID) {
        accounts.removeAll { $0.id == id }
        KeychainHelper.deleteSecret(account: id.uuidString)
        maskCache[id] = nil
        // 被删账号若是 Widget 选中账号，清空选择（nil = 跟随主账号）
        if selectedWidgetAccountId == id.uuidString {
            selectedWidgetAccountId = nil
        }
        persist()
    }

    /// 账号 secret 的展示掩码（带缓存）。首次访问查一次 Keychain，
    /// 之后走内存缓存；secret 未配置返回 nil（UI 显示无掩码）。
    public func cachedMask(for id: UUID) -> String? {
        if let cached = maskCache[id] { return cached }
        guard let secret = KeychainHelper.loadSecret(account: id.uuidString),
              !secret.isEmpty else { return nil }
        let mask = KeychainHelper.maskSecret(secret)
        maskCache[id] = mask
        return mask
    }

    /// 读取账号 secret（查询额度用；调用方不得展示或记录该值）。
    public func secret(for id: UUID) -> String? {
        KeychainHelper.loadSecret(account: id.uuidString)
    }

    /// 回写最近校验结论（查询完成后调用）。
    public func recordCheck(id: UUID, status: String, message: String?) {
        guard let idx = accounts.firstIndex(where: { $0.id == id }) else { return }
        accounts[idx].lastCheck = AccountLastCheck(
            status: status,
            at: Int(Date().timeIntervalSince1970 * 1000),
            message: message)
        persist()
    }

    // MARK: - 主账号与配置门槛

    /// Widget 主账号：账号列表第一个 zai 账号；无 zai 时第一个直连账号；
    /// 无任何账号返回 nil。选中账号为空/无效时的回退目标。
    public func primaryAccount() -> AccountEntry? {
        accounts.first { $0.agent == .zai } ?? accounts.first { $0.agent.isDirect }
    }

    /// 是否配置了任一直连账号（设置页/额度页的配置门槛）。
    public var hasDirectAccounts: Bool {
        accounts.contains { $0.agent.isDirect }
    }

    // MARK: - Widget 主账号选择（批2：存 App Group，主 App 与 Widget Intent 双向读写）

    /// 同步型分组的选项 id 前缀（"sync:codex" 等）。
    public static func syncOptionId(_ agent: AgentKind) -> String {
        "sync:\(agent.rawValue)"
    }

    /// Widget 显示的选中账号 id：nil/"primary" = 跟随主账号；UUID = 直连账号；
    /// "sync:<agent>" = 只读同步分组。存 App Group（Widget 进程可读写），
    /// 供设置页 Picker 与 Widget 编辑界面的 Intent 双向联动。
    public var selectedWidgetAccountId: String? {
        get {
            let value = sharedDefaults?.string(forKey: WidgetCacheKey.selectedAccountId)
            // "primary" 与 nil 同义（跟随主账号），归一为 nil 简化判断
            return (value == "primary") ? nil : value
        }
        set {
            guard let sd = sharedDefaults else { return }
            if let v = newValue, !v.isEmpty, v != "primary" {
                sd.set(v, forKey: WidgetCacheKey.selectedAccountId)
            } else {
                sd.removeObject(forKey: WidgetCacheKey.selectedAccountId)
            }
        }
    }

    /// 批1 的选中值存在标准域（Widget 进程读不到），一次性搬到 App Group。
    private func migrateSelectedWidgetAccountToAppGroup() {
        let legacyKey = "selectedWidgetAccountId"
        guard sharedDefaults?.object(forKey: WidgetCacheKey.selectedAccountId) == nil,
              let legacy = UserDefaults.standard.string(forKey: legacyKey),
              !legacy.isEmpty else { return }
        sharedDefaults?.set(legacy, forKey: WidgetCacheKey.selectedAccountId)
        UserDefaults.standard.removeObject(forKey: legacyKey)
    }

    /// 把账号清单摘要写入 App Group（Widget Intent 的动态选项数据源）：
    /// 全部直连账号 + 只读同步分组。「跟随主账号」由 Intent 端固定构造。
    /// syncAgents 为同步分组清单：默认列 CLI 同步型（codex/claude/cursor/kimi，
    /// server 长期支持）；查询后会以「实际有同步数据的 agent」重写（双形态
    /// provider 无同步数据时不占选项位）。选中 id 为 "sync:<agent>"，多账号
    /// 分组时 Widget 取该 agent 的第一个分组。
    public func writeWidgetAccountOptions(planNames: [String: String] = [:],
                                          syncAgents: [AgentKind] =
                                            [.codex, .claude, .cursor, .kimi]) {
        guard let sd = sharedDefaults else { return }
        var options: [WidgetAccountOption] = accounts.map { account in
            WidgetAccountOption(id: account.id.uuidString,
                                agent: account.agent.rawValue,
                                label: account.label,
                                kind: "direct",
                                planName: planNames[account.id.uuidString])
        }
        for agent in syncAgents {
            options.append(WidgetAccountOption(
                id: Self.syncOptionId(agent),
                agent: agent.rawValue,
                label: agent.displayName,
                kind: "sync",
                planName: planNames[Self.syncOptionId(agent)]))
        }
        if let data = try? JSONEncoder().encode(options) {
            sd.set(data, forKey: WidgetCacheKey.accountOptions)
        }
    }

    /// 最近一次查询后合并各账号 planName（Intent 选项副标题用）。
    /// 同步分组清单 = 默认兜底四项（codex/claude/cursor/kimi，server 长期
    /// 支持）∪ 实际有同步数据的 agent。兜底不收缩：server 不可达 / 查询
    /// 失败时 syncedGroups 为空，若按空清单重写会让已钉住 "sync:xxx" 的
    /// Widget 选项静默消失（离线时被迫回退主账号）。
    public func refreshOptionPlanNames(directEntries: [QuotaEntry],
                                       syncedGroups: [SyncedAgentQuotaGroup]) {
        var planNames: [String: String] = [:]
        for entry in directEntries where entry.status == "ok" {
            if let plan = entry.planName, !plan.isEmpty {
                planNames[entry.accountId] = plan
            }
        }
        // 默认兜底在前，实际有数据的分组追加（去重），保证清单只增不减
        var syncedAgents: [AgentKind] = [.codex, .claude, .cursor, .kimi]
        for group in syncedGroups {
            if let plan = group.planBadge, !plan.isEmpty {
                planNames[Self.syncOptionId(group.agent)] = plan
            }
            if !syncedAgents.contains(group.agent) {
                syncedAgents.append(group.agent)
            }
        }
        writeWidgetAccountOptions(planNames: planNames, syncAgents: syncedAgents)
    }

    // MARK: - 旧版单 GLM 账号迁移

    /// 把旧版 QuotaConfig（单 GLM token + endpoint）迁移为第一个 zai 账号。
    /// region 按 endpoint 映射：api.z.ai → global，open.bigmodel.cn → cn。
    /// 迁移后旧配置清理（token 置空），secret 不丢（转存 Keychain）。
    /// 已有账号（迁移过）时跳过；旧 token 为空时只清理不建账号。
    ///
    /// 失败保护：写 Keychain 后回读校验（受管设备限制/磁盘加密策略等会导致
    /// SecItemAdd 静默失败）——校验不过则不追加账号、不清空旧 token，保留
    /// 旧配置原样下次启动重试，并在控制台留固定文案（不含 token 内容）。
    private func migrateLegacyQuotaConfigIfNeeded() {
        guard accounts.isEmpty else { return }
        guard let data = UserDefaults.standard.data(forKey: Self.legacyQuotaKey),
              let legacy = try? JSONDecoder().decode(QuotaConfig.self, from: data) else {
            return
        }
        let token = legacy.token.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !token.isEmpty else { return }

        let region: AccountRegion = (legacy.endpoint == .global) ? .global : .cn
        let entry = AccountEntry(agent: .zai,
                                 label: "默认 GLM 账号",
                                 region: region)
        let saved = KeychainHelper.saveSecret(token, account: entry.id.uuidString)
        // 回读校验：写入成功且能读回非空值才算迁移完成
        let readBack = KeychainHelper.loadSecret(account: entry.id.uuidString)
        guard saved, let readBack, !readBack.isEmpty else {
            // 回滚本次写入残留，旧 quotaConfig 原样保留（下次启动重试）
            KeychainHelper.deleteSecret(account: entry.id.uuidString)
            print("[agenthud] 旧版额度账号迁移失败：Keychain 写入校验未通过，已保留旧配置待下次重试")
            return
        }
        accounts.append(entry)
        persist()
        // 默认 Widget 主账号指向迁移账号
        selectedWidgetAccountId = entry.id.uuidString

        // 旧配置清理：token 清空（保留结构，避免老代码读到脏值）。
        // 只写 UserDefaults 不回写 AppSettings.shared——AppSettings 若尚未
        // 初始化，这里触发的重入会卡死单例；启动后 UI 一律走 AccountStore，
        // 不再消费 quotaConfig.token。
        let cleaned = QuotaConfig(token: "", endpoint: legacy.endpoint)
        if let cleanedData = try? JSONEncoder().encode(cleaned) {
            UserDefaults.standard.set(cleanedData, forKey: Self.legacyQuotaKey)
        }
    }

    // MARK: - 辅助

    /// 备注名截断到 32 字；空白回退为 agent 显示名。
    private static func clampLabel(_ label: String, agent: AgentKind) -> String {
        let trimmed = label.trimmingCharacters(in: .whitespacesAndNewlines)
        let fallback = agent.displayName
        let base = trimmed.isEmpty ? fallback : trimmed
        return String(base.prefix(labelLimit))
    }
}
