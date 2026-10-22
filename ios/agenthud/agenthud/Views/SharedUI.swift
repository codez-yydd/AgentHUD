//
//  SharedUI.swift
//  agenthud
//
//  跨视图共用的数据口径（来源显示名 / 用量聚合）与旧版 Card 兼容包装。
//  视觉组件（卡片、指标、chip、进度环等）统一在 DesignSystem.swift。
//

import SwiftUI

// MARK: - 额度百分比颜色

/// 按百分比选颜色：< 60 mint，60-85 amber，> 85 coral。
/// 保留旧函数名，避免引用点改动；实际色值由 Theme 统一维护。
public func quotaColor(_ pct: Int) -> Color {
    Theme.quota(pct)
}

// MARK: - 旧版卡片包装

/// 兼容旧调用点的卡片（标题 + 内容）。新代码直接用 GlassCard + CardHeader。
struct Card<Content: View>: View {
    let title: String?
    let content: Content

    init(title: String? = nil, @ViewBuilder content: () -> Content) {
        self.title = title
        self.content = content()
    }

    var body: some View {
        GlassCard {
            VStack(alignment: .leading, spacing: 14) {
                if let t = title {
                    CardHeader(title: t)
                }
                content
            }
        }
    }
}

// MARK: - 用量来源显示名（总览页 / 日志页共用）

/// 用量明细 source → 显示名。文案对齐桌面请求日志页 sourceLabel 的最终
/// 口径（经 agentLabel 的品牌元数据），未知来源原样回显。
/// 注意与额度 provider 源是两套命名（如 DeepSeek 明细源是 dsh，额度
/// provider 源是 deepseek），不要混用。
enum UsageSourceLabels {
    /// 展示顺序与桌面 LOG_SOURCES 一致
    static let all: [String] = [
        "zcode", "codex", "claude", "kimi", "dsh", "opencode", "minimaxcode",
    ]

    static func label(_ source: String) -> String {
        switch source {
        case "zcode": return "Z.ai"
        case "codex": return "Codex"
        case "claude": return "Claude"
        case "kimi": return "Kimi Code"
        case "dsh": return "DeepSeek Harness"
        case "opencode": return "OpenCode"
        case "minimaxcode": return "MiniMax Code"
        default: return source
        }
    }
}

// MARK: - 用量聚合小工具（总览页 / 日志页共用）

enum UsageAggregate {
    /// 缓存命中率（分源口径，对齐桌面 request_logs 的 cache_denominator：
    /// claude / kimi 的 input_tokens 不含缓存读写，分母补回缓存读 + 缓存写；
    /// 其余来源 input 已含缓存读，分母就是 input。分母为 0 → nil）。
    static func cacheHitRate(_ stats: [UsageRecordsModelStat]) -> Double? {
        var numerator = 0
        var denominator = 0
        for s in stats {
            numerator += s.cache_read_tokens
            if s.source == "claude" || s.source == "kimi" {
                denominator += s.input_tokens + s.cache_read_tokens + s.cache_write_tokens
            } else {
                denominator += s.input_tokens
            }
        }
        return denominator > 0 ? Double(numerator) / Double(denominator) : nil
    }

    /// by_model 分组的 token 总量。
    static func totalTokens(_ stats: [UsageRecordsModelStat]) -> Int {
        stats.reduce(0) { $0 + $1.total_tokens }
    }
}
