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
                               isLoading: refresh.loading,
                               onRefresh: { refresh.refreshAll() })
                        .padding(.horizontal, Theme.pagePadding)
                        .padding(.top, 4)
                        .padding(.bottom, 6)

                    Form {
                        Section {
                            SyncConfigRow()
                            // 已连接时提供同步数据诊断（哪些来源实际上传了快照）
                            if settings.isSyncConfigured {
                                NavigationLink {
                                    SyncDiagnosticsView()
                                } label: {
                                    rowLabel("同步数据诊断", icon: "stethoscope")
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
                                PricingInfoView()
                            } label: {
                                HStack(spacing: 8) {
                                    rowLabel("模型价格", icon: "tag")
                                    Spacer()
                                    Text(settings.pricingMeta.isRemote ? "跟随桌面端" : "本地参考价")
                                        .font(.system(size: 11))
                                        .foregroundStyle(Theme.textTertiary)
                                }
                            }
                            NavigationLink {
                                PeakEditView()
                            } label: {
                                rowLabel("高峰期折算", icon: "clock.arrow.circlepath")
                            }
                        } header: {
                            sectionHeader("计费", icon: "yensign.circle")
                        } footer: {
                            Text("价格由桌面端统一维护，手机端只读；连接同步服务后自动下发。")
                                .font(.system(size: 10))
                                .foregroundStyle(Theme.textTertiary)
                        }
                        .listRowBackground(Color.white.opacity(0.045))

                        Section {
                            Button {
                                refresh.refreshAll()
                            } label: {
                                rowLabel("立即刷新全部数据", icon: "arrow.clockwise")
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
                    .padding(.bottom, 8)
                }
            }
            .toolbar(.hidden, for: .navigationBar)
        }
    }

    /// 设置页行标题：统一 14.5pt + 小图标，避免 Form 默认 17pt 与
    /// 自绘卡片（14/10pt）的字号断层。
    private func rowLabel(_ title: String, icon: String) -> some View {
        Label {
            Text(title)
                .font(.system(size: 14.5))
        } icon: {
            Image(systemName: icon)
                .font(.system(size: 13, weight: .medium))
                .foregroundStyle(Theme.accent)
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
                // 断开后不再有远端来源：保留缓存价，但状态切回本地
                var meta = settings.pricingMeta
                meta.isRemote = false
                settings.pricingMeta = meta
                settings.savePricing()
                refresh.pricingError = nil
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
        Picker("锁屏账号", selection: selection) {
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
        HStack(spacing: 6) {
            Image(systemName: "info.circle")
                .font(.system(size: 10, weight: .semibold))
            Text("主屏中 / 大尺寸展示全部账号；这里只选择锁屏显示的账号")
                .font(.system(size: 10.5))
                .fixedSize(horizontal: false, vertical: true)
            Spacer(minLength: 0)
        }
        .foregroundStyle(Theme.textTertiary)
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

// MARK: - 模型价格（只读）

/// 手机端不再编辑价格：展示桌面端发布到同步服务的价格表。
/// CNY 由 USD × 桌面端汇率实时折算，与桌面端花费口径一致。
private struct PricingInfoView: View {
    @EnvironmentObject var settings: AppSettings
    @EnvironmentObject var refresh: RefreshService

    @State private var query = ""
    @State private var currency: Currency = .cny

    private var meta: PricingSyncMeta { settings.pricingMeta }
    private var fxRate: Double { meta.fxRate > 0 ? meta.fxRate : 7.2 }
    private var usdMap: [String: ModelPrice] { settings.pricing.usd }
    private var modelIds: [String] {
        let all = usdMap.keys.sorted()
        let q = query.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        guard !q.isEmpty else { return all }
        return all.filter { $0.lowercased().contains(q) }
    }

    var body: some View {
        ZStack {
            AppBackground()
            ScrollView {
                VStack(alignment: .leading, spacing: 14) {
                    sourceCard
                    controls
                    priceList
                    Text("价格由桌面端「设置 → 价格」统一维护；手机端仅展示，不能编辑或新增模型。")
                        .font(.system(size: 10))
                        .foregroundStyle(Theme.textTertiary)
                        .padding(.horizontal, 2)
                }
                .padding(.horizontal, Theme.pagePadding)
                .padding(.top, 10)
                .padding(.bottom, 90)
            }
            .scrollIndicators(.hidden)
        }
        .navigationTitle("模型价格")
        .navigationBarTitleDisplayMode(.inline)
        .toolbar(.visible, for: .navigationBar)
        .onAppear { currency = settings.currency }
    }

    private var sourceCard: some View {
        GlassCard(tint: meta.isRemote ? Theme.accent : nil, elevated: meta.isRemote) {
            VStack(alignment: .leading, spacing: 12) {
                HStack(spacing: 10) {
                    ZStack {
                        Circle().fill((meta.isRemote ? Theme.accent : Theme.textTertiary).opacity(0.16))
                        Image(systemName: meta.isRemote ? "desktopcomputer" : "shippingbox")
                            .font(.system(size: 15, weight: .medium))
                            .foregroundStyle(meta.isRemote ? Theme.accent : Theme.textSecondary)
                    }
                    .frame(width: 34, height: 34)

                    VStack(alignment: .leading, spacing: 2) {
                        Text(meta.isRemote ? "跟随桌面端" : "本地参考价")
                            .font(.system(size: 14, weight: .semibold))
                        Text(sourceSubtitle)
                            .font(.system(size: 11))
                            .foregroundStyle(Theme.textSecondary)
                    }
                    Spacer(minLength: 6)
                    refreshButton
                }

                if meta.isRemote {
                    HStack(spacing: 16) {
                        miniStat("模型", "\(meta.modelCount)")
                        miniStat("汇率", String(format: "%.2f", fxRate))
                        miniStat("更新", shortTime)
                    }
                } else {
                    Text("桌面端保存价格后，会在下一次同步时自动下发。")
                        .font(.system(size: 11))
                        .foregroundStyle(Theme.textSecondary)
                }

                if let err = refresh.pricingError {
                    HStack(alignment: .top, spacing: 5) {
                        Image(systemName: "exclamationmark.triangle.fill")
                        Text(err).lineLimit(3)
                    }
                    .font(.system(size: 10))
                    .foregroundStyle(Theme.amber)
                }
            }
        }
    }

    private var sourceSubtitle: String {
        if !settings.isSyncConfigured { return "尚未配置同步服务" }
        if meta.isRemote { return "由桌面端统一维护，手机端只读" }
        return "尚未同步，当前使用本地价格表"
    }

    private var shortTime: String {
        guard meta.updatedAt > 0 else { return "—" }
        let f = DateFormatter()
        f.dateFormat = "MM-dd HH:mm"
        f.timeZone = .current
        return f.string(from: Date(timeIntervalSince1970: TimeInterval(meta.updatedAt) / 1000))
    }

    private var refreshButton: some View {
        Button {
            Task { await refresh.refreshPricing() }
        } label: {
            if refresh.pricingRefreshing {
                ProgressView().controlSize(.small)
            } else {
                Image(systemName: "arrow.triangle.2.circlepath")
                    .font(.system(size: 13, weight: .semibold))
            }
        }
        .buttonStyle(.plain)
        .foregroundStyle(Theme.accent)
        .frame(width: 30, height: 30)
        .background(Circle().fill(Color.white.opacity(0.06)))
        .disabled(refresh.pricingRefreshing || !settings.isSyncConfigured)
        .opacity(settings.isSyncConfigured ? 1 : 0.4)
    }

    private func miniStat(_ label: String, _ value: String) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(label).font(.system(size: 9)).foregroundStyle(Theme.textTertiary)
            Text(value).font(.system(size: 12, weight: .medium, design: .monospaced))
                .foregroundStyle(Theme.textSecondary)
        }
    }

    private var controls: some View {
        VStack(spacing: 10) {
            HStack {
                Picker("", selection: $currency) {
                    Text("¥ CNY").tag(Currency.cny)
                    Text("$ USD").tag(Currency.usd)
                }
                .pickerStyle(.segmented)
                .frame(width: 160)
                Spacer()
                Text("\(currency == .cny ? "¥" : "$") / 百万 Token")
                    .font(.system(size: 10))
                    .foregroundStyle(Theme.textTertiary)
            }

            HStack(spacing: 6) {
                Image(systemName: "magnifyingglass")
                    .font(.system(size: 12))
                    .foregroundStyle(Theme.textTertiary)
                TextField("搜索模型", text: $query)
                    .font(.system(size: 13))
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                if !query.isEmpty {
                    Button { query = "" } label: {
                        Image(systemName: "xmark.circle.fill")
                            .foregroundStyle(Theme.textTertiary)
                    }
                    .buttonStyle(.plain)
                }
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 9)
            .background(RoundedRectangle(cornerRadius: 11, style: .continuous)
                .fill(Color.white.opacity(0.05)))
            .overlay(RoundedRectangle(cornerRadius: 11, style: .continuous)
                .strokeBorder(Color.white.opacity(0.07), lineWidth: 1))
        }
    }

    @ViewBuilder
    private var priceList: some View {
        if modelIds.isEmpty {
            EmptyState(icon: "tag.slash",
                       title: query.isEmpty ? "暂无价格数据" : "没有匹配的模型",
                       message: query.isEmpty ? "在桌面端保存价格并同步一次" : nil)
                .padding(.vertical, 10)
        } else {
            GlassCard(padding: 0) {
                LazyVStack(spacing: 0) {
                    ForEach(Array(modelIds.enumerated()), id: \.element) { index, id in
                        if index > 0 {
                            Rectangle().fill(Color.white.opacity(0.05)).frame(height: 0.5)
                                .padding(.leading, 16)
                        }
                        priceRow(id)
                    }
                }
            }
        }
    }

    private func priceRow(_ id: String) -> some View {
        let price = usdMap[id] ?? ModelPrice()
        let scale = currency == .cny ? fxRate : 1
        return HStack(alignment: .center, spacing: 10) {
            VStack(alignment: .leading, spacing: 3) {
                Text(id)
                    .font(.system(size: 12, weight: .medium))
                    .foregroundStyle(Color.white.opacity(0.92))
                    .lineLimit(1)
                Text("入 \(fmt(price.input * scale))  ·  出 \(fmt(price.output * scale))  ·  缓存 \(fmt(price.cache_read * scale))")
                    .font(.system(size: 10, design: .monospaced))
                    .foregroundStyle(Theme.textSecondary)
                    .lineLimit(1)
                    .minimumScaleFactor(0.8)
            }
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 10)
    }

    private func fmt(_ n: Double) -> String {
        String(format: "%.6g", n)
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
