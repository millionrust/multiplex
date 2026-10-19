import Foundation

/// What a phone asks of the coding agent running in a terminal: up to `limit` items, the newest
/// unless `before` names the item to read back from. A `sinceRevision` the computer still has
/// answers `unchanged`, so asking often costs little.
struct ControllerAgentChatRequest: Equatable, Sendable {
    var before: String?
    var sinceRevision: String?
    var limit: UInt16 = 60

    var command: [String: Any] {
        [
            "kind": "agent_chat",
            "before": before ?? NSNull(),
            "since_revision": sinceRevision ?? NSNull(),
            "limit": limit,
        ]
    }
}

enum ControllerAgentProvider: String, Decodable, Equatable, Sendable {
    case claudeCode = "claude_code"
    case codex

    var displayName: String {
        switch self {
        case .claudeCode: "Claude Code"
        case .codex: "Codex"
        }
    }
}

enum ControllerAgentChatState: String, Decodable, Equatable, Sendable {
    case working, ready, unknown

    init(from decoder: Decoder) throws {
        let raw = try decoder.singleValueContainer().decode(String.self)
        self = ControllerAgentChatState(rawValue: raw) ?? .unknown
    }
}

enum ControllerAgentChatItemKind: String, Decodable, Equatable, Sendable {
    case user, assistant, tool, notice

    /// A kind from a newer computer reads as a notice rather than failing the conversation.
    init(from decoder: Decoder) throws {
        let raw = try decoder.singleValueContainer().decode(String.self)
        self = ControllerAgentChatItemKind(rawValue: raw) ?? .notice
    }
}

enum ControllerAgentToolState: String, Decodable, Equatable, Sendable {
    case running, succeeded, failed

    init(from decoder: Decoder) throws {
        let raw = try decoder.singleValueContainer().decode(String.self)
        self = ControllerAgentToolState(rawValue: raw) ?? .succeeded
    }
}

struct ControllerAgentChatItem: Decodable, Equatable, Identifiable, Sendable {
    let id: String
    let kind: ControllerAgentChatItemKind
    let title: String?
    let text: String
    let toolState: ControllerAgentToolState?
    let truncated: Bool
}

struct ControllerAgentChat: Decodable, Equatable, Sendable {
    let provider: ControllerAgentProvider
    let title: String?
    let state: ControllerAgentChatState
    let revision: String
    let unchanged: Bool
    let items: [ControllerAgentChatItem]
    let earlier: Bool
    let historyTruncated: Bool

    /// Reads one answer, checking it answers `commandID`. An error answer throws its code.
    static func decodeAnswer(_ payload: Data, commandID: UUID) throws -> ControllerAgentChat? {
        guard let object = try JSONSerialization.jsonObject(with: payload) as? [String: Any],
              let kind = object["kind"] as? String else {
            throw ControllerConnectionError.malformedResponse
        }
        if kind == "error" {
            throw ControllerConnectionError.hostError((object["code"] as? String) ?? "unknown")
        }
        guard kind == "agent_chat",
              let answered = object["command_id"] as? String,
              UUID(uuidString: answered) == commandID else {
            throw ControllerConnectionError.malformedResponse
        }
        struct Answer: Decodable { let chat: ControllerAgentChat? }
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        do {
            return try decoder.decode(Answer.self, from: payload).chat
        } catch {
            throw ControllerConnectionError.malformedResponse
        }
    }
}

/// A message split into prose and fenced code, which the chat draws differently.
enum ControllerChatSegment: Equatable {
    case prose(String)
    case code(language: String?, text: String)

    static func split(_ text: String) -> [ControllerChatSegment] {
        var segments: [ControllerChatSegment] = []
        var prose: [Substring] = []
        var code: [Substring]?
        var language: String?
        func flushProse() {
            let joined = prose.joined(separator: "\n").trimmingCharacters(in: .newlines)
            if !joined.isEmpty { segments.append(.prose(joined)) }
            prose = []
        }
        for line in text.split(separator: "\n", omittingEmptySubsequences: false) {
            let trimmed = line.trimmingCharacters(in: .whitespaces)
            if trimmed.hasPrefix("```") {
                if let open = code {
                    segments.append(.code(language: language, text: open.joined(separator: "\n")))
                    code = nil
                    language = nil
                } else {
                    flushProse()
                    let tag = trimmed.dropFirst(3).trimmingCharacters(in: .whitespaces)
                    language = tag.isEmpty ? nil : tag
                    code = []
                }
            } else if code != nil {
                code?.append(line)
            } else {
                prose.append(line)
            }
        }
        if let open = code {
            // A fence the cut-off text never closed is still code.
            segments.append(.code(language: language, text: open.joined(separator: "\n")))
        }
        flushProse()
        return segments
    }
}

extension ControllerConnecting {
    /// A route that cannot carry the chat says so, rather than every caller knowing which can.
    func agentChat(
        host: PairedHostRecord,
        target: ControllerGitTarget,
        request: ControllerAgentChatRequest
    ) async throws -> ControllerAgentChat? {
        _ = (host, target, request)
        throw ControllerConnectionError.hostError("chat_route_unavailable")
    }
}
