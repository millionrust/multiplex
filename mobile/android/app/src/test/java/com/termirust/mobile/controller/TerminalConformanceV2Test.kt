package com.termirust.mobile.controller

import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json
import org.junit.Assert.assertEquals
import org.junit.Test

class TerminalConformanceV2Test {
    @Serializable
    private data class Fixture(
        @SerialName("schema_version") val schemaVersion: Int,
        @SerialName("unicode_width_version") val unicodeWidthVersion: String,
        val styles: List<Style>,
        val cases: List<Case>,
    )

    @Serializable
    private data class Case(
        val name: String,
        val columns: Int,
        val rows: Int,
        val scrollback: Int,
        val operations: List<Operation>,
        val expected: Expected,
    )

    @Serializable
    private data class Operation(
        val kind: String,
        val bytes: List<Int>? = null,
        val columns: Int? = null,
        val rows: Int? = null,
    )

    @Serializable
    private data class Expected(
        val lines: List<String>,
        val cells: List<List<Cell>>,
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

    @Serializable
    private data class Cell(val text: String, val width: Int, val style: Int)

    @Serializable
    private data class Style(
        val foreground: Color,
        val background: Color,
        val bold: Boolean,
        val dim: Boolean,
        val italic: Boolean,
        val underline: Boolean,
        val inverse: Boolean,
    ) {
        fun matches(style: TerminalCellStyle) =
            foreground.matches(style.foreground) &&
                background.matches(style.background) &&
                bold == style.bold &&
                dim == style.dim &&
                italic == style.italic &&
                underline == style.underline &&
                inverse == style.inverse
    }

    @Serializable
    private data class Color(
        val kind: String,
        val value: Int? = null,
        val red: Int? = null,
        val green: Int? = null,
        val blue: Int? = null,
    ) {
        fun matches(color: TerminalCellColor) = when {
            kind == "default" && color == TerminalCellColor.Default -> true
            kind == "indexed" && color is TerminalCellColor.Indexed -> value == color.value
            kind == "rgb" && color is TerminalCellColor.Rgb ->
                red == color.red && green == color.green && blue == color.blue
            else -> false
        }
    }

    @Test
    fun styledUnicodeAndResizeConformanceAtEveryProcessSplit() {
        val fixture = loadFixture()
        assertEquals(2, fixture.schemaVersion)
        assertEquals(GeneratedTerminalCellWidth.UNICODE_WIDTH_VERSION, fixture.unicodeWidthVersion)

        for (case in fixture.cases) {
            assertEquals(case.name, case.expected, render(case, fixture.styles, null))
            case.operations.forEachIndexed { operationIndex, operation ->
                if (operation.kind == "process") {
                    val bytes = checkNotNull(operation.bytes)
                    for (split in 0..bytes.size) {
                        assertEquals(
                            "${case.name} operation $operationIndex split at $split",
                            case.expected,
                            render(case, fixture.styles, operationIndex to split),
                        )
                    }
                }
            }
        }
    }

    private fun render(
        case: Case,
        styles: List<Style>,
        split: Pair<Int, Int>?,
    ): Expected {
        val terminal = BoundedControllerTerminal(
            TerminalViewport(case.columns, case.rows),
            TerminalLimits(maxScrollbackRows = case.scrollback),
        )
        case.operations.forEachIndexed { index, operation ->
            when (operation.kind) {
                "process" -> {
                    val bytes = bytes(checkNotNull(operation.bytes))
                    if (split?.first == index) {
                        terminal.process(bytes.copyOfRange(0, split.second))
                        terminal.process(bytes.copyOfRange(split.second, bytes.size))
                    } else {
                        terminal.process(bytes)
                    }
                }
                "resize" -> terminal.resize(
                    TerminalViewport(checkNotNull(operation.columns), checkNotNull(operation.rows)),
                )
                else -> error("unknown fixture operation ${operation.kind}")
            }
        }
        val snapshot = terminal.snapshot()
        return Expected(
            lines = snapshot.lines.map { it.trimEnd(' ') },
            cells = snapshot.cells.map { row ->
                row.map { cell ->
                    Cell(
                        text = cell.text,
                        width = cell.width.columns,
                        style = styles.indexOfFirst { it.matches(cell.style) },
                    )
                }
            },
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
        val stream = checkNotNull(
            javaClass.classLoader?.getResourceAsStream("terminal-conformance-v2.json"),
        )
        return stream.use { Json.decodeFromString<Fixture>(it.readBytes().decodeToString()) }
    }

    private fun bytes(values: List<Int>) = ByteArray(values.size) { values[it].toByte() }
}
