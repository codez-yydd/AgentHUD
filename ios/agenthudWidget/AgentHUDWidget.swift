//
//  AgentHUDWidget.swift
//  agenthudWidget
//
//  WidgetKit 小组件入口。支持锁屏（accessoryRectangular/Circular/Inline）
//  和主屏（systemSmall/Medium）。数据从 App Group 共享容器读取。
//
//  ⚠️ 使用 StaticConfiguration 而不是 AppIntentConfiguration：
//  小组件的账号选择统一放到 App「设置 → 小组件显示」里（写 App Group），
//  这里只按快照渲染。AppIntent 配置在部分真机/系统版本上会出现扩展无法
//  生成 timeline、桌面只显示占位图的问题；静态配置更稳，也少一层配置状态。
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
        StaticConfiguration(kind: kind, provider: AgentHUDProvider()) { entry in
            AgentHUDHomeWidgetView(entry: entry)
                .homeWidgetBackground()
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
        StaticConfiguration(kind: kind, provider: AgentHUDProvider()) { entry in
            // 锁屏 accessory family 也必须用 containerBackground，
            // 否则 iOS 17+ 会有运行时警告；系统会自动渲染成单色/透明。
            AgentHUDLockScreenView(entry: entry)
                .lockScreenWidgetBackground()
        }
        .configurationDisplayName("AgentHUD 额度")
        .description("锁屏快速查看额度与花费（可选显示账号）")
        .supportedFamilies([.accessoryRectangular, .accessoryCircular, .accessoryInline])
    }
}

// MARK: - Provider（静态配置：账号由主 App 设置页写 App Group，Provider
// 只读快照与选中账号；不再依赖 AppIntent 配置）

struct AgentHUDProvider: TimelineProvider {
    func placeholder(in context: Context) -> AgentHUDEntry {
        AgentHUDEntry(date: Date(), snapshot: .preview, currency: .cny, stale: false)
    }

    func getSnapshot(in context: Context,
                     completion: @escaping (AgentHUDEntry) -> Void) {
        completion(currentEntry())
    }

    func getTimeline(in context: Context,
                     completion: @escaping (Timeline<AgentHUDEntry>) -> Void) {
        let entry = currentEntry()
        // 每 30 分钟由系统调度一次刷新机会（实际频率由系统决定）。
        // 主 App 每次刷新后会主动 WidgetCenter.reloadAllTimelines()，
        // 所以这里只是兜底。
        let next = Date().addingTimeInterval(30 * 60)
        completion(Timeline(entries: [entry], policy: .after(next)))
    }

    private func currentEntry() -> AgentHUDEntry {
        // 读不到 App Group 快照时退回空数据（而不是示例价格），
        // 视图会提示「打开 App 同步」，避免把 12.34 之类的演示值当真。
        let snap = SharedReader.readSnapshot() ?? WidgetSnapshot.empty
        let currency = SharedReader.readCurrency()
        // 选中账号与快照数据不一致（Intent 刚切换、主 App 尚未重写快照）：
        // 标记 stale，视图层提示「打开 App 刷新」。Widget 进程无法自行查询
        // 额度（查询代码在主 App target），等待主 App 下一次写入。
        let selected = WidgetSharedStore.readSelectedAccountId()
        // 未显式选账号（跟随主账号/自动回退到第一个同步分组）时不算 stale；
        // 只有用户选了 A、快照还是 B 时才提示打开 App 刷新。
        let stale: Bool
        if let selected, !selected.isEmpty, selected != "primary" {
            stale = snap.accountId != selected
        } else {
            stale = false
        }
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
    /// 主屏小组件：与 App 一致的深空底 + 青蓝/紫色极光，强制深色文字。
    @ViewBuilder
    func homeWidgetBackground() -> some View {
        if #available(iOS 17.0, *) {
            self.environment(\.colorScheme, .dark)
                .containerBackground(for: .widget) {
                    ZStack {
                        Color(red: 0.035, green: 0.055, blue: 0.086)
                        LinearGradient(
                            colors: [Color(red: 0.29, green: 0.78, blue: 1.0).opacity(0.20),
                                     Color(red: 0.66, green: 0.58, blue: 1.0).opacity(0.10)],
                            startPoint: .topLeading, endPoint: .bottomTrailing)
                    }
                }
        } else {
            self.background(
                ZStack {
                    Color(red: 0.035, green: 0.055, blue: 0.086)
                    LinearGradient(
                        colors: [Color(red: 0.29, green: 0.78, blue: 1.0).opacity(0.20),
                                 Color(red: 0.66, green: 0.58, blue: 1.0).opacity(0.10)],
                        startPoint: .topLeading, endPoint: .bottomTrailing)
                })
        }
    }

    /// 锁屏 accessory 家族：交给系统渲染，容器保持透明（不要深色底板）。
    @ViewBuilder
    func lockScreenWidgetBackground() -> some View {
        if #available(iOS 17.0, *) {
            self.containerBackground(for: .widget) { Color.clear }
        } else {
            self
        }
    }
}

/// 按百分比选颜色（与 App 端一致）
func widgetQuotaColor(_ pct: Int) -> Color {
    if pct >= 85 { return .red }
    if pct >= 60 { return .orange }
    return .green
}
