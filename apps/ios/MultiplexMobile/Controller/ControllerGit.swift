import Foundation

/// The terminal a Git request is about: the repository is the one its shell is in now.
struct ControllerGitTarget: Equatable, Sendable {
    let sessionID: UUID
    let occupantGeneration: UInt64
}

/// What a phone asks of a terminal's repository. `worktree` shows another worktree of the same
/// repository, by the path the overview gave.
enum ControllerGitRequest: Equatable, Sendable {
    case overview(worktree: String?)
    case log(worktree: String?, offset: UInt32, limit: UInt16)
    case diff(worktree: String?, commit: String?, path: String?)
    case switchBranch(worktree: String?, branch: String)

    /// Reading needs what watching a terminal needs; switching needs what typing into one does.
    var requiresInput: Bool {
        if case .switchBranch = self { return true }
        return false
    }

    var command: [String: Any] {
        func optional(_ value: String?) -> Any { value ?? NSNull() }
        switch self {
        case .overview(let worktree):
            return ["kind": "git_overview", "worktree": optional(worktree)]
        case .log(let worktree, let offset, let limit):
            return [
                "kind": "git_log",
                "worktree": optional(worktree),
                "offset": offset,
                "limit": limit,
            ]
        case .diff(let worktree, let commit, let path):
            return [
                "kind": "git_diff",
                "worktree": optional(worktree),
                "commit": optional(commit),
                "path": optional(path),
            ]
        case .switchBranch(let worktree, let branch):
            return ["kind": "git_switch", "worktree": optional(worktree), "branch": branch]
        }
    }
}

enum ControllerGitAnswer: Equatable, Sendable {
    /// `nil` when the terminal's folder is in no repository.
    case overview(ControllerGitRepository?)
    case log(commits: [ControllerGitCommit], nextOffset: UInt32?)
    case diff(ControllerGitDiff)
    case switched(ControllerGitSwitchResult)

    /// Reads one answer, checking it answers `commandID`. An error answer throws its code.
    static func decode(_ payload: Data, commandID: UUID) throws -> ControllerGitAnswer {
        guard let object = try JSONSerialization.jsonObject(with: payload) as? [String: Any],
              let kind = object["kind"] as? String else {
            throw ControllerConnectionError.malformedResponse
        }
        if kind == "error" {
            throw ControllerConnectionError.hostError((object["code"] as? String) ?? "unknown")
        }
        guard let answered = object["command_id"] as? String,
              UUID(uuidString: answered) == commandID else {
            throw ControllerConnectionError.malformedResponse
        }
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        do {
            switch kind {
            case "git_overview":
                return .overview(try decoder.decode(OverviewPayload.self, from: payload).repository)
            case "git_log":
                let page = try decoder.decode(LogPayload.self, from: payload)
                return .log(commits: page.commits, nextOffset: page.nextOffset)
            case "git_diff":
                return .diff(try decoder.decode(DiffPayload.self, from: payload).diff)
            case "git_switched":
                return .switched(try decoder.decode(ControllerGitSwitchResult.self, from: payload))
            default:
                throw ControllerConnectionError.malformedResponse
            }
        } catch let error as ControllerConnectionError {
            throw error
        } catch {
            throw ControllerConnectionError.malformedResponse
        }
    }

    private struct OverviewPayload: Decodable { let repository: ControllerGitRepository? }
    private struct LogPayload: Decodable {
        let commits: [ControllerGitCommit]
        let nextOffset: UInt32?
    }
    private struct DiffPayload: Decodable { let diff: ControllerGitDiff }
}

struct ControllerGitRepository: Decodable, Equatable, Sendable {
    let folder: String
    let root: String
    let branch: String?
    let head: String?
    let upstream: String?
    let ahead: UInt32
    let behind: UInt32
    let changes: [ControllerGitChange]
    let changesTruncated: Bool
    let branches: [ControllerGitBranch]
    let branchesTruncated: Bool
    let worktrees: [ControllerGitWorktree]

    var localBranches: [ControllerGitBranch] { branches.filter { !$0.remote } }
    var remoteBranches: [ControllerGitBranch] { branches.filter(\.remote) }
}

enum ControllerGitChangeStatus: String, Decodable, Equatable, Sendable {
    case modified, added, deleted, renamed, copied
    case typeChanged = "type_changed"
    case untracked, conflicted

    /// The letter a Git status column would show.
    var letter: String {
        switch self {
        case .modified: "M"
        case .added: "A"
        case .deleted: "D"
        case .renamed: "R"
        case .copied: "C"
        case .typeChanged: "T"
        case .untracked: "?"
        case .conflicted: "U"
        }
    }

    /// Codes from a newer computer read as a modification rather than failing the page.
    init(from decoder: Decoder) throws {
        let raw = try decoder.singleValueContainer().decode(String.self)
        self = ControllerGitChangeStatus(rawValue: raw) ?? .modified
    }
}

struct ControllerGitChange: Decodable, Equatable, Hashable, Sendable {
    let path: String
    let originalPath: String?
    let status: ControllerGitChangeStatus
    let staged: Bool
    let unstaged: Bool
}

struct ControllerGitBranch: Decodable, Equatable, Hashable, Sendable, Identifiable {
    let name: String
    let remote: Bool
    let current: Bool
    let upstream: String?
    let commit: String
    let subject: String
    let unixTime: UInt64

    var id: String { (remote ? "remote/" : "local/") + name }
}

struct ControllerGitWorktree: Decodable, Equatable, Hashable, Sendable, Identifiable {
    let path: String
    let branch: String?
    let head: String?
    /// The worktree shown.
    let current: Bool
    /// The worktree the terminal is in.
    let terminal: Bool
    let locked: Bool
    let prunable: Bool

    var id: String { path }
}

struct ControllerGitCommit: Decodable, Equatable, Hashable, Sendable, Identifiable {
    let oid: String
    let short: String
    let subject: String
    let author: String
    let unixTime: UInt64
    let parents: UInt32
    let refs: [String]

    var id: String { oid }
}

struct ControllerGitCommitDetail: Decodable, Equatable, Sendable {
    let oid: String
    let short: String
    let author: String
    let email: String
    let unixTime: UInt64
    let parents: [String]
    let message: String
}

struct ControllerGitDiff: Decodable, Equatable, Sendable {
    let commit: ControllerGitCommitDetail?
    let files: [ControllerGitDiffFile]
    let filesTruncated: Bool
    let patch: String
    let patchTruncated: Bool
}

struct ControllerGitDiffFile: Decodable, Equatable, Hashable, Sendable, Identifiable {
    let path: String
    let originalPath: String?
    let status: ControllerGitChangeStatus
    /// `nil` for a binary file or one Git does not track yet.
    let additions: UInt32?
    let deletions: UInt32?

    var id: String { path }
}

struct ControllerGitSwitchResult: Decodable, Equatable, Sendable {
    let switched: Bool
    let branch: String
    let message: String
}

extension ControllerConnecting {
    /// A route that cannot carry Git requests says so, rather than every caller knowing which can.
    func git(
        host: PairedHostRecord,
        target: ControllerGitTarget,
        request: ControllerGitRequest
    ) async throws -> ControllerGitAnswer {
        _ = (host, target, request)
        throw ControllerConnectionError.hostError("git_route_unavailable")
    }
}
