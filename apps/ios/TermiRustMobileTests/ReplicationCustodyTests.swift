import Foundation
import Security
import XCTest
@testable import TermiRustMobile

final class ReplicationCustodyTests: XCTestCase, @unchecked Sendable {
    private func namespace() -> String {
        "com.termirust.test.replication.c02.\(UUID().uuidString)"
    }

    func testProductEnrollmentReopensAndCancelsThroughRealKeychain() throws {
        let service = namespace()
        let folder = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: false)
        defer { cleanup(service); try? FileManager.default.removeItem(at: folder) }
        let path = folder.resolvingSymlinksInPath().path
        let store = ReplicationKeychainStore(service: service)
        let custody = ReplicationCustody(store: store)
        let sentinel = try custody.createDeviceIdentity()
        let facade = try MobileReplicationProduct(privateDirectory: path, store: store)
        XCTAssertNil(try facade.pendingEnrollment())
        let request = try facade.prepareEnrollment(localExchangeDirectory: path)
        XCTAssertFalse(request.canonicalRequest.isEmpty)
        XCTAssertThrowsError(try facade.prepareEnrollment(localExchangeDirectory: path)) {
            guard case MobileReplicationError.AlreadyConfigured = $0 else { return XCTFail("Expected AlreadyConfigured") }
        }
        let reopened = try MobileReplicationProduct(privateDirectory: path, store: ReplicationKeychainStore(service: service))
        XCTAssertEqual(try reopened.pendingEnrollment()?.canonicalRequest, request.canonicalRequest)
        XCTAssertTrue(try reopened.cancelPendingEnrollment(expectedRequest: request.canonicalRequest))
        XCTAssertFalse(try reopened.cancelPendingEnrollment(expectedRequest: request.canonicalRequest))
        XCTAssertNil(try reopened.pendingEnrollment())
        XCTAssertEqual(try custody.devicePublicKey(secretReference: sentinel.secretReference), sentinel.publicKey)
        XCTAssertThrowsError(try facade.prepareEnrollment(localExchangeDirectory: "content://provider/test"))
    }

    private func query(_ service: String, account: String? = nil) -> [String: Any] {
        var result: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrSynchronizable as String: false
        ]
        if let account { result[kSecAttrAccount as String] = account }
        return result
    }

    private func cleanup(_ service: String) {
        let status = SecItemDelete(query(service) as CFDictionary)
        XCTAssertTrue(status == errSecSuccess || status == errSecItemNotFound)
    }

    func testRealRustIdentityReopenAndExactDeletionPreserveController() throws {
        let service = namespace()
        let controllerService = namespace() + ".controller"
        defer { cleanup(service); cleanup(controllerService) }
        let controller = ControllerKeychainBlobStore(service: controllerService)
        let sentinel = Data([1, 2, 3, 4])
        try controller.store(keyId: "sentinel", value: sentinel)
        let engine = ReplicationCustody(store: ReplicationKeychainStore(service: service))
        let identity = try engine.createDeviceIdentity()
        let other = try engine.createDeviceIdentity()
        let reopened = ReplicationCustody(store: ReplicationKeychainStore(service: service))
        XCTAssertEqual(try reopened.devicePublicKey(secretReference: identity.secretReference), identity.publicKey)
        XCTAssertEqual(try reopened.devicePublicKey(secretReference: other.secretReference), other.publicKey)
        XCTAssertTrue(try reopened.deleteDeviceIdentity(secretReference: identity.secretReference))
        XCTAssertFalse(try reopened.deleteDeviceIdentity(secretReference: identity.secretReference))
        XCTAssertThrowsError(try reopened.devicePublicKey(secretReference: identity.secretReference)) {
            guard case ReplicationStorageError.Missing = $0 else { return XCTFail("Expected Missing") }
        }
        XCTAssertEqual(try reopened.devicePublicKey(secretReference: other.secretReference), other.publicKey)
        XCTAssertEqual(try controller.load(keyId: "sentinel"), sentinel)
    }

    func testConcurrentCreateAcrossInstancesNeverOverwrites() async throws {
        let service = namespace()
        defer { cleanup(service) }
        let results = await withTaskGroup(of: Int.self, returning: [Int].self) { group in
            for value in [7, 8] {
                group.addTask {
                    do {
                        try ReplicationKeychainStore(service: service).create(
                            account: "race", value: Data(repeating: UInt8(value), count: 47))
                        return value
                    } catch ReplicationStorageError.Collision { return 0 }
                    catch { return -1 }
                }
            }
            var values: [Int] = []
            for await result in group { values.append(result) }
            return values
        }
        XCTAssertEqual(results.filter { $0 == 0 }.count, 1)
        let winners = results.filter { $0 == 7 || $0 == 8 }
        XCTAssertEqual(winners.count, 1)
        let winner = try XCTUnwrap(winners.first)
        XCTAssertEqual(try ReplicationKeychainStore(service: service).load(account: "race"),
                       Data(repeating: UInt8(winner), count: 47))
    }

    func testMalformedRecordRemainsPresentAndCannotBeReplaced() throws {
        let service = namespace()
        defer { cleanup(service) }
        let store = ReplicationKeychainStore(service: service)
        try store.create(account: "corrupt", value: Data(repeating: 3, count: 47))
        for malformed in [Data(), Data([1]), Data(repeating: 2, count: 48)] {
            XCTAssertEqual(SecItemUpdate(query(service, account: "corrupt") as CFDictionary,
                [kSecValueData as String: malformed] as CFDictionary), errSecSuccess)
            XCTAssertThrowsError(try store.load(account: "corrupt")) {
                guard case ReplicationStorageError.Invalid = $0 else { return XCTFail("Expected Invalid") }
            }
            XCTAssertThrowsError(try store.create(account: "corrupt", value: Data(repeating: 4, count: 47))) {
                guard case ReplicationStorageError.Collision = $0 else { return XCTFail("Expected Collision") }
            }
            var raw: CFTypeRef?
            let read = query(service, account: "corrupt").merging([
                kSecReturnData as String: true
            ]) { _, new in new }
            XCTAssertEqual(SecItemCopyMatching(read as CFDictionary, &raw), errSecSuccess)
            XCTAssertEqual(raw as? Data, malformed)
        }
        XCTAssertTrue(try store.delete(account: "corrupt"))
        XCTAssertFalse(try store.delete(account: "corrupt"))
    }

    func testInvalidInputsAndNamespaceIsolation() throws {
        let service = namespace()
        let otherService = namespace()
        defer { cleanup(service); cleanup(otherService) }
        let store = ReplicationKeychainStore(service: service)
        let other = ReplicationKeychainStore(service: otherService)
        let sentinel = Data(repeating: 9, count: 47)
        try store.create(account: "same", value: sentinel)
        try other.create(account: "same", value: sentinel)
        XCTAssertTrue(try store.delete(account: "same"))
        XCTAssertEqual(try other.load(account: "same"), sentinel)
        for account in ["", "has space", "line\nbreak", "\u{e9}", String(repeating: "a", count: 129)] {
            XCTAssertThrowsError(try store.create(account: account, value: sentinel)) {
                guard case ReplicationStorageError.Invalid = $0 else { return XCTFail("Expected Invalid") }
            }
            XCTAssertThrowsError(try store.load(account: account))
            XCTAssertThrowsError(try store.delete(account: account))
        }
        XCTAssertThrowsError(try store.create(account: "short", value: Data([1])))
        XCTAssertFalse(try store.delete(account: "short"))
        XCTAssertThrowsError(try ReplicationCustody(store: store).devicePublicKey(secretReference: Data("not-a-reference".utf8)))
    }

    func testKeychainAttributesAndSimulatedErrorMapping() throws {
        let service = namespace()
        defer { cleanup(service) }
        try ReplicationKeychainStore(service: service).create(account: "attributes", value: Data(repeating: 1, count: 47))
        var item: CFTypeRef?
        let attributesQuery = query(service, account: "attributes").merging([
            kSecReturnAttributes as String: true
        ]) { _, new in new }
        XCTAssertEqual(SecItemCopyMatching(attributesQuery as CFDictionary, &item), errSecSuccess)
        let attributes = try XCTUnwrap(item as? [String: Any])
        XCTAssertEqual(attributes[kSecAttrAccessible as String] as? String, kSecAttrAccessibleWhenUnlockedThisDeviceOnly as String)
        XCTAssertEqual(attributes[kSecAttrSynchronizable as String] as? Bool, false)
        // Status mapping is simulated, not evidence of physical lock/unlock behavior.
        for status in [errSecInteractionNotAllowed, errSecAuthFailed, errSecUserCanceled, errSecMissingEntitlement] {
            guard case .Locked = ReplicationKeychainStore.map(status) else { return XCTFail("Expected Locked") }
        }
        guard case .Unavailable = ReplicationKeychainStore.map(errSecNotAvailable) else { return XCTFail("Expected Unavailable") }
    }
}
