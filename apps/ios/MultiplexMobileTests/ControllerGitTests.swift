import Foundation
import XCTest
@testable import MultiplexMobile

@MainActor
final class ControllerGitTests: XCTestCase {
    func testAnswersDecodeFromTheWire() throws {
        let id = UUID()
        let overview = """
        {"kind":"git_overview","command_id":"\(id.uuidString.lowercased())","session_id":"\(UUID().uuidString.lowercased())",
         "repository":{"folder":"/r/src","root":"/r","branch":"main","head":"abc1234","upstream":"origin/main",
         "ahead":1,"behind":0,"changes":[{"path":"a.txt","original_path":null,"status":"modified","staged":false,"unstaged":true},
         {"path":"b.txt","original_path":null,"status":"something_new","staged":true,"unstaged":false}],
         "changes_truncated":false,
         "branches":[{"name":"main","remote":false,"current":true,"upstream":"origin/main","commit":"abc1234","subject":"s","unix_time":1},
         {"name":"origin/feature","remote":true,"current":false,"upstream":null,"commit":"def5678","subject":"t","unix_time":2}],
         "branches_truncated":false,
         "worktrees":[{"path":"/r","branch":"main","head":"abc1234","current":true,"terminal":true,"locked":false,"prunable":false}]}}
        """
        guard case .overview(let repository?) = try ControllerGitAnswer.decode(Data(overview.utf8), commandID: id) else {
            return XCTFail("expected a repository")
        }
        XCTAssertEqual(repository.branch, "main")
        XCTAssertEqual(repository.changes.map(\.status), [.modified, .modified])
        XCTAssertEqual(repository.localBranches.map(\.name), ["main"])
        XCTAssertEqual(ControllerGitViewModel.localName(repository.remoteBranches[0]), "feature")

        let none = #"{"kind":"git_overview","command_id":"\#(id.uuidString)","session_id":"x","repository":null}"#
        XCTAssertEqual(try ControllerGitAnswer.decode(Data(none.utf8), commandID: id), .overview(nil))

        let error = #"{"kind":"error","code":"git_not_local","message":"m"}"#
        XCTAssertThrowsError(try ControllerGitAnswer.decode(Data(error.utf8), commandID: id)) { error in
            XCTAssertEqual(error as? ControllerConnectionError, .hostError("git_not_local"))
        }
        let other = #"{"kind":"git_overview","command_id":"\#(UUID().uuidString)","repository":null}"#
        XCTAssertThrowsError(try ControllerGitAnswer.decode(Data(other.utf8), commandID: id))
    }

    func testRequestsSayWhatTheyNeed() {
        XCTAssertFalse(ControllerGitRequest.overview(worktree: nil).requiresInput)
        XCTAssertTrue(ControllerGitRequest.switchBranch(worktree: nil, branch: "x").requiresInput)
        let command = ControllerGitRequest.log(worktree: "/w", offset: 40, limit: 40).command
        XCTAssertEqual(command["kind"] as? String, "git_log")
        XCTAssertEqual(command["worktree"] as? String, "/w")
        XCTAssertTrue(ControllerGitRequest.diff(worktree: nil, commit: nil, path: nil).command["commit"] is NSNull)
    }

    func testPatchLinesAreColouredByWhatTheyAre() {
        let lines = ControllerGitPatchLine.parse(
            "diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -1 +1 @@\n-old\n+new\n same\n"
        )
        XCTAssertEqual(lines.map(\.kind), [.file, .file, .file, .hunk, .removed, .added, .context])
        XCTAssertEqual(lines.last?.text, " same")
    }

    func testLoadsPagesAndSwitchesBranch() async throws {
        let connection = FixtureGitConnection()
        let model = ControllerGitViewModel(
            host: try Self.host(),
            target: ControllerGitTarget(sessionID: UUID(), occupantGeneration: 3),
            sessionTitle: "Tests",
            canSwitch: true,
            makeConnection: { connection }
        )
        model.load()
        try await waitUntil { model.state == .loaded && model.commits.count == 2 }
        XCTAssertEqual(model.repository?.branch, "main")
        XCTAssertEqual(model.historyNextOffset, 2)

        model.loadMoreHistory()
        try await waitUntil { model.commits.count == 3 && !model.historyLoading }
        XCTAssertNil(model.historyNextOffset)

        let feature = ControllerGitBranch(
            name: "feature", remote: false, current: false, upstream: nil,
            commit: "b", subject: "s", unixTime: 0
        )
        model.switchTo(feature)
        try await waitUntil { model.notice != nil && model.switchingBranch == nil }
        XCTAssertEqual(model.notice, "Switched to feature")
        let requests = await connection.requests
        XCTAssertTrue(requests.contains(.switchBranch(worktree: nil, branch: "feature")))
        model.stop()
    }

    func testARouteWithoutGitSaysSo() async throws {
        let model = ControllerGitViewModel(
            host: try Self.host(),
            target: ControllerGitTarget(sessionID: UUID(), occupantGeneration: 3),
            sessionTitle: "Tests",
            canSwitch: false,
            makeConnection: { nil }
        )
        model.load()
        try await waitUntil {
            if case .failed = model.state { return true }
            return false
        }
        XCTAssertEqual(
            model.state,
            .failed(ControllerGitViewModel.message(for: ControllerConnectionError.hostError("git_route_unavailable")))
        )
    }

    private static func host() throws -> PairedHostRecord {
        try PairedHostRecord(
            id: "host-id",
            displayName: "Build Mac",
            route: try HostRoute(address: "192.168.1.20", port: 22_222),
            hostStaticPublicKey: Data(repeating: 7, count: 32),
            deviceStaticKeyId: "fixture-key",
            deviceId: UUID(),
            identityGeneration: 1,
            revocationEpoch: 1,
            sessionGeneration: 1,
            capabilityBits: 0b110
        )
    }

    private func waitUntil(
        timeout: Duration = .seconds(2),
        condition: @escaping @MainActor () async -> Bool
    ) async throws {
        let clock = ContinuousClock()
        let deadline = clock.now.advanced(by: timeout)
        while clock.now < deadline {
            if await condition() { return }
            try await Task.sleep(for: .milliseconds(10))
        }
        XCTFail("condition did not become true before timeout")
    }
}

private actor FixtureGitConnection: ControllerConnecting {
    private(set) var requests: [ControllerGitRequest] = []

    func git(
        host: PairedHostRecord,
        target: ControllerGitTarget,
        request: ControllerGitRequest
    ) async throws -> ControllerGitAnswer {
        _ = (host, target)
        requests.append(request)
        switch request {
        case .overview:
            return .overview(ControllerGitRepository(
                folder: "/r", root: "/r", branch: "main", head: "a", upstream: nil,
                ahead: 0, behind: 0, changes: [], changesTruncated: false,
                branches: [], branchesTruncated: false, worktrees: []
            ))
        case .log(_, let offset, _):
            if offset == 0 {
                return .log(commits: [Self.commit("1"), Self.commit("2")], nextOffset: 2)
            }
            return .log(commits: [Self.commit("3")], nextOffset: nil)
        case .diff:
            throw ControllerConnectionError.malformedResponse
        case .switchBranch(_, let branch):
            return .switched(ControllerGitSwitchResult(switched: true, branch: branch, message: ""))
        }
    }

    private static func commit(_ oid: String) -> ControllerGitCommit {
        ControllerGitCommit(oid: oid, short: oid, subject: "s", author: "a", unixTime: 0, parents: 1, refs: [])
    }

    func beginPairing(
        offerText: String,
        hostName: String,
        deviceName: String,
        deviceID: UUID
    ) async throws -> ControllerPairingChallenge {
        _ = (offerText, hostName, deviceName, deviceID)
        throw ControllerConnectionError.capabilityDenied
    }

    func finishPairing(matches: Bool) async throws -> PairedHostRecord {
        _ = matches
        throw ControllerConnectionError.capabilityDenied
    }

    func fetchSessions(
        host: PairedHostRecord,
        progress: @escaping @Sendable (ControllerConnectionProgress) async -> Void
    ) async throws -> ControllerFleetSnapshot {
        _ = (host, progress)
        throw ControllerConnectionError.capabilityDenied
    }

    func attachReadOnly(
        host: PairedHostRecord,
        cursor: TerminalStreamCursor,
        viewport: TerminalViewportState,
        onEvent: @escaping @Sendable (ControllerReadOnlyWireEvent) async throws -> Void
    ) async throws {
        _ = (host, cursor, viewport, onEvent)
        throw ControllerConnectionError.capabilityDenied
    }

    func forgetDeviceSecret(host: PairedHostRecord) async throws { _ = host }

    func cancel() async {}
}
