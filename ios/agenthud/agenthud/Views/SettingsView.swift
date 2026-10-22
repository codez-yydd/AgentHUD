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
            ZStack {
                AppBackground()

                VStack(spacing: 0) {
                    PageHeader(title: "设置",
                               subtitle: "同步服务 · 小组件 · 显示",
                               isLoading: refresh.loading,
                               onRefresh: { refresh.refreshAll() })
                        .padding(.horizontal, Theme.pagePadding)
                        .padding(.top, 4)
                        .padding(.bottom, 8)

                    Form {
                        Section {
                            SyncConfigRow()
                            // 已连接时提供同步数据诊断（哪些来源实际上传了快照）
                            if settings.isSyncConfigured {
                                NavigationLink {
                                    SyncDiagnosticsView()
                                } label: {
                                    Label("同步数据诊断", systemImage: "stethoscope")
                                }
                            }
                        } header: {
                            sectionHeader("同步服务", icon: "antenna.radiowaves.left.and.right")
                        }
                        .listRowBackground(Color.white.opacity(0.045))

                        Section {
                            WidgetDisplayPickerRow()
                        } header: {
                            sectionHeader("小组件显示", icon: "square.grid.2x2")
                        }
                        .listRowBackground(Color.white.opacity(0.045))

                        Section {
                            CurrencyRow()
                        } header: {
                            sectionHeader("显示", icon: "circle.lefthalf.filled")
                        }
                        .listRowBackground(Color.white.opacity(0.045))

                        Section {
                            NavigationLink {
                                PricingEditView()
                            } label: {
                                Label("价格配置", systemImage: "tag")
                            }
                            NavigationLink {
                                PeakEditView()
                            } label: {
                                Label("高峰期折算", systemImage: "clock.arrow.circlepath")
                            }
                        } header: {
                            sectionHeader("计费", icon: "yensign.circle")
                        }
                        .listRowBackground(Color.white.opacity(0.045))

                        Section {
                            Button {
                                refresh.refreshAll()
                            } label: {
                                Label("立即刷新全部数据", systemImage: "arrow.clockwise")
                            }
                        }
                        .listRowBackground(Color.white.opacity(0.045))

                        Section {
                            aboutCard
                        } header: {
                            sectionHeader("关于", icon: "info.circle")
                        }
                        .listRowBackground(Color.white.opacity(0.045))
                    }
                    .scrollContentBackground(.hidden)
                    .tint(Theme.accent)
                    .environment(\.defaultMinListRowHeight, 38)
                    .listSectionSpacing(16)
                    .padding(.bottom, 74)
                }
            }
            .toolbar(.hidden, for: .navigationBar)
        }
    }

    /// 设置页分组标题（图标 + 中文小标，替代系统大写英文样式）。
    private func sectionHeader(_ title: String, icon: String) -> some View {
        HStack(spacing: 5) {
            Image(systemName: icon)
                .font(.system(size: 9, weight: .semibold))
            Text(title)
                .font(.system(size: 11, weight: .semibold))
        }
        .foregroundStyle(Theme.textSecondary)
        .textCase(nil)
    }

    private var aboutCard: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 10) {
                Image("AppLogo")
                    .resizable()
                    .scaledToFit()
                    .frame(width: 38, height: 38)
                    .clipShape(RoundedRectangle(cornerRadius: 10, style: .continuous))
                    .overlay {
                        RoundedRectangle(cornerRadius: 10, style: .continuous)
                            .strokeBorder(Color.white.opacity(0.12), lineWidth: 1)
                    }
                VStack(alignment: .leading, spacing: 2) {
                    Text("AgentHUD")
                        .font(.system(size: 15, weight: .semibold, design: .rounded))
                    Text("iOS 版 · 0.1.0")
                        .font(.system(size: 10))
                        .foregroundStyle(Theme.textTertiary)
                }
                Spacer()
            }
            Text("桌面版 ZBar 的 iOS 配套应用。用量与额度统一由桌面端通过自托管同步服务上传，iOS 只读展示。")
                .font(.system(size: 11))
                .foregroundStyle(Theme.textSecondary)
                .fixedSize(horizontal: false, vertical: true)
        }
        .padding(.vertical, 4)
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
        VStack(alignment: .leading, spacing: 12) {
            HStack(spacing: 10) {
                AgentTile(icon: "antenna.radiowaves.left.and.right",
                          tint: settings.isSyncConfigured ? Theme.mint : Theme.amber,
                          size: 36)

                VStack(alignment: .leading, spacing: 2) {
                    HStack(spacing: 6) {
                        Text(settings.isSyncConfigured ? "已连接" : "未连接")
                            .font(.system(size: 14, weight: .semibold))
                        if settings.isSyncConfigured {
                            Text("在线")
                                .font(.system(size: 9, weight: .medium))
                                .padding(.horizontal, 6)
                                .padding(.vertical, 2)
                                .background(Capsule().fill(Theme.mint.opacity(0.13)))
                                .foregroundStyle(Theme.mint)
                        }
                    }
                    Text(settings.isSyncConfigured
                         ? settings.sync.device_name
                         : "连接自托管同步服务后开始拉取用量")
                        .font(.system(size: 10))
                        .foregroundStyle(Theme.textSecondary)
                        .lineLimit(1)
                }
                Spacer(minLength: 4)
            }

            if settings.isSyncConfigured {
                VStack(alignment: .leading, spacing: 4) {
                    HStack(spacing: 5) {
                        Image(systemName: "server.rack")
                            .font(.system(size: 9))
                        Text(settings.sync.server_url)
                            .lineLimit(1)
                            .truncationMode(.middle)
                    }
                    HStack(spacing: 5) {
                        Image(systemName: "iphone")
                            .font(.system(size: 9))
                        Text("设备 ID \(settings.sync.device_id.prefix(8))…")
                    }
                }
                .font(.system(size: 10, design: .monospaced))
                .foregroundStyle(Theme.textTertiary)

                HStack(spacing: 8) {
                    Button {
                        editing = true
                    } label: {
                        Label("重新注册", systemImage: "arrow.triangle.2.circlepath")
                            .font(.system(size: 11, weight: .medium))
                    }
                    .buttonStyle(.bordered)
                    .controlSize(.small)
                    .tint(Theme.accent)

                    Button(role: .destructive) {
                        showDisconnect = true
                    } label: {
                        Text("断开连接")
                            .font(.system(size: 11, weight: .medium))
                    }
                    .buttonStyle(.bordered)
                    .controlSize(.small)
                    .tint(Theme.coral)

                    Spacer()
                }
            } else {
                Button {
                    editing = true
                } label: {
                    Label("连接同步服务…", systemImage: "link")
                        .font(.system(size: 12, weight: .medium))
                }
                .buttonStyle(.bordered)
                .controlSize(.small)
                .tint(Theme.accent)
            }
        }
        .padding(.vertical, 2)
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

// MARK: - 小组件显示选择（批2）

/// 「小组件显示」区块：选择 Widget 展示的额度账号（跟随主账号 / 现有直连
/// 账号 / 只读同步分组）。选中值写 App Group，主 App 每次刷新后按它重写快照
/// 并 reload Timeline；Widget 本身不再提供编辑配置。
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
            Text("自动选择（优先主账号）").tag("")
            ForEach(store.accounts) { account in
                Text("\(account.agent.displayName) · \(account.label)")
                    .tag(account.id.uuidString)
            }
            ForEach(AgentKind.allCases.filter { $0.isSyncOnly }, id: \.rawValue) { agent in
                Text("\(agent.displayName)（桌面同步）")
                    .tag(AccountStore.syncOptionId(agent))
            }
        }
        Text("小组件跟随桌面端同步的额度数据；在这里切换后回到桌面等待刷新即可，App 内刷新也会立即触发重绘。")
            .font(.caption)
            .foregroundColor(.secondary)
    }
}

/// 添加 / 编辑账号表单。编辑时 secret 只显示掩码，输入框留空 = 不修改。
// MARK: - 货币切换

private struct CurrencyRow: View {
    @EnvironmentObject var settings: AppSettings

    var body: some View {
        HStack(spacing: 8) {
            currencyButton(.cny, title: "人民币 ¥")
            currencyButton(.usd, title: "美元 $")
        }
        .padding(.vertical, 2)
    }

    private func currencyButton(_ currency: Currency, title: String) -> some View {
        let selected = settings.currency == currency
        return Button {
            settings.currency = currency
            settings.saveCurrency()
        } label: {
            Text(title)
                .font(.system(size: 12.5, weight: selected ? .semibold : .regular))
                .foregroundStyle(selected ? Theme.accent : Theme.textSecondary)
                .frame(maxWidth: .infinity)
                .padding(.vertical, 9)
                .background {
                    RoundedRectangle(cornerRadius: 11, style: .continuous)
                        .fill(selected ? Theme.accent.opacity(0.14)
                                       : Color.white.opacity(0.04))
                }
                .overlay {
                    RoundedRectangle(cornerRadius: 11, style: .continuous)
                        .strokeBorder(selected ? Theme.accent.opacity(0.38)
                                              : Color.white.opacity(0.06), lineWidth: 1)
                }
        }
        .buttonStyle(.plain)
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
        .toolbar(.visible, for: .navigationBar)
        .safeAreaInset(edge: .bottom) { Color.clear.frame(height: 78) }
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

// MARK: - 同步数据诊断

/// 同步数据诊断：并发拉近 7 天的两路快照（/agent-quota-snapshots 与
/// /snapshots），逐行展示各来源的最新快照时间与条数，让用户能自己看出
/// 哪些来源有数据上传。两路各自独立成败，互不阻塞。
private struct SyncDiagnosticsView: View {
    @EnvironmentObject var settings: AppSettings

    /// 单个来源的诊断行
    struct DiagRow: Identifiable {
        /// 原始 source（GLM 账号行为 nil；provider 缺失判定用）
        let source: String?
        /// 显示名（AgentKind.displayName 或「账号 xxxxxxxx」）
        let title: String
        let latestMs: Int?
        let count: Int
        var id: String { title }
    }

    @State private var agentRows: [DiagRow] = []
    @State private var glmRows: [DiagRow] = []
    @State private var agentError: String?
    @State private var glmError: String?
    @State private var loading = false

    /// provider 类额度来源（proto 7 全量 provider 通道；与明细源命名不同，
    /// 如这里是 deepseek 而明细源是 dsh）
    private static let providerSources: Set<String> = [
        "deepseek", "moonshot", "minimax", "opencodego", "alibaba",
        "qoder", "longcat", "mimo", "stepfun", "grok", "alibabatoken",
    ]

    /// 诊断行展示顺序（CLI 同步型 → 双形态 provider → 纯同步 provider）
    private static let sourceOrder: [String] = [
        "codex", "claude", "cursor", "kimi",
        "deepseek", "moonshot", "minimax", "opencodego", "alibaba",
        "qoder", "longcat", "mimo", "stepfun", "grok", "alibabatoken",
    ]

    var body: some View {
        List {
            if loading {
                Section {
                    HStack(spacing: 8) {
                        ProgressView().controlSize(.small)
                        Text("加载中…")
                            .font(.caption)
                            .foregroundColor(.secondary)
                    }
                }
            }
            if let err = agentError {
                Section {
                    Text("额度快照（Agent 通道）加载失败：\(err)")
                        .font(.caption)
                        .foregroundColor(.red)
                }
            }
            if let err = glmError {
                Section {
                    Text("GLM 快照加载失败：\(err)")
                        .font(.caption)
                        .foregroundColor(.red)
                }
            }

            Section("Agent 额度快照（近 7 天）") {
                if agentRows.isEmpty && !loading && agentError == nil {
                    Text("未收到任何快照")
                        .font(.caption)
                        .foregroundColor(.secondary)
                }
                ForEach(agentRows) { row in
                    diagRow(row)
                }
            }

            Section("GLM 额度快照（近 7 天）") {
                if glmRows.isEmpty && !loading && glmError == nil {
                    Text("未收到任何快照")
                        .font(.caption)
                        .foregroundColor(.secondary)
                }
                ForEach(glmRows) { row in
                    diagRow(row)
                }
            }

            // provider 通道整段缺失时的提示（通道加载失败时无法判定，不提示）
            if showProviderHint {
                Section {
                    Text("未收到 provider 额度快照：请确认自建服务端已更新到最新版本并重启（proto≥7），桌面端为 v1.7.0 及以上且正在运行（升级后约 30 分钟内开始上传）；claude/kimi 需要桌面机对应 CLI 处于登录状态。")
                        .font(.caption)
                        .foregroundColor(.orange)
                }
            }
        }
        .navigationTitle("同步数据诊断")
        .navigationBarTitleDisplayMode(.inline)
        .toolbar(.visible, for: .navigationBar)
        .safeAreaInset(edge: .bottom) { Color.clear.frame(height: 78) }
        .refreshable { await performLoad() }
        .onAppear { load() }
    }

    private func diagRow(_ row: DiagRow) -> some View {
        HStack {
            Text(row.title)
                .font(.subheadline)
            Spacer()
            VStack(alignment: .trailing, spacing: 2) {
                Text(relativeTime(row.latestMs))
                    .font(.caption)
                    // 最新快照超过 6 小时视为可能停报，橙色提示
                    .foregroundColor(isStale(row.latestMs) ? .orange : .secondary)
                Text("7 天 \(row.count) 条")
                    .font(.caption2)
                    .foregroundColor(.secondary)
            }
        }
    }

    /// 相对时间（如「3 分钟前」）；无快照 → 「—」。
    private func relativeTime(_ ms: Int?) -> String {
        guard let ms = ms else { return "—" }
        let diff = Int(Date().timeIntervalSince1970) - ms / 1000
        if diff < 60 { return "\(max(diff, 0)) 秒前" }
        if diff < 3_600 { return "\(diff / 60) 分钟前" }
        if diff < 86_400 { return "\(diff / 3_600) 小时前" }
        return "\(diff / 86_400) 天前"
    }

    /// 最新快照是否超过 6 小时（无快照不算停报——由条数与提示文案覆盖）。
    private func isStale(_ ms: Int?) -> Bool {
        guard let ms = ms else { return false }
        return Date().timeIntervalSince1970 - TimeInterval(ms) / 1000 > 6 * 3_600
    }

    /// provider 缺失判定：Agent 通道加载成功且 7 天内没有任何 provider 来源。
    private var showProviderHint: Bool {
        guard !loading, agentError == nil else { return false }
        let sources = Set(agentRows.compactMap { $0.source })
        return sources.isDisjoint(with: Self.providerSources)
    }

    private func load() {
        Task { await performLoad() }
    }

    /// 拉两路快照并聚合（异步可等待版，refreshable 共用）。
    /// 不做 !loading 拦截：下拉刷新与 onAppear 都允许重复触发，后发覆盖先发。
    private func performLoad() async {
        guard settings.isSyncConfigured else { return }
        loading = true
        agentError = nil
        glmError = nil
        let server = settings.sync.server_url
        let token = settings.sync.device_token
        let nowMs = Int(Date().timeIntervalSince1970 * 1000)
        let fromMs = nowMs - 7 * 86_400_000

        // 两路并发；各自独立 do/catch，单路失败不阻塞另一路
        async let agentTask = APIClient.shared.agentQuotaSnapshots(
            serverURL: server, deviceToken: token,
            fromMs: fromMs, toMs: nowMs)
        async let glmTask = APIClient.shared.snapshots(
            serverURL: server, deviceToken: token,
            fromMs: fromMs, toMs: nowMs)

        do {
            let snaps = try await agentTask
            let grouped = Dictionary(grouping: snaps, by: { $0.source })
            var rows: [DiagRow] = grouped.map { src, items in
                DiagRow(source: src,
                        title: AgentQuotaSync.syncAgents[src]?.displayName ?? src,
                        latestMs: items.map { $0.ts }.max(),
                        count: items.count)
            }
            rows.sort {
                let a = Self.sourceOrder.firstIndex(of: $0.source ?? "") ?? .max
                let b = Self.sourceOrder.firstIndex(of: $1.source ?? "") ?? .max
                return a < b
            }
            await MainActor.run { self.agentRows = rows }
        } catch {
            await MainActor.run { self.agentError = error.localizedDescription }
        }

        do {
            let snaps = try await glmTask
            // 按 account 指纹分组（nil/空 = 旧客户端无指纹）
            let grouped = Dictionary(grouping: snaps) { snap in
                snap.account?.trimmingCharacters(in: .whitespaces) ?? ""
            }
            var rows: [DiagRow] = grouped.map { account, items in
                DiagRow(source: nil,
                        title: account.isEmpty
                            ? "无账号指纹（旧客户端）"
                            : "账号 \(account.prefix(8))",
                        latestMs: items.map { $0.ts }.max(),
                        count: items.count)
            }
            rows.sort { $0.title < $1.title }
            await MainActor.run { self.glmRows = rows }
        } catch {
            await MainActor.run { self.glmError = error.localizedDescription }
        }

        await MainActor.run { self.loading = false }
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
        .toolbar(.visible, for: .navigationBar)
        .safeAreaInset(edge: .bottom) { Color.clear.frame(height: 78) }
    }
}
