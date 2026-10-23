//
//  AgentHUDWidget.swift
//  agenthudWidget
//
//  WidgetKit 小组件入口。支持锁屏（accessoryRectangular/Circular/Inline）
//  和主屏（systemSmall/Medium/Large）。数据从 App Group 共享容器读取。
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

// MARK: - 主屏 Widget（systemSmall / systemMedium / systemLarge）

struct AgentHUDHomeWidget: Widget {
    let kind = "AgentHUDHomeWidget"

    var body: some WidgetConfiguration {
        StaticConfiguration(kind: kind, provider: AgentHUDProvider()) { entry in
            AgentHUDHomeWidgetView(entry: entry)
                .homeWidgetBackground()
        }
        .configurationDisplayName("AgentHUD 用量与额度")
        .description("今日花费、Token 与全部账号额度（HUD 圆环）")
        .supportedFamilies([.systemSmall, .systemMedium, .systemLarge])
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
        let base = currentEntry()

        // 账号多于中尺寸一页（5 个）时，按 20 分钟一轮在该页之间轮换，
        // 保证「订阅很多」时每个账号最终都能被看到，而不是被静默省略。
        // 页数上限 4（20 个账号），再多也不会有意义地增加信息量。
        let count = base.snapshot.allQuotaItems.count
        let perPage = MediumHomeView.pageSize
        let pages = max(1, min(4, Int(ceil(Double(count) / Double(perPage)))))

        let now = Date()
        var entries: [AgentHUDEntry] = []
        for i in 0..<pages {
            entries.append(AgentHUDEntry(
                date: now.addingTimeInterval(Double(i) * 20 * 60),
                snapshot: base.snapshot,
                currency: base.currency,
                stale: base.stale,
                page: i,
                pageCount: pages))
        }
        // 一轮结束后回到第 1 页重新生成（实际刷新时机由系统调度）。
        completion(Timeline(entries: entries,
                            policy: .after(now.addingTimeInterval(Double(pages) * 20 * 60))))
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
    /// 中尺寸账号分页：当前页（0 基）与总页数；1 页时恒为 0/1。
    var page: Int = 0
    var pageCount: Int = 1
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
                    WidgetBackdrop()
                }
        } else {
            self.background(WidgetBackdrop())
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

/// Widget 端与 App 一致的视觉 token（App 的 Theme 在另一个 target，不能直接引用）。
/// 小组件底色：深空底 + 两团相对尺寸的极光（青蓝偏左上、紫色偏右下），
/// 和 App 的 AppBackground 同源，又不会在小尺寸上糊成一片亮色。
struct WidgetBackdrop: View {
    var body: some View {
        GeometryReader { geo in
            let w = geo.size.width
            let h = geo.size.height
            let reach = max(w, h)

            ZStack {
                Color(red: 0.035, green: 0.055, blue: 0.086)

                RadialGradient(
                    colors: [Color(red: 0.29, green: 0.78, blue: 1.0).opacity(0.28),
                             Color(red: 0.29, green: 0.78, blue: 1.0).opacity(0)],
                    center: UnitPoint(x: 0.12, y: 0.02),
                    startRadius: 0,
                    endRadius: reach * 0.85)

                RadialGradient(
                    colors: [Color(red: 0.66, green: 0.58, blue: 1.0).opacity(0.20),
                             Color(red: 0.66, green: 0.58, blue: 1.0).opacity(0)],
                    center: UnitPoint(x: 1.0, y: 1.0),
                    startRadius: 0,
                    endRadius: reach * 0.75)
            }
            .frame(width: w, height: h)
        }
    }
}

enum WidgetTheme {
    static let mint = Color(red: 0.30, green: 0.88, blue: 0.68)
    static let amber = Color(red: 1.00, green: 0.76, blue: 0.35)
    static let coral = Color(red: 1.00, green: 0.45, blue: 0.51)
    static let secondary = Color.white.opacity(0.62)
    static let tertiary = Color.white.opacity(0.50)
    static let track = Color.white.opacity(0.09)
    static let card = Color.white.opacity(0.045)
}

/// 按百分比选颜色（与 App 端一致）
func widgetQuotaColor(_ pct: Int) -> Color {
    if pct >= 85 { return WidgetTheme.coral }
    if pct >= 60 { return WidgetTheme.amber }
    return WidgetTheme.mint
}
