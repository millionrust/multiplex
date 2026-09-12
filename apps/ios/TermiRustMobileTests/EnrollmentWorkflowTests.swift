import Foundation
import Security
import XCTest
@testable import TermiRustMobile

final class EnrollmentWorkflowTests: XCTestCase, @unchecked Sendable {
    func testRealKeychainRequestAndCancellationRecovery() async throws {
        let service = "com.termirust.test.c05.\(UUID().uuidString)"
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer {
            try? FileManager.default.removeItem(at: directory)
            SecItemDelete([kSecClass: kSecClassGenericPassword, kSecAttrService: service] as CFDictionary)
        }
        let store = ReplicationKeychainStore(service: service)
        let repository = NativeEnrollmentRepository(directory: directory, store: store)
        let empty = try await repository.load()
        XCTAssertNil(empty.request)
        let prepared = try await repository.prepare()
        let request = try XCTUnwrap(prepared.request)
        let reopened = NativeEnrollmentRepository(directory: directory, store: store)
        let pending = try await reopened.load()
        XCTAssertEqual(pending.request, request)
        let denied = NativeEnrollmentRepository(directory: directory, store: DeleteDeniedStore(base: store))
        do { _ = try await denied.cancel(request); XCTFail("Expected denial") }
        catch MobileReplicationError.Locked { }
        let recovered = try await reopened.load()
        XCTAssertTrue(recovered.cancellationPending)
        XCTAssertEqual(recovered.request, request)
        let cancelled = try await reopened.cancel(request)
        XCTAssertNil(cancelled.request)
        let fresh = try await repository.load()
        XCTAssertNil(fresh.request)
        // Preparing a later request must also work when exchange staging already exists.
        let next = try await repository.prepare()
        XCTAssertNotNil(next.request)
    }

    @MainActor
    func testViewModelDoesNotPrepareAtInitializationAndPreservesFailedCancellation() async {
        let repo = EnrollmentFixtureRepository()
        let model = EnrollmentViewModel(repository: repo)
        XCTAssertFalse(model.loading)
        let initialCalls = await repo.prepares
        XCTAssertEqual(initialCalls, 0)
        await model.reload()
        await model.prepare()
        XCTAssertEqual(model.snapshot.request, Data([1]))
        await model.cancel(Data([1]))
        XCTAssertTrue(model.snapshot.cancellationPending)
        XCTAssertNotNil(model.problem)
        await model.prepare()
        let calls = await repo.prepares
        XCTAssertEqual(calls, 1)
    }

    func testCorruptJournalIsNotTreatedAsEmpty() async throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: false)
        defer { try? FileManager.default.removeItem(at: directory) }
        let journal = directory.appendingPathComponent("reviewed-cancellation.json")
        try Data(repeating: 0, count: 4097).write(to: journal)
        do { _ = try await NativeEnrollmentRepository(directory: directory).load(); XCTFail("Expected invalid") }
        catch MobileReplicationError.Invalid { }
        XCTAssertEqual(try Data(contentsOf: journal).count, 4097)
    }
}

private final class DeleteDeniedStore: ReplicationSecureStore {
    let base: ReplicationKeychainStore
    init(base: ReplicationKeychainStore) { self.base = base }
    func create(account: String, value: Data) throws { try base.create(account: account, value: value) }
    func load(account: String) throws -> Data { try base.load(account: account) }
    func delete(account: String) throws -> Bool { throw ReplicationStorageError.Locked }
}

private actor EnrollmentFixtureRepository: EnrollmentRepository {
    var prepares = 0
    func load() async throws -> EnrollmentSnapshot { EnrollmentSnapshot() }
    func prepare() async throws -> EnrollmentSnapshot {
        prepares += 1
        return EnrollmentSnapshot(request: Data([1]))
    }
    func cancel(_ request: Data) async throws -> EnrollmentSnapshot { throw MobileReplicationError.Locked }
}
