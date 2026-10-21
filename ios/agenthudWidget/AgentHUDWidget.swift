//
//  AgentHUDWidget.swift
//  agenthudWidget
//
//  WidgetKit 小组件入口。支持锁屏（accessoryRectangular/Circular/Inline）
//  和主屏（systemSmall/Medium）。数据从 App Group 共享容器读取。
//  批2：两个 Widget 均改 AppIntentConfiguration，共用 SelectAccountIntent
//  （长按→编辑小组件 可选显示账号），配置一处生效两处。
//

import WidgetKit
import SwiftUI

@main
struct AgentHUDWidgetBundle: WidgetBundle {
    var body: some Widget {
        AgentHUDHomeWidget()      // 主屏/中尺寸
        AgentHUDLockScreenWidget() // 锁屏
    }
}

// MARK: - 主屏 Widget（systemSmall / systemMedium）

struct AgentHUDHomeWidget: Widget {
    let kind = "AgentHUDHomeWidget"

    var body: some WidgetConfiguration {
        AppIntentConfiguration(kind: kind,
                               intent: SelectAccountIntent.self,
                               provider: AgentHUDProvider()) { entry in
            AgentHUDHomeWidgetView(entry: entry)
                .containerBackgroundForWidget()
        }
        .configurationDisplayName("AgentHUD 用量")
        .description("今日花费、Token 与额度进度（可选显示账号）")
        .supportedFamilies([.systemSmall, .systemMedium])
    }
}

// MARK: - 锁屏 Widget（accessory 家族）

struct AgentHUDLockScreenWidget: Widget {
    let kind = "AgentHUDLockWidget"

    var body: some WidgetConfiguration {
        AppIntentConfiguration(kind: kind,
                               intent: SelectAccountIntent.self,
                               provider: AgentHUDProvider()) { entry in
            // 锁屏 accessory family 也必须用 containerBackground，
            // 否则 iOS 17+ 会有运行时警告；系统会自动渲染成单色/透明。
            AgentHUDLockScreenView(entry: entry)
                .containerBackgroundForWidget()
        }
        .configurationDisplayName("AgentHUD 额度")
        .description("锁屏快速查看额度与花费（可选显示账号）")
        .supportedFamilies([.accessoryRectangular, .accessoryCircular, .accessoryInline])
    }
}

// MARK: - Provider（AppIntentTimelineProvider：配置随 Intent 传入，
// 数据仍统一读 App Group 快照，配置本身已由 Intent.perform 持久化）

struct AgentHUDProvider: AppIntentTimelineProvider {
    func placeholder(in context: Context) -> AgentHUDEntry {
        AgentHUDEntry(date: Date(), snapshot: .preview, currency: .cny, stale: false)
    }

    func snapshot(for configuration: SelectAccountIntent,
                  in context: Context) async -> AgentHUDEntry {
        currentEntry()
    }

    func timeline(for configuration: SelectAccountIntent,
                  in context: Context) async -> Timeline<AgentHUDEntry> {
        let entry = currentEntry()
        // 每 30 分钟由系统调度一次刷新机会（实际频率由系统决定）
        let next = Date().addingTimeInterval(30 * 60)
        return Timeline(entries: [entry], policy: .after(next))
    }

    private func currentEntry() -> AgentHUDEntry {
        let snap = SharedReader.readSnapshot() ?? WidgetSnapshot.placeholder
        let currency = SharedReader.readCurrency()
        // 选中账号与快照数据不一致（Intent 刚切换、主 App 尚未重写快照）：
        // 标记 stale，视图层提示「打开 App 刷新」。Widget 进程无法自行查询
        // 额度（查询代码在主 App target），等待主 App 下一次写入。
        let selected = WidgetSharedStore.readSelectedAccountId()
        let snapshotAccount = (snap.accountId == "primary") ? nil : snap.accountId
        let stale = snap.accountId != nil && snapshotAccount != selected
        return AgentHUDEntry(date: Date(), snapshot: snap, currency: currency, stale: stale)
    }
}

// MARK: - Entry

struct AgentHUDEntry: TimelineEntry {
    let date: Date
    let snapshot: WidgetSnapshot
    let currency: Currency
    /// 选中账号与快照数据不一致（等待主 App 刷新）
    let stale: Bool
}

// MARK: - 共享读取（Widget 进程侧）

enum SharedReader {
    static let group = "group.com.chacca.agenthud"

    static func readSnapshot() -> WidgetSnapshot? {
        guard let sd = UserDefaults(suiteName: group) else { return nil }
        guard let data = sd.data(forKey: "widget.snapshot") else { return nil }
        return try? JSONDecoder().decode(WidgetSnapshot.self, from: data)
    }

    static func readCurrency() -> Currency {
        guard let sd = UserDefaults(suiteName: group) else { return .cny }
        return Currency(rawValue: sd.string(forKey: "widget.currency") ?? "cny") ?? .cny
    }
}

// MARK: - 小工具扩展

extension View {
    @ViewBuilder
    func containerBackgroundForWidget() -> some View {
        if #available(iOS 17.0, *) {
            self.containerBackground(for: .widget) {
                LinearGradient(colors: [Color.blue.opacity(0.25), Color.purple.opacity(0.15)],
                               startPoint: .topLeading, endPoint: .bottomTrailing)
            }
        } else {
            self.background(
                LinearGradient(colors: [Color.blue.opacity(0.25), Color.purple.opacity(0.15)],
                               startPoint: .topLeading, endPoint: .bottomTrailing)
            )
        }
    }
}

/// 按百分比选颜色（与 App 端一致）
func widgetQuotaColor(_ pct: Int) -> Color {
    if pct >= 85 { return .red }
    if pct >= 60 { return .orange }
    return .green
}
