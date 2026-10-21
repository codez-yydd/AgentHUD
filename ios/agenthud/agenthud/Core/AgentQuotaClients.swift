//
//  AgentQuotaClients.swift
//  agenthud
//
//  多 Agent 直连额度查询（翻译桌面 src-tauri/src/ 的 quota.rs / deepseek.rs /
//  moonshot.rs / minimax.rs / opencodego.rs / alibaba.rs，端点/鉴权/解析
//  逐字段对齐源文件）。
//
//  工程纪律（对齐桌面 provider_quota.rs）：
//  - 查询失败不抛错，收敛为 status=error/expired 的 QuotaEntry（message 中文
//    且不含 secret 片段）——单账号失败不阻塞其他账号；
//  - 超时分层：单请求 12s / 总资源 20s（沿用原 QuotaClient 口径）；
//  - secret 只在内存构造鉴权头，绝不进任何错误消息或日志。
//
//  桌面各源的语义陷阱（翻译时已核对源文件）：
//  - zai：Authorization 头直放 key（无 Bearer 前缀）；多订阅窗口按
//    nextResetTime 升序稳定 pick；
//  - minimax：current_interval_usage_count 实为【剩余量】；百分比以
//    *_remaining_percent 为准（usedPercent = 100 - 剩余）；404/501 回退旧版
//    开放平台端点（其响应 model_remains 在顶层，无 data 信封）；重置时间
//    epoch 秒/毫秒自适应（>10^12 判毫秒）；
//  - opencodego：只发 Authorization: Bearer（x-api-key 实测 401）；resetsAt
//    是 RFC3339 字符串；percent 是窗口唯一必填项，缺失跳过该窗；
//  - alibaba：三个鉴权头同时发（Authorization Bearer + x-api-key +
//    X-DashScope-API-Key）；响应的 data 可能是「JSON 字符串再包一层」，
//    需递归展开；多实例取信号分最高的一条。
//

import Foundation

public actor AgentQuotaClients {
    public static let shared = AgentQuotaClients()

    private let session: URLSession

    public init() {
        let cfg = URLSessionConfiguration.default
        cfg.timeoutIntervalForRequest = 12
        cfg.timeoutIntervalForResource = 20
        self.session = URLSession(configuration: cfg)
    }

    // MARK: - 分发入口

    /// 查询单个直连账号的额度。任何失败都收敛为 error/expired 条目，不抛错。
    public func query(_ entry: AccountEntry, secret: String) async -> QuotaEntry {
        let token = secret.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !token.isEmpty else {
            return QuotaEntry(accountId: entry.id.uuidString,
                              label: entry.label,
                              agent: entry.agent,
                              status: "error",
                              message: "凭证为空，请在设置中填写")
        }
        switch entry.agent {
        case .zai:
            return await queryZai(entry, token: token)
        case .deepseek:
            return await queryDeepSeek(entry, token: token)
        case .moonshot:
            return await queryMoonshot(entry, token: token)
        case .minimax:
            return await queryMinimax(entry, token: token)
        case .opencodego:
            return await queryOpencodeGo(entry, token: token)
        case .alibaba:
            return await queryAlibaba(entry, token: token)
        case .codex, .claude, .cursor:
            return QuotaEntry(accountId: entry.id.uuidString,
                              label: entry.label,
                              agent: entry.agent,
                              status: "error",
                              message: "该服务为桌面同步型，不支持直连查询")
        }
    }

    // MARK: - GLM Coding Plan（zai，翻译 quota.rs::query_quota_with）

    /// 智谱额度接口原始结构（BigModel）。
    private struct ZaiResponse: Decodable {
        var success: Bool?
        var msg: String?
        var data: ZaiData?
    }

    private struct ZaiData: Decodable {
        var level: String?
        var limits: [QuotaLimit]?
    }

    private func queryZai(_ entry: AccountEntry, token: String) async -> QuotaEntry {
        let base = (entry.region == .global) ? "https://api.z.ai" : "https://open.bigmodel.cn"
        guard let url = URL(string: "\(base)/api/monitor/usage/quota/limit") else {
            return fail(entry, "error", "非法的额度接口 URL")
        }
        // Authorization 直放 key，无 Bearer 前缀（对齐桌面 quota.rs:371）
        switch await perform(url, headers: ["Authorization": token]) {
        case .failed(let msg):
            return networkFail(entry, "请求额度接口", msg)
        case .ok(let code, let body):
            if !(200...299).contains(code) {
                return fail(entry, "error", "请求额度接口失败（HTTP \(code)）")
            }
            guard let body, let parsed = try? JSONDecoder().decode(ZaiResponse.self, from: body) else {
                return fail(entry, "error", "解析额度响应失败")
            }
            guard parsed.success == true else {
                return fail(entry, "error",
                            parsed.msg?.isEmpty == false ? parsed.msg! : "额度接口返回失败")
            }
            guard let d = parsed.data else {
                return fail(entry, "error", "额度响应缺少 data 字段")
            }
            return zaiEntry(entry, data: d)
        }
    }

    /// limits → 三档窗口（翻译 quota.rs:387-440 的稳定 pick 逻辑）。
    private func zaiEntry(_ entry: AccountEntry, data: ZaiData) -> QuotaEntry {
        let limits = data.limits ?? []
        // MCP 月度额度：type=TIME_LIMIT
        let mcp = limits.first { $0.type == "TIME_LIMIT" }
        // token 额度：窗口由 (unit, number) 识别，(3,5)=5 小时，(6,1)=每周
        let tokenLimits = limits.filter { $0.type == "TOKENS_LIMIT" }

        // 多订阅同窗口：按 nextResetTime 升序取最近重置的一条（最活跃订阅），
        // None 排最后，保证轮询间选择稳定（对齐桌面 pick_stable）
        func pickStable(_ pred: (QuotaLimit) -> Bool) -> QuotaLimit? {
            tokenLimits.filter(pred)
                .sorted { ($0.nextResetTime ?? .max) < ($1.nextResetTime ?? .max) }
                .first
        }

        let hour5 = pickStable { $0.unit == 3 && $0.number == 5 }
            // 兼容旧接口：刚刷新后的短窗口通常没有 nextResetTime
            ?? tokenLimits.first { $0.nextResetTime == nil }
            ?? tokenLimits.first
        let weekly = pickStable { $0.unit == 6 && $0.number == 1 }
            ?? tokenLimits.first { l in
                hour5.map { $0.unit != l.unit || $0.number != l.number } ?? true
            }

        var windows: [QuotaWindow] = []
        if let h = hour5 {
            windows.append(QuotaWindow(key: "hour5", title: "5h",
                                       usedPercent: Double(h.percentage),
                                       resetsAt: h.nextResetTime))
        }
        if let w = weekly {
            windows.append(QuotaWindow(key: "weekly", title: "本周",
                                       usedPercent: Double(w.percentage),
                                       resetsAt: w.nextResetTime))
        }
        if let m = mcp {
            windows.append(QuotaWindow(key: "mcp", title: "MCP 月度",
                                       usedPercent: Double(m.percentage),
                                       used: m.currentValue.map(Double.init),
                                       total: m.usage.map(Double.init),
                                       unit: "次",
                                       resetsAt: m.nextResetTime))
        }
        let level = (data.level ?? "").trimmingCharacters(in: .whitespaces)
        return QuotaEntry(accountId: entry.id.uuidString,
                          label: entry.label,
                          agent: .zai,
                          status: "ok",
                          windows: windows,
                          planName: level.isEmpty ? nil : level)
    }

    // MARK: - DeepSeek（翻译 deepseek.rs）

    private func queryDeepSeek(_ entry: AccountEntry, token: String) async -> QuotaEntry {
        guard let url = URL(string: "https://api.deepseek.com/user/balance") else {
            return fail(entry, "error", "非法的余额接口 URL")
        }
        switch await perform(url, headers: [
            "Authorization": "Bearer \(token)",
            "Accept": "application/json",
        ]) {
        case .failed(let msg):
            return networkFail(entry, "DeepSeek 余额", msg)
        case .ok(let code, let data):
            if code == 401 || code == 403 {
                return fail(entry, "expired", "API Key 无效或已过期")
            }
            guard code == 200 else {
                return fail(entry, "error", "DeepSeek 余额查询失败（HTTP \(code)）")
            }
            let body = data.flatMap { String(data: $0, encoding: .utf8) } ?? ""
            guard let v = vObject(v: parseJSON(body)) else {
                return fail(entry, "error", "DeepSeek 余额响应解析失败")
            }
            guard let info = pickDeepSeekInfo(v) else {
                return fail(entry, "error", "DeepSeek 余额响应缺少 balance_infos")
            }
            guard let amount = flexDouble(info["total_balance"]) else {
                return fail(entry, "error", "DeepSeek 余额响应缺少 total_balance")
            }
            // is_available=false 且余额非 0：账户异常冻结等场景，余额仍展示但提示
            let unavailable = (v["is_available"] as? Bool) == false
            let message = (unavailable && amount != 0) ? "余额暂不可用于 API 调用" : nil
            return QuotaEntry(accountId: entry.id.uuidString,
                              label: entry.label,
                              agent: .deepseek,
                              status: "ok",
                              windows: [],
                              balance: QuotaBalance(
                                  amount: amount,
                                  currency: (info["currency"] as? String) ?? "CNY",
                                  granted: flexDouble(info["granted_balance"]),
                                  toppedUp: flexDouble(info["topped_up_balance"])),
                              message: message)
        }
    }

    /// balance_infos 多条时优先 USD，否则第一条（对齐 pick_balance_info）。
    private func pickDeepSeekInfo(_ v: Any) -> [String: Any]? {
        guard let infos = (v as? [String: Any])?["balance_infos"] as? [[String: Any]] else {
            return nil
        }
        return infos.first { ($0["currency"] as? String) == "USD" } ?? infos.first
    }

    // MARK: - Kimi 开放平台（moonshot，翻译 moonshot.rs）

    private func queryMoonshot(_ entry: AccountEntry, token: String) async -> QuotaEntry {
        let isGlobal = entry.region == .global
        let host = isGlobal ? "https://api.moonshot.ai" : "https://api.moonshot.cn"
        guard let url = URL(string: "\(host)/v1/users/me/balance") else {
            return fail(entry, "error", "非法的余额接口 URL")
        }
        switch await perform(url, headers: [
            "Authorization": "Bearer \(token)",
            "Accept": "application/json",
        ]) {
        case .failed(let msg):
            return networkFail(entry, "Kimi 余额", msg)
        case .ok(let code, let data):
            if code == 401 || code == 403 {
                // 国际站 Key 误填默认区域是最常见的「假过期」场景，附加切换提示
                return fail(entry, "expired", isGlobal
                            ? "API Key 无效或已过期"
                            : "API Key 无效或已过期（如为国际站 Key 请在编辑中将区域切换为国际站）")
            }
            guard code == 200 else {
                return fail(entry, "error", "Kimi 余额查询失败（HTTP \(code)）")
            }
            let body = data.flatMap { String(data: $0, encoding: .utf8) } ?? ""
            guard let obj = vObject(v: parseJSON(body)) else {
                return fail(entry, "error", "Kimi 余额响应解析失败")
            }
            // 业务错误码：code != 0 时 smsg 承载平台侧原因
            let code = flexDouble(obj["code"]) ?? 0
            if code != 0 {
                let smsg = (obj["smsg"] as? String)?
                    .trimmingCharacters(in: .whitespaces)
                let reason = (smsg?.isEmpty == false) ? smsg! : "未知错误"
                return fail(entry, "error", "Kimi 平台返回错误: \(reason)")
            }
            guard let dataObj = vObject(v: obj["data"]) else {
                return fail(entry, "error", "Kimi 余额响应缺少 data 字段")
            }
            guard let amount = flexDouble(dataObj["available_balance"]) else {
                return fail(entry, "error", "Kimi 余额响应缺少 available_balance")
            }
            return QuotaEntry(accountId: entry.id.uuidString,
                              label: entry.label,
                              agent: .moonshot,
                              status: "ok",
                              windows: [],
                              balance: QuotaBalance(
                                  amount: amount,
                                  currency: isGlobal ? "USD" : "CNY",
                                  granted: flexDouble(dataObj["voucher_balance"]),
                                  toppedUp: flexDouble(dataObj["cash_balance"])))
        }
    }

    // MARK: - MiniMax Coding Plan（翻译 minimax.rs）

    private func queryMinimax(_ entry: AccountEntry, token: String) async -> QuotaEntry {
        let isGlobal = entry.region == .global
        let host = isGlobal ? "https://api.minimax.io" : "https://api.minimaxi.com"
        let headers = [
            "Authorization": "Bearer \(token)",
            "accept": "application/json",
            "MM-API-Source": "ZBar",
        ]
        // 主端点 → 404/501（站点未部署）时回退旧版开放平台路径
        guard let primary = URL(string: "\(host)/v1/token_plan/remains") else {
            return fail(entry, "error", "非法的额度接口 URL")
        }
        var outcome = await perform(primary, headers: headers)
        if case .ok(let code, _) = outcome, code == 404 || code == 501 {
            guard let fallback = URL(
                string: "\(host)/v1/api/openplatform/coding_plan/remains") else {
                return fail(entry, "error", "非法的额度接口 URL")
            }
            outcome = await perform(fallback, headers: headers)
        }

        let code: Int
        let data: Data?
        switch outcome {
        case .failed(let msg):
            return networkFail(entry, "MiniMax 额度", msg)
        case .ok(let c, let d):
            code = c
            data = d
        }
        if code == 401 || code == 403 {
            return fail(entry, "expired", isGlobal
                        ? "Coding Plan Token 无效或已过期"
                        : "Coding Plan Token 无效或已过期（如为国际站 Key 请在编辑中将区域切换为国际站）")
        }
        guard code == 200 else {
            return fail(entry, "error", "MiniMax 额度查询失败（HTTP \(code)）")
        }
        let body = data.flatMap { String(data: $0, encoding: .utf8) } ?? ""
        guard let obj = vObject(v: parseJSON(body)) else {
            return fail(entry, "error", "MiniMax 额度响应解析失败")
        }
        // 业务信封：base_resp.status_code != 0 时按语义分流
        let baseResp = vObject(v: obj["base_resp"]) ?? [:]
        let statusCode = flexDouble(baseResp["status_code"]) ?? 0
        let statusMsg = (baseResp["status_msg"] as? String) ?? ""
        let expiredMsg = isGlobal
            ? "Coding Plan Token 无效或已过期"
            : "Coding Plan Token 无效或已过期（如为国际站 Key 请在编辑中将区域切换为国际站）"
        if statusCode != 0 {
            // 1004 为平台约定的登录态失效码；status_msg 含 cookie/login 同归过期
            let lower = statusMsg.lowercased()
            if statusCode == 1004 || lower.contains("cookie") || lower.contains("login") {
                return fail(entry, "expired", expiredMsg)
            }
            let reason = statusMsg.trimmingCharacters(in: .whitespaces)
            return fail(entry, "error", reason.isEmpty
                        ? "MiniMax 平台返回错误: status_code \(Int(statusCode))"
                        : "MiniMax 平台返回错误: \(reason)")
        }
        // 回退端点把 model_remains 放在顶层（无 data 信封），两处都取
        let models: [[String: Any]]
        if let wrapped = vObject(v: obj["data"]),
           let arr = wrapped["model_remains"] as? [[String: Any]] {
            models = arr
        } else if let arr = obj["model_remains"] as? [[String: Any]] {
            models = arr
        } else {
            return fail(entry, "error", "MiniMax 额度响应缺少 model_remains")
        }
        guard !models.isEmpty, let model = pickMinimaxPrimary(models) else {
            return fail(entry, "error", "MiniMax 额度响应缺少 model_remains")
        }
        return minimaxEntry(entry, model: model)
    }

    /// 多条 model_remains 取「剩余百分比最低」（最接近耗尽）的一条做主窗口。
    private func pickMinimaxPrimary(_ models: [[String: Any]]) -> [String: Any]? {
        guard let first = models.first else { return nil }
        return models.min { a, b in
            let pa = minimaxRemainingPct(a) ?? .infinity
            let pb = minimaxRemainingPct(b) ?? .infinity
            return pa < pb
        } ?? first
    }

    /// 剩余百分比：优先 current_interval_remaining_percent，缺失按计数回退
    /// （usage_count 语义是剩余量）。
    private func minimaxRemainingPct(_ model: [String: Any]) -> Double? {
        if let pct = flexDouble(model["current_interval_remaining_percent"]) {
            return pct
        }
        guard let total = flexDouble(model["current_interval_total_count"]),
              let remaining = flexDouble(model["current_interval_usage_count"]),
              total > 0 else { return nil }
        return remaining / total * 100.0
    }

    /// 已用百分比：优先 100 - remaining_percent；缺失按计数回退；clamp 0-100。
    private func minimaxUsedPercent(remainingPercent: Double?, total: Double?,
                                    remainingCount: Double?) -> Double? {
        if let pct = remainingPercent {
            return min(max(100.0 - pct, 0.0), 100.0)
        }
        guard let total = total, let remaining = remainingCount, total > 0 else { return nil }
        return min(max((total - remaining) / total * 100.0, 0.0), 100.0)
    }

    private func minimaxEntry(_ entry: AccountEntry, model: [String: Any]) -> QuotaEntry {
        let nowMs = Int(Date().timeIntervalSince1970 * 1000)
        // 窗口重置时间：end_time 优先；缺失时 now + remains_time 兜底（秒/毫秒自适应）
        let intervalReset = flexDouble(model["end_time"]).map(epochToMs)
            ?? flexDouble(model["remains_time"]).map { nowMs + epochToMs($0) }
        let weeklyReset = flexDouble(model["weekly_start_time"]).map {
            epochToMs($0) + 7 * 86_400_000
        }

        var windows: [QuotaWindow] = []
        // 会话窗口（主窗）：interval 三字段任一有值才产出
        if hasField(model, "current_interval_total_count")
            || hasField(model, "current_interval_usage_count")
            || hasField(model, "current_interval_remaining_percent") {
            windows.append(minimaxWindow(key: "interval", title: "当前窗口",
                                         total: flexDouble(model["current_interval_total_count"]),
                                         remainingCount: flexDouble(model["current_interval_usage_count"]),
                                         remainingPercent: flexDouble(model["current_interval_remaining_percent"]),
                                         resetsAt: intervalReset))
        }
        // 周窗（副窗）：weekly 三字段任一有值才产出
        if hasField(model, "current_weekly_total_count")
            || hasField(model, "current_weekly_usage_count")
            || hasField(model, "current_weekly_remaining_percent") {
            windows.append(minimaxWindow(key: "weekly", title: "本周",
                                         total: flexDouble(model["current_weekly_total_count"]),
                                         remainingCount: flexDouble(model["current_weekly_usage_count"]),
                                         remainingPercent: flexDouble(model["current_weekly_remaining_percent"]),
                                         resetsAt: weeklyReset))
        }
        return QuotaEntry(accountId: entry.id.uuidString,
                          label: entry.label,
                          agent: .minimax,
                          status: "ok",
                          windows: windows)
    }

    private func minimaxWindow(key: String, title: String, total: Double?,
                               remainingCount: Double?, remainingPercent: Double?,
                               resetsAt: Int?) -> QuotaWindow {
        let used: Double?
        if let total = total, let remaining = remainingCount {
            used = max(total - remaining, 0.0)
        } else {
            used = nil
        }
        return QuotaWindow(key: key,
                           title: title,
                           usedPercent: minimaxUsedPercent(remainingPercent: remainingPercent,
                                                           total: total,
                                                           remainingCount: remainingCount),
                           used: used,
                           total: total,
                           unit: "次",
                           resetsAt: resetsAt)
    }

    // MARK: - OpenCode Go（翻译 opencodego.rs::fetch_api_entry + entry_from_api_usage）

    private func queryOpencodeGo(_ entry: AccountEntry, token: String) async -> QuotaEntry {
        guard let url = URL(string: "https://opencode.ai/zen/go/v1/usage") else {
            return fail(entry, "error", "非法的用量接口 URL")
        }
        // 只发 Bearer：实测 x-api-key 返回 401，双头并发易踩鉴权歧义
        switch await perform(url, headers: [
            "Authorization": "Bearer \(token)",
            "Accept": "application/json",
        ]) {
        case .failed(let msg):
            return networkFail(entry, "OpenCode 用量", msg)
        case .ok(let code, let data):
            if code == 401 || code == 403 {
                return fail(entry, "expired", "API Key 无效或已过期")
            }
            guard code == 200 else {
                return fail(entry, "error", "OpenCode 用量查询失败（HTTP \(code)）")
            }
            let body = data.flatMap { String(data: $0, encoding: .utf8) } ?? ""
            guard let obj = vObject(v: parseJSON(body)),
                  let usage = vObject(v: obj["usage"]) else {
                return fail(entry, "error", "OpenCode 用量响应解析失败")
            }
            // (api 字段, 窗口 key, 展示名)；rolling = 滚动 5 小时窗
            let specs: [(String, String, String)] = [
                ("rolling", "hour5", "5 小时窗口"),
                ("weekly", "weekly", "本周"),
                ("monthly", "monthly", "本月"),
            ]
            var windows: [QuotaWindow] = []
            for (field, key, title) in specs {
                guard let win = vObject(v: usage[field]) else { continue }
                // percent 是唯一必填项；缺则视为该窗未接入（跳过而非报错）
                guard let percent = flexDouble(win["percent"]) else { continue }
                windows.append(QuotaWindow(
                    key: key,
                    title: title,
                    usedPercent: min(max(percent, 0.0), 100.0),
                    // 官方只给百分比，不透出金额：不虚构 used/total/unit
                    resetsAt: firstString(win, ["resetsAt", "resets_at"]).flatMap(parseRFC3339Ms)))
            }
            guard !windows.isEmpty else {
                return fail(entry, "error", "OpenCode 用量响应解析失败")
            }
            return QuotaEntry(accountId: entry.id.uuidString,
                              label: entry.label,
                              agent: .opencodego,
                              status: "ok",
                              windows: windows)
        }
    }

    // MARK: - 阿里 Coding Plan（翻译 alibaba.rs）

    /// 单个 region 的端点与参数集合（对齐桌面 RegionConf）。
    private struct AlibabaRegionConf {
        let host: String
        let regionId: String
        let commodityCode: String
        let referer: String
    }

    private func alibabaConf(_ region: AccountRegion?) -> AlibabaRegionConf {
        if region == .global {
            return AlibabaRegionConf(
                host: "https://modelstudio.console.alibabacloud.com",
                regionId: "ap-southeast-1",
                commodityCode: "sfm_codingplan_public_intl",
                referer: "https://modelstudio.console.alibabacloud.com/ap-southeast-1/?tab=coding-plan#/efm/coding_plan")
        }
        return AlibabaRegionConf(
            host: "https://bailian.console.aliyun.com",
            regionId: "cn-beijing",
            commodityCode: "sfm_codingplan_public_cn",
            referer: "https://bailian.console.aliyun.com/cn-beijing/?tab=model#/efm/coding_plan")
    }

    private func queryAlibaba(_ entry: AccountEntry, token: String) async -> QuotaEntry {
        let conf = alibabaConf(entry.region)
        let path = "/data/api.json?action=zeldaEasy.broadscope-bailian.codingPlan"
            + ".queryCodingPlanInstanceInfoV2&product=broadscope-bailian"
            + "&api=queryCodingPlanInstanceInfoV2&currentRegionId=\(conf.regionId)"
        guard let url = URL(string: conf.host + path) else {
            return fail(entry, "error", "非法的额度接口 URL")
        }
        let bodyObj: [String: Any] = [
            "queryCodingPlanInstanceInfoRequest": [
                "commodityCode": conf.commodityCode,
            ],
        ]
        guard let bodyData = try? JSONSerialization.data(withJSONObject: bodyObj) else {
            return fail(entry, "error", "构造请求体失败")
        }
        // 三个鉴权头同时发（Bearer / x-api-key / X-DashScope-API-Key）
        // + 浏览器仿真 UA + Origin/Referer（对齐桌面 fetch_instance_info_raw）
        switch await perform(url, method: "POST", headers: [
            "Content-Type": "application/json",
            "Accept": "application/json",
            "Authorization": "Bearer \(token)",
            "x-api-key": token,
            "X-DashScope-API-Key": token,
            "User-Agent": "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/143.0.0.0 Safari/537.36",
            "Origin": conf.host,
            "Referer": conf.referer,
        ], body: bodyData) {
        case .failed(let msg):
            return networkFail(entry, "阿里 Coding Plan 额度", msg)
        case .ok(let code, let data):
            if code == 401 || code == 403 {
                return fail(entry, "expired", "API Key 无效或已过期")
            }
            guard code == 200 else {
                return fail(entry, "error", "阿里 Coding Plan 额度查询失败（HTTP \(code)）")
            }
            let body = data.flatMap { String(data: $0, encoding: .utf8) } ?? ""
            guard let bodyObj = bodyObjValue(body), !bodyObj.isEmpty else {
                return fail(entry, "error", "阿里 Coding Plan 额度响应解析失败")
            }
            return alibabaEntry(entry, v: expandJSONStrings(bodyObj))
        }
    }

    /// 响应解析（对齐 entry_from_raw：信封错误码 → 实例选主 → 三窗口）。
    private func alibabaEntry(_ entry: AccountEntry, v: Any) -> QuotaEntry {
        let nowMs = Int(Date().timeIntervalSince1970 * 1000)
        // 信封错误码（数值 statusCode/code 非 0/200）
        if let statusCode = firstNum(v, ["statusCode", "status_code", "code"]),
           statusCode != 0 && statusCode != 200 {
            let message = firstString(v, ["statusMessage", "status_msg", "message", "msg"])
                ?? "unknown error"
            let lower = message.lowercased()
            if lower.contains("api key") || lower.contains("unauthorized") {
                return fail(entry, "expired", "API Key 无效或已过期")
            }
            return fail(entry, "error", "阿里云平台返回错误: \(message)（code \(Int(statusCode))）")
        }
        // 需要网页登录会话（ConsoleNeedLogin）：API Key 模式拿不到该账号数据
        let needLoginMsg = "该账号需要网页登录会话，API Key 模式暂不支持此账号"
        if let codeText = firstString(v, ["code", "status", "statusCode"]) {
            let lower = codeText.lowercased()
            if lower.contains("needlogin") || lower.contains("login") {
                return fail(entry, "error", needLoginMsg)
            }
        }
        if let messageText = firstString(v, ["message", "msg", "statusMessage"]) {
            let lower = messageText.lowercased()
            if lower.contains("log in") || lower.contains("login")
                || lower.contains("console session")
                || lower.contains("api key mode may be unavailable") {
                return fail(entry, "error", needLoginMsg)
            }
        }

        // 实例与配额提取
        let instances = firstArrayWithKeys(v, ["codingPlanInstanceInfos",
                                              "coding_plan_instance_infos"]) ?? []
        let instance = pickAlibabaInstance(instances, now: nowMs)
        let quota = instance.flatMap { findAlibabaQuotaInfo($0) } ?? findAlibabaQuotaInfo(v)
        guard let quota else {
            return fail(entry, "error", "阿里 Coding Plan 响应缺少配额数据")
        }
        let planName = alibabaPlanName(instance: instance, payload: v)

        // 三窗口：used/total 任一有值才产出（camelCase 优先，别名回退）
        let windowKeys: [(String, [String], [String], [String], String)] = [
            ("hour5",
             ["per5HourUsedQuota", "perFiveHourUsedQuota"],
             ["per5HourTotalQuota", "perFiveHourTotalQuota"],
             ["per5HourQuotaNextRefreshTime", "perFiveHourQuotaNextRefreshTime"],
             "5h"),
            ("weekly",
             ["perWeekUsedQuota"], ["perWeekTotalQuota"],
             ["perWeekQuotaNextRefreshTime"], "本周"),
            ("monthly",
             ["perBillMonthUsedQuota", "perMonthUsedQuota"],
             ["perBillMonthTotalQuota", "perMonthTotalQuota"],
             ["perBillMonthQuotaNextRefreshTime", "perMonthQuotaNextRefreshTime"],
             "本月"),
        ]
        var windows: [QuotaWindow] = []
        for (key, usedKeys, totalKeys, resetKeys, title) in windowKeys {
            let used = firstNumIn(quota, usedKeys)
            let total = firstNumIn(quota, totalKeys)
            let resetsAt = firstAny(quota, resetKeys).flatMap(parseTimeFlexible)
            if used == nil && total == nil { continue }
            windows.append(QuotaWindow(
                key: key,
                title: title,
                usedPercent: ratioPercent(used: used, total: total),
                used: used,
                total: total,
                unit: "次",
                resetsAt: resetsAt))
        }
        guard !windows.isEmpty else {
            return fail(entry, "error", "阿里 Coding Plan 响应缺少配额数据")
        }
        return QuotaEntry(accountId: entry.id.uuidString,
                          label: entry.label,
                          agent: .alibaba,
                          status: "ok",
                          windows: windows,
                          planName: planName)
    }

    /// 实例有效信号分（对齐 active_signal_score）：VALID/ACTIVE=3，
    /// expired 类=-1，endTime 在未来=1，其余 0。
    private func alibabaActiveScore(_ info: Any, now: Int) -> Int {
        if let status = firstString(info, ["status", "instanceStatus"]) {
            let upper = status.uppercased()
            if upper == "VALID" || upper == "ACTIVE" { return 3 }
            if ["EXPIRED", "INVALID", "INACTIVE", "DISABLED",
                "TERMINATED", "STOPPED"].contains(upper) { return -1 }
        }
        if let active = firstAny(info, ["isActive", "active"]) {
            if let b = active as? Bool { return b ? 3 : -1 }
            if let s = active as? String {
                switch s.trimmingCharacters(in: .whitespaces).lowercased() {
                case "true", "1", "yes", "active", "valid": return 3
                case "false", "0", "no", "inactive", "invalid", "expired": return -1
                default: break
                }
            }
        }
        if let t = firstAny(info, ["endTime", "periodEndTime", "expireTime",
                                   "expirationTime"]).flatMap(parseTimeFlexible),
           t > now {
            return 1
        }
        return 0
    }

    /// 多实例选主：信号分最高的实例优先（同分保留先出现者）；全部无正分回退第一条。
    private func pickAlibabaInstance(_ infos: [Any], now: Int) -> Any? {
        guard let first = infos.first else { return nil }
        var best = first
        var bestScore = alibabaActiveScore(first, now: now)
        for info in infos.dropFirst() {
            let score = alibabaActiveScore(info, now: now)
            if score > bestScore {
                best = info
                bestScore = score
            }
        }
        return best
    }

    /// 配额对象：先找 codingPlanQuotaInfo 键，回退「含任一窗口 used/total 键的对象」。
    private func findAlibabaQuotaInfo(_ v: Any) -> Any? {
        firstObjWithKeys(v, ["codingPlanQuotaInfo", "coding_plan_quota_info"])
            ?? firstObjWithKeys(v, [
                "per5HourUsedQuota", "perFiveHourUsedQuota",
                "per5HourTotalQuota", "perFiveHourTotalQuota",
                "perWeekUsedQuota", "perWeekTotalQuota",
                "perBillMonthUsedQuota", "perBillMonthTotalQuota",
            ])
    }

    /// 实例内取 plan 名：planName > instanceName > packageName，缺失回退父负载。
    private func alibabaPlanName(instance: Any?, payload: Any) -> String? {
        if let instance,
           let obj = instance as? [String: Any] {
            for keys in [["planName", "plan_name"],
                         ["instanceName", "instance_name"],
                         ["packageName", "package_name"]] {
                if let name = firstStringIn(obj, keys) { return name }
            }
        }
        for keys in [["planName", "plan_name"], ["packageName", "package_name"]] {
            if let name = firstString(payload, keys) { return name }
        }
        return nil
    }

    /// 已用百分比：used/total*100，clamp 0-100；total 无效返回 nil。
    private func ratioPercent(used: Double?, total: Double?) -> Double? {
        guard let total, total > 0, let used else { return nil }
        return min(max(used / total * 100.0, 0.0), 100.0)
    }

    // MARK: - 共用底层

    /// HTTP 结果：成功带状态码与响应体；失败带中文原因（不含 secret）。
    private enum HTTPResult {
        case ok(Int, Data?)
        case failed(String)
    }

    /// 统一错误条目（status 仅鉴权失败用 expired，其余 error）。
    private func fail(_ entry: AccountEntry, _ status: String, _ message: String) -> QuotaEntry {
        QuotaEntry(accountId: entry.id.uuidString,
                   label: entry.label,
                   agent: entry.agent,
                   status: status,
                   message: message)
    }

    /// 网络层失败的错误条目（文案 = 业务前缀 + 原因）。
    private func networkFail(_ entry: AccountEntry, _ prefix: String, _ reason: String) -> QuotaEntry {
        fail(entry, "error", "\(prefix)失败：\(reason)")
    }

    /// HTTP 请求（GET/POST）；网络层失败返回 .failed（中文原因，不含 secret）。
    private func perform(_ url: URL, method: String = "GET",
                         headers: [String: String],
                         body: Data? = nil) async -> HTTPResult {
        var req = URLRequest(url: url)
        req.httpMethod = method
        for (k, v) in headers { req.setValue(v, forHTTPHeaderField: k) }
        req.httpBody = body
        do {
            let (data, resp) = try await session.data(for: req)
            guard let http = resp as? HTTPURLResponse else {
                return .failed("非 HTTP 响应")
            }
            return .ok(http.statusCode, data)
        } catch {
            // 超时/DNS/连接被拒等：只描述错误类型，绝不带请求头（含 secret）
            let ns = error as NSError
            switch ns.code {
            case NSURLErrorTimedOut: return .failed("请求超时")
            case NSURLErrorCannotFindHost, NSURLErrorCannotConnectToHost:
                return .failed("无法连接服务")
            case NSURLErrorNetworkConnectionLost: return .failed("网络连接中断")
            case NSURLErrorNotConnectedToInternet: return .failed("无网络连接")
            default: return .failed("网络错误或服务不可用")
            }
        }
    }

    // MARK: - JSON 动态解析辅助（对齐桌面 serde_json::Value 的弹性取值）

    private func parseJSON(_ body: String) -> Any? {
        body.data(using: .utf8).flatMap { try? JSONSerialization.jsonObject(with: $0) }
    }

    private func bodyObjValue(_ body: String) -> [String: Any]? {
        vObject(v: parseJSON(body))
    }

    private func vObject(v: Any?) -> [String: Any]? {
        v as? [String: Any]
    }

    /// 弹性数值解析（对齐 parse_flexible_f64）：数字与字符串数字双形态；
    /// 空白/脏值/布尔返回 nil。
    private func flexDouble(_ any: Any?) -> Double? {
        switch any {
        case is Bool: return nil
        case let n as NSNumber: return n.doubleValue
        case let s as String:
            let trimmed = s.trimmingCharacters(in: .whitespaces)
            return Double(trimmed)
        default: return nil
        }
    }

    /// epoch 秒/毫秒自适应（对齐 epoch_to_ms）：>10^12 视为毫秒。
    private func epochToMs(_ raw: Double) -> Int {
        if raw > 1_000_000_000_000.0 {
            return Int(raw)
        }
        return Int(raw * 1000.0)
    }

    /// RFC3339 时间戳 → 毫秒（对齐 parse_rfc3339_ms）。
    private func parseRFC3339Ms(_ s: String) -> Int? {
        let iso = ISO8601DateFormatter()
        iso.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        if let d = iso.date(from: s.trimmingCharacters(in: .whitespaces)) {
            return Int(d.timeIntervalSince1970 * 1000)
        }
        let plain = ISO8601DateFormatter()
        if let d = plain.date(from: s.trimmingCharacters(in: .whitespaces)) {
            return Int(d.timeIntervalSince1970 * 1000)
        }
        return nil
    }

    /// 时间字段自适应解析（对齐 cookie_util::parse_time_flexible）：
    /// 数字（epoch 秒/毫秒自适应）、字符串数字、RFC3339、yyyy-MM-dd HH:mm:ss、
    /// yyyy-MM-dd（无时区按 UTC）。
    private func parseTimeFlexible(_ any: Any?) -> Int? {
        if let n = flexDouble(any) {
            return epochToMs(n)
        }
        guard let s = any as? String else { return nil }
        let trimmed = s.trimmingCharacters(in: .whitespaces)
        guard !trimmed.isEmpty else { return nil }
        if let n = Double(trimmed) { return epochToMs(n) }
        if let ms = parseRFC3339Ms(trimmed) { return ms }
        // yyyy-MM-dd HH:mm:ss（无时区按 UTC，对齐 chrono NaiveDateTime::and_utc）
        let f = DateFormatter()
        f.locale = Locale(identifier: "en_US_POSIX")
        f.timeZone = TimeZone(identifier: "UTC")
        f.dateFormat = "yyyy-MM-dd HH:mm:ss"
        if let d = f.date(from: trimmed) {
            return Int(d.timeIntervalSince1970 * 1000)
        }
        f.dateFormat = "yyyy-MM-dd"
        if let d = f.date(from: trimmed) {
            return Int(d.timeIntervalSince1970 * 1000)
        }
        return nil
    }

    /// 字段是否有值（窗口裁剪用）：JSON null / NSNull / 缺失都算无值。
    private func hasField(_ obj: [String: Any], _ key: String) -> Bool {
        guard let v = obj[key] else { return false }
        return !(v is NSNull)
    }

    /// 本层按键序尝试取第一个存在的值（对齐 get_any）。
    private func firstAny(_ v: Any, _ keys: [String]) -> Any? {
        guard let obj = v as? [String: Any] else { return nil }
        for key in keys {
            if let val = obj[key], !(val is NSNull) {
                return val
            }
        }
        return nil
    }

    /// 深度优先找第一个非空字符串（对齐 find_first_str）。
    private func firstString(_ v: Any, _ keys: [String]) -> String? {
        if let obj = v as? [String: Any] {
            for key in keys {
                if let s = obj[key] as? String {
                    let trimmed = s.trimmingCharacters(in: .whitespaces)
                    if !trimmed.isEmpty { return trimmed }
                }
            }
            for val in obj.values {
                if let found = firstString(val, keys) { return found }
            }
        } else if let arr = v as? [Any] {
            for item in arr {
                if let found = firstString(item, keys) { return found }
            }
        }
        return nil
    }

    /// 仅本层按键序找非空字符串（alibabaPlanName 用）。
    private func firstStringIn(_ obj: [String: Any], _ keys: [String]) -> String? {
        for key in keys {
            if let s = obj[key] as? String {
                let trimmed = s.trimmingCharacters(in: .whitespaces)
                if !trimmed.isEmpty { return trimmed }
            }
        }
        return nil
    }

    /// 深度优先找第一个可解析数值（对齐 find_first_num）。
    private func firstNum(_ v: Any, _ keys: [String]) -> Double? {
        if let obj = v as? [String: Any] {
            for key in keys {
                if let n = flexDouble(obj[key]) { return n }
            }
            for val in obj.values {
                if let found = firstNum(val, keys) { return found }
            }
        } else if let arr = v as? [Any] {
            for item in arr {
                if let found = firstNum(item, keys) { return found }
            }
        }
        return nil
    }

    /// 仅本层按键序找数值（三窗口字段用，配额对象已定位）。
    private func firstNumIn(_ obj: Any, _ keys: [String]) -> Double? {
        guard let dict = obj as? [String: Any] else { return nil }
        for key in keys {
            if let n = flexDouble(dict[key]) { return n }
        }
        return nil
    }

    /// 深度优先找第一个「数组值且含任一目标键」（对齐 find_first_arr_with_keys）。
    private func firstArrayWithKeys(_ v: Any, _ keys: [String]) -> [Any]? {
        if let obj = v as? [String: Any] {
            for key in keys {
                if let arr = obj[key] as? [Any] { return arr }
            }
            for val in obj.values {
                if let found = firstArrayWithKeys(val, keys) { return found }
            }
        } else if let arr = v as? [Any] {
            for item in arr {
                if let found = firstArrayWithKeys(item, keys) { return found }
            }
        }
        return nil
    }

    /// 深度优先找第一个「对象值且含任一目标键」（对齐 find_first_obj_with_keys）。
    private func firstObjWithKeys(_ v: Any, _ keys: [String]) -> Any? {
        if let obj = v as? [String: Any] {
            for key in keys {
                if let o = obj[key] as? [String: Any] { return o }
            }
            for val in obj.values {
                if let found = firstObjWithKeys(val, keys) { return found }
            }
        } else if let arr = v as? [Any] {
            for item in arr {
                if let found = firstObjWithKeys(item, keys) { return found }
            }
        }
        return nil
    }

    /// 嵌套 JSON 字符串展开（对齐 expand_json_strings）：阿里 console 网关
    /// 有时把对象序列化成字符串塞在值里，递归遇到「能完整解析为对象/数组
    /// 的字符串值」就替换为解析结果再展开。
    private func expandJSONStrings(_ v: Any) -> Any {
        switch v {
        case let s as String:
            let trimmed = s.trimmingCharacters(in: .whitespaces)
            if trimmed.hasPrefix("{") || trimmed.hasPrefix("[") {
                if let nested = parseJSON(trimmed),
                   nested is [String: Any] || nested is [Any] {
                    return expandJSONStrings(nested)
                }
            }
            return s
        case let arr as [Any]:
            return arr.map { expandJSONStrings($0) }
        case let obj as [String: Any]:
            var out: [String: Any] = [:]
            for (k, val) in obj {
                out[k] = expandJSONStrings(val)
            }
            return out
        default:
            return v
        }
    }
}
