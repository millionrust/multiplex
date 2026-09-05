package com.termirust.mobile.controller

import kotlin.system.measureTimeMillis
import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class TerminalAcceptanceTest {
    private companion object {
        val JSON = Json { ignoreUnknownKeys = true }
    }

    @Serializable
    private data class Fixture(
        @SerialName("schema_version") val schemaVersion: Int,
        val limits: Limits,
        @SerialName("layout_cases") val layoutCases: List<LayoutCase>,
        @SerialName("accessible_output_cases") val accessibleOutputCases: List<AccessibleOutputCase>,
        @SerialName("background_cases") val backgroundCases: List<BackgroundCase>,
    )

    @Serializable
    private data class Limits(
        @SerialName("minimum_font_size") val minimumFontSize: Double,
        @SerialName("maximum_font_size") val maximumFontSize: Double,
        @SerialName("maximum_accessibility_characters") val maximumAccessibilityCharacters: Int,
        @SerialName("android_minimum_target") val androidMinimumTarget: Double,
    )

    @Serializable
    private data class LayoutCase(
        val name: String,
        val width: Double,
        val height: Double,
        @SerialName("requested_font_size") val requestedFontSize: Double,
        @SerialName("text_scale") val textScale: Double,
        @SerialName("expected_font_size") val expectedFontSize: Double,
        @SerialName("expected_columns") val expectedColumns: Int,
        @SerialName("expected_rows") val expectedRows: Int,
        @SerialName("expected_compact") val expectedCompact: Boolean,
    )

    @Serializable
    private data class AccessibleOutputCase(
        val name: String,
        val lines: List<String>,
        @SerialName("maximum_characters") val maximumCharacters: Int,
        val expected: String,
    )

    @Serializable
    private data class BackgroundCase(
        val name: String,
        @SerialName("writer_held") val writerHeld: Boolean,
        @SerialName("expected_release_writer") val expectedReleaseWriter: Boolean,
        @SerialName("expected_cover_privacy") val expectedCoverPrivacy: Boolean,
        @SerialName("expected_clear_pending_input") val expectedClearPendingInput: Boolean,
        @SerialName("expected_clear_pending_resize") val expectedClearPendingResize: Boolean,
    )

    @Test
    fun canonicalAcceptanceFixtureMatchesProduction() {
        val fixture = fixture()
        assertEquals(1, fixture.schemaVersion)
        assertEquals(TerminalAcceptance.MINIMUM_FONT_SIZE, fixture.limits.minimumFontSize, 0.0)
        assertEquals(TerminalAcceptance.MAXIMUM_FONT_SIZE, fixture.limits.maximumFontSize, 0.0)
        assertEquals(
            TerminalAcceptance.MAXIMUM_ACCESSIBILITY_CHARACTERS,
            fixture.limits.maximumAccessibilityCharacters,
        )
        assertEquals(TerminalAcceptance.MINIMUM_TOUCH_TARGET, fixture.limits.androidMinimumTarget, 0.0)

        fixture.layoutCases.forEach { item ->
            val layout = TerminalAcceptance.layout(
                item.width,
                item.height,
                item.requestedFontSize,
                item.textScale,
            )
            assertEquals(item.name, item.expectedFontSize, layout.fontSize, 0.0)
            assertEquals(item.name, item.expectedColumns, layout.columns)
            assertEquals(item.name, item.expectedRows, layout.rows)
            assertEquals(item.name, item.expectedCompact, layout.compactControls)
        }
        fixture.accessibleOutputCases.forEach { item ->
            assertEquals(
                item.name,
                item.expected,
                TerminalAcceptance.accessibleOutput(item.lines, item.maximumCharacters),
            )
        }
        fixture.backgroundCases.forEach { item ->
            val decision = TerminalAcceptance.backgroundDecision(item.writerHeld)
            assertEquals(item.name, item.expectedReleaseWriter, decision.releaseWriter)
            assertEquals(item.name, item.expectedCoverPrivacy, decision.coverPrivacy)
            assertEquals(item.name, item.expectedClearPendingInput, decision.clearPendingInput)
            assertEquals(item.name, item.expectedClearPendingResize, decision.clearPendingResize)
        }
    }

    @Test
    fun boundedParserWorkloadCompletesWithinAcceptanceBudget() {
        val payload = ByteArray(64 * 1_024) { 65 }
        val elapsed = measureTimeMillis {
            repeat(3) {
                val terminal = BoundedControllerTerminal(TerminalViewport(120, 40))
                terminal.process(payload)
                terminal.snapshot()
            }
        }
        println("Terminal acceptance bounded 192 KiB parser workload passed in ${elapsed}ms.")
        assertTrue("bounded 192 KiB parser workload took ${elapsed}ms", elapsed < 5_000)
    }

    private fun fixture(): Fixture {
        val stream = checkNotNull(javaClass.classLoader?.getResourceAsStream("terminal-acceptance-v1.json"))
        return stream.use {
            JSON.decodeFromString<Fixture>(it.reader().readText())
        }
    }
}
