//
//  AgentQuotaSync.swift
//  agenthud
//
//  只读同步型额度（Codex / Claude / Cursor / Kimi + 全部 provider）：
//  从自托管 server 拉桌面端已上传的额度快照（GET /agent-quota-snapshots），
//  合并多设备采样后按 (source, label) 分组供额度页展示。合并语义翻译自
//  桌面 src/agentQuota.ts:46-85：
//   - 同秒去重（各设备同一轮采样时间相差几毫秒）；
//   - 同 key 百分比取较高值不相加（额度百分比不相加，取高避免低估）；
//   - plan_type / plan_name 空则补，ts 取较小值。
//  方案 B（proto 7）扩展：
//   - label 维度：同一 provider 多凭证按账号拆分条目（label 空 fallback 到
//     agent 显示名，与桌面端「凭证备注名」语义一致）；
//   - 余额型：windows 为空但有 balance 的快照产出余额卡片数据；
//   - 错误态：message 有值的快照标记错误，额度页显示失败原因。
//  iOS 没有本机实时锚点，不做桌面 filterInflatedRemoteCursorSnapshots 的
//  Cursor 失真过滤（桌面在锚点缺失时也是原样返回，宁缺毋滥）。
//  服务端不支持该接口（404/空）时调用方降级为隐藏该分组，不报错。
//

import Foundation

/// 合并后的单条采样（已按展示形状归一）。
public struct AgentQuotaSample: Hashable {
    public var source: String
    /// 账号维度键（凭证备注名；空串 = 无账号维度的 CLI 通道）
    public var label: String
    public var ts: Int
    public var planType: String?
    public var planName: String?
    public var windows: [QuotaWindow]
    public var balance: QuotaBalance?
    /// 查询失败原因（错误态快照；nil = 正常态）
    public var message: String?
}

/// 只读同步型的一个展示分组（一个 (source, label) 一组，取最新一条采样）。
public struct SyncedAgentQuotaGroup: Identifiable {
    public var agent: AgentKind
    /// 展示名：多凭证拆分条目的账号名（空 → agent 显示名）
    public var label: String
    public var planType: String?
    /// 套餐展示名（provider 通道的 plan_name）
    public var planName: String?
    /// 最近一次采样时间（ms）
    public var updatedAtMs: Int
    public var windows: [QuotaWindow]
    /// 按量余额（余额型 provider）
    public var balance: QuotaBalance?
    /// 查询失败原因（错误态）
    public var message: String?

    public var id: String { "\(agent.rawValue)|\(label)" }

    /// 错误态：快照带失败原因（进度条/余额都不展示，卡片显示原因）
    public var isError: Bool { message != nil }

    /// 套餐徽标：planType（CLI 通道）优先，plan_name（provider 通道）兜底
    public var planBadge: String? {
        if let p = planType, !p.isEmpty { return p }
        if let p = planName, !p.isEmpty { return p }
        return nil
    }

    /// 卡片标题：多凭证时「服务名 · 账号名」，单凭证时服务名。
    public var displayTitle: String {
        label.isEmpty ? agent.displayName : "\(agent.displayName) · \(label)"
    }
}

public enum AgentQuotaSync {

    /// 各 source 允许的窗口 key（对齐桌面 AGENT_QUOTA_WINDOWS + 各 provider
    /// 归一后的稳定键；服务端已按白名单过滤，这里做展示前的第二道防线）。
    private static let validKeys: Set<String> = [
        "hour5", "weekly", "monthly", "interval", "cursor_auto", "cursor_api",
        "credits", "quota", "fuel", "sub_credits", "topup_credits",
    ]

    /// iOS 侧消费的同步型 source → AgentKind（GLM 走 /snapshots 通道，
    /// 不在此表——由 RefreshService 单独拼装为 .zai 分组）。
    public static let syncAgents: [String: AgentKind] = [
        "codex": .codex,
        "claude": .claude,
        "cursor": .cursor,
        "kimi": .kimi,
        "deepseek": .deepseek,
        "moonshot": .moonshot,
        "minimax": .minimax,
        "opencodego": .opencodego,
        "alibaba": .alibaba,
        "qoder": .qoder,
        "longcat": .longcat,
        "mimo": .mimo,
        "stepfun": .stepfun,
        "grok": .grok,
        "alibabatoken": .alibabatoken,
    ]

    /// 展示顺序：CLI 同步型 → 双形态 provider → 纯同步 provider。
    private static let sourceOrder: [String] = [
        "codex", "claude", "cursor", "kimi",
        "deepseek", "moonshot", "minimax", "opencodego", "alibaba",
        "qoder", "longcat", "mimo", "stepfun", "grok", "alibabatoken",
    ]

    /// 合并多设备快照：同 (source, label) 同秒去重、同 key 百分比取高不相加；
    /// 输出按 ts 升序。
    public static func mergeSnapshots(_ input: [RemoteAgentQuotaSnapshotDTO]) -> [AgentQuotaSample] {
        struct Key: Hashable { let source: String; let label: String; let second: Int }

        var byKey: [Key: AgentQuotaSample] = [:]
        for snap in input {
            guard syncAgents[snap.source] != nil else { continue }
            // 空白串归一（对齐桌面 previous.plan_type ||= snapshot.plan_type
            // 的语义——空串同样视为"无值"，让补值逻辑生效）
            let planType = normalized(snap.plan_type)
            let planName = normalized(snap.plan_name)
            let label = snap.label?.trimmingCharacters(in: .whitespaces) ?? ""
            let balance = snap.balance?.toLocal()
            let message = normalized(snap.message)
            let windows = snap.windows
                .filter { validKeys.contains($0.key) && isValidPct($0.used_pct) }
                .map { w in
                    QuotaWindow(key: w.key,
                                title: title(for: w.key),
                                usedPercent: w.used_pct,
                                resetsAt: w.reset_at)
                }
            // 无窗口且无任何扩展载荷的快照跳过（旧服务端/脏数据防御）
            guard !windows.isEmpty || balance != nil || planName != nil || message != nil else {
                continue
            }
            let key = Key(source: snap.source, label: label, second: snap.ts / 1000)
            guard var previous = byKey[key] else {
                byKey[key] = AgentQuotaSample(source: snap.source,
                                              label: label,
                                              ts: snap.ts,
                                              planType: planType,
                                              planName: planName,
                                              windows: windows,
                                              balance: balance,
                                              message: message)
                continue
            }
            // 同一轮多设备采样：百分比取高，plan 空则补，ts 取小，
            // 余额取较大者（保守展示），错误原因空则补
            var map: [String: QuotaWindow] = [:]
            for w in previous.windows { map[w.key] = w }
            for w in windows where isValidPct(w.usedPercent ?? -1) {
                if let old = map[w.key] {
                    if (w.usedPercent ?? 0) > (old.usedPercent ?? 0) {
                        map[w.key] = w
                    }
                } else {
                    map[w.key] = w
                }
            }
            previous.windows = map.values.sorted { $0.key < $1.key }
            if previous.planType == nil { previous.planType = planType }
            if previous.planName == nil { previous.planName = planName }
            if previous.balance == nil
                || (balance?.amount ?? 0) > (previous.balance?.amount ?? 0) {
                previous.balance = balance ?? previous.balance
            }
            if previous.message == nil { previous.message = message }
            previous.ts = min(previous.ts, snap.ts)
            byKey[key] = previous
        }
        return byKey.values.sorted { $0.ts < $1.ts }
    }

    /// 由合并序列构造展示分组：每个 (source, label) 取最新一条（桌面同步的
    /// 当前状态）。输出按固定 source 顺序、同 source 内按 label 字典序。
    public static func buildGroups(from merged: [AgentQuotaSample]) -> [SyncedAgentQuotaGroup] {
        var latest: [String: AgentQuotaSample] = [:]
        for sample in merged {
            let key = "\(sample.source)|\(sample.label)"
            if let old = latest[key] {
                if sample.ts >= old.ts { latest[key] = sample }
            } else {
                latest[key] = sample
            }
        }
        var out: [SyncedAgentQuotaGroup] = []
        for source in sourceOrder {
            let samples = merged
                .filter { $0.source == source }
                .compactMap { sample -> (String, AgentQuotaSample)? in
                    guard latest["\(sample.source)|\(sample.label)"]?.ts == sample.ts else {
                        return nil
                    }
                    return (sample.label, sample)
                }
            for (label, sample) in samples.sorted(by: { $0.0 < $1.0 }) {
                guard let agent = syncAgents[source] else { continue }
                out.append(SyncedAgentQuotaGroup(agent: agent,
                                                 label: label,
                                                 planType: sample.planType,
                                                 planName: sample.planName,
                                                 updatedAtMs: sample.ts,
                                                 windows: sample.windows,
                                                 balance: sample.balance,
                                                 message: sample.message))
            }
        }
        return out
    }

    /// 空白串归一为 nil。
    private static func normalized(_ value: String?) -> String? {
        guard let v = value?.trimmingCharacters(in: .whitespaces), !v.isEmpty else {
            return nil
        }
        return v
    }

    /// 百分比合法性（对齐 isValidUsedPct：有限且 0-100）。
    private static func isValidPct(_ v: Double) -> Bool {
        v.isFinite && v >= 0 && v <= 100
    }

    /// 窗口展示标题（对齐桌面各 provider 模块的窗口词表）。
    private static func title(for key: String) -> String {
        switch key {
        case "hour5": return "5h"
        case "weekly": return "本周"
        case "monthly": return "本月"
        case "interval": return "当前窗口"
        case "cursor_auto": return "Auto 用量"
        case "cursor_api": return "API 用量"
        case "credits": return "积分"
        case "quota": return "Token 配额"
        case "fuel": return "加油包"
        case "sub_credits": return "订阅积分"
        case "topup_credits": return "充值积分"
        default: return key
        }
    }
}
