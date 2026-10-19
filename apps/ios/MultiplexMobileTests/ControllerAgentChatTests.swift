import Foundation
import XCTest
@testable import MultiplexMobile

@MainActor
final class ControllerAgentChatTests: XCTestCase {
    func testAnswersDecodeFromTheWire() throws {
        let id = UUID()
        let answer = """
        {"kind":"agent_chat","command_id":"\(id.uuidString.lowercased())","session_id":"s",
         "chat":{"provider":"claude_code","title":"Fix the build","state":"working","revision":"r1",
         "unchanged":false,"earlier":true,"history_truncated":false,
         "items":[{"id":"u1","kind":"user","title":null,"text":"Why?","tool_state":null,"truncated":false},
                  {"id":"t1","kind":"tool","title":"Bash · cargo build","text":"error","tool_state":"failed","truncated":true},
                  {"id":"n1","kind":"something_new","title":null,"text":"x","tool_state":null,"truncated":false}]}}
        """
        let chat = try XCTUnwrap(ControllerAgentChat.decodeAnswer(Data(answer.utf8), commandID: id))
        XCTAssertEqual(chat.provider, .claudeCode)
        XCTAssertEqual(chat.state, .working)
        XCTAssertEqual(chat.items.map(\.kind), [.user, .tool, .notice])
        XCTAssertEqual(chat.items[1].toolState, .failed)
        XCTAssertTrue(chat.historyTruncated == false && chat.earlier)

        let none = #"{"kind":"agent_chat","command_id":"\#(id.uuidString)","session_id":"s","chat":null}"#
        XCTAssertNil(try ControllerAgentChat.decodeAnswer(Data(none.utf8), commandID: id))
        let error = #"{"kind":"error","code":"unsupported_command"}"#
        XCTAssertThrowsError(try ControllerAgentChat.decodeAnswer(Data(error.utf8), commandID: id)) {
            XCTAssertEqual($0 as? ControllerConnectionError, .hostError("unsupported_command"))
        }
        let other = #"{"kind":"agent_chat","command_id":"\#(UUID().uuidString)","chat":null}"#
        XCTAssertThrowsError(try ControllerAgentChat.decodeAnswer(Data(other.utf8), commandID: id))
    }

    func testCodeFencesSplitFromProse() {
        XCTAssertEqual(
            ControllerChatSegment.split("Here:\n```rust\nlet x = 1;\n```\nDone.\n```\nunclosed"),
            [
                .prose("Here:"),
                .code(language: "rust", text: "let x = 1;"),
                .prose("Done."),
                .code(language: nil, text: "unclosed"),
            ]
        )
    }

    func testNewestPagesMergeBehindWhatWasReadBack() throws {
        let model = ControllerAgentChatViewModel(
            host: try Self.host(),
            target: ControllerGitTarget(sessionID: UUID(), occupantGeneration: 1),
            makeConnection: { nil }
        )
        model.apply(Self.chat(revision: "r1", items: ["a", "b", "c"], earlier: true))
        XCTAssertEqual(model.items.map(\.id), ["a", "b", "c"])
        XCTAssertTrue(model.hasEarlier)
        // A newer page that starts inside what is shown replaces from there on.
        model.apply(Self.chat(revision: "r2", items: ["b", "c", "d"], earlier: true))
        XCTAssertEqual(model.items.map(\.id), ["a", "b", "c", "d"])
        // Nothing changed: nothing moves, and the state still updates.
        model.apply(Self.chat(revision: "r2", items: [], earlier: false, unchanged: true, state: "ready"))
        XCTAssertEqual(model.items.map(\.id), ["a", "b", "c", "d"])
        XCTAssertEqual(model.chat?.state, .ready)
        // A page that shares nothing starts over.
        model.apply(Self.chat(revision: "r3", items: ["x"], earlier: false))
        XCTAssertEqual(model.items.map(\.id), ["x"])
        model.apply(nil)
        XCTAssertFalse(model.available)
    }

    func testFindsTheAgentReadsEarlierAndStopsWhereUnsupported() async throws {
        let connection = FixtureChatConnection()
        let model = ControllerAgentChatViewModel(
            host: try Self.host(),
            target: ControllerGitTarget(sessionID: UUID(), occupantGeneration: 1),
            makeConnection: { connection }
        )
        await model.refresh()
        XCTAssertTrue(model.available)
        XCTAssertEqual(model.items.map(\.id), ["m3", "m4"])
        model.loadEarlier()
        try await waitUntil { model.items.count == 4 && !model.loadingEarlier }
        XCTAssertEqual(model.items.map(\.id), ["m1", "m2", "m3", "m4"])
        XCTAssertFalse(model.hasEarlier)
        let requests = await connection.requests
        XCTAssertEqual(requests.last?.before, "m3")

        let old = ControllerAgentChatViewModel(
            host: try Self.host(),
            target: ControllerGitTarget(sessionID: UUID(), occupantGeneration: 1),
            makeConnection: { nil }
        )
        await old.refresh()
        XCTAssertFalse(old.available)
        old.start()
        XCTAssertFalse(old.available, "a route without the chat is not asked again")
        old.stop()
    }

    static func host() throws -> PairedHostRecord {
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

    static func chat(
        revision: String,
        items: [String],
        earlier: Bool,
        unchanged: Bool = false,
        state: String = "working"
    ) -> ControllerAgentChat {
        let items = items.map {
            #"{"id":"\#($0)","kind":"assistant","title":null,"text":"\#($0)","tool_state":null,"truncated":false}"#
        }.joined(separator: ",")
        let json = """
        {"provider":"codex","title":null,"state":"\(state)","revision":"\(revision)","unchanged":\(unchanged),
         "items":[\(items)],"earlier":\(earlier),"history_truncated":false}
        """
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        return try! decoder.decode(ControllerAgentChat.self, from: Data(json.utf8))
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

private actor FixtureChatConnection: ControllerConnecting {
    private(set) var requests: [ControllerAgentChatRequest] = []

    func agentChat(
        host: PairedHostRecord,
        target: ControllerGitTarget,
        request: ControllerAgentChatRequest
    ) async throws -> ControllerAgentChat? {
        _ = (host, target)
        requests.append(request)
        if request.before == "m3" {
            return await ControllerAgentChatTests.chat(revision: "r1", items: ["m1", "m2"], earlier: false)
        }
        return await ControllerAgentChatTests.chat(revision: "r1", items: ["m3", "m4"], earlier: true)
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
