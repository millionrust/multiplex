package com.termirust.mobile.controller

import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Test

class MobileCrossRouteAcceptanceTest {
    private companion object {
        val JSON = Json { ignoreUnknownKeys = true }
    }

    @Serializable
    private data class Fixture(
        @SerialName("schema_version") val schemaVersion: Int,
        val cases: List<Case>,
    )

    @Serializable
    private data class Case(
        val name: String,
        val route: String,
        val event: String,
        @SerialName("tmux_enabled") val tmuxEnabled: Boolean,
        @SerialName("tmux_available") val tmuxAvailable: Boolean,
        @SerialName("host_key_matches") val hostKeyMatches: Boolean,
        @SerialName("authority_valid") val authorityValid: Boolean,
        @SerialName("writer_held") val writerHeld: Boolean,
        @SerialName("pending_input") val pendingInput: Boolean,
        val expected: MobileCrossRouteDecision,
    ) {
        fun decision() = MobileCrossRouteAcceptance.decide(
            route = checkNotNull(MobileTerminalRoute.entries.firstOrNull { it.wireValue == route }),
            event = checkNotNull(MobileRouteEvent.entries.firstOrNull { it.wireValue == event }),
            tmuxEnabled = tmuxEnabled,
            tmuxAvailable = tmuxAvailable,
            hostKeyMatches = hostKeyMatches,
            authorityValid = authorityValid,
            writerHeld = writerHeld,
            pendingInput = pendingInput,
        )
    }

    @Test
    fun canonicalCrossRouteDecisions() {
        val fixture = fixture()
        assertEquals(1, fixture.schemaVersion)
        assertEquals(15, fixture.cases.size)
        fixture.cases.forEach { item ->
            assertEquals(item.name, item.expected, item.decision())
            assertFalse(item.name, item.expected.replayTerminalInput)
        }
    }

    private fun fixture(): Fixture {
        val stream = checkNotNull(javaClass.classLoader?.getResourceAsStream("mobile-cross-route-acceptance-v1.json"))
        return stream.use { JSON.decodeFromString<Fixture>(it.reader().readText()) }
    }
}
