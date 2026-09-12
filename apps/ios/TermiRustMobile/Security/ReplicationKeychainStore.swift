import Foundation
import Security

/// Replication custody is isolated from Controller pairing and SSH credentials.
final class ReplicationKeychainStore: ReplicationSecureStore, Sendable {
    private let service: String
    private static let secretBytes = 47

    init(service: String = "com.termirust.mobile.replication.v1") {
        self.service = service
    }

    func create(account: String, value: Data) throws {
        try validate(account)
        guard value.count == Self.secretBytes else { throw ReplicationStorageError.Invalid }
        // SecItemAdd provides atomic collision rejection across store instances.
        let status = SecItemAdd(query(account).merging([
            kSecAttrAccessible as String: kSecAttrAccessibleWhenUnlockedThisDeviceOnly,
            kSecValueData as String: value
        ]) { _, new in new } as CFDictionary, nil)
        guard status == errSecSuccess else { throw Self.map(status) }
    }

    func load(account: String) throws -> Data {
        try validate(account)
        var item: CFTypeRef?
        let status = SecItemCopyMatching(query(account).merging([
            kSecReturnData as String: true,
            kSecMatchLimit as String: kSecMatchLimitOne
        ]) { _, new in new } as CFDictionary, &item)
        guard status == errSecSuccess else { throw Self.map(status) }
        guard let data = item as? Data, data.count == Self.secretBytes else {
            throw ReplicationStorageError.Invalid
        }
        return data
    }

    func delete(account: String) throws -> Bool {
        try validate(account)
        let status = SecItemDelete(query(account) as CFDictionary)
        if status == errSecItemNotFound { return false }
        guard status == errSecSuccess else { throw Self.map(status) }
        return true
    }

    private func query(_ account: String) -> [String: Any] {
        [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
            kSecAttrSynchronizable as String: false
        ]
    }

    private func validate(_ account: String) throws {
        guard (1...128).contains(account.utf8.count),
              account.utf8.allSatisfy({ (33...126).contains($0) }) else {
            throw ReplicationStorageError.Invalid
        }
    }

    static func map(_ status: OSStatus) -> ReplicationStorageError {
        switch status {
        case errSecItemNotFound: .Missing
        case errSecDuplicateItem: .Collision
        case errSecInteractionNotAllowed, errSecAuthFailed, errSecUserCanceled,
             errSecMissingEntitlement: .Locked
        case errSecParam, errSecDecode: .Invalid
        default: .Unavailable
        }
    }
}
