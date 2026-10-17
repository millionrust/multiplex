import SwiftUI

/// The repository a terminal's shell is in, opened from the terminal's menu.
struct ControllerGitView: View {
    @ObservedObject var model: ControllerGitViewModel
    @Environment(\.dismiss) private var dismiss
    @State private var path: [ControllerGitRoute] = []
    @State private var pendingSwitch: ControllerGitBranch?

    var body: some View {
        NavigationStack(path: $path) {
            VStack(spacing: 0) {
                content
            }
            .background(Flow.canvas)
            .navigationTitle(model.repository?.branch ?? String(localized: "Git"))
            .navigationBarTitleDisplayMode(.inline)
            .toolbarBackground(Flow.canvas, for: .navigationBar)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button(String(localized: "Done")) { dismiss() }
                }
                ToolbarItem(placement: .primaryAction) {
                    Button {
                        model.load()
                    } label: {
                        Image(systemName: "arrow.clockwise")
                    }
                    .accessibilityLabel(String(localized: "Refresh"))
                }
            }
            .navigationDestination(for: ControllerGitRoute.self) { route in
                ControllerGitDiffView(model: model, route: route) { file in
                    path.append(ControllerGitRoute(commit: route.commit, path: file.path, title: file.path))
                }
            }
        }
        .preferredColorScheme(.dark)
        .onAppear { if model.repository == nil { model.load() } }
        .onDisappear { model.stop() }
        .confirmationDialog(
            pendingSwitch.map { String(localized: "Switch to \(ControllerGitViewModel.localName($0))?") } ?? "",
            isPresented: Binding(
                get: { pendingSwitch != nil },
                set: { if !$0 { pendingSwitch = nil } }
            ),
            titleVisibility: .visible,
            presenting: pendingSwitch
        ) { branch in
            Button(String(localized: "Switch Branch")) { model.switchTo(branch) }
        } message: { _ in
            Text(String(localized: "The terminal's folder changes to this branch, as git switch would. Uncommitted changes come along when they do not conflict; otherwise git refuses."))
        }
    }

    @ViewBuilder
    private var content: some View {
        switch model.state {
        case .loading where model.repository == nil:
            ProgressView()
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        case .notRepository:
            ContentUnavailableView(
                String(localized: "Not a Git Repository"),
                systemImage: "folder",
                description: Text(String(localized: "This terminal's folder is not inside a Git repository. cd into one and refresh."))
            )
        case .failed(let message) where model.repository == nil:
            ContentUnavailableView {
                Label(String(localized: "Git Is Not Available"), systemImage: "exclamationmark.triangle")
            } description: {
                Text(message)
            } actions: {
                FlowButton(title: String(localized: "Try Again"), systemImage: "arrow.clockwise") {
                    model.load()
                }
            }
        default:
            if let repository = model.repository {
                loaded(repository)
            }
        }
    }

    private func loaded(_ repository: ControllerGitRepository) -> some View {
        VStack(spacing: 0) {
            header(repository)
            FlowSegmented(
                options: [
                    (ControllerGitViewModel.Section.changes, String(localized: "Changes")),
                    (.history, String(localized: "History")),
                    (.branches, String(localized: "Branches")),
                    (.worktrees, String(localized: "Worktrees")),
                ],
                selection: $model.section
            )
            .padding(.horizontal, 14)
            .padding(.bottom, 4)
            if let notice = model.notice {
                Text(notice)
                    .font(.system(size: 12.5))
                    .foregroundStyle(Flow.text2)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(10)
                    .background(Flow.raised, in: RoundedRectangle(cornerRadius: Flow.radiusSmall))
                    .padding(.horizontal, 14)
                    .padding(.top, 6)
                    .onTapGesture { model.notice = nil }
            }
            FlowBody {
                switch model.section {
                case .changes: changes(repository)
                case .history: history
                case .branches: branches(repository)
                case .worktrees: worktrees(repository)
                }
            }
            .refreshable { model.load() }
        }
    }

    private func header(_ repository: ControllerGitRepository) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 6) {
                Image(systemName: "arrow.triangle.branch").foregroundStyle(Flow.accent)
                Text(repository.branch ?? String(localized: "Detached at \(repository.head ?? "—")"))
                    .font(.system(size: 15, weight: .semibold))
                    .foregroundStyle(Flow.text)
                    .lineLimit(1)
                Spacer(minLength: 4)
                if repository.ahead > 0 {
                    FlowBadge(text: "↑\(repository.ahead)", tone: .accent)
                }
                if repository.behind > 0 {
                    FlowBadge(text: "↓\(repository.behind)", tone: .warn)
                }
                if model.worktree != nil {
                    FlowBadge(text: String(localized: "Other worktree"), tone: .warn)
                }
            }
            Text(repository.upstream.map { String(localized: "Tracking \($0)") }
                ?? String(localized: "No upstream"))
                .font(.system(size: 12))
                .foregroundStyle(Flow.muted)
            Text(repository.root)
                .font(.system(size: 11.5, design: .monospaced))
                .foregroundStyle(Flow.muted)
                .lineLimit(1)
                .truncationMode(.head)
        }
        .padding(.horizontal, 18)
        .padding(.vertical, 10)
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    @ViewBuilder
    private func changes(_ repository: ControllerGitRepository) -> some View {
        if repository.changes.isEmpty {
            FlowCard {
                FlowRow(
                    name: String(localized: "Working tree clean"),
                    meta: String(localized: "Nothing to commit"),
                    glyph: "checkmark.circle",
                    divider: false
                ) { EmptyView() }
            }
        } else {
            FlowCard {
                FlowRow(
                    name: String(localized: "All changes"),
                    meta: String(localized: "\(repository.changes.count) files"),
                    glyph: "doc.on.doc",
                    glyphOn: true,
                    onTap: {
                        path.append(ControllerGitRoute(
                            commit: nil,
                            path: nil,
                            title: String(localized: "Uncommitted Changes")
                        ))
                    }
                )
                ForEach(Array(repository.changes.enumerated()), id: \.element) { index, change in
                    FlowRow(
                        name: ControllerGitText.fileName(change.path),
                        meta: ControllerGitText.changeMeta(change),
                        glyph: ControllerGitText.glyph(change.status),
                        divider: index < repository.changes.count - 1,
                        onTap: {
                            path.append(ControllerGitRoute(commit: nil, path: change.path, title: change.path))
                        }
                    ) {
                        FlowBadge(
                            text: change.status.letter,
                            tone: change.status == .conflicted ? .warn : (change.staged ? .good : .plain)
                        )
                    }
                }
            }
            if repository.changesTruncated {
                footnote(String(localized: "More files changed than fit here."))
            }
        }
    }

    @ViewBuilder
    private var history: some View {
        if model.commits.isEmpty && !model.historyLoading {
            footnote(model.historyMessage ?? String(localized: "No commits yet."))
        } else {
            FlowCard {
                ForEach(Array(model.commits.enumerated()), id: \.element.oid) { index, commit in
                    FlowRow(
                        name: commit.subject,
                        meta: "\(commit.short) · \(commit.author) · \(ControllerGitText.relative(commit.unixTime))",
                        glyph: commit.parents > 1 ? "arrow.triangle.merge" : "smallcircle.filled.circle",
                        glyphOn: index == 0,
                        divider: index < model.commits.count - 1,
                        onTap: {
                            path.append(ControllerGitRoute(commit: commit.oid, path: nil, title: commit.short))
                        }
                    ) {
                        if let name = commit.refs.first {
                            FlowBadge(text: name, tone: .accent)
                        } else {
                            FlowChevron()
                        }
                    }
                    .onAppear {
                        if index == model.commits.count - 1 { model.loadMoreHistory() }
                    }
                }
            }
            if model.historyLoading {
                ProgressView().padding(.top, 12)
            } else if let message = model.historyMessage {
                footnote(message)
            }
        }
    }

    @ViewBuilder
    private func branches(_ repository: ControllerGitRepository) -> some View {
        branchGroup(String(localized: "Local"), repository.localBranches)
        if !repository.remoteBranches.isEmpty {
            branchGroup(String(localized: "Remote"), repository.remoteBranches)
        }
        if repository.branchesTruncated {
            footnote(String(localized: "Only the most recently used branches are shown."))
        }
        if !model.canSwitch {
            footnote(String(localized: "Switching branch needs permission to type into this computer's terminals."))
        }
    }

    private func branchGroup(_ title: String, _ branches: [ControllerGitBranch]) -> some View {
        VStack(spacing: 0) {
            FlowGroupLabel(title)
            FlowCard {
                ForEach(Array(branches.enumerated()), id: \.element.id) { index, branch in
                    FlowRow(
                        name: branch.name,
                        meta: "\(branch.commit) · \(branch.subject)",
                        glyph: branch.remote ? "cloud" : "arrow.triangle.branch",
                        glyphOn: branch.current,
                        divider: index < branches.count - 1
                    ) {
                        if branch.current {
                            FlowBadge(text: String(localized: "Current"), tone: .good)
                        } else if model.switchingBranch == branch.name {
                            ProgressView()
                        } else if model.canSwitch {
                            FlowAction(title: String(localized: "Switch")) { pendingSwitch = branch }
                                .disabled(model.switchingBranch != nil)
                        } else {
                            Text(ControllerGitText.relative(branch.unixTime))
                                .font(.system(size: 11.5))
                                .foregroundStyle(Flow.muted)
                        }
                    }
                }
            }
        }
    }

    private func worktrees(_ repository: ControllerGitRepository) -> some View {
        VStack(spacing: 0) {
            FlowCard {
                ForEach(Array(repository.worktrees.enumerated()), id: \.element.id) { index, worktree in
                    FlowRow(
                        name: ControllerGitText.fileName(worktree.path),
                        meta: worktree.branch ?? String(localized: "Detached at \(worktree.head ?? "—")"),
                        glyph: "folder",
                        glyphOn: worktree.current,
                        divider: index < repository.worktrees.count - 1,
                        onTap: { model.showWorktree(worktree) }
                    ) {
                        HStack(spacing: 4) {
                            if worktree.terminal {
                                FlowBadge(text: String(localized: "Terminal"), tone: .accent)
                            }
                            if worktree.current {
                                FlowBadge(text: String(localized: "Shown"), tone: .good)
                            } else {
                                FlowChevron()
                            }
                        }
                    }
                }
            }
            footnote(String(localized: "Showing another worktree changes what you see here, not where the terminal is."))
        }
    }

    private func footnote(_ text: String) -> some View {
        Text(text)
            .font(.system(size: 12))
            .foregroundStyle(Flow.muted)
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.horizontal, 4)
            .padding(.top, 10)
    }
}

/// A diff to show: a commit's, or the uncommitted changes for `commit == nil`, narrowed to `path`.
struct ControllerGitRoute: Hashable {
    let commit: String?
    let path: String?
    let title: String
}

struct ControllerGitDiffView: View {
    @ObservedObject var model: ControllerGitViewModel
    let route: ControllerGitRoute
    let onOpenFile: (ControllerGitDiffFile) -> Void
    @State private var diff: ControllerGitDiff?
    @State private var lines: [ControllerGitPatchLine] = []
    @State private var failure: String?

    var body: some View {
        Group {
            if let diff {
                ScrollView([.vertical]) {
                    VStack(alignment: .leading, spacing: 0) {
                        if let commit = diff.commit { commitHeader(commit) }
                        if route.path == nil && diff.files.count > 1 { fileList(diff) }
                        patch(diff)
                    }
                    .padding(.bottom, 24)
                }
                .background(Flow.canvas)
            } else if let failure {
                ContentUnavailableView(
                    String(localized: "No Diff"),
                    systemImage: "exclamationmark.triangle",
                    description: Text(failure)
                )
            } else {
                ProgressView().frame(maxWidth: .infinity, maxHeight: .infinity)
            }
        }
        .background(Flow.canvas)
        .navigationTitle(route.title)
        .navigationBarTitleDisplayMode(.inline)
        .task(id: route) {
            do {
                let loaded = try await model.diff(commit: route.commit, path: route.path)
                diff = loaded
                lines = ControllerGitPatchLine.parse(loaded.patch)
            } catch {
                failure = ControllerGitViewModel.message(for: error)
            }
        }
    }

    private func commitHeader(_ commit: ControllerGitCommitDetail) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(commit.message)
                .font(.system(size: 14))
                .foregroundStyle(Flow.text)
                .textSelection(.enabled)
            Text("\(commit.author) · \(ControllerGitText.relative(commit.unixTime))")
                .font(.system(size: 12))
                .foregroundStyle(Flow.muted)
            Text(commit.oid)
                .font(.system(size: 11, design: .monospaced))
                .foregroundStyle(Flow.muted)
                .textSelection(.enabled)
        }
        .padding(14)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Flow.surface)
    }

    private func fileList(_ diff: ControllerGitDiff) -> some View {
        VStack(spacing: 0) {
            FlowGroupLabel(String(localized: "\(diff.files.count) files"))
            FlowCard {
                ForEach(Array(diff.files.enumerated()), id: \.element.id) { index, file in
                    FlowRow(
                        name: ControllerGitText.fileName(file.path),
                        meta: ControllerGitText.counts(file),
                        glyph: ControllerGitText.glyph(file.status),
                        divider: index < diff.files.count - 1,
                        onTap: file.status == .untracked ? nil : { onOpenFile(file) }
                    ) {
                        FlowBadge(text: file.status.letter)
                    }
                }
            }
            if diff.filesTruncated {
                Text(String(localized: "More files changed than fit here."))
                    .font(.system(size: 12))
                    .foregroundStyle(Flow.muted)
                    .padding(.top, 6)
            }
        }
        .padding(.horizontal, 14)
        .padding(.bottom, 10)
    }

    @ViewBuilder
    private func patch(_ diff: ControllerGitDiff) -> some View {
        if lines.isEmpty {
            Text(diff.files.isEmpty
                ? String(localized: "No changes.")
                : String(localized: "No text changes to show; the files may be binary or new."))
                .font(.system(size: 12.5))
                .foregroundStyle(Flow.muted)
                .padding(14)
        } else {
            ScrollView(.horizontal) {
                LazyVStack(alignment: .leading, spacing: 0) {
                    ForEach(lines) { line in
                        Text(line.text.isEmpty ? " " : line.text)
                            .font(.system(size: 11.5, design: .monospaced))
                            .foregroundStyle(line.kind.foreground)
                            .fixedSize(horizontal: true, vertical: false)
                            .padding(.horizontal, 10)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .background(line.kind.background)
                    }
                }
                .textSelection(.enabled)
            }
            .background(Color(hex: 0x0B0D10))
            if diff.patchTruncated {
                Text(String(localized: "The diff is too long to show in full. Open a single file to see more of it."))
                    .font(.system(size: 12))
                    .foregroundStyle(Flow.warn)
                    .padding(14)
            }
        }
    }
}

/// One line of a unified diff and how to colour it.
struct ControllerGitPatchLine: Identifiable, Equatable {
    enum Kind: Equatable {
        case file, hunk, added, removed, context

        var foreground: Color {
            switch self {
            case .file: Flow.text
            case .hunk: Flow.accent
            case .added: Flow.good
            case .removed: Flow.danger
            case .context: Flow.text2
            }
        }

        var background: Color {
            switch self {
            case .file: Flow.raised
            case .hunk: Flow.selection.opacity(0.35)
            case .added: Color(hex: 0x17301F).opacity(0.7)
            case .removed: Color(hex: 0x3A1D1B).opacity(0.7)
            case .context: .clear
            }
        }
    }

    let id: Int
    let text: String
    let kind: Kind

    static func parse(_ patch: String) -> [ControllerGitPatchLine] {
        var inHeader = false
        var raws = patch.split(separator: "\n", omittingEmptySubsequences: false)
        if raws.last?.isEmpty == true { raws.removeLast() }
        return raws
            .enumerated()
            .map { index, raw in
                let text = String(raw).replacingOccurrences(of: "\t", with: "    ")
                let kind: Kind
                if text.hasPrefix("diff --git") {
                    inHeader = true
                    kind = .file
                } else if text.hasPrefix("@@") {
                    inHeader = false
                    kind = .hunk
                } else if inHeader {
                    kind = .file
                } else if text.hasPrefix("+") {
                    kind = .added
                } else if text.hasPrefix("-") {
                    kind = .removed
                } else {
                    kind = .context
                }
                return ControllerGitPatchLine(id: index, text: text, kind: kind)
            }
    }
}

enum ControllerGitText {
    static func fileName(_ path: String) -> String {
        path.split(separator: "/").last.map(String.init) ?? path
    }

    static func changeMeta(_ change: ControllerGitChange) -> String {
        let folder = folderName(change.path)
        let state: String
        switch (change.staged, change.unstaged) {
        case (true, true): state = String(localized: "staged and changed")
        case (true, false): state = String(localized: "staged")
        default: state = statusName(change.status)
        }
        if let original = change.originalPath {
            return String(localized: "from \(original)") + " · " + state
        }
        return folder.isEmpty ? state : "\(folder) · \(state)"
    }

    static func counts(_ file: ControllerGitDiffFile) -> String {
        guard let additions = file.additions, let deletions = file.deletions else {
            return file.status == .untracked
                ? String(localized: "untracked")
                : String(localized: "binary")
        }
        return "+\(additions) −\(deletions)"
    }

    static func statusName(_ status: ControllerGitChangeStatus) -> String {
        switch status {
        case .modified: String(localized: "modified")
        case .added: String(localized: "added")
        case .deleted: String(localized: "deleted")
        case .renamed: String(localized: "renamed")
        case .copied: String(localized: "copied")
        case .typeChanged: String(localized: "type changed")
        case .untracked: String(localized: "untracked")
        case .conflicted: String(localized: "conflict")
        }
    }

    static func glyph(_ status: ControllerGitChangeStatus) -> String {
        switch status {
        case .added, .untracked: "doc.badge.plus"
        case .deleted: "trash"
        case .renamed, .copied: "arrow.right.doc.on.clipboard"
        case .conflicted: "exclamationmark.triangle"
        default: "doc.text"
        }
    }

    static func relative(_ unixTime: UInt64) -> String {
        guard unixTime > 0 else { return "" }
        let formatter = RelativeDateTimeFormatter()
        formatter.unitsStyle = .short
        return formatter.localizedString(for: Date(timeIntervalSince1970: TimeInterval(unixTime)), relativeTo: Date())
    }

    private static func folderName(_ path: String) -> String {
        guard let slash = path.lastIndex(of: "/") else { return "" }
        return String(path[..<slash])
    }
}
