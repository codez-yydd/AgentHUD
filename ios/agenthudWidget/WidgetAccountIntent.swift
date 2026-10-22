//
//  WidgetAccountIntent.swift
//  agenthudWidget
//
//  ⚠️ 小组件已改为 StaticConfiguration（见 AgentHUDWidget.swift），不再读取
//  下面的 SelectAccountIntent。账号选择统一在 App「设置 → 小组件显示」完成。
//  本文件保留 WidgetSharedStore（App Group 读写工具）与旧 Intent 定义，
//  避免和主 App 共享模块；Intent 本身不参与 Widget 配置。
//
//  数据流（与主 App Settings.swift / AccountStore.swift / RefreshService.swift
//  约定的 App Group key 保持一致；两 target 不共享模块，常量各自维护同值）：
//   1. 主 App 把账号清单摘要写入 App Group "widget.accountOptions"；
//   2. EntityQuery 从 App Group 读清单构造动态选项（含固定「跟随主账号」项）；
//   3. 用户在编辑界面选择 → Intent.perform() 把选中 id 写入 App Group
//      "widget.selectedAccountId" 并 reload 全部 timeline；
//   4. 主 App RefreshService.writeWidgetSnapshot 读该 key 按选中账号写快照
//      （选中被删/为空回退主账号）；设置页的「小组件显示」Picker 写同一
//      key，双向联动。
//
//  ⚠️ 本文件【只加入 Widget target】，不要加入主 App target
//  （主 App 侧的对应类型在 Settings.swift，重复定义会编译冲突）。
//

import Foundation
import AppIntents
import WidgetKit

// MARK: - App Group 共享读写

/// Widget 进程侧的 App Group 访问（与 AgentHUDWidget.swift 的 SharedReader
/// 同容器；账号选项与选中值两个 key 在此集中管理）。
enum WidgetSharedStore {
    static let group = "group.com.chacca.agenthud"
    static let accountOptionsKey = "widget.accountOptions"
    static let selectedAccountIdKey = "widget.selectedAccountId"

    static var defaults: UserDefaults? {
        UserDefaults(suiteName: group)
    }

    /// 账号选项摘要（与主 App WidgetAccountOption 字段一致）。
    struct AccountOption: Codable, Identifiable, Hashable {
        var id: String
        var agent: String
        var label: String
        var kind: String   // "direct" | "sync"
        var planName: String?
    }

    /// 读账号清单（App Group 未写入/损坏时返回空数组，Intent 只剩固定项）。
    static func readOptions() -> [AccountOption] {
        guard let sd = defaults,
              let data = sd.data(forKey: accountOptionsKey) else { return [] }
        return (try? JSONDecoder().decode([AccountOption].self, from: data)) ?? []
    }

    /// 读选中账号 id（"primary" 归一为 nil，语义 = 跟随主账号）。
    static func readSelectedAccountId() -> String? {
        let value = defaults?.string(forKey: selectedAccountIdKey)
        return (value == "primary") ? nil : value
    }

    /// 写选中账号 id（nil 统一落 "primary"，避免缺 key 与显式跟随语义混淆）。
    static func writeSelectedAccountId(_ id: String?) {
        guard let sd = defaults else { return }
        let value = (id?.isEmpty == false) ? id! : "primary"
        sd.set(value, forKey: selectedAccountIdKey)
    }

    /// agent rawValue → 展示名（Intent 副标题用；与主 App AgentKind.displayName 一致）。
    static func agentDisplayName(_ raw: String) -> String {
        switch raw {
        case "zai": return "GLM Coding Plan"
        case "deepseek": return "DeepSeek"
        case "moonshot": return "Kimi 开放平台"
        case "minimax": return "MiniMax"
        case "opencodego": return "OpenCode Go"
        case "alibaba": return "阿里 Coding Plan"
        case "codex": return "Codex"
        case "claude": return "Claude"
        case "cursor": return "Cursor"
        case "kimi": return "Kimi Code"
        case "qoder": return "Qoder"
        case "longcat": return "LongCat"
        case "mimo": return "MiMo"
        case "stepfun": return "StepFun"
        case "grok": return "Grok"
        case "alibabatoken": return "百炼Token包"
        default: return raw
        }
    }
}

// MARK: - AppEntity（账号选项实体）

/// Widget 配置里的可选账号实体。id 约定：
/// "primary"（跟随主账号）/ 直连账号 UUID / "sync:<agent>"（只读同步分组）。
struct AccountEntity: AppEntity {
    static let typeDisplayRepresentation: TypeDisplayRepresentation =
        TypeDisplayRepresentation(name: "额度账号")
    static let defaultQuery = AccountEntityQuery()

    let id: String
    let label: String
    let agentName: String
    /// "primary" | "direct" | "sync"
    let kind: String
    /// 最近一次查询的套餐名（副标题附带；可无）
    let planName: String?

    var displayRepresentation: DisplayRepresentation {
        var subtitle: LocalizedStringResource
        switch kind {
        case "primary":
            subtitle = "跟随主账号（默认）"
        case "sync":
            subtitle = "\(agentName) · 桌面同步"
        default:
            subtitle = "\(agentName)"
        }
        if let plan = planName, !plan.isEmpty {
            subtitle = "\(subtitle) · \(plan)"
        }
        return DisplayRepresentation(title: "\(label)", subtitle: subtitle)
    }

    /// 固定的「跟随主账号」项（不落 App Group，Query 端构造）。
    static let primaryEntity = AccountEntity(id: "primary",
                                             label: "跟随主账号",
                                             agentName: "",
                                             kind: "primary",
                                             planName: nil)
}

extension WidgetSharedStore.AccountOption {
    /// 摘要 → Intent 实体。
    var entity: AccountEntity {
        AccountEntity(id: id,
                      label: label,
                      agentName: WidgetSharedStore.agentDisplayName(agent),
                      kind: kind,
                      planName: planName)
    }
}

// MARK: - EntityQuery（动态选项）

struct AccountEntityQuery: EntityQuery {
    func entities(for identifiers: [String]) async throws -> [AccountEntity] {
        // 系统回填已选项时调用：按 id 过滤（含固定项，保证旧配置可解析）
        let all = [AccountEntity.primaryEntity]
            + WidgetSharedStore.readOptions().map { $0.entity }
        return all.filter { identifiers.contains($0.id) }
    }

    func suggestedEntities() async throws -> [AccountEntity] {
        [AccountEntity.primaryEntity]
            + WidgetSharedStore.readOptions().map { $0.entity }
    }

    func defaultEntity() async -> AccountEntity? {
        AccountEntity.primaryEntity
    }
}

// MARK: - Widget 配置 Intent（主屏 + 锁屏共用）

struct SelectAccountIntent: WidgetConfigurationIntent {
    static var title: LocalizedStringResource = "选择账号"
    static var description = IntentDescription(
        "小组件显示的额度账号（今日花费与 Token 恒为本机数据）。")

    @Parameter(title: "账号", default: nil)
    var account: AccountEntity?

    /// 配置完成即把选中 id 写入 App Group（主 App 下次写快照时读取），
    /// 并 reload 全部 timeline 让 Widget 立即按当前快照重绘。
    func perform() async throws -> some IntentResult {
        WidgetSharedStore.writeSelectedAccountId(account?.id)
        WidgetCenter.shared.reloadAllTimelines()
        return .result()
    }
}
