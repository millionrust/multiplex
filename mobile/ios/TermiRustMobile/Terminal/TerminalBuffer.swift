import Foundation

@MainActor
final class TerminalBuffer: ObservableObject {
    @Published private(set) var screen: BoundedTerminalSnapshot

    var lines: [String] {
        Self.visibleLines(screen.lines)
    }

    private var terminal: BoundedTerminalBuffer

    init(maxLines: Int = 2_000) {
        let viewport = TerminalViewportState(columns: 80, rows: 24)
        let limits = TerminalLimits(maxScrollbackRows: max(1, maxLines))
        terminal = try! BoundedTerminalBuffer(viewport: viewport, limits: limits)
        screen = terminal.snapshot()
    }

    func append(_ text: String) {
        append(Data(text.utf8))
    }

    func append(_ data: Data) {
        guard !data.isEmpty else { return }
        do {
            try terminal.process(data)
            publishSnapshot()
        } catch {
            // The bounded parser keeps its last safe screen when a frame is rejected.
            publishSnapshot()
        }
    }

    func resize(columns: Int, rows: Int) {
        let viewport = TerminalViewportState(
            columns: max(columns, 1),
            rows: max(rows, 1)
        )
        guard viewport != terminal.viewport else { return }
        do {
            try terminal.resize(viewport)
            publishSnapshot()
        } catch {
            publishSnapshot()
        }
    }

    func clear() {
        try? terminal.reset()
        publishSnapshot()
    }

    private func publishSnapshot() {
        screen = terminal.snapshot()
    }

    private static func visibleLines(_ lines: [String]) -> [String] {
        var lines = lines
        while lines.last?.isEmpty == true { lines.removeLast() }
        return lines
    }
}
