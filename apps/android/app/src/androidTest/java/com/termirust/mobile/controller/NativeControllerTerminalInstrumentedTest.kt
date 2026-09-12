package com.termirust.mobile.controller

import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import com.termirust.mobile.terminal.TerminalBuffer
import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class NativeControllerTerminalInstrumentedTest {
    @Test
    fun productionNativeEngineHandlesFullScreenEditing() {
        assertTrue("mobile terminal JNI library must load", NativeControllerTerminal.loaded)
        val terminal = NativeControllerTerminalSession.openOrNull(
            TerminalViewport(columns = 12, rows = 4),
            TerminalLimits(),
        )
        assertNotNull("native terminal session must open", terminal)

        terminal.use {
            val snapshot = checkNotNull(it).process(
                "one\r\ntwo\r\nthree\u001b[2;1H\u001b[Linsert\u001b[3;1H\u001b[2@>>"
                    .encodeToByteArray(),
            )
            assertEquals(listOf("one", "insert", ">>two"), snapshot.lines.take(3))
            assertEquals(2, snapshot.cursorRow)
            assertEquals(2, snapshot.cursorColumn)
        }
    }

    @Test
    fun directSshBufferUsesProductionNativeEngine() {
        val terminal = TerminalBuffer()
        terminal.use {
            it.resize(columns = 12, rows = 4)
            it.append(
                "one\r\ntwo\r\nthree\u001b[2;1H\u001b[L\u001b[1;31minsert" +
                    "\u001b[0m\u001b[3;1H\u001b[2@>>",
            )

            val snapshot = it.screen.value
            assertEquals(listOf("one", "insert", ">>two"), it.lines.value.take(3))
            assertEquals(2, snapshot.cursorRow)
            assertEquals(2, snapshot.cursorColumn)
            assertEquals(TerminalCellColor.Indexed(1), snapshot.contentCells[1][0].style.foreground)
        }
    }

    @Test
    fun productionNativeEngineHandlesInteractiveFixtureAtEveryNetworkSplit() {
        assertTrue("mobile terminal JNI library must load", NativeControllerTerminal.loaded)
        val fixture = InstrumentationRegistry.getInstrumentation().context.assets
            .open("terminal-interactive-v1.json")
            .use { JSON.decodeFromString<InteractiveFixture>(it.readBytes().decodeToString()) }
        assertEquals(1, fixture.schemaVersion)

        fixture.cases.forEach { case ->
            val input = case.input.encodeToByteArray()
            for (split in 0..input.size) {
                val terminal = NativeControllerTerminalSession.openOrNull(
                    TerminalViewport(columns = case.columns, rows = case.rows),
                    TerminalLimits(maxScrollbackRows = case.scrollback),
                )
                assertNotNull("native terminal session must open", terminal)
                terminal.use {
                    checkNotNull(it).feed(input.copyOfRange(0, split))
                    it.feed(input.copyOfRange(split, input.size))
                    val snapshot = it.snapshot()
                    val message = "${case.name}, split $split"
                    assertEquals(message, case.expected.lines, snapshot.lines)
                    assertEquals(message, case.expected.cursorRow, snapshot.cursorRow)
                    assertEquals(message, case.expected.cursorColumn, snapshot.cursorColumn)
                    assertEquals(message, case.expected.cursorVisible, snapshot.cursorVisible)
                    assertEquals(message, case.expected.applicationCursor, snapshot.applicationCursor)
                    assertEquals(message, case.expected.applicationKeypad, snapshot.applicationKeypad)
                    assertEquals(message, case.expected.alternateScreen, snapshot.alternateScreen)
                    assertEquals(message, case.expected.bracketedPaste, snapshot.bracketedPaste)
                    assertEquals(message, case.expected.mouseMode, snapshot.mouseMode.wireName)
                    assertEquals(message, case.expected.mouseEncoding, snapshot.mouseEncoding.wireName)
                    assertEquals(message, case.expected.scrollbackRows, snapshot.scrollbackRows)
                }
            }
        }
    }

    companion object {
        private val JSON = Json { ignoreUnknownKeys = false }
    }
}

@Serializable
private data class InteractiveFixture(
    @SerialName("schema_version") val schemaVersion: Int,
    val cases: List<InteractiveCase>,
)

@Serializable
private data class InteractiveCase(
    val name: String,
    val columns: Int,
    val rows: Int,
    val scrollback: Int,
    val input: String,
    val expected: InteractiveExpected,
)

@Serializable
private data class InteractiveExpected(
    val lines: List<String>,
    @SerialName("cursor_row") val cursorRow: Int,
    @SerialName("cursor_column") val cursorColumn: Int,
    @SerialName("cursor_visible") val cursorVisible: Boolean,
    @SerialName("application_cursor") val applicationCursor: Boolean,
    @SerialName("application_keypad") val applicationKeypad: Boolean,
    @SerialName("alternate_screen") val alternateScreen: Boolean,
    @SerialName("bracketed_paste") val bracketedPaste: Boolean,
    @SerialName("mouse_mode") val mouseMode: String,
    @SerialName("mouse_encoding") val mouseEncoding: String,
    @SerialName("scrollback_rows") val scrollbackRows: Int,
)
