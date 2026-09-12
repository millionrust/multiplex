import Foundation

struct TmuxBootstrap {
    let host: MobileHost

    func startupCommand() -> String? {
        guard host.persistentSession.enabled else {
            return host.startupCommand
        }

        let session = shellSingleQuote(host.persistentSession.sessionName ?? defaultSessionName())
        let attach = host.persistentSession.detachOthers
            ? "exec tmux attach-session -d -t \(session)"
            : "exec tmux attach-session -t \(session)"

        var create = "exec tmux new-session -s \(session)"
        if let startupDirectory = host.startupDirectory, !startupDirectory.isEmpty {
            create += " -c \(shellSingleQuote(startupDirectory))"
        }
        if let startupCommand = host.startupCommand, !startupCommand.isEmpty {
            let shellCommand = "\(startupCommand); exec \"${SHELL:-/bin/sh}\" -l"
            create += " -- \"${SHELL:-/bin/sh}\" -lc \(shellSingleQuote(shellCommand))"
        }

        return """
        if command -v tmux >/dev/null 2>&1; then
          if tmux has-session -t \(session) 2>/dev/null; then
            \(attach)
          else
            \(create)
          fi
        else
          printf 'TermiRust mobile persistent sessions require tmux on this host. Install tmux, then reconnect.\\n' >&2
          exec "${SHELL:-/bin/sh}"
        fi
        """
    }

    private func defaultSessionName() -> String {
        let raw = "tr-\(host.username)-\(host.host)-\(host.port)"
        let allowed = CharacterSet.alphanumerics.union(CharacterSet(charactersIn: "-"))
        return raw.unicodeScalars.map { allowed.contains($0) ? Character($0) : "-" }.map(String.init).joined()
    }
}

func shellSingleQuote(_ value: String) -> String {
    "'\(value.replacingOccurrences(of: "'", with: "'\"'\"'"))'"
}
