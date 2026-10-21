//
//  AgentQuotaSync.swift
//  agenthud
//
//  只读同步型额度（Codex / Claude / Cursor）：从自托管 server 拉桌面端已上传
//  的额度快照（GET /agent-quota-snapshots），合并多设备采样后按 source 分组
//  供额度页展示。合并语义翻译自桌面 src/agentQuota.ts:46-85：
//   - 同秒去重（各设备同一轮采样时间相差几毫秒）；
//   - 同 key 百分比取较高值不相加（额度百分比不相加，取高避免低估）；
//   - plan_type 空则补，ts 取较小值。
//  iOS 没有本机实时锚点，不做桌面 filterInflatedRemoteCursorSnapshots 的
//  Cursor 失真过滤（桌面在锚点缺失时也是原样返回，宁缺毋滥）。
//  服务端不支持该接口（404/空）时调用方降级为隐藏该分组，不报错。
//

import Foundation

/// 合并后的单条采样（已按展示形状归一）。
public struct AgentQuotaSample: Hashable {
    public var source: String
    public var ts: Int
    public var planType: String?
    public var windows: [QuotaWindow]
}

/// 只读同步型的一个展示分组（一个 source 一组，取最新一条采样）。
public struct SyncedAgentQuotaGroup: Identifiable {
    public var agent: AgentKind
    public var planType: String?
    /// 最近一次采样时间（ms）
    public var updatedAtMs: Int
    public var windows: [QuotaWindow]

    public var id: String { agent.rawValue }
}

public enum AgentQuotaSync {

    /// 各 source 允许的窗口 key（对齐桌面 AGENT_QUOTA_WINDOWS）。
    private static let validKeys: Set<String> = [
        "hour5", "weekly", "cursor_auto", "cursor_api",
    ]

    /// iOS 侧消费的同步型 source → AgentKind。
    private static let syncAgents: [String: AgentKind] = [
        "codex": .codex,
        "claude": .claude,
        "cursor": .cursor,
    ]

    /// 合并多设备快照：同秒去重、同 key 百分比取高不相加；输出按 ts 升序。
    public static func mergeSnapshots(_ input: [RemoteAgentQuotaSnapshotDTO]) -> [AgentQuotaSample] {
        struct Key: Hashable { let source: String; let second: Int }

        var byKey: [Key: AgentQuotaSample] = [:]
        for snap in input {
            guard syncAgents[snap.source] != nil else { continue }
            // 空白串归一为 nil（对齐桌面 previous.plan_type ||= snapshot.plan_type
            // 的语义——空串同样视为"无值"，让补值逻辑生效）
            let planType: String? = {
                guard let p = snap.plan_type?
                    .trimmingCharacters(in: .whitespaces), !p.isEmpty else {
                    return nil
                }
                return p
            }()
            let windows = snap.windows
                .filter { validKeys.contains($0.key) && isValidPct($0.used_pct) }
                .map { w in
                    QuotaWindow(key: w.key,
                                title: title(for: w.key),
                                usedPercent: w.used_pct,
                                resetsAt: w.reset_at)
                }
            guard !windows.isEmpty else { continue }
            let key = Key(source: snap.source, second: snap.ts / 1000)
            guard var previous = byKey[key] else {
                byKey[key] = AgentQuotaSample(source: snap.source,
                                              ts: snap.ts,
                                              planType: planType,
                                              windows: windows)
                continue
            }
            // 同一轮多设备采样：百分比取高，plan_type 空则补，ts 取小
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
            previous.ts = min(previous.ts, snap.ts)
            byKey[key] = previous
        }
        return byKey.values.sorted { $0.ts < $1.ts }
    }

    /// 由合并序列构造展示分组：每个 source 取最新一条（桌面同步的当前状态）。
    public static func buildGroups(from merged: [AgentQuotaSample]) -> [SyncedAgentQuotaGroup] {
        var latest: [String: AgentQuotaSample] = [:]
        for sample in merged {
            if let old = latest[sample.source] {
                if sample.ts >= old.ts { latest[sample.source] = sample }
            } else {
                latest[sample.source] = sample
            }
        }
        // 固定顺序输出：codex → claude → cursor
        return ["codex", "claude", "cursor"].compactMap { source in
            guard let sample = latest[source],
                  let agent = syncAgents[source] else { return nil }
            return SyncedAgentQuotaGroup(agent: agent,
                                         planType: sample.planType,
                                         updatedAtMs: sample.ts,
                                         windows: sample.windows)
        }
    }

    /// 百分比合法性（对齐 isValidUsedPct：有限且 0-100）。
    private static func isValidPct(_ v: Double) -> Bool {
        v.isFinite && v >= 0 && v <= 100
    }

    /// 窗口展示标题（对齐桌面 QuotaEntryCard 的窗口词表）。
    private static func title(for key: String) -> String {
        switch key {
        case "hour5": return "5h"
        case "weekly": return "本周"
        case "cursor_auto": return "Auto 用量"
        case "cursor_api": return "API 用量"
        default: return key
        }
    }
}
