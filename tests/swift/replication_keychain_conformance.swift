import Foundation
import Security

@main
private enum ReplicationKeychainConformance {
    static func main() throws {
        let service = "com.termirust.test.replication.\(UUID().uuidString)"
        let cleanupQuery: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrSynchronizable as String: false
        ]
        defer {
            let status = SecItemDelete(cleanupQuery as CFDictionary)
            precondition(status == errSecSuccess || status == errSecItemNotFound)
        }
        let first = ReplicationKeychainStore(service: service)
        let second = ReplicationKeychainStore(service: service)
        let separateNamespace = ReplicationKeychainStore(service: service + ".separate")
        defer { _ = try? separateNamespace.delete(account: "collision-test") }
        let original = Data(repeating: 7, count: 47)
        try separateNamespace.create(account: "collision-test", value: original)
        try first.create(account: "collision-test", value: original)
        do {
            try second.create(account: "collision-test", value: Data(repeating: 8, count: 47))
            preconditionFailure("Duplicate creation must fail")
        } catch ReplicationStorageError.Collision {}
        let loaded = try second.load(account: "collision-test")
        precondition(loaded == original)

        for invalid in ["", "has space", "line\nbreak", String(repeating: "a", count: 129)] {
            do {
                _ = try first.delete(account: invalid)
                preconditionFailure("Invalid account must fail")
            } catch ReplicationStorageError.Invalid {}
        }
        do {
            try first.create(account: "short", value: Data([1]))
            preconditionFailure("Invalid envelope length must fail")
        } catch ReplicationStorageError.Invalid {}
        let shortAbsent = try first.delete(account: "short")
        precondition(!shortAbsent)

        let engine = ReplicationCustody(store: first)
        let identity = try engine.createDeviceIdentity()
        let other = try engine.createDeviceIdentity()
        let reopened = ReplicationCustody(store: second)
        let publicKey = try reopened.devicePublicKey(secretReference: identity.secretReference)
        precondition(publicKey == identity.publicKey)
        let deleted = try reopened.deleteDeviceIdentity(secretReference: identity.secretReference)
        precondition(deleted)
        let deletedAgain = try reopened.deleteDeviceIdentity(secretReference: identity.secretReference)
        precondition(!deletedAgain)
        do {
            _ = try reopened.devicePublicKey(secretReference: identity.secretReference)
            preconditionFailure("Deleted identity must be missing")
        } catch ReplicationStorageError.Missing {}
        let otherKey = try reopened.devicePublicKey(secretReference: other.secretReference)
        precondition(otherKey == other.publicKey)
        let preserved = try first.load(account: "collision-test")
        precondition(preserved == original)

        let corruptQuery = cleanupQuery.merging([kSecAttrAccount as String: "collision-test"]) { _, new in new }
        precondition(SecItemUpdate(corruptQuery as CFDictionary, [
            kSecValueData as String: Data([1])
        ] as CFDictionary) == errSecSuccess)
        do {
            _ = try first.load(account: "collision-test")
            preconditionFailure("Corrupt envelope must fail")
        } catch ReplicationStorageError.Invalid {}

        let removedCorrupt = try first.delete(account: "collision-test")
        precondition(removedCorrupt)
        let separatelyStored = try separateNamespace.load(account: "collision-test")
        precondition(separatelyStored == original)

        for status in [errSecInteractionNotAllowed, errSecAuthFailed, errSecUserCanceled, errSecMissingEntitlement] {
            guard case .Locked = ReplicationKeychainStore.map(status) else {
                preconditionFailure("Access failures must not become missing")
            }
        }
        print("Native Keychain replication custody conformance passed")
    }
}
