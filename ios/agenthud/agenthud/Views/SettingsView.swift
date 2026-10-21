//
//  SettingsView.swift
//  agenthud
//
//  设置：同步服务注册、价格编辑、额度 Token、货币、高峰期配置。
//  注册流程复用桌面端的 Master Token → Device Token 流程。
//

import SwiftUI

struct SettingsView: View {
    @EnvironmentObject var settings: AppSettings
    @EnvironmentObject var refresh: RefreshService

    var body: some View {
        NavigationStack {
            Form {
                Section("同步服务") {
                    SyncConfigRow()
                }
                Section("额度账号") {
                    AccountsSection()
                }
                Section("小组件显示") {
                    WidgetDisplayPickerRow()
                }
                Section("显示") {
                    CurrencyRow()
                }
                Section {
                    NavigationLink("价格配置") { PricingEditView() }
                    NavigationLink("高峰期折算") { PeakEditView() }
                }
                Section {
                    Button("立即刷新全部数据") {
                        refresh.refreshAll()
                    }
                    .foregroundColor(.accentColor)
                }
                Section("关于") {
                    HStack {
                        Text("版本")
                        Spacer()
                        Text("0.1.0 (iOS)").foregroundColor(.secondary)
                    }
                    Text("桌面版 ZBar 的 iOS 配套应用。用量数据通过自托管同步服务获取；额度支持 GLM / DeepSeek / Kimi / MiniMax / OpenCode / 阿里直连账号，Codex / Claude / Cursor 由桌面端同步。")
                        .font(.caption)
                        .foregroundColor(.secondary)
                }
            }
            .navigationTitle("设置")
        }
    }
}

// MARK: - 同步服务配置

private struct SyncConfigRow: View {
    @EnvironmentObject var settings: AppSettings
    @EnvironmentObject var refresh: RefreshService

    @State private var editing = false
    @State private var serverURL = ""
    @State private var masterToken = ""
    @State private var deviceName = "iPhone"
    @State private var registering = false
    @State private var error: String?
    @State private var showDisconnect = false

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            if settings.isSyncConfigured {
                HStack {
                    Image(systemName: "checkmark.circle.fill").foregroundColor(.green)
                    Text("已连接").font(.subheadline.weight(.medium))
                    Spacer()
                    Text(settings.sync.device_name).foregroundColor(.secondary).font(.caption)
                }
                Text(settings.sync.server_url)
                    .font(.caption.monospaced())
                    .foregroundColor(.secondary)
                    .lineLimit(1)
                Text("设备 ID：\(settings.sync.device_id.prefix(8))…")
                    .font(.caption2).foregroundColor(.secondary)

                HStack {
                    Button("重新注册…") { editing = true }
                    Spacer()
                    Button("断开连接", role: .destructive) { showDisconnect = true }
                }
                .font(.caption)
            } else {
                Text("未连接")
                    .font(.subheadline)
                    .foregroundColor(.orange)
                Button("连接同步服务…") { editing = true }
            }
        }
        .sheet(isPresented: $editing) {
            RegisterSheet(serverURL: $serverURL,
                          masterToken: $masterToken,
                          deviceName: $deviceName,
                          registering: $registering,
                          error: $error,
                          onRegister: doRegister)
                .presentationDetents([.medium])
                .onAppear {
                    serverURL = settings.sync.server_url.isEmpty
                        ? "http://192.168.1.100:3838" : settings.sync.server_url
                    deviceName = settings.sync.device_name.isEmpty ? "iPhone" : settings.sync.device_name
                }
        }
        .confirmationDialog("断开连接后需要重新注册。确定？",
                            isPresented: $showDisconnect) {
            Button("断开", role: .destructive) {
                settings.sync.enabled = false
                settings.sync.server_url = ""
                settings.sync.device_token = ""
                settings.sync.device_id = ""
                settings.saveSync()
            }
        }
    }

    private func doRegister() {
        registering = true
        error = nil
        Task {
            do {
                let resp = try await APIClient.shared.register(
                    serverURL: serverURL,
                    masterToken: masterToken,
                    deviceName: deviceName)
                await MainActor.run {
                    settings.sync.enabled = true
                    settings.sync.server_url = serverURL
                    settings.sync.device_id = resp.device_id
                    settings.sync.device_token = resp.device_token
                    settings.sync.device_name = resp.device_name
                    settings.saveSync()
                    registering = false
                    editing = false
                    refresh.refreshAll()
                }
            } catch {
                await MainActor.run {
                    self.error = error.localizedDescription
                    registering = false
                }
            }
        }
    }
}

private struct RegisterSheet: View {
    @Binding var serverURL: String
    @Binding var masterToken: String
    @Binding var deviceName: String
    @Binding var registering: Bool
    @Binding var error: String?
    let onRegister: () -> Void

    var body: some View {
        NavigationStack {
            Form {
                Section("服务器") {
                    TextField("http://IP:3838", text: $serverURL)
                        .keyboardType(.URL)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                }
                Section {
                    SecureField("Master Token", text: $masterToken)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                    TextField("设备名", text: $deviceName)
                }
                if let err = error {
                    Section { Text(err).foregroundColor(.red).font(.caption) }
                }
            }
            .navigationTitle("连接同步服务")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("取消") { error = nil }
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button(registering ? "注册中…" : "连接并注册") {
                        onRegister()
                    }
                    .disabled(serverURL.isEmpty || masterToken.isEmpty || deviceName.isEmpty || registering)
                }
            }
        }
    }
}

// MARK: - 额度账号（多 agent 多账号）

/// 额度账号区块：账号列表（agent + label + 最近校验徽章 + 掩码）、
/// 添加/编辑/删除、「查询全部」。secret 只显示掩码，留空 = 不修改。
private struct AccountsSection: View {
    @EnvironmentObject var settings: AppSettings
    @EnvironmentObject var refresh: RefreshService
    @EnvironmentObject var store: AccountStore

    @State private var editingAccount: AccountEntry?
    @State private var showAdd = false
    @State private var pendingDelete: AccountEntry?

    var body: some View {
        // 直连账号列表（旧版单 GLM 账号已自动迁移为第一个 zai 账号）
        ForEach(store.accounts) { account in
            Button {
                editingAccount = account
            } label: {
                AccountRow(account: account,
                           isWidgetSelected: store.selectedWidgetAccountId == account.id.uuidString,
                           onDelete: { pendingDelete = account },
                           onSetWidget: {
                               store.selectedWidgetAccountId = account.id.uuidString
                               refresh.writeWidgetSnapshot()
                           })
            }
            .buttonStyle(.plain)
        }
        if store.accounts.isEmpty {
            Text("未配置额度账号。支持 GLM / DeepSeek / Kimi / MiniMax / OpenCode / 阿里直连账号。")
                .font(.caption)
                .foregroundColor(.secondary)
        }
        Button {
            showAdd = true
        } label: {
            Label("添加账号…", systemImage: "plus.circle")
        }
        Button {
            Task { await refresh.refreshQuotaOnly() }
        } label: {
            if refresh.quotaRefreshing {
                ProgressView().controlSize(.small)
            } else {
                Label("查询全部", systemImage: "arrow.clockwise")
            }
        }
        .disabled(!store.hasDirectAccounts || refresh.quotaRefreshing)
        .sheet(isPresented: $showAdd) {
            AccountEditSheet(editing: nil)
        }
        .sheet(item: $editingAccount) { account in
            AccountEditSheet(editing: account)
        }
        .confirmationDialog(
            "删除账号「\(pendingDelete?.label ?? "")」？该操作不可撤销。",
            isPresented: Binding(
                get: { pendingDelete != nil },
                set: { if !$0 { pendingDelete = nil } }),
            titleVisibility: .visible
        ) {
            Button("删除", role: .destructive) {
                if let account = pendingDelete {
                    store.remove(id: account.id)
                    Task { await refresh.refreshQuotaOnly() }
                }
                pendingDelete = nil
            }
        }
    }
}

/// 单个账号行：agent 名 + 备注名 + 最近校验徽章 + secret 掩码。
/// onDelete 由父视图承接（弹确认框后调 store.remove）；
/// onSetWidget 把该账号设为小组件显示账号。
private struct AccountRow: View {
    @EnvironmentObject var store: AccountStore
    let account: AccountEntry
    var isWidgetSelected: Bool = false
    let onDelete: () -> Void
    var onSetWidget: (() -> Void)? = nil

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 8) {
                Text(account.agent.displayName)
                    .font(.subheadline.weight(.medium))
                if let region = account.region {
                    Tag(region.displayName)
                }
                if isWidgetSelected {
                    // 小组件选中标记（与「小组件显示」Picker 联动）
                    Label("小组件", systemImage: "square.grid.2x2.fill")
                        .font(.caption2)
                        .padding(.horizontal, 6).padding(.vertical, 2)
                        .background(Capsule().fill(Color.blue.opacity(0.15)))
                        .foregroundColor(.blue)
                }
                Spacer()
                if let check = account.lastCheck {
                    StatusBadge(status: check.status)
                } else {
                    StatusBadge(status: "pending")
                }
            }
            HStack(spacing: 8) {
                Text(account.label)
                    .font(.caption)
                    .foregroundColor(.secondary)
                if let masked = maskedSecret {
                    Text(masked)
                        .font(.caption.monospaced())
                        .foregroundColor(.secondary)
                }
                Spacer()
                if let check = account.lastCheck, let at = check.at {
                    Text(Self.timeText(at))
                        .font(.caption2)
                        .foregroundColor(.secondary)
                }
            }
        }
        .contentShape(Rectangle())
        .swipeActions(edge: .trailing, allowsFullSwipe: false) {
            Button(role: .destructive, action: onDelete) {
                Label("删除", systemImage: "trash")
            }
        }
        .contextMenu {
            if let onSetWidget {
                Button(action: onSetWidget) {
                    Label(isWidgetSelected ? "正在小组件显示" : "设为小组件显示",
                          systemImage: "square.grid.2x2")
                }
            }
            Button(role: .destructive, action: onDelete) {
                Label("删除账号", systemImage: "trash")
            }
        }
    }

    /// 掩码走 AccountStore 缓存（增删改时失效），body 内不逐帧查 Keychain。
    /// 契约不变：secret 永不回显原文，掩码是设计上有意暴露的形态。
    private var maskedSecret: String? {
        store.cachedMask(for: account.id)
    }

    private static func timeText(_ ms: Int) -> String {
        let f = DateFormatter()
        f.dateFormat = "MM-dd HH:mm"
        return f.string(from: Date(timeIntervalSince1970: TimeInterval(ms) / 1000))
    }
}

// MARK: - 小组件显示选择（批2）

/// 「小组件显示」区块：选择 Widget 展示的额度账号（跟随主账号 / 任一直连
/// 账号 / 只读同步分组）。选中值写 App Group，与 Widget 编辑界面的
/// AppIntent 双向联动——任一处修改，两处配置同步生效。
private struct WidgetDisplayPickerRow: View {
    @EnvironmentObject var refresh: RefreshService
    @EnvironmentObject var store: AccountStore

    /// Picker 绑定用非 Optional："" = 跟随主账号（nil 的展示形态）。
    private var selection: Binding<String> {
        Binding(
            get: { store.selectedWidgetAccountId ?? "" },
            set: { newValue in
                store.selectedWidgetAccountId = newValue.isEmpty ? nil : newValue
                // 立即按新选中重写快照（数据已在内存，无需重新查询）
                refresh.writeWidgetSnapshot()
            })
    }

    var body: some View {
        Picker("显示账号", selection: selection) {
            Text("跟随主账号").tag("")
            ForEach(store.accounts) { account in
                Text("\(account.agent.displayName) · \(account.label)")
                    .tag(account.id.uuidString)
            }
            ForEach([AgentKind.codex, .claude, .cursor], id: \.rawValue) { agent in
                Text("\(agent.displayName)（桌面同步）")
                    .tag(AccountStore.syncOptionId(agent))
            }
        }
        if store.accounts.isEmpty {
            Text("未配置额度账号时小组件额度区为空。")
                .font(.caption)
                .foregroundColor(.secondary)
        } else {
            Text("也可长按桌面/锁屏小组件，在「编辑小组件」中选择账号。")
                .font(.caption)
                .foregroundColor(.secondary)
        }
    }
}

/// 添加 / 编辑账号表单。编辑时 secret 只显示掩码，输入框留空 = 不修改。
private struct AccountEditSheet: View {
    @EnvironmentObject var refresh: RefreshService
    @EnvironmentObject var store: AccountStore
    @Environment(\.dismiss) private var dismiss

    /// nil = 新增
    let editing: AccountEntry?

    @State private var agent: AgentKind = .zai
    @State private var label = ""
    @State private var region: AccountRegion = .cn
    @State private var token = ""
    @State private var checkNow = true
    @State private var checking = false
    @State private var checkResult: String?

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    Picker("服务", selection: $agent) {
                        ForEach(AgentKind.allCases.filter { $0.isDirect }) { kind in
                            Text(kind.displayName).tag(kind)
                        }
                    }
                    .disabled(editing != nil) // 凭证一经创建不改服务（对齐桌面）
                    TextField("备注名（可留空）", text: $label)
                    if agent.supportsRegion {
                        Picker("区域", selection: $region) {
                            ForEach(AccountRegion.allCases) { r in
                                Text(r.displayName).tag(r)
                            }
                        }
                    }
                }
                Section(agent.displayName + " 凭证") {
                    if let masked = editingMaskedSecret {
                        Text("当前：\(masked)")
                            .font(.caption.monospaced())
                            .foregroundColor(.secondary)
                    }
                    SecureField(editing == nil ? agent.tokenPlaceholder : "留空表示不修改",
                                text: $token)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                }
                if editing == nil {
                    Section {
                        Toggle("保存后立即查询验证", isOn: $checkNow)
                    }
                }
                if checking {
                    Section {
                        HStack {
                            ProgressView().controlSize(.small)
                            Text("查询中…").font(.caption).foregroundColor(.secondary)
                        }
                    }
                }
                if let result = checkResult {
                    Section {
                        Text(result)
                            .font(.caption)
                            .foregroundColor(.secondary)
                    }
                }
            }
            .navigationTitle(editing == nil ? "添加账号" : "编辑账号")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("取消") { dismiss() }
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button(editing == nil ? "添加" : "保存") {
                        Task { await save() }
                    }
                    .disabled(!canSave || checking)
                }
            }
        }
        .onAppear { loadInitial() }
    }

    private var canSave: Bool {
        if editing == nil {
            return !token.trimmingCharacters(in: .whitespaces).isEmpty
        }
        return true // 编辑至少可改 label/region（secret 留空=不修改）
    }

    /// 编辑时的掩码（走 AccountStore 缓存；原文只在缓存未命中时瞬时读取，
    /// 不落任何 @State）。
    private var editingMaskedSecret: String? {
        guard let editing else { return nil }
        return store.cachedMask(for: editing.id)
    }

    private func loadInitial() {
        guard let editing else { return }
        agent = editing.agent
        label = editing.label
        region = editing.region ?? .cn
        checkNow = false
    }

    private func save() async {
        let trimmedToken = token.trimmingCharacters(in: .whitespacesAndNewlines)
        if let editing {
            store.update(id: editing.id,
                         label: label,
                         region: region,
                         secret: trimmedToken.isEmpty ? nil : trimmedToken)
            Task { await refresh.refreshQuotaOnly() }
            dismiss()
            return
        }
        // 新增
        let entry = store.add(agent: agent, label: label, region: region,
                              secret: trimmedToken)
        guard checkNow else {
            Task { await refresh.refreshQuotaOnly() }
            dismiss()
            return
        }
        // 立即查询单账号验证（结果回写 lastCheck，消息展示在表单内）
        checking = true
        let result = await AgentQuotaClients.shared.query(entry, secret: trimmedToken)
        store.recordCheck(id: entry.id,
                          status: result.status == "ok" ? "ok" : "error",
                          message: result.status == "ok" ? nil : result.message)
        checkResult = result.status == "ok"
            ? "查询成功：\(successSummary(result))"
            : "保存成功，但查询失败：\(result.message ?? "未知原因")"
        checking = false
        Task { await refresh.refreshQuotaOnly() }
        // 展示结果后由用户手动关闭（保留 dismiss 由用户触发，能看到校验结论）
    }

    /// 成功时的简要摘要（窗口数 / 余额）。
    private func successSummary(_ entry: QuotaEntry) -> String {
        if let balance = entry.balance {
            return "余额 \(balance.currency)\(String(format: "%.2f", balance.amount))"
        }
        return "\(entry.windows.count) 个额度窗口"
    }
}

// MARK: - 货币切换

private struct CurrencyRow: View {
    @EnvironmentObject var settings: AppSettings

    var body: some View {
        Picker("显示货币", selection: Binding(
            get: { settings.currency },
            set: {
                settings.currency = $0
                settings.saveCurrency()
            })) {
            Text("人民币 ¥").tag(Currency.cny)
            Text("美元 $").tag(Currency.usd)
        }
        .pickerStyle(.segmented)
    }
}

// MARK: - 价格编辑

private struct PricingEditView: View {
    @EnvironmentObject var settings: AppSettings
    @State private var currency: Currency = .cny
    @State private var newModel = ""
    @State private var newInput = ""
    @State private var newOutput = ""
    @State private var newCacheRead = ""

    var body: some View {
        Form {
            Picker("货币", selection: $currency) {
                Text("¥ CNY").tag(Currency.cny)
                Text("$ USD").tag(Currency.usd)
            }
            .pickerStyle(.segmented)

            Section("已配置模型") {
                let map = (currency == .cny) ? settings.pricing.cny : settings.pricing.usd
                if map.isEmpty {
                    Text("尚无配置，下方添加").foregroundColor(.secondary)
                }
                ForEach(map.keys.sorted(), id: \.self) { id in
                    PricingRow(currency: currency, modelId: id) { newPrice in
                        if currency == .cny { settings.pricing.cny[id] = newPrice }
                        else { settings.pricing.usd[id] = newPrice }
                        settings.savePricing()
                    } onDelete: {
                        if currency == .cny { settings.pricing.cny.removeValue(forKey: id) }
                        else { settings.pricing.usd.removeValue(forKey: id) }
                        settings.savePricing()
                    }
                }
            }

            Section("新增模型") {
                TextField("model_id（如 glm-4.6）", text: $newModel)
                    .textInputAutocapitalization(.never)
                HStack {
                    numField("入", $newInput)
                    numField("出", $newOutput)
                    numField("缓存", $newCacheRead)
                }
                Button("添加") {
                    guard let i = Double(newInput), let o = Double(newOutput),
                          let c = Double(newCacheRead), !newModel.isEmpty else { return }
                    let price = ModelPrice(input: i, output: o, cache_read: c)
                    if currency == .cny { settings.pricing.cny[newModel] = price }
                    else { settings.pricing.usd[newModel] = price }
                    settings.savePricing()
                    newModel = ""; newInput = ""; newOutput = ""; newCacheRead = ""
                }
                .disabled(newModel.isEmpty)
            }

            Section {
                Button("重置为内置默认表") {
                    settings.pricing = AppSettings.loadBuiltinPricingDefaults()
                    settings.savePricing()
                }
                .foregroundColor(.orange)
            }
        }
        .navigationTitle("价格配置")
        .navigationBarTitleDisplayMode(.inline)
    }

    private func numField(_ label: String, _ text: Binding<String>) -> some View {
        VStack(alignment: .leading) {
            Text(label).font(.caption).foregroundColor(.secondary)
            TextField("0", text: text)
                .keyboardType(.decimalPad)
        }
    }
}

private struct PricingRow: View {
    let currency: Currency
    let modelId: String
    let onSave: (ModelPrice) -> Void
    let onDelete: () -> Void

    @State private var input = ""
    @State private var output = ""
    @State private var cacheRead = ""

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack {
                Text(modelId).font(.subheadline.weight(.semibold))
                Spacer()
                Button(role: .destructive, action: onDelete) {
                    Image(systemName: "trash")
                }
                .buttonStyle(.borderless)
            }
            HStack {
                numField("入", $input)
                numField("出", $output)
                numField("缓存", $cacheRead)
                Button("存") {
                    guard let i = Double(input), let o = Double(output), let c = Double(cacheRead) else { return }
                    onSave(ModelPrice(input: i, output: o, cache_read: c))
                }
                .buttonStyle(.bordered)
                .controlSize(.small)
            }
        }
        .onAppear {
            let map = (currency == .cny)
                ? AppSettings.shared.pricing.cny : AppSettings.shared.pricing.usd
            if let p = map[modelId] {
                input = String(p.input)
                output = String(p.output)
                cacheRead = String(p.cache_read)
            }
        }
    }

    private func numField(_ label: String, _ text: Binding<String>) -> some View {
        VStack(alignment: .leading) {
            Text(label).font(.caption2).foregroundColor(.secondary)
            TextField("0", text: text)
                .keyboardType(.decimalPad)
                .font(.caption.monospacedDigit())
        }
    }
}

// MARK: - 高峰期配置

private struct PeakEditView: View {
    @EnvironmentObject var settings: AppSettings

    var body: some View {
        Form {
            Section("折算开关") {
                Toggle("启用高峰期折算", isOn: Binding(
                    get: { settings.peak.enabled },
                    set: { settings.peak.enabled = $0; settings.savePeak() }))
                Toggle("ZCode 150% 提额优惠", isOn: Binding(
                    get: { settings.peak.zcode_discount },
                    set: { settings.peak.zcode_discount = $0; settings.savePeak() }))
                Picker("订阅类型", selection: Binding(
                    get: { settings.peak.plan_type ?? .v2 },
                    set: { settings.peak.plan_type = $0; settings.savePeak() })) {
                    Text("V2（等效 token）").tag(PlanType.v2)
                    Text("V3（积分）").tag(PlanType.v3)
                }
            }
            Section("时段") {
                if settings.peak.segments.isEmpty {
                    Text("未配置时段（默认全 1.0 倍率）").foregroundColor(.secondary).font(.caption)
                }
                ForEach(settings.peak.segments.indices, id: \.self) { i in
                    Text("\(settings.peak.segments[i].start)–\(settings.peak.segments[i].end) ×\(settings.peak.segments[i].multiplier)")
                        .font(.caption)
                }
                Text("iOS 版暂不支持可视化编辑时段，请保持默认或与桌面端同步。")
                    .font(.caption2).foregroundColor(.secondary)
            }
        }
        .navigationTitle("高峰期折算")
        .navigationBarTitleDisplayMode(.inline)
    }
}
