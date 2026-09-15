package com.termirust.mobile.ssh

import com.termirust.mobile.data.MobileHost

class TmuxBootstrap(private val host: MobileHost) {
    fun startupCommand(): String? {
        if (!host.persistentSession.enabled) {
            return host.startupCommand
        }

        val session = shellSingleQuote(sessionName())
        val attach = if (host.persistentSession.detachOthers) {
            "exec tmux attach-session -d -t $session"
        } else {
            "exec tmux attach-session -t $session"
        }
        val create = buildString {
            append("exec tmux new-session -s $session")
            host.startupDirectory?.takeIf { it.isNotBlank() }?.let {
                append(" -c ${shellSingleQuote(it)}")
            }
            host.startupCommand?.takeIf { it.isNotBlank() }?.let {
                val shellCommand = "$it; exec \"${'$'}{SHELL:-/bin/sh}\" -l"
                append(" -- \"${'$'}{SHELL:-/bin/sh}\" -lc ${shellSingleQuote(shellCommand)}")
            }
        }

        return """
            if command -v tmux >/dev/null 2>&1; then
              if tmux has-session -t $session 2>/dev/null; then
                $attach
              else
                $create
              fi
            else
              printf 'TermiRust mobile persistent sessions require tmux on this host. Install tmux, then reconnect.\n' >&2
              exec "${'$'}{SHELL:-/bin/sh}"
            fi
        """.trimIndent()
    }

    internal fun sessionName(): String =
        host.persistentSession.sessionName?.takeIf { it.isNotBlank() } ?: defaultSessionName()

    private fun defaultSessionName(): String =
        "tr-${host.username}-${host.host}-${host.port}".replace(Regex("[^A-Za-z0-9-]"), "-")
}

fun shellSingleQuote(value: String): String = "'${value.replace("'", "'\"'\"'")}'"
