import Foundation
import LocalAuthentication
import Security

enum SecretStoreError: Error, LocalizedError {
    case encodingFailed
    case accessControlUnavailable
    case authenticationRequired
    case unhandledStatus(OSStatus)

    var errorDescription: String? {
        switch self {
        case .encodingFailed:
            return "Unable to encode secret for Keychain storage."
        case .accessControlUnavailable:
            return "Unable to require device authentication for Keychain storage."
        case .authenticationRequired:
            return "Unlock this device with passcode or biometrics before using TermiRust mobile SSH credentials."
        case .unhandledStatus(let status):
            return "Keychain operation failed with status \(status)."
        }
    }
}

protocol SecretStoring {
    func saveSecret(_ secret: String, account: String) throws
    func readSecret(account: String) throws -> String?
    func deleteSecret(account: String) throws
}

final class KeychainSecretStore: SecretStoring {
    private let service: String

    init(service: String) {
        self.service = service
    }

    func saveSecret(_ secret: String, account: String) throws {
        guard let data = secret.data(using: .utf8) else {
            throw SecretStoreError.encodingFailed
        }

        try deleteSecret(account: account)

        guard let accessControl = SecAccessControlCreateWithFlags(
            nil,
            kSecAttrAccessibleWhenPasscodeSetThisDeviceOnly,
            [.userPresence],
            nil
        ) else {
            throw SecretStoreError.accessControlUnavailable
        }

        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
            kSecAttrAccessControl as String: accessControl,
            kSecValueData as String: data
        ]
        let status = SecItemAdd(query as CFDictionary, nil)
        guard status == errSecSuccess else {
            throw Self.error(for: status)
        }
    }

    func readSecret(account: String) throws -> String? {
        let context = LAContext()
        context.localizedReason = "Unlock TermiRust mobile SSH credential."
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
            kSecReturnData as String: true,
            kSecMatchLimit as String: kSecMatchLimitOne,
            kSecUseAuthenticationContext as String: context
        ]
        var item: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &item)
        if status == errSecItemNotFound {
            return nil
        }
        guard status == errSecSuccess else {
            throw Self.error(for: status)
        }
        guard let data = item as? Data else {
            return nil
        }
        return String(data: data, encoding: .utf8)
    }

    func deleteSecret(account: String) throws {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account
        ]
        let status = SecItemDelete(query as CFDictionary)
        guard status == errSecSuccess || status == errSecItemNotFound else {
            throw Self.error(for: status)
        }
    }

    private static func error(for status: OSStatus) -> SecretStoreError {
        switch status {
        case errSecAuthFailed, errSecInteractionNotAllowed, errSecUserCanceled:
            return .authenticationRequired
        default:
            return .unhandledStatus(status)
        }
    }
}
