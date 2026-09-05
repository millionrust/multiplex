package com.termirust.mobile.controller

import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.fail
import org.junit.Test

class MobileRouteContractTest {
    private companion object {
        val JSON = Json { ignoreUnknownKeys = true }
    }

    @Serializable
    private data class Fixture(
        @SerialName("schema_version") val schemaVersion: Int,
        @SerialName("capability_vocabulary") val capabilityVocabulary: List<String>,
        val routes: List<Route>,
        @SerialName("invalid_cases") val invalidCases: List<InvalidCase>,
    )

    @Serializable
    private data class Route(
        val id: String,
        @SerialName("item_kind") val itemKind: String,
        @SerialName("display_kind") val displayKind: String,
        @SerialName("terminal_badge") val terminalBadge: String?,
        @SerialName("credential_owner") val credentialOwner: String,
        @SerialName("continuity_owner") val continuityOwner: String,
        val capabilities: List<String>,
        @SerialName("can_open_terminal") val canOpenTerminal: Boolean,
    ) {
        fun projection() = MobileRouteProjection.validated(
            itemKind,
            credentialOwner,
            continuityOwner,
            capabilities,
            canOpenTerminal,
        )
    }

    @Serializable
    private data class InvalidCase(
        val name: String,
        @SerialName("item_kind") val itemKind: String,
        @SerialName("credential_owner") val credentialOwner: String,
        @SerialName("continuity_owner") val continuityOwner: String,
        val capabilities: List<String>,
        @SerialName("can_open_terminal") val canOpenTerminal: Boolean,
        @SerialName("expected_error") val expectedError: String,
    ) {
        fun projection() = MobileRouteProjection.validated(
            itemKind,
            credentialOwner,
            continuityOwner,
            capabilities,
            canOpenTerminal,
        )
    }

    @Test
    fun canonicalRoutesAndInvalidCombinations() {
        val fixture = fixture()
        assertEquals(1, fixture.schemaVersion)
        assertEquals(
            MobileRouteCapability.entries.map { it.wireValue }.toSet(),
            fixture.capabilityVocabulary.toSet(),
        )
        fixture.routes.forEach { assertNotNull(it.id, it.projection()) }
        fixture.invalidCases.forEach { item ->
            try {
                item.projection()
                fail("${item.name}: invalid route unexpectedly passed")
            } catch (error: MobileRouteContractException) {
                assertEquals(item.name, item.expectedError, error.reason.wireValue)
            }
        }
    }

    @Test
    fun onlyTerminalOwningKindsCreateTerminalDestinations() {
        fixture().routes.forEach { item ->
            val projection = item.projection()
            try {
                MobileTerminalDestination(item.id, item.displayKind, item.terminalBadge.orEmpty(), projection)
                if (!projection.canOpenTerminal) fail("${item.id}: non-terminal item opened a terminal")
            } catch (error: MobileRouteContractException) {
                if (projection.canOpenTerminal) throw error
                assertEquals(MobileRouteContractError.TERMINAL_OWNERSHIP_MISMATCH, error.reason)
            }
        }
    }

    private fun fixture(): Fixture {
        val stream = checkNotNull(javaClass.classLoader?.getResourceAsStream("mobile-route-contract-v1.json"))
        return stream.use { JSON.decodeFromString<Fixture>(it.reader().readText()) }
    }
}
