import Foundation
import XCTest
@testable import TermiRustMobile

final class ControllerFleetCacheTests: XCTestCase {
    func testPageRejectsRecordAndByteLimits() throws {
        let oversizedCount = SessionSummaryPage(
            revision: 1,
            updateSequence: 1,
            sessions: (0...ControllerCacheLimits.maxPageRecords).map { session($0) },
            nextCursor: nil
        )
        XCTAssertThrowsError(try oversizedCount.validate())

        let oversizedBytes = SessionSummaryPage(
            revision: 1,
            updateSequence: 1,
            sessions: (0..<ControllerCacheLimits.maxPageRecords).map {
                session($0, expanded: true)
            },
            nextCursor: nil
        )
        XCTAssertThrowsError(try oversizedBytes.validate())
    }

    func testCacheEvictsWholeLeastRecentlyViewedHostWithFingerprintTieBreak() throws {
        var cache = ControllerFleetCache()
        let timestamp = Date(timeIntervalSince1970: 10)
        for index in 0..<ControllerCacheLimits.maxHosts {
            let fingerprint = String(format: "host-%02d", index)
            try cache.replace(
                hostFingerprint: fingerprint,
                revision: 1,
                updateSequence: 1,
                sessions: [session(index)],
                selectedHostFingerprint: "host-15",
                now: timestamp
            )
        }

        try cache.replace(
            hostFingerprint: "host-16",
            revision: 1,
            updateSequence: 1,
            sessions: [session(16)],
            selectedHostFingerprint: "host-15",
            now: timestamp
        )

        XCTAssertEqual(cache.hosts.count, ControllerCacheLimits.maxHosts)
        XCTAssertNil(cache.hosts["host-00"])
        XCTAssertNotNil(cache.hosts["host-15"])
        XCTAssertNotNil(cache.hosts["host-16"])
    }

    func testCacheRejectsDuplicateRevisionWithoutReplacingCompleteSnapshot() throws {
        var cache = ControllerFleetCache()
        try cache.replace(
            hostFingerprint: "selected",
            revision: 4,
            updateSequence: 8,
            sessions: [session(1)],
            selectedHostFingerprint: "selected",
            now: .now
        )

        XCTAssertThrowsError(try cache.replace(
            hostFingerprint: "selected",
            revision: 4,
            updateSequence: 8,
            sessions: [session(2)],
            selectedHostFingerprint: "selected",
            now: .now
        )) { error in
            XCTAssertEqual(error as? ControllerCacheError, .staleUpdate)
        }
        XCTAssertEqual(cache.hosts["selected"]?.sessions.first?.title, "Session 1")
    }

    func testAtomicStoreRoundTripsAndRejectsNewerSchema() async throws {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        let url = directory.appendingPathComponent("cache.json")
        let store = try ControllerFleetCacheStore(fileURL: url)
        var cache = ControllerFleetCache()
        try cache.replace(
            hostFingerprint: "host",
            revision: 1,
            updateSequence: 1,
            sessions: [session(1)],
            selectedHostFingerprint: "host",
            now: Date(timeIntervalSince1970: 1)
        )

        try await store.save(cache)
        let loaded = try await store.load()
        XCTAssertEqual(loaded, cache)
        try await store.delete()
        let deleted = try await store.load()
        XCTAssertEqual(deleted, ControllerFleetCache())
    }

    private func session(_ index: Int, expanded: Bool = false) -> ControllerSessionSummary {
        let long = String(repeating: "\u{1F600}", count: ControllerCacheLimits.maxTitleScalars)
        return ControllerSessionSummary(
            id: UUID(uuidString: String(format: "00000000-0000-0000-0000-%012d", index)) ?? UUID(),
            title: expanded ? long : "Session \(index)",
            project: expanded ? long : nil,
            group: expanded ? long : nil,
            lifecycle: "running",
            activity: nil,
            occupantGeneration: 1,
            lastOutputSequence: UInt64(index),
            hasWriter: false,
            unreadCount: 0
        )
    }
}
