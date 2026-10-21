//
//  KeychainHelper.swift
//  agenthud
//
//  额度账号 secret 的 Keychain 存取（kSecClassGenericPassword）。
//  安全契约（对齐桌面 provider_credentials）：
//   - secret 永不回显、永不写日志、永不进错误消息；
//   - 元数据（AccountEntry）存 UserDefaults JSON，本类只管 secret 本体；
//   - service 固定，account 用账号 id.uuidString，一条账号一条 secret。
//

import Foundation
import Security

enum KeychainHelper {
    /// 账号 secret 的 Keychain service。
    static let service = "com.chacca.agenthud.accounts"

    // MARK: - CRUD

    /// 写入（或覆盖）一条 secret。空串等价于删除。
    /// 返回是否写入成功（SecItemAdd 状态码）：调用方需要对失败敏感的场景
    /// （如旧配置迁移）应检查返回值并回读校验，避免"以为存上了"。
    @discardableResult
    static func saveSecret(_ secret: String, account: String) -> Bool {
        let trimmed = secret.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else {
            deleteSecret(account: account)
            return false
        }
        let data = Data(trimmed.utf8)
        // 先删后写：SecItemUpdate 在属性变化时的行为跨版本不稳，覆盖语义最简单
        deleteSecret(account: account)
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
            kSecValueData as String: data,
            // 仅本 App 可读（默认即如此，显式声明防止全局配置漂移）
            kSecAttrAccessible as String: kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly,
        ]
        // 返回值不携带 secret 内容，仅状态；读回为 nil 时 UI 呈现未配置
        return SecItemAdd(query as CFDictionary, nil) == errSecSuccess
    }

    /// 读取一条 secret；不存在/失败返回 nil。
    static func loadSecret(account: String) -> String? {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
            kSecReturnData as String: true,
            kSecMatchLimit as String: kSecMatchLimitOne,
        ]
        var result: AnyObject?
        let status = SecItemCopyMatching(query as CFDictionary, &result)
        guard status == errSecSuccess, let data = result as? Data else { return nil }
        return String(data: data, encoding: .utf8)
    }

    /// 删除一条 secret；不存在时静默成功。
    static func deleteSecret(account: String) {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
        ]
        _ = SecItemDelete(query as CFDictionary)
    }

    // MARK: - 掩码（编辑表单展示用，绝不展示原文）

    /// secret 掩码：前 6 + … + 后 4；过短时只给省略号。
    static func maskSecret(_ secret: String) -> String {
        let trimmed = secret.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return "" }
        guard trimmed.count > 12 else { return "••••••" }
        return "\(trimmed.prefix(6))…\(trimmed.suffix(4))"
    }
}
