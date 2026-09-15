import Foundation

private final class MemoryReplicationStore: ReplicationSecureStore, @unchecked Sendable {
    private let lock = NSLock()
    private var records: [String: Data] = [:]

    func create(account: String, value: Data) throws {
        lock.lock()
        defer { lock.unlock() }
        guard records[account] == nil else { throw ReplicationStorageError.Collision }
        records[account] = value
    }

    func load(account: String) throws -> Data {
        lock.lock()
        defer { lock.unlock() }
        guard let value = records[account] else { throw ReplicationStorageError.Missing }
        return value
    }

    func delete(account: String) throws -> Bool {
        lock.lock()
        defer { lock.unlock() }
        return records.removeValue(forKey: account) != nil
    }
}

@main
private enum ReplicationCustodyConformance {
    static func main() throws {
        let store = MemoryReplicationStore()
        let engine = ReplicationCustody(store: store)
        let identity = try engine.createDeviceIdentity()
        let other = try engine.createDeviceIdentity()
        precondition(identity.publicKey.count == 32)
        precondition(identity.secretReference.count == 47)

        let reopened = ReplicationCustody(store: store)
        let publicKey = try reopened.devicePublicKey(secretReference: identity.secretReference)
        precondition(publicKey == identity.publicKey)
        let deleted = try reopened.deleteDeviceIdentity(secretReference: identity.secretReference)
        precondition(deleted)
        let deletedAgain = try reopened.deleteDeviceIdentity(secretReference: identity.secretReference)
        precondition(!deletedAgain)
        do {
            _ = try reopened.devicePublicKey(secretReference: identity.secretReference)
            preconditionFailure("Deleted identity unexpectedly loaded")
        } catch ReplicationStorageError.Missing {
            // Missing must cross the callback and FFI boundary without becoming success.
        }
        let otherKey = try reopened.devicePublicKey(secretReference: other.secretReference)
        precondition(otherKey == other.publicKey)
        print("Swift replication custody round trip passed")
    }
}
