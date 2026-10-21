//
//  Types.swift
//  agenthud
//
//  1:1 翻译自 src/types.ts + src-tauri/src/quota.rs + server/db.py 的数据结构。
//  字段名严格对齐服务端 JSON（snake_case），用 CodingKeys 保证解码正确。
//

import Foundation

// MARK: - 统计（对应 Stats / OverallStat / ModelStat）

public struct OverallStat: Codable, Hashable {
    public var requests: Int = 0
    public var input_tokens: Int = 0
    public var output_tokens: Int = 0
    public var cache_read_tokens: Int = 0
    public var cache_write_tokens: Int = 0
    public var reasoning_tokens: Int = 0
    public var total_tokens: Int = 0
}

public struct ModelStat: Codable, Hashable, Identifiable {
    public var model_id: String
    public var provider_id: String
    public var requests: Int
    public var input_tokens: Int
    public var output_tokens: Int
    public var cache_read_tokens: Int
    public var cache_write_tokens: Int
    public var reasoning_tokens: Int
    public var total_tokens: Int

    // Identifiable 用，避免 SwiftUI ForEach 警告
    public var id: String { "\(provider_id)|\(model_id)" }
}

public struct Stats: Codable, Hashable {
    public var from_ms: Int
    public var to_ms: Int
    public var overall: OverallStat
    public var by_model: [ModelStat]
    public var earliest_ms: Int?
    public var latest_ms: Int?
}

// MARK: - 价格（对应 PricingConfig / ModelPrice / PricingDefaults）

public struct ModelPrice: Codable, Hashable {
    public var input: Double
    public var output: Double
    public var cache_read: Double

    public init(input: Double = 0, output: Double = 0, cache_read: Double = 0) {
        self.input = input
        self.output = output
        self.cache_read = cache_read
    }
}

public struct PricingConfig: Codable, Hashable {
    public var cny: [String: ModelPrice]
    public var usd: [String: ModelPrice]

    public init(cny: [String: ModelPrice] = [:], usd: [String: ModelPrice] = [:]) {
        self.cny = cny
        self.usd = usd
    }
}

/// 内置默认价格表（public/pricing-defaults.json 的结构）
public struct PricingDefaults: Codable {
    public var version: String
    public var cny: [String: ModelPrice]
    public var usd: [String: ModelPrice]
}

public enum Currency: String, Codable, CaseIterable {
    case cny, usd
}

// MARK: - 花费结果（对应 CostResult）

public struct ModelCost: Codable, Hashable, Identifiable {
    public var model_id: String
    public var cost: Double
    public var id: String { model_id }
}

public struct CostResult: Codable, Hashable {
    public var total_cny: Double
    public var total_usd: Double
    public var per_model_cny: [ModelCost]
    public var per_model_usd: [ModelCost]
}

// MARK: - 趋势（对应 TrendPoint / TrendBucket）

public enum TrendBucket: String, Codable, CaseIterable {
    case hour, day
}

public struct TrendPoint: Codable, Hashable, Identifiable {
    public var label: String
    public var total_tokens: Int
    public var requests: Int
    public var cost_cny: Double
    public var cost_usd: Double

    public var id: String { label }
}

// MARK: - 远端用量聚合（对应 RemoteUsage / RemoteOverall / RemoteModelStat / RemoteTrendBucket）

public struct RemoteTrendBucketModel: Codable, Hashable {
    public var model_id: String
    public var provider_id: String
    public var requests: Int
    public var input_tokens: Int
    public var output_tokens: Int
    public var cache_read_tokens: Int
    public var total_tokens: Int
}

public struct RemoteTrendBucket: Codable, Hashable {
    /// 服务端按 UTC 分桶，label 是 ISO 字符串或毫秒字符串
    public var label: String
    public var by_model: [RemoteTrendBucketModel]
    public var total_tokens: Int
    public var requests: Int
}

public struct RemoteUsage: Codable, Hashable {
    public var from_ms: Int
    public var to_ms: Int
    public var overall: OverallStat
    public var by_model: [ModelStat]
    public var trend: [RemoteTrendBucket]
}

// MARK: - 额度（对应 QuotaResult / QuotaLimit，字段驼峰对齐智谱接口）

public struct McpUsageDetail: Codable, Hashable {
    // 注意：智谱接口 usageDetails[] 里字段就是 snake_case（与 Rust 对齐，未做 rename）
    public var model_code: String
    public var usage: Int
}

public struct QuotaLimit: Codable, Hashable {
    public var type: String                 // "TOKENS_LIMIT" | "TIME_LIMIT"
    public var unit: Int?                   // 3=小时, 6=周
    public var number: Int?                 // 5=5小时, 1=每周
    public var percentage: Int              // 0-100
    public var nextResetTime: Int?          // 毫秒时间戳
    public var currentValue: Int?           // MCP 已用次数
    public var usage: Int?                  // MCP 总额度次数（注意：不是 total）
    public var usageDetails: [McpUsageDetail]?
}

public enum QuotaEndpoint: String, Codable, CaseIterable {
    case cn, global

    public var base: String {
        switch self {
        case .cn: return "https://open.bigmodel.cn"
        case .global: return "https://api.z.ai"
        }
    }
}

/// 额度查询配置（对应 quota.json）
public struct QuotaConfig: Codable, Hashable {
    public var token: String
    public var endpoint: QuotaEndpoint

    public init(token: String = "", endpoint: QuotaEndpoint = .cn) {
        self.token = token
        self.endpoint = endpoint
    }
}

// MARK: - 多 Agent 多账号额度体系（对齐桌面 provider_credentials + provider_quota.rs）

/// 额度 agent 类型。前 6 个为直连型（本机配凭证直接查官方接口）；
/// codex / claude / cursor / kimi 为只读同步型（凭证只在桌面端配置并上传
/// 快照，iOS 从 server 拉取展示，不可在本机配置凭证）；qoder / longcat /
/// mimo / stepfun / grok / alibabatoken 同为纯同步型（桌面 provider 通道）；
/// deepseek / moonshot / minimax / opencodego / alibaba 为双形态——既支持
/// 本机直连，也接收桌面同步的快照（数据来源标「桌面同步」区分）。
public enum AgentKind: String, Codable, CaseIterable, Identifiable {
    case zai, deepseek, moonshot, minimax, opencodego, alibaba
    case codex, claude, cursor
    case kimi, qoder, longcat, mimo, stepfun, grok, alibabatoken

    public var id: String { rawValue }

    /// 直连型：可在本机配置凭证并查询。
    public var isDirect: Bool {
        switch self {
        case .zai, .deepseek, .moonshot, .minimax, .opencodego, .alibaba:
            return true
        case .codex, .claude, .cursor, .kimi, .qoder, .longcat,
             .mimo, .stepfun, .grok, .alibabatoken:
            return false
        }
    }

    /// 纯同步型：无直连形态，额度数据只能来自桌面端上传的快照。
    public var isSyncOnly: Bool {
        !isDirect
    }

    /// 需要国内/国际站区域选择的 agent（查询端点按 region 分流）。
    public var supportsRegion: Bool {
        switch self {
        case .zai, .moonshot, .minimax, .alibaba:
            return true
        case .deepseek, .opencodego, .codex, .claude, .cursor, .kimi,
             .qoder, .longcat, .mimo, .stepfun, .grok, .alibabatoken:
            return false
        }
    }

    /// 设置页/额度页的显示名（对齐桌面端 agentVisibility.ts 的 label 口径）。
    public var displayName: String {
        switch self {
        case .zai: return "GLM Coding Plan"
        case .deepseek: return "DeepSeek"
        case .moonshot: return "Kimi 开放平台"
        case .minimax: return "MiniMax"
        case .opencodego: return "OpenCode Go"
        case .alibaba: return "阿里 Coding Plan"
        case .codex: return "Codex"
        case .claude: return "Claude"
        case .cursor: return "Cursor"
        case .kimi: return "Kimi Code"
        case .qoder: return "Qoder"
        case .longcat: return "LongCat"
        case .mimo: return "MiMo"
        case .stepfun: return "StepFun"
        case .grok: return "Grok"
        case .alibabatoken: return "百炼Token包"
        }
    }

    /// 添加账号表单里的凭证输入提示。
    public var tokenPlaceholder: String {
        switch self {
        case .zai: return "Coding Plan API Key"
        case .deepseek: return "sk-... 开放平台 API Key"
        case .moonshot: return "sk-... 开放平台 API Key"
        case .minimax: return "sk-cp-... Coding Plan Token"
        case .opencodego: return "OpenCode API Key"
        case .alibaba: return "sk-... DashScope API Key"
        case .codex, .claude, .cursor, .kimi, .qoder, .longcat,
             .mimo, .stepfun, .grok, .alibabatoken:
            return ""
        }
    }
}

/// 账号区域（仅 supportsRegion 的 agent 使用）。
public enum AccountRegion: String, Codable, CaseIterable, Identifiable {
    case cn, global
    public var id: String { rawValue }
    public var displayName: String {
        switch self {
        case .cn: return "国内站"
        case .global: return "国际站"
        }
    }
}

/// 账号最近一次校验结论（对齐桌面 provider_credentials 的 last_check）。
public struct AccountLastCheck: Codable, Hashable {
    /// "ok" | "expired" | "error" | "pending"
    public var status: String
    /// 校验完成时刻（ms）
    public var at: Int?
    /// 原因（中文，不含 secret 片段）
    public var message: String?

    public init(status: String, at: Int? = nil, message: String? = nil) {
        self.status = status
        self.at = at
        self.message = message
    }
}

/// 直连型额度账号的元数据（secret 存 Keychain，绝不落本结构）。
public struct AccountEntry: Codable, Hashable, Identifiable {
    public var id: UUID
    public var agent: AgentKind
    /// 备注名（≤32 字）
    public var label: String
    /// 仅 supportsRegion 的 agent 有值
    public var region: AccountRegion?
    public var createdAt: Int
    public var updatedAt: Int
    public var lastCheck: AccountLastCheck?

    public init(id: UUID = UUID(),
                agent: AgentKind,
                label: String,
                region: AccountRegion? = nil,
                createdAt: Int = Int(Date().timeIntervalSince1970 * 1000),
                updatedAt: Int = Int(Date().timeIntervalSince1970 * 1000),
                lastCheck: AccountLastCheck? = nil) {
        self.id = id
        self.agent = agent
        self.label = label
        self.region = region
        self.createdAt = createdAt
        self.updatedAt = updatedAt
        self.lastCheck = lastCheck
    }
}

/// 单个用量窗口（对齐桌面 ProviderQuotaWindow，camelCase）。
public struct QuotaWindow: Codable, Hashable, Identifiable {
    /// 窗口标识（"hour5" / "weekly" / "mcp" / "interval" / "monthly" / ...）
    public var key: String
    /// 展示标题（"5h" / "本周" / "MCP 月度" ...）
    public var title: String
    /// 已用百分比 0-100（缺省时只展示 used/total）
    public var usedPercent: Double?
    /// 已用量（配 unit 展示）
    public var used: Double?
    /// 总量
    public var total: Double?
    /// 数量单位（"次" / "token"）
    public var unit: String?
    /// 下次重置时间（ms 时间戳）
    public var resetsAt: Int?

    public var id: String { key }

    public init(key: String, title: String, usedPercent: Double? = nil,
                used: Double? = nil, total: Double? = nil,
                unit: String? = nil, resetsAt: Int? = nil) {
        self.key = key
        self.title = title
        self.usedPercent = usedPercent
        self.used = used
        self.total = total
        self.unit = unit
        self.resetsAt = resetsAt
    }
}

/// 按量计费余额（对齐桌面 ProviderQuotaBalance）。
public struct QuotaBalance: Codable, Hashable {
    public var amount: Double
    /// 币种符号或代码（"$" / "¥" / "CNY"）
    public var currency: String
    /// 累计赠送
    public var granted: Double?
    /// 累计充值
    public var toppedUp: Double?

    public init(amount: Double, currency: String,
                granted: Double? = nil, toppedUp: Double? = nil) {
        self.amount = amount
        self.currency = currency
        self.granted = granted
        self.toppedUp = toppedUp
    }
}

/// 单个账号一次额度查询的展示条目（对齐桌面 ProviderQuotaEntry）。
/// 查询失败也产出条目（status=error/expired + message），不抛错——
/// 与桌面"单凭证失败不阻塞其他凭证"的语义一致。
public struct QuotaEntry: Codable, Hashable, Identifiable {
    /// 关联账号 id（与 AccountEntry.id 对应）
    public var accountId: String
    public var label: String
    public var agent: AgentKind
    /// "ok" | "expired" | "error" | "pending"
    public var status: String
    public var windows: [QuotaWindow]
    public var balance: QuotaBalance?
    /// 套餐名（"pro" / "Coding Plan Lite" ...）
    public var planName: String?
    /// 失败/过期原因（中文，不含 secret）
    public var message: String?
    /// 查询完成时刻（ms）
    public var updatedAt: Int

    public var id: String { accountId }

    public init(accountId: String, label: String, agent: AgentKind,
                status: String, windows: [QuotaWindow] = [],
                balance: QuotaBalance? = nil, planName: String? = nil,
                message: String? = nil, updatedAt: Int = Int(Date().timeIntervalSince1970 * 1000)) {
        self.accountId = accountId
        self.label = label
        self.agent = agent
        self.status = status
        self.windows = windows
        self.balance = balance
        self.planName = planName
        self.message = message
        self.updatedAt = updatedAt
    }
}

// MARK: - 远端 Agent 额度快照（server GET /agent-quota-snapshots 的返回形状）

/// 快照内单个窗口（snake_case 对齐服务端 JSON）。
public struct RemoteAgentQuotaWindowDTO: Codable, Hashable {
    public var key: String
    public var used_pct: Double
    public var reset_at: Int?
}

/// 快照携带的按量余额（proto 7 扩展；服务端 balance_json 列还原结果）。
public struct RemoteAgentQuotaBalanceDTO: Codable, Hashable {
    public var amount: Double
    public var currency: String?
    public var granted: Double?
    public var topped_up: Double?

    public func toLocal() -> QuotaBalance {
        QuotaBalance(amount: amount,
                     currency: currency ?? "",
                     granted: granted,
                     toppedUp: topped_up)
    }
}

/// 单条远端快照（桌面已上传的 Codex / Claude / Cursor / Kimi 及各 provider 采样）。
/// proto 7 扩展字段全部 Optional：旧服务端不回传时解码为 nil（向后兼容）。
public struct RemoteAgentQuotaSnapshotDTO: Codable, Hashable {
    public var source: String
    public var ts: Int
    public var plan_type: String?
    public var windows: [RemoteAgentQuotaWindowDTO]
    public var device_id: String?
    /// 凭证备注名（同 provider 多凭证的账号维度）
    public var label: String?
    /// 套餐展示名（provider 通道）
    public var plan_name: String?
    /// 按量余额（余额型 provider）
    public var balance: RemoteAgentQuotaBalanceDTO?
    /// 查询失败原因（错误态快照）
    public var message: String?
}

// MARK: - 同步配置（对应 sync.json）

public enum SyncMode: String, Codable {
    case manual, auto
}

public struct SyncConfig: Codable, Hashable {
    public var enabled: Bool
    public var mode: SyncMode
    public var interval_seconds: Int
    public var server_url: String
    public var device_id: String
    public var device_name: String
    public var device_token: String
    public var last_uploaded_rowid: Int
    public var last_sync_at: Int
}

public struct RegisterRequest: Codable {
    public let server_url: String
    public let master_token: String
    public let device_name: String
}

public struct RegisterResponse: Codable {
    public let device_id: String
    public let device_token: String
    public let device_name: String
}

public struct SyncOutcome: Codable, Hashable {
    public var uploaded: Int
    public var new_max_rowid: Int
    public var last_sync_at: Int
}

public struct DeviceInfo: Codable, Hashable, Identifiable {
    public var device_id: String
    public var device_name: String
    public var created_at: Int
    public var record_count: Int?

    public var id: String { device_id }
}

// MARK: - 周额度 / 快照（对应 QuotaSnapshot / WeeklyPeriod）

public struct QuotaSnapshot: Codable, Hashable {
    public var ts: Int
    public var level: String
    public var weekly_pct: Int
    public var weekly_reset: Int?
    public var hour5_pct: Int
    public var mcp_pct: Int
    public var mcp_used: Int?
    public var mcp_total: Int?
}

public struct RemoteSnapshot: Codable, Hashable {
    public var ts: Int
    public var level: String
    public var weekly_pct: Int
    public var weekly_reset: Int?
    public var hour5_pct: Int
    public var mcp_pct: Int
    public var mcp_used: Int?
    public var mcp_total: Int?
    public var device_id: String
    /// 归属账号指纹（proto 4 起服务端回传；旧服务端/旧数据为 nil，
    /// 多账号同步展示时归入无 label 的单卡）
    public var account: String?

    // 与本地 QuotaSnapshot 互转的便利
    public func toLocal() -> QuotaSnapshot {
        QuotaSnapshot(ts: ts, level: level, weekly_pct: weekly_pct,
                      weekly_reset: weekly_reset, hour5_pct: hour5_pct,
                      mcp_pct: mcp_pct, mcp_used: mcp_used, mcp_total: mcp_total)
    }
}

public struct WeeklyPeriod: Codable, Hashable, Identifiable {
    public var reset_at: Int
    public var end_at: Int
    public var is_current: Bool
    public var pct_start: Int
    public var pct_peak: Int
    public var pct_end: Int
    public var sample_count: Int

    public var id: Int { reset_at }
}

// MARK: - 远端周期明细 + 折算（对应 RemotePeriodDetail / WeeklyTokenBucket / ConsumedBucket）

public struct RemoteDetailRow: Codable, Hashable {
    public var started_at: Int
    public var model_id: String
    public var input_tokens: Int
    public var output_tokens: Int
    public var cache_read_tokens: Int
    public var total_tokens: Int
    /// 聚合协议新增：该行代表的请求次数（服务端按 (小时桶, 模型) 聚合后
    /// 一行 = 多次请求；started_at 为小时桶起点）。旧服务端逐行响应无此键，
    /// 解码为 1 —— 语义仍是每行一次请求，新旧服务端口径都正确。
    /// 合成 Codable 解码不读属性默认值（缺键直接抛错），兼容旧响应必须
    /// 自定义 init(from:) 走 decodeIfPresent。
    public var requests: Int = 1

    private enum CodingKeys: String, CodingKey {
        case started_at, model_id, input_tokens, output_tokens
        case cache_read_tokens, total_tokens, requests
    }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        started_at = try c.decode(Int.self, forKey: .started_at)
        model_id = try c.decode(String.self, forKey: .model_id)
        input_tokens = try c.decode(Int.self, forKey: .input_tokens)
        output_tokens = try c.decode(Int.self, forKey: .output_tokens)
        cache_read_tokens = try c.decode(Int.self, forKey: .cache_read_tokens)
        total_tokens = try c.decode(Int.self, forKey: .total_tokens)
        requests = try c.decodeIfPresent(Int.self, forKey: .requests) ?? 1
    }
}

public struct RemotePeriodDetail: Codable, Hashable {
    public var reset_at: Int
    public var end_at: Int
    public var rows: [RemoteDetailRow]
}

public struct WeeklyTokenBucket: Codable, Hashable, Identifiable {
    public var reset_at: Int
    public var end_at: Int
    public var total_tokens: Int
    public var requests: Int
    public var id: Int { reset_at }
}

public enum PlanType: String, Codable, CaseIterable {
    case v2, v3
}

public struct PeakSegment: Codable, Hashable {
    public var start: String        // "HH:MM"
    public var end: String          // "HH:MM"
    public var multiplier: Double
    public var weekday_mask: Int    // bit0=周日...bit6=周六
}

public struct PeakConfig: Codable, Hashable {
    public var plan_type: PlanType?
    public var zcode_discount: Bool
    public var enabled: Bool
    public var segments: [PeakSegment]
}

public struct ConsumedBucket: Codable, Hashable, Identifiable {
    public var reset_at: Int
    public var end_at: Int
    public var consumed: Double
    public var requests: Int
    public var id: Int { reset_at }
}

// MARK: - 清理相关

public struct AutoCleanupConfig: Codable, Hashable {
    public var auto_enabled: Bool
    public var auto_days: Int
}

public struct CleanupStatus: Codable, Hashable {
    public var total_records: Int
    public var devices: [DeviceInfo]
    public var auto_config: AutoCleanupConfig
}

public struct CleanupResult: Codable, Hashable {
    public var action: String
    public var records_deleted: Int
    public var devices_deleted: Int?
}

// MARK: - 设备筛选

public enum DeviceFilter: Hashable {
    case all
    case local
    case specific(String)   // device_id
}

// MARK: - 时间范围预设

public enum RangePreset: String, CaseIterable, Identifiable {
    case today, d1 = "1d", d7 = "7d", d30 = "30d", custom

    public var id: String { rawValue }

    public var displayName: String {
        switch self {
        case .today: return "今日"
        case .d1: return "24小时"
        case .d7: return "7天"
        case .d30: return "30天"
        case .custom: return "自定义"
        }
    }
}

// MARK: - 额度预警 / 快捷键（对应 NotifyConfig / ShortcutConfig，iOS 仅保留 notify）

public struct NotifyConfig: Codable, Hashable {
    public var enabled: Bool
    public var hour5_threshold: Int
    public var weekly_threshold: Int
    public var mcp_threshold: Int
}
