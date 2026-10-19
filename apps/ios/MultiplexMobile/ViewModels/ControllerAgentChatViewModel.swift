import Foundation
import SwiftUI

/// The conversation of the Claude Code or Codex session running in a terminal, read from the
/// computer on a connection of its own so the terminal stays attached. It looks for an agent while
/// the terminal is open and follows the conversation while Chat is showing.
@MainActor
final class ControllerAgentChatViewModel: ObservableObject {
    @Published private(set) var chat: ControllerAgentChat?
    @Published private(set) var items: [ControllerAgentChatItem] = []
    @Published private(set) var hasEarlier = false
    @Published private(set) var loadingEarlier = false
    @Published private(set) var message: String?
    /// Kept here so it survives switching to the terminal and back.
    @Published var draft = ""
    @Published var expandedTools: Set<String> = []

    /// How often the conversation is read while it shows, and how often an agent is looked for
    /// while it does not.
    static let followInterval: Duration = .milliseconds(1_500)
    static let lookInterval: Duration = .seconds(5)
    static let page: UInt16 = 60

    private let host: PairedHostRecord
    private let target: ControllerGitTarget
    private let makeConnection: @Sendable () async throws -> (any ControllerConnecting)?
    private var connection: (any ControllerConnecting)?
    private var loop: Task<Void, Never>?
    private var following = false
    private var unsupported = false

    init(
        host: PairedHostRecord,
        target: ControllerGitTarget,
        makeConnection: @escaping @Sendable () async throws -> (any ControllerConnecting)?
    ) {
        self.host = host
        self.target = target
        self.makeConnection = makeConnection
    }

    /// Whether an agent runs in the terminal, so the page offers Chat.
    var available: Bool { chat != nil }

    func start() {
        guard loop == nil, !unsupported else { return }
        loop = Task { [weak self] in
            while !Task.isCancelled {
                guard let self else { return }
                await self.refresh()
                let interval = self.following ? Self.followInterval : Self.lookInterval
                try? await Task.sleep(for: interval)
            }
        }
    }

    /// Chat is showing: read the conversation often, starting now.
    func setFollowing(_ following: Bool) {
        guard following != self.following else { return }
        self.following = following
        if following {
            loop?.cancel()
            loop = nil
            start()
        }
    }

    func stop() {
        loop?.cancel()
        loop = nil
        let connection = connection
        self.connection = nil
        Task { await connection?.cancel() }
    }

    func refresh() async {
        do {
            let answer = try await request(
                ControllerAgentChatRequest(sinceRevision: chat?.revision, limit: Self.page)
            )
            apply(answer)
            message = nil
        } catch ControllerConnectionError.hostError(let code)
            where code == "unsupported_command" || code == "chat_route_unavailable" {
            // A computer or route without the chat: nothing to offer, and nothing to keep asking.
            unsupported = true
            chat = nil
            loop?.cancel()
            loop = nil
        } catch is CancellationError {
        } catch {
            if chat != nil { message = Self.message(for: error) }
            connection = nil
        }
    }

    func loadEarlier() {
        guard !loadingEarlier, hasEarlier, let first = items.first else { return }
        loadingEarlier = true
        Task { [weak self] in
            guard let self else { return }
            defer { self.loadingEarlier = false }
            do {
                guard let page = try await self.request(
                    ControllerAgentChatRequest(before: first.id, limit: Self.page)
                ) else { return }
                let known = Set(self.items.map(\.id))
                self.items.insert(contentsOf: page.items.filter { !known.contains($0.id) }, at: 0)
                self.hasEarlier = page.earlier
            } catch {
                self.message = Self.message(for: error)
            }
        }
    }

    /// Takes in a newest page: what it holds replaces the same items here, and what was read
    /// back before it stays in front of it.
    func apply(_ answer: ControllerAgentChat?) {
        guard let answer else {
            chat = nil
            items = []
            hasEarlier = false
            return
        }
        if answer.unchanged, chat != nil {
            chat = answer
            return
        }
        let providerChanged = chat.map { $0.provider != answer.provider } ?? false
        chat = answer
        if providerChanged || answer.items.isEmpty {
            items = answer.items
            hasEarlier = answer.earlier
            return
        }
        if let first = answer.items.first, let at = items.firstIndex(where: { $0.id == first.id }) {
            items = Array(items[..<at]) + answer.items
            if at == 0 { hasEarlier = answer.earlier }
        } else {
            items = answer.items
            hasEarlier = answer.earlier
        }
    }

    private func request(_ request: ControllerAgentChatRequest) async throws -> ControllerAgentChat? {
        if connection == nil {
            connection = try await makeConnection()
        }
        guard let connection else {
            throw ControllerConnectionError.hostError("chat_route_unavailable")
        }
        return try await connection.agentChat(host: host, target: target, request: request)
    }

    static func message(for error: Error) -> String {
        switch error as? ControllerConnectionError {
        case .capabilityDenied:
            return String(localized: "This phone does not have permission to see this terminal.")
        case .hostError(let code) where code == "stale_generation" || code == "host_unavailable":
            return String(localized: "This terminal has ended or restarted.")
        default:
            return String(localized: "Reconnecting to the conversation…")
        }
    }
}
