import Foundation
import SwiftUI

/// The Git repository a terminal's shell is in: its changes, history, branches and worktrees,
/// read from the computer on a connection of its own so the terminal stays attached.
@MainActor
final class ControllerGitViewModel: ObservableObject, Identifiable {
    enum Section: String, CaseIterable, Hashable {
        case changes, history, branches, worktrees
    }

    enum LoadState: Equatable {
        case loading
        case loaded
        case notRepository
        case failed(String)
    }

    let id = UUID()
    let sessionTitle: String
    /// Whether this phone may type into the terminal, which is what switching branch needs.
    let canSwitch: Bool

    @Published private(set) var state: LoadState = .loading
    @Published private(set) var repository: ControllerGitRepository?
    @Published var section: Section = .changes
    @Published private(set) var commits: [ControllerGitCommit] = []
    @Published private(set) var historyNextOffset: UInt32?
    @Published private(set) var historyLoading = false
    @Published private(set) var historyMessage: String?
    /// Another worktree of the repository, by path, or `nil` for the terminal's own.
    @Published private(set) var worktree: String?
    @Published private(set) var switchingBranch: String?
    @Published var notice: String?

    static let historyPage: UInt16 = 40

    private let host: PairedHostRecord
    private let target: ControllerGitTarget
    private let makeConnection: @Sendable () async throws -> (any ControllerConnecting)?
    private var connection: (any ControllerConnecting)?
    private var loadTask: Task<Void, Never>?
    private var historyTask: Task<Void, Never>?
    private var generation = 0

    init(
        host: PairedHostRecord,
        target: ControllerGitTarget,
        sessionTitle: String,
        canSwitch: Bool,
        makeConnection: @escaping @Sendable () async throws -> (any ControllerConnecting)?
    ) {
        self.host = host
        self.target = target
        self.sessionTitle = sessionTitle
        self.canSwitch = canSwitch
        self.makeConnection = makeConnection
    }

    /// Reads the overview and the first page of history again.
    func load() {
        loadTask?.cancel()
        historyTask?.cancel()
        generation += 1
        let generation = generation
        if repository == nil { state = .loading }
        loadTask = Task { [weak self] in
            guard let self else { return }
            do {
                let answer = try await self.request(.overview(worktree: self.worktree))
                guard !Task.isCancelled, generation == self.generation else { return }
                guard case .overview(let repository) = answer else {
                    throw ControllerConnectionError.malformedResponse
                }
                self.repository = repository
                self.state = repository == nil ? .notRepository : .loaded
                self.commits = []
                self.historyNextOffset = 0
                if repository != nil { self.loadMoreHistory() }
            } catch {
                guard !Task.isCancelled, generation == self.generation else { return }
                self.state = .failed(Self.message(for: error))
            }
        }
    }

    func loadMoreHistory() {
        guard let offset = historyNextOffset, !historyLoading, state == .loaded else { return }
        historyLoading = true
        historyMessage = nil
        let generation = generation
        historyTask = Task { [weak self] in
            guard let self else { return }
            defer { if generation == self.generation { self.historyLoading = false } }
            do {
                let answer = try await self.request(
                    .log(worktree: self.worktree, offset: offset, limit: Self.historyPage)
                )
                guard !Task.isCancelled, generation == self.generation,
                      case .log(let page, let next) = answer else { return }
                let known = Set(self.commits.map(\.oid))
                self.commits.append(contentsOf: page.filter { !known.contains($0.oid) })
                self.historyNextOffset = next
            } catch {
                guard !Task.isCancelled, generation == self.generation else { return }
                self.historyMessage = Self.message(for: error)
            }
        }
    }

    /// Shows another worktree of the same repository, or the terminal's own for `nil`.
    func showWorktree(_ worktree: ControllerGitWorktree) {
        self.worktree = worktree.terminal ? nil : worktree.path
        repository = nil
        load()
    }

    func switchTo(_ branch: ControllerGitBranch) {
        guard canSwitch, switchingBranch == nil, !branch.current else { return }
        switchingBranch = branch.name
        notice = nil
        Task { [weak self] in
            guard let self else { return }
            defer { self.switchingBranch = nil }
            do {
                let answer = try await self.request(
                    .switchBranch(worktree: self.worktree, branch: branch.name)
                )
                guard case .switched(let result) = answer else {
                    throw ControllerConnectionError.malformedResponse
                }
                self.notice = result.switched
                    ? String(localized: "Switched to \(Self.localName(branch))")
                    : (result.message.isEmpty
                        ? String(localized: "Git did not switch branch.")
                        : result.message)
                self.load()
            } catch {
                self.notice = Self.message(for: error)
            }
        }
    }

    /// A commit's changes, or the uncommitted ones for `nil`, narrowed to `path` when given.
    func diff(commit: String?, path: String?) async throws -> ControllerGitDiff {
        let answer = try await request(.diff(worktree: worktree, commit: commit, path: path))
        guard case .diff(let diff) = answer else {
            throw ControllerConnectionError.malformedResponse
        }
        return diff
    }

    func stop() {
        loadTask?.cancel()
        historyTask?.cancel()
        let connection = connection
        self.connection = nil
        Task { await connection?.cancel() }
    }

    private func request(_ request: ControllerGitRequest) async throws -> ControllerGitAnswer {
        if connection == nil {
            connection = try await makeConnection()
        }
        guard let connection else {
            throw ControllerConnectionError.hostError("git_route_unavailable")
        }
        return try await connection.git(host: host, target: target, request: request)
    }

    /// `origin/feature` becomes the local `feature` git switches to.
    static func localName(_ branch: ControllerGitBranch) -> String {
        guard branch.remote, let slash = branch.name.firstIndex(of: "/") else { return branch.name }
        return String(branch.name[branch.name.index(after: slash)...])
    }

    static func message(for error: Error) -> String {
        switch error as? ControllerConnectionError {
        case .capabilityDenied:
            return String(localized: "This phone does not have permission. Change it in Remote Devices on the computer.")
        case .hostError(let code):
            switch code {
            case "git_not_local":
                return String(localized: "This terminal's folder is on another machine. Git is shown for terminals running on the computer itself.")
            case "git_route_unavailable":
                return String(localized: "Git is available when this phone reaches the computer over the local network or Tailscale.")
            case "unsupported_command":
                return String(localized: "Update Multiplex on the computer to see Git here.")
            case "git_unavailable":
                return String(localized: "Git is not installed on the computer.")
            case "git_timeout":
                return String(localized: "Git took too long to answer.")
            case "git_folder_unknown":
                return String(localized: "The computer could not tell which folder this terminal is in.")
            case "git_unknown_worktree":
                return String(localized: "That worktree is gone.")
            case "git_unknown_commit":
                return String(localized: "That commit is not in this repository.")
            case "git_unknown_branch":
                return String(localized: "That branch is gone.")
            case "stale_generation", "host_unavailable":
                return String(localized: "This terminal has ended or restarted.")
            default:
                return String(localized: "The computer could not read the repository.")
            }
        default:
            return String(localized: "The computer could not be reached.")
        }
    }
}
