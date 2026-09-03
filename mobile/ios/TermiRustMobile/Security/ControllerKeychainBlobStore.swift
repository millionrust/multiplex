import Foundation
import Security

final class ControllerKeychainBlobStore: SecureBlobStore, @unchecked Sendable {
    private let service: String

    init(service: String = "com.termirust.mobile.controller.v1") {
        self.service = service
    }

    func load(keyId: String) throws -> Data? {
        var item: CFTypeRef?
        let status = SecItemCopyMatching(baseQuery(keyId: keyId).merging([
            kSecReturnData as String: true,
            kSecMatchLimit as String: kSecMatchLimitOne
        ]) { _, new in new } as CFDictionary, &item)
        if status == errSecItemNotFound { return nil }
        guard status == errSecSuccess, let data = item as? Data else {
            throw map(status)
        }
        return data
    }

    func store(keyId: String, value: Data) throws {
        guard !value.isEmpty, value.count <= 4 * 1_024 else {
            throw SecureBlobError.Invalid
        }
        let query = baseQuery(keyId: keyId)
        let updateStatus = SecItemUpdate(
            query as CFDictionary,
            [kSecValueData as String: value] as CFDictionary
        )
        if updateStatus == errSecSuccess { return }
        guard updateStatus == errSecItemNotFound else { throw map(updateStatus) }

        let status = SecItemAdd(query.merging([
            kSecAttrAccessible as String: kSecAttrAccessibleWhenUnlockedThisDeviceOnly,
            kSecValueData as String: value
        ]) { _, new in new } as CFDictionary, nil)
        guard status == errSecSuccess else { throw map(status) }
    }

    func delete(keyId: String) throws {
        let status = SecItemDelete(baseQuery(keyId: keyId) as CFDictionary)
        guard status == errSecSuccess || status == errSecItemNotFound else {
            throw map(status)
        }
    }

    private func baseQuery(keyId: String) -> [String: Any] {
        [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: keyId,
            kSecAttrSynchronizable as String: false
        ]
    }

    private func map(_ status: OSStatus) -> SecureBlobError {
        switch status {
        case errSecItemNotFound:
            return .Missing
        case errSecInteractionNotAllowed, errSecAuthFailed, errSecUserCanceled:
            return .Locked
        case errSecMissingEntitlement:
            return .PermissionDenied
        case errSecParam, errSecDecode:
            return .Invalid
        default:
            return .Unavailable
        }
    }
}
