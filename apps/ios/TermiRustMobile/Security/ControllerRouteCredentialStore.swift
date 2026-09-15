import Foundation
import Security

protocol ControllerRouteCredentialStoring: Sendable {
    func store(
        _ secret: Data,
        hostID: String,
        reference: ControllerRouteCredentialReference
    ) throws
    func load(
        hostID: String,
        reference: ControllerRouteCredentialReference
    ) throws -> Data?
    func delete(
        hostID: String,
        reference: ControllerRouteCredentialReference
    ) throws
}

final class ControllerRouteCredentialStore: ControllerRouteCredentialStoring, @unchecked Sendable {
    private let service: String

    init(service: String = "com.termirust.mobile.controller.routes.v1") {
        self.service = service
    }

    func store(
        _ secret: Data,
        hostID: String,
        reference: ControllerRouteCredentialReference
    ) throws {
        guard !secret.isEmpty, secret.count <= 64 * 1_024 else {
            throw ControllerRouteCredentialStoreError.invalidSecret
        }
        let query = try baseQuery(hostID: hostID, reference: reference)
        let update = SecItemUpdate(
            query as CFDictionary,
            [kSecValueData as String: secret] as CFDictionary
        )
        if update == errSecSuccess { return }
        guard update == errSecItemNotFound else { throw map(update) }
        let status = SecItemAdd(query.merging([
            kSecAttrAccessible as String: kSecAttrAccessibleWhenUnlockedThisDeviceOnly,
            kSecValueData as String: secret,
        ]) { _, new in new } as CFDictionary, nil)
        guard status == errSecSuccess else { throw map(status) }
    }

    func load(
        hostID: String,
        reference: ControllerRouteCredentialReference
    ) throws -> Data? {
        let query = try baseQuery(hostID: hostID, reference: reference).merging([
            kSecReturnData as String: true,
            kSecMatchLimit as String: kSecMatchLimitOne,
        ]) { _, new in new }
        var item: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &item)
        if status == errSecItemNotFound { return nil }
        guard status == errSecSuccess, let data = item as? Data else { throw map(status) }
        return data
    }

    func delete(
        hostID: String,
        reference: ControllerRouteCredentialReference
    ) throws {
        let status = SecItemDelete(
            try baseQuery(hostID: hostID, reference: reference) as CFDictionary
        )
        guard status == errSecSuccess || status == errSecItemNotFound else { throw map(status) }
    }

    private func baseQuery(
        hostID: String,
        reference: ControllerRouteCredentialReference
    ) throws -> [String: Any] {
        let host = hostID.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !host.isEmpty,
              host.utf8.count <= 128,
              host.unicodeScalars.allSatisfy({
                  CharacterSet.alphanumerics.contains($0) || "-_.".unicodeScalars.contains($0)
              }) else {
            throw ControllerRouteCredentialStoreError.invalidHost
        }
        return [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String:
                "\(host):\(reference.route.rawValue):\(reference.purpose.rawValue):\(reference.id)",
            kSecAttrSynchronizable as String: false,
        ]
    }

    private func map(_ status: OSStatus) -> ControllerRouteCredentialStoreError {
        switch status {
        case errSecInteractionNotAllowed, errSecAuthFailed, errSecUserCanceled:
            return .locked
        case errSecMissingEntitlement:
            return .permissionDenied
        default:
            return .unavailable
        }
    }
}

enum ControllerRouteCredentialStoreError: Error, Equatable, Sendable {
    case invalidHost
    case invalidSecret
    case locked
    case permissionDenied
    case unavailable
}
