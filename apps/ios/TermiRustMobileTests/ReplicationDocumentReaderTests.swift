import Foundation
import Darwin
import XCTest
@testable import TermiRustMobile

final class ReplicationDocumentReaderTests: XCTestCase, @unchecked Sendable {
    private func fixture(_ body: (URL) async throws -> Void) async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: false)
        defer { try? FileManager.default.removeItem(at: root) }
        try await body(root)
    }

    func testCoordinatedReadAndReopenPreserveBytes() async throws {
        try await fixture { root in
            let file = root.appendingPathComponent("document")
            let expected = Data([0, 1, 2, 255])
            try expected.write(to: file)
            let first = try await ReplicationDocumentReader().read(file, kind: .enrollmentBundle)
            let reopened = try await ReplicationDocumentReader().read(file, kind: .enrollmentBundle)
            XCTAssertEqual(first, expected)
            XCTAssertEqual(reopened, first)
        }
    }

    func testExactLimitAndOverflow() async throws {
        try await fixture { root in
            let file = root.appendingPathComponent("document")
            try Data(repeating: 1, count: 192 * 1024).write(to: file)
            let exact = try await ReplicationDocumentReader().read(file, kind: .enrollmentBundle)
            XCTAssertEqual(exact.count, 192 * 1024)
            try Data(repeating: 1, count: 192 * 1024 + 1).write(to: file)
            do { _ = try await ReplicationDocumentReader().read(file, kind: .enrollmentBundle); XCTFail("Expected overflow") }
            catch ReplicationTransferFailure.tooLarge { }
            try Data(repeating: 1, count: 8 * 1024 * 1024).write(to: file)
            let replica = try await ReplicationDocumentReader().read(file, kind: .encryptedReplica)
            XCTAssertEqual(replica.count, 8 * 1024 * 1024)
            try Data(repeating: 1, count: 8 * 1024 * 1024 + 1).write(to: file)
            do { _ = try await ReplicationDocumentReader().read(file, kind: .encryptedReplica); XCTFail("Expected replica overflow") }
            catch ReplicationTransferFailure.tooLarge { }
        }
    }

    func testEmptyMissingAndDirectoryAreNotAbsentReplicas() async throws {
        try await fixture { root in
            let file = root.appendingPathComponent("document")
            do { _ = try await ReplicationDocumentReader().read(file, kind: .enrollmentBundle); XCTFail("Expected missing") }
            catch ReplicationTransferFailure.unavailable { }
            try Data().write(to: file)
            do { _ = try await ReplicationDocumentReader().read(file, kind: .enrollmentBundle); XCTFail("Expected empty") }
            catch ReplicationTransferFailure.empty { }
            do { _ = try await ReplicationDocumentReader().read(root, kind: .enrollmentBundle); XCTFail("Expected directory rejection") }
            catch ReplicationTransferFailure.invalidLocation { }
            let cancelled = Task {
                withUnsafeCurrentTask { $0?.cancel() }
                return try await ReplicationDocumentReader().read(file, kind: .enrollmentBundle)
            }
            do { _ = try await cancelled.value; XCTFail("Expected cancellation") }
            catch is CancellationError { }
        }
    }

    func testNonFileURLAndSymlinkRejected() async throws {
        do { _ = try await ReplicationDocumentReader().read(URL(string: "https://example.invalid/document")!, kind: .encryptedReplica); XCTFail("Expected invalid URL") }
        catch ReplicationTransferFailure.invalidLocation { }
        try await fixture { root in
            let target = root.appendingPathComponent("target")
            let link = root.appendingPathComponent("link")
            try Data([1]).write(to: target)
            try FileManager.default.createSymbolicLink(at: link, withDestinationURL: target)
            do { _ = try await ReplicationDocumentReader().read(link, kind: .enrollmentBundle); XCTFail("Expected symlink rejection") }
            catch ReplicationTransferFailure.invalidLocation { }
        }
    }

    func testPublicationUnsupported() async throws {
        do { try await ReplicationDocumentReader().publish() }
        catch ReplicationTransferFailure.unsupportedPublication { }
    }

    func testPermissionDenialAndRestoredAccess() async throws {
        try await fixture { root in
            let file = root.appendingPathComponent("protected-document")
            let expected = Data([1, 2, 3])
            try expected.write(to: file)
            defer { try? FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: file.path) }
            try FileManager.default.setAttributes([.posixPermissions: 0o000], ofItemAtPath: file.path)
            do { _ = try await ReplicationDocumentReader().read(file, kind: .enrollmentBundle); XCTFail("Expected denied access") }
            catch ReplicationTransferFailure.denied { }
            try FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: file.path)
            let restored = try await ReplicationDocumentReader().read(file, kind: .enrollmentBundle)
            XCTAssertEqual(restored, expected)
        }
    }

    func testStructuredPermissionErrorMapping() {
        for code in [EACCES, EPERM] {
            let underlying = NSError(domain: NSPOSIXErrorDomain, code: Int(code))
            let wrapped = NSError(domain: NSCocoaErrorDomain, code: NSFileReadUnknownError,
                userInfo: [NSUnderlyingErrorKey: underlying])
            XCTAssertEqual(ReplicationDocumentReader.classify(wrapped), .denied)
        }
        XCTAssertEqual(ReplicationDocumentReader.classify(NSError(domain: NSPOSIXErrorDomain, code: Int(ENOENT))), .unavailable)
        XCTAssertEqual(ReplicationDocumentReader.classify(NSError(domain: "fixture.unknown", code: Int(EACCES))), .unavailable)
    }
}
