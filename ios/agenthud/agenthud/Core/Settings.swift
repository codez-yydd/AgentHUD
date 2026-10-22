//
//  Settings.swift
//  agenthud
//
//  配置持久化层。所有用户配置存在 UserDefaults（标准域），
//  Widget 需要的数据额外写入 App Group 共享容器。
//
//  桌面端用 ~/.zbar/*.json，iOS 用 UserDefaults 对应：
//   - sync.json      → SyncConfig
//   - pricing.json   → PricingConfig
//   - quota.json     → QuotaConfig
//   - currency 偏好  → Currency
//   - peak.json      → PeakConfig
//

import Foundation
import Combine

/// App Group 标识符（Widget 与 App 共享数据用）。
/// 免费账号也能用 App Group，只需在 Xcode 给两个 target 都勾选同一 group。
public let APP_GROUP = "group.com.chacca.agenthud"

/// Widget 读取的共享 key。
public enum WidgetCacheKey {
    public static let snapshot = "widget.snapshot"          // WidgetSnapshot（JSON）
    public static let lastUpdate = "widget.lastUpdate"      // Double（时间戳）
    public static let currency = "widget.currency"
    /// 批2：Widget Intent 的动态账号选项清单（[WidgetAccountOption] JSON）
    public static let accountOptions = "widget.accountOptions"
    /// 批2：Widget 显示的选中账号 id（"primary"/直连账号 UUID/"sync:<agent>"），
    /// 存 App Group（主 App 设置页与 Widget Intent 双向写入，一处生效）
    public static let selectedAccountId = "widget.selectedAccountId"
}

/// Widget Intent 的账号选项摘要（主 App 写入 App Group，Widget 进程读取；
/// 字段与 Widget 侧 WidgetAccountIntent.swift 的同名结构保持一致）。
public struct WidgetAccountOption: Codable, Hashable, Identifiable {
    /// "primary"（跟随主账号，由 Intent 端固定构造，一般不落盘）/
    /// 直连账号 UUID / "sync:codex" 等同步分组 id
    public var id: String
    /// AgentKind rawValue（"zai" / "codex" ...）
    public var agent: String
    /// 展示名（直连=备注名；同步=agent 显示名）
    public var label: String
    /// "direct"（直连）| "sync"（只读同步）
    public var kind: String
    /// 套餐名（最近一次查询得到；可为空）
    public var planName: String?

    public init(id: String, agent: String, label: String,
                kind: String, planName: String? = nil) {
        self.id = id
        self.agent = agent
        self.label = label
        self.kind = kind
        self.planName = planName
    }
}

/// Widget 渲染所需的全部数据快照（App 拉好数据后写入共享容器）。
/// 批2 扩展字段全部 Optional + Codable 合成解码走 decodeIfPresent：
/// 旧版 JSON 缺新字段 → 解码为 nil（向后兼容）；新版 JSON 多字段对旧代码
/// 无影响（Codable 忽略未知 key）。
public struct WidgetSnapshot: Codable, Hashable {
    public var todayCostCny: Double
    public var todayCostUsd: Double
    public var todayTokens: Int
    public var todayRequests: Int
    public var hour5Pct: Int?
    public var weeklyPct: Int?
    public var mcpPct: Int?
    public var mcpUsed: Int?
    public var mcpTotal: Int?
    public var weeklyResetMs: Int?
    public var hour5ResetMs: Int?
    public var level: String
    public var deviceName: String
    public var updatedAt: Double
    // ===== 批2 新增（全部可选，向后兼容）=====
    /// 本快照额度数据所属账号 id（"primary"/UUID/"sync:<agent>"）
    public var accountId: String?
    /// 账号展示名（余额型/同步型卡片标题）
    public var accountLabel: String?
    /// AgentKind rawValue（决定 Widget 渲染形态）
    public var agentKind: String?
    /// 只读同步型标记（显示「桌面同步」小标）
    public var isSynced: Bool?
    /// 余额型账号的余额（deepseek/moonshot）
    public var balanceAmount: Double?
    public var balanceCurrency: String?
    public var balanceGranted: Double?
    public var balanceToppedUp: Double?

    public init(todayCostCny: Double = 0, todayCostUsd: Double = 0,
                todayTokens: Int = 0, todayRequests: Int = 0,
                hour5Pct: Int? = nil, weeklyPct: Int? = nil,
                mcpPct: Int? = nil, mcpUsed: Int? = nil, mcpTotal: Int? = nil,
                weeklyResetMs: Int? = nil, hour5ResetMs: Int? = nil,
                level: String = "", deviceName: String = "",
                updatedAt: Double = 0,
                accountId: String? = nil, accountLabel: String? = nil,
                agentKind: String? = nil, isSynced: Bool? = nil,
                balanceAmount: Double? = nil, balanceCurrency: String? = nil,
                balanceGranted: Double? = nil, balanceToppedUp: Double? = nil) {
        self.todayCostCny = todayCostCny
        self.todayCostUsd = todayCostUsd
        self.todayTokens = todayTokens
        self.todayRequests = todayRequests
        self.hour5Pct = hour5Pct
        self.weeklyPct = weeklyPct
        self.mcpPct = mcpPct
        self.mcpUsed = mcpUsed
        self.mcpTotal = mcpTotal
        self.weeklyResetMs = weeklyResetMs
        self.hour5ResetMs = hour5ResetMs
        self.level = level
        self.deviceName = deviceName
        self.updatedAt = updatedAt
        self.accountId = accountId
        self.accountLabel = accountLabel
        self.agentKind = agentKind
        self.isSynced = isSynced
        self.balanceAmount = balanceAmount
        self.balanceCurrency = balanceCurrency
        self.balanceGranted = balanceGranted
        self.balanceToppedUp = balanceToppedUp
    }
}

/// 全局配置中心，ObservableObject 供 SwiftUI 视图订阅。
@MainActor
public final class AppSettings: ObservableObject {
    public static let shared = AppSettings()

    // 用户配置（UserDefaults 标准域）
    @Published public var sync: SyncConfig
    @Published public var pricing: PricingConfig
    /// 价格来源元信息（远端更新时间/汇率/模型数；内置参考价时 isRemote=false）
    @Published public var pricingMeta: PricingSyncMeta
    @Published public var quotaConfig: QuotaConfig
    @Published public var currency: Currency
    @Published public var peak: PeakConfig

    // 共享容器（Widget 读取）
    private let sharedDefaults: UserDefaults?

    private init() {
        let d = UserDefaults.standard
        self.sync = Self.loadSync(d)
        self.pricing = Self.loadPricing(d)
        self.pricingMeta = Self.loadPricingMeta(d)
        self.quotaConfig = Self.loadQuota(d)
        self.currency = Self.loadCurrency(d)
        self.peak = Self.loadPeak(d)
        self.sharedDefaults = UserDefaults(suiteName: APP_GROUP)
    }

    // MARK: - 保存

    public func saveSync() {
        save("sync", sync)
    }
    public func savePricing() {
        save("pricing", pricing)
        save("pricingMeta", pricingMeta)
    }

    /// 应用桌面端下发的价格表：USD 原样保存，CNY 按同一汇率实时折算，
    /// 保证手机端花费与桌面端逐模型一致。空表/未同步状态直接忽略，
    /// 保留现有缓存或内置参考价。
    @discardableResult
    public func applyRemotePricing(_ remote: RemotePricing) -> Bool {
        // 服务端有表时允许空表（用户删光模型是合法状态，费用归零）；
        // 无表（synced=false / updated_at=0）时保留本地参考价。
        guard (remote.synced ?? true) else { return false }
        guard !remote.usd.isEmpty || remote.updated_at > 0 else { return false }
        let rate = (remote.fx_rate.isFinite && remote.fx_rate > 0) ? remote.fx_rate : 7.2
        let cny = remote.usd.mapValues { p in
            ModelPrice(input: p.input * rate,
                       output: p.output * rate,
                       cache_read: p.cache_read * rate)
        }
        pricing = PricingConfig(cny: cny, usd: remote.usd)
        pricingMeta = PricingSyncMeta(fxRate: rate,
                                      updatedAt: remote.updated_at,
                                      revision: remote.revision,
                                      modelCount: remote.model_count ?? remote.usd.count,
                                      isRemote: true)
        savePricing()
        return true
    }
    public func saveQuotaConfig() {
        save("quotaConfig", quotaConfig)
    }
    public func saveCurrency() {
        UserDefaults.standard.set(currency.rawValue, forKey: "currency")
        sharedDefaults?.set(currency.rawValue, forKey: WidgetCacheKey.currency)
    }
    public func savePeak() {
        save("peak", peak)
    }

    private func save<T: Encodable>(_ key: String, _ value: T) {
        if let data = try? JSONEncoder().encode(value) {
            UserDefaults.standard.set(data, forKey: key)
        }
    }

    // MARK: - Widget 缓存（写入 App Group）

    /// 把 Widget 渲染所需的快照写入共享容器。
    public func writeWidgetSnapshot(_ snap: WidgetSnapshot) {
        guard let sd = sharedDefaults else { return }
        if let data = try? JSONEncoder().encode(snap) {
            sd.set(data, forKey: WidgetCacheKey.snapshot)
            sd.set(Date().timeIntervalSince1970, forKey: WidgetCacheKey.lastUpdate)
        }
    }

    /// 从共享容器读 Widget 快照（Widget 进程也会用这个）。
    public static func readWidgetSnapshot() -> WidgetSnapshot? {
        guard let sd = UserDefaults(suiteName: APP_GROUP) else { return nil }
        guard let data = sd.data(forKey: WidgetCacheKey.snapshot) else { return nil }
        return try? JSONDecoder().decode(WidgetSnapshot.self, from: data)
    }

    // MARK: - 加载（含内置默认值）

    private static func loadSync(_ d: UserDefaults) -> SyncConfig {
        if let s = decode(SyncConfig.self, d, "sync") { return s }
        return SyncConfig(enabled: false, mode: .manual, interval_seconds: 60,
                          server_url: "", device_id: "", device_name: "iPhone",
                          device_token: "", last_uploaded_rowid: 0, last_sync_at: 0)
    }

    private static func loadPricing(_ d: UserDefaults) -> PricingConfig {
        if let p = decode(PricingConfig.self, d, "pricing") { return p }
        // 首次启动用内置默认表
        return loadBuiltinPricingDefaults()
    }

    private static func loadPricingMeta(_ d: UserDefaults) -> PricingSyncMeta {
        if let m = decode(PricingSyncMeta.self, d, "pricingMeta") { return m }
        return PricingSyncMeta()
    }

    private static func loadQuota(_ d: UserDefaults) -> QuotaConfig {
        if let q = decode(QuotaConfig.self, d, "quotaConfig") { return q }
        return QuotaConfig()
    }

    private static func loadCurrency(_ d: UserDefaults) -> Currency {
        Currency(rawValue: d.string(forKey: "currency") ?? "cny") ?? .cny
    }

    private static func loadPeak(_ d: UserDefaults) -> PeakConfig {
        if let p = decode(PeakConfig.self, d, "peak") {
            return p
        }
        return PeakConfig(plan_type: nil, zcode_discount: true,
                          enabled: false, segments: [])
    }

    private static func decode<T: Decodable>(_ type: T.Type, _ d: UserDefaults, _ key: String) -> T? {
        guard let data = d.data(forKey: key) else { return nil }
        return try? JSONDecoder().decode(T.self, from: data)
    }

    /// 从 bundle 加载内置默认价格表（public/pricing-defaults.json 的副本）。
    public static func loadBuiltinPricingDefaults() -> PricingConfig {
        guard let url = Bundle.main.url(forResource: "PricingDefaults", withExtension: "json"),
              let data = try? Data(contentsOf: url),
              let parsed = try? JSONDecoder().decode(PricingDefaults.self, from: data) else {
            return PricingConfig()
        }
        return PricingConfig(cny: parsed.cny, usd: parsed.usd)
    }
}

// MARK: - 已配置检查

extension AppSettings {
    /// 同步服务是否已配置（有 server_url + device_token）。
    public var isSyncConfigured: Bool {
        !sync.server_url.isEmpty && !sync.device_token.isEmpty
    }

    /// 额度监控是否已配置：以多账号仓库为准（含旧版单 GLM 账号自动迁移
    /// 进来的第一个 zai 账号）。quotaConfig 仅作为迁移源保留，不再消费。
    /// 计算属性在运行期访问 AccountStore，不参与 AppSettings.init（避免
    /// 单例重入）。
    public var isQuotaConfigured: Bool {
        AccountStore.shared.hasDirectAccounts
    }
}
