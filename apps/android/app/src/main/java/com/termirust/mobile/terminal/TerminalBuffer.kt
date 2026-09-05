package com.termirust.mobile.terminal

import com.termirust.mobile.controller.BoundedControllerTerminal
import com.termirust.mobile.controller.BoundedTerminalSnapshot
import com.termirust.mobile.controller.TerminalLimits
import com.termirust.mobile.controller.TerminalViewport
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow

class TerminalBuffer(maxLines: Int = 2_000) : AutoCloseable {
    private val terminal = BoundedControllerTerminal(
        viewport = TerminalViewport(columns = 80, rows = 24),
        limits = TerminalLimits(maxScrollbackRows = maxLines.coerceAtLeast(1)),
    )
    private val _screen = MutableStateFlow(terminal.snapshot())
    val screen: StateFlow<BoundedTerminalSnapshot> = _screen
    private val _lines = MutableStateFlow(visibleLines(_screen.value.lines))
    val lines: StateFlow<List<String>> = _lines

    fun append(text: String) = append(text.encodeToByteArray())

    fun append(bytes: ByteArray) {
        if (bytes.isEmpty()) return
        runCatching { terminal.process(bytes) }
        publishSnapshot()
    }

    fun resize(columns: Int, rows: Int) {
        val viewport = TerminalViewport(
            columns = columns.coerceAtLeast(1),
            rows = rows.coerceAtLeast(1),
        )
        if (viewport == terminal.viewport) return
        runCatching { terminal.resize(viewport) }
        publishSnapshot()
    }

    fun clear() {
        terminal.reset()
        publishSnapshot()
    }

    override fun close() = terminal.close()

    private fun publishSnapshot() {
        val snapshot = terminal.snapshot()
        _screen.value = snapshot
        _lines.value = visibleLines(snapshot.lines)
    }

    private companion object {
        fun visibleLines(lines: List<String>): List<String> = lines.dropLastWhile(String::isEmpty)
    }
}
