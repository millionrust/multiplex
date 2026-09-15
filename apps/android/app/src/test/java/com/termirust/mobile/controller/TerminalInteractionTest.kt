package com.termirust.mobile.controller

import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class TerminalInteractionTest {
    @Serializable
    private data class Fixture(
        @SerialName("schema_version") val schemaVersion: Int,
        val limits: Limits,
        @SerialName("key_cases") val keyCases: List<KeyCase>,
        @SerialName("paste_cases") val pasteCases: List<PasteCase>,
        @SerialName("selection_cases") val selectionCases: List<SelectionCase>,
        @SerialName("ime_cases") val imeCases: List<ImeCase>,
        @SerialName("url_cases") val urlCases: List<UrlCase>,
    )

    @Serializable
    private data class Limits(
        @SerialName("max_paste_bytes") val maxPasteBytes: Int,
        @SerialName("paste_confirmation_bytes") val pasteConfirmationBytes: Int,
        @SerialName("max_url_bytes") val maxUrlBytes: Int,
        @SerialName("max_urls") val maxUrls: Int,
    )

    @Serializable
    private data class KeyCase(
        val name: String,
        val key: String,
        val text: String? = null,
        val shift: Boolean = false,
        val control: Boolean = false,
        val alt: Boolean = false,
        @SerialName("application_cursor") val applicationCursor: Boolean = false,
        val expected: List<Int>,
    )

    @Serializable
    private data class PasteCase(
        val name: String,
        val input: String? = null,
        @SerialName("input_repeat") val inputRepeat: Repeat? = null,
        val bracketed: Boolean,
        @SerialName("requires_confirmation") val requiresConfirmation: Boolean,
        val expected: List<Int>? = null,
    )

    @Serializable private data class Repeat(val value: String, val count: Int)
    @Serializable private data class Point(val row: Int, val column: Int)
    @Serializable private data class Cell(val text: String, val width: Int)
    @Serializable
    private data class SelectionCase(
        val name: String,
        val rows: List<List<Cell>>,
        val start: Point,
        val end: Point,
        val expected: String,
    )
    @Serializable
    private data class ImeCase(
        val name: String,
        val operations: List<ImeOperation>,
        @SerialName("expected_emissions") val expectedEmissions: List<List<Int>>,
    )
    @Serializable private data class ImeOperation(val kind: String, val text: String? = null)
    @Serializable private data class UrlCase(val name: String, val text: String, val expected: List<String>)

    @Test
    fun canonicalTerminalInteractionFixtureMatchesProduction() {
        val fixture = fixture()
        assertEquals(1, fixture.schemaVersion)
        assertEquals(TerminalInteraction.MAX_PASTE_BYTES, fixture.limits.maxPasteBytes)
        assertEquals(TerminalInteraction.PASTE_CONFIRMATION_BYTES, fixture.limits.pasteConfirmationBytes)
        assertEquals(TerminalInteraction.MAX_URL_BYTES, fixture.limits.maxUrlBytes)
        assertEquals(TerminalInteraction.MAX_URLS, fixture.limits.maxUrls)

        fixture.keyCases.forEach { item ->
            val key = runCatching { TerminalInputKey.valueOf(item.key.uppercase()) }
                .getOrDefault(TerminalInputKey.TEXT)
            assertArrayEquals(
                item.name,
                item.expected.toBytes(),
                TerminalInteraction.encode(
                    key,
                    item.text,
                    TerminalInputModifiers(item.shift, item.control, item.alt),
                    item.applicationCursor,
                ),
            )
        }

        fixture.pasteCases.forEach { item ->
            val input = item.input ?: item.inputRepeat?.let { it.value.repeat(it.count) }
                ?: error("${item.name}: missing paste input")
            val normalized = TerminalInteraction.normalizePaste(input)
            assertTrue(item.name, normalized.size <= TerminalInteraction.MAX_PASTE_BYTES)
            assertEquals(
                item.name,
                item.requiresConfirmation,
                TerminalInteraction.pasteRequiresConfirmation(normalized),
            )
            item.expected?.let { expected ->
                assertArrayEquals(
                    item.name,
                    expected.toBytes(),
                    TerminalInteraction.preparePaste(normalized, item.bracketed),
                )
            }
        }

        fixture.selectionCases.forEach { item ->
            val rows = item.rows.map { row ->
                row.map { cell ->
                    BoundedTerminalCell(
                        cell.text,
                        TerminalCellWidth.entries.first { it.columns == cell.width },
                        TerminalCellStyle(),
                    )
                }
            }
            assertEquals(
                item.name,
                item.expected,
                TerminalInteraction.selectionText(
                    rows,
                    TerminalSelectionPoint(item.start.row, item.start.column),
                    TerminalSelectionPoint(item.end.row, item.end.column),
                ),
            )
        }

        fixture.imeCases.forEach { item ->
            val state = TerminalImeState()
            val emissions = mutableListOf<ByteArray>()
            item.operations.forEach { operation ->
                when (operation.kind) {
                    "update" -> state.update(operation.text.orEmpty())
                    "cancel" -> state.cancel()
                    "commit" -> state.commit(operation.text.orEmpty())?.let(emissions::add)
                    "finish" -> state.finish()?.let(emissions::add)
                    else -> error("${item.name}: unknown IME operation")
                }
            }
            assertEquals(item.name, item.expectedEmissions.map { it.toBytes().toList() }, emissions.map { it.toList() })
        }

        fixture.urlCases.forEach { item ->
            assertEquals(item.name, item.expected, TerminalInteraction.visibleHttpUrls(item.text))
        }
    }

    @Test
    fun selectableContentExcludesPaddingAndOscLinks() {
        val terminal = BoundedControllerTerminal(TerminalViewport(40, 2))
        terminal.process("hi https://example.com\u001b]8;;https://evil.example\u0007!".encodeToByteArray())
        val snapshot = terminal.snapshot()
        assertEquals(40, snapshot.cells.first().size)
        assertTrue(snapshot.contentCells.first().size < snapshot.cells.first().size)
        assertEquals(
            listOf("https://example.com"),
            TerminalInteraction.visibleHttpUrls(snapshot.lines.joinToString("\n")),
        )
    }

    @Test
    fun controllerFollowTargetStopsAtCursorOrLatestContentNotBlankPadding() {
        assertEquals(0, ControllerTerminalFollowTarget.row(listOf("prompt", "", "", ""), 0, 0))
        assertEquals(2, ControllerTerminalFollowTarget.row(listOf("old", "new", "", ""), 1, 1))
        assertEquals(1, ControllerTerminalFollowTarget.row(listOf("prompt", "later output", "", ""), 0, 0))
        assertEquals(null, ControllerTerminalFollowTarget.row(emptyList(), 0, 0))
        assertEquals(6, ControllerTerminalFollowTarget.firstVisibleRow(targetRow = 9, visibleRows = 4))
        assertEquals(0, ControllerTerminalFollowTarget.firstVisibleRow(targetRow = 1, visibleRows = 10))
    }

    @Test
    fun controllerCursorMapsViewportRowAndWideCellContinuation() {
        val cells = listOf(
            BoundedTerminalCell("界", TerminalCellWidth.WIDE, TerminalCellStyle()),
            BoundedTerminalCell.continuation(TerminalCellStyle()),
        )
        assertEquals(0, ControllerTerminalCursor.column(4, cells, 2, 1, 2, true))
        assertEquals(5, ControllerTerminalCursor.column(4, cells, 2, 5, 2, true))
        assertEquals(null, ControllerTerminalCursor.column(3, cells, 2, 1, 2, true))
        assertEquals(null, ControllerTerminalCursor.column(4, cells, 2, 1, 2, false))
    }

    @Test
    fun controllerTerminalUsesCompactChromeOnlyForFocusedLandscape() {
        assertTrue(ControllerTerminalLayout.usesFocusedLandscape(true, true))
        assertFalse(ControllerTerminalLayout.usesFocusedLandscape(false, true))
        assertFalse(ControllerTerminalLayout.usesFocusedLandscape(true, false))
    }

    @Test
    fun controllerTerminalDesktopWidthPreservesAtLeastEightyColumns() {
        assertEquals(39, ControllerTerminalWidth.columns(39, usesDesktopWidth = false))
        assertEquals(80, ControllerTerminalWidth.columns(39, usesDesktopWidth = true))
        assertEquals(96, ControllerTerminalWidth.columns(96, usesDesktopWidth = true))
    }

    private fun fixture(): Fixture {
        val stream = checkNotNull(javaClass.classLoader?.getResourceAsStream("terminal-interaction-v1.json"))
        return stream.use { Json.decodeFromString<Fixture>(it.reader().readText()) }
    }

    private fun List<Int>.toBytes() = map(Int::toByte).toByteArray()
}
