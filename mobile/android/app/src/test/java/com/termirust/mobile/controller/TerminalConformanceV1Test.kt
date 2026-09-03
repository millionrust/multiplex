package com.termirust.mobile.controller

import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json
import org.junit.Assert.assertEquals
import org.junit.Test

class TerminalConformanceV1Test {
    @Serializable
    private data class Fixture(
        @SerialName("schema_version") val schemaVersion: Int,
        val cases: List<Case>,
    )

    @Serializable
    private data class Case(
        val name: String,
        val columns: Int,
        val rows: Int,
        val scrollback: Int,
        val chunks: List<List<Int>>,
        val expected: Expected,
    )

    @Serializable
    private data class Expected(
        val lines: List<String>,
        @SerialName("cursor_row") val cursorRow: Int,
        @SerialName("cursor_column") val cursorColumn: Int,
        @SerialName("cursor_visible") val cursorVisible: Boolean,
        @SerialName("application_cursor") val applicationCursor: Boolean,
        @SerialName("alternate_screen") val alternateScreen: Boolean,
        @SerialName("bracketed_paste") val bracketedPaste: Boolean,
        @SerialName("mouse_mode") val mouseMode: String,
        @SerialName("mouse_encoding") val mouseEncoding: String,
        @SerialName("scrollback_rows") val scrollbackRows: Int,
    )

    @Test
    fun terminalConformanceV1MatchesCanonicalFixtureAtEverySplit() {
        val fixture = loadFixture()
        assertEquals(1, fixture.schemaVersion)

        for (case in fixture.cases) {
            assertEquals(case.name, case.expected, render(case, case.chunks.map(::bytes)))
            val allBytes = bytes(case.chunks.flatten())
            for (split in 0..allBytes.size) {
                assertEquals(
                    "${case.name} split at $split",
                    case.expected,
                    render(case, listOf(allBytes.copyOfRange(0, split), allBytes.copyOfRange(split, allBytes.size))),
                )
            }
        }
    }

    private fun render(case: Case, chunks: List<ByteArray>): Expected {
        val terminal = BoundedControllerTerminal(
            TerminalViewport(case.columns, case.rows),
            TerminalLimits(maxScrollbackRows = case.scrollback),
        )
        chunks.forEach(terminal::process)
        val snapshot = terminal.snapshot()
        return Expected(
            lines = snapshot.lines.map { it.trimEnd(' ') },
            cursorRow = snapshot.cursorRow,
            cursorColumn = snapshot.cursorColumn,
            cursorVisible = snapshot.cursorVisible,
            applicationCursor = snapshot.applicationCursor,
            alternateScreen = snapshot.alternateScreen,
            bracketedPaste = snapshot.bracketedPaste,
            mouseMode = snapshot.mouseMode.wireName,
            mouseEncoding = snapshot.mouseEncoding.wireName,
            scrollbackRows = snapshot.scrollbackRows,
        )
    }

    private fun loadFixture(): Fixture {
        val stream = checkNotNull(javaClass.classLoader?.getResourceAsStream("terminal-conformance-v1.json"))
        return stream.use { Json.decodeFromString<Fixture>(it.readBytes().decodeToString()) }
    }

    private fun bytes(values: List<Int>) = ByteArray(values.size) { values[it].toByte() }
}
