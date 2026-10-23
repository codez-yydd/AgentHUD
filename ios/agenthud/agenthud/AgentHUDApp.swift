//
//  AgentHUDApp.swift
//  agenthud
//
//  ZCode Token 监控 · iOS 版
//  复用桌面版 ZBar 的同步服务和计费逻辑。
//

import SwiftUI
import UIKit

@main
struct AgentHUDApp: App {
    @StateObject private var settings = AppSettings.shared
    @StateObject private var refresh = RefreshService.shared
    @StateObject private var store = AccountStore.shared
    @StateObject private var tabRouter = TabRouter()

    var body: some Scene {
        WindowGroup {
            RootView()
                .environmentObject(settings)
                .environmentObject(refresh)
                .environmentObject(store)
                .environmentObject(tabRouter)
                .preferredColorScheme(.dark)
                .task {
                    // 启动即拉一次
                    refresh.refreshAll()
                    // 启动写一次 Widget 缓存，避免 Widget 显示旧数据
                    refresh.writeWidgetSnapshot()
                }
        }
    }
}

/// 全局 Tab 选中状态（从 RootView 的 @State 提升为可注入对象，
/// 供各页面「去设置」等跨 Tab 跳转使用）。
final class TabRouter: ObservableObject {
    /// DEBUG 下支持用 AGENTHUD_TAB=quota 启动到指定 Tab，方便截图/联调。
    @Published var selected: RootView.Tab = {
        #if DEBUG
        if let raw = ProcessInfo.processInfo.environment["AGENTHUD_TAB"],
           let tab = RootView.Tab(rawValue: raw) {
            return tab
        }
        #endif
        return .overview
    }()
}

/// 主导航容器
struct RootView: View {
    @EnvironmentObject var settings: AppSettings
    @EnvironmentObject var refresh: RefreshService
    @EnvironmentObject var tabRouter: TabRouter

    /// iPhone TabBar 放不下 7 个入口，对齐桌面 dashboard 的视图划分收敛为
    /// 5 个 Tab：总览 / 统计 / 额度 / 日志 / 设置（趋势并入总览的趋势卡，
    /// 周额度对比改为额度页内容区的 push 入口）。
    enum Tab: String, Hashable {
        case overview, stats, quota, logs, settings
    }

    /// 已访问过的 Tab。首次点开才创建对应页面，既避免启动时五个页面
    /// 同时发请求，又能在来回切换时保留各页滚动位置与已加载数据。
    @State private var visited: Set<Tab> = []

    private static let tabOrder: [Tab] = [.overview, .stats, .quota, .logs, .settings]

    var body: some View {
        ZStack(alignment: .bottom) {
            ZStack {
                ForEach(Self.tabOrder, id: \.self) { tab in
                    if visited.contains(tab) || tabRouter.selected == tab {
                        content(for: tab)
                            .opacity(tabRouter.selected == tab ? 1 : 0)
                            .allowsHitTesting(tabRouter.selected == tab)
                            .accessibilityHidden(tabRouter.selected != tab)
                            .zIndex(tabRouter.selected == tab ? 1 : 0)
                    }
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)

            // 底部渐隐：让滚动内容贴近悬浮 Tab 栏时淡出到背景色，
            // 避免玻璃胶囊里透出半截卡片文字。
            LinearGradient(colors: [Theme.bgBottom.opacity(0),
                                    Theme.bgBottom.opacity(0.9),
                                    Theme.bgBottom],
                           startPoint: .top, endPoint: .bottom)
                .frame(height: 124)
                .allowsHitTesting(false)
                .ignoresSafeArea(edges: .bottom)
                .zIndex(1)

            // 自绘悬浮 Tab 栏：统一 App 的玻璃质感，避免系统 TabBar 在
            // iOS 26/27 上的选中态与深色内容叠加后过重。
            HUDFloatingTabBar(selection: $tabRouter.selected)
                .frame(maxWidth: 520)
                .padding(.horizontal, 16)
                .padding(.bottom, 4)
                .zIndex(2)
        }
        .background(AppBackground())
        .onAppear { visited.insert(tabRouter.selected) }
        .onChange(of: tabRouter.selected) { _, newValue in
            visited.insert(newValue)
        }
    }

    @ViewBuilder
    private func content(for tab: Tab) -> some View {
        switch tab {
        case .overview: OverviewView()
        case .stats: StatsView()
        case .quota: QuotaView()
        case .logs: LogsView()
        case .settings: SettingsView()
        }
    }
}

// MARK: - 自绘悬浮 Tab 栏

struct HUDFloatingTabBar: View {
    @Binding var selection: RootView.Tab
    @Namespace private var selectionNS

    private struct Item {
        let tab: RootView.Tab
        let title: String
        let icon: String
    }

    private let items: [Item] = [
        Item(tab: .overview, title: "总览", icon: "square.grid.2x2.fill"),
        Item(tab: .stats, title: "统计", icon: "chart.bar.xaxis"),
        Item(tab: .quota, title: "额度", icon: "gauge.with.dots.needle.67percent"),
        Item(tab: .logs, title: "日志", icon: "list.bullet.rectangle.fill"),
        Item(tab: .settings, title: "设置", icon: "gearshape.fill"),
    ]

    var body: some View {
        HStack(spacing: 2) {
            ForEach(items, id: \.tab) { item in
                let selected = selection == item.tab
                Button {
                    select(item.tab)
                } label: {
                    VStack(spacing: 3) {
                        Image(systemName: item.icon)
                            .font(.system(size: 17, weight: .semibold))
                            .frame(height: 20)
                        Text(item.title)
                            .font(.system(size: 9.5, weight: .semibold))
                    }
                    .foregroundStyle(selected ? Theme.accent : Theme.textSecondary)
                    .frame(maxWidth: .infinity)
                    .padding(.vertical, 7)
                    .background {
                        if selected {
                            Capsule()
                                .fill(Theme.accent.opacity(0.13))
                                .overlay {
                                    Capsule()
                                        .strokeBorder(Theme.accent.opacity(0.22),
                                                      lineWidth: 1)
                                }
                                .matchedGeometryEffect(id: "selectedTab", in: selectionNS)
                        }
                    }
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityLabel(item.title)
                .accessibilityAddTraits(selected ? .isSelected : [])
            }
        }
        .padding(5)
        .background {
            Capsule()
                .fill(.ultraThinMaterial)
                .overlay {
                    Capsule()
                        .fill(LinearGradient(
                            colors: [.white.opacity(0.075), .white.opacity(0.015)],
                            startPoint: .topLeading,
                            endPoint: .bottomTrailing))
                }
        }
        .overlay {
            Capsule()
                .strokeBorder(LinearGradient(
                    colors: [.white.opacity(0.18), .white.opacity(0.04)],
                    startPoint: .topLeading,
                    endPoint: .bottomTrailing),
                    lineWidth: 1)
        }
        .shadow(color: .black.opacity(0.45), radius: 20, x: 0, y: 10)
    }

    private func select(_ tab: RootView.Tab) {
        guard selection != tab else { return }
        UIImpactFeedbackGenerator(style: .soft).impactOccurred()
        withAnimation(.spring(response: 0.32, dampingFraction: 0.84)) {
            selection = tab
        }
    }
}
