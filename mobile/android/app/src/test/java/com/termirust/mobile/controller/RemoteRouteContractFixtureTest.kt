package com.termirust.mobile.controller

import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json
import org.junit.Assert.assertEquals
import org.junit.Assert.assertThrows
import org.junit.Test

class RemoteRouteContractFixtureTest {
    @Test
    fun canonicalPoliciesTransitionsMutationsAndSwitchesMatchSharedFixture() {
        val fixture = fixture()
        assertEquals(1, fixture.schemaVersion)
        assertEquals(4, fixture.routes.size)
        fixture.routes.forEach { expected ->
            assertEquals(expected, ControllerRemoteRoutePolicy.canonical(expected.kind))
        }
        fixture.transitionCases.forEach { item ->
            assertEquals(item.name, item.expected, item.initial.transition(item.event.value()))
        }
        fixture.invalidTransitionCases.forEach { item ->
            val error = assertThrows(ControllerRemoteRouteException::class.java) {
                item.initial.transition(item.event.value())
            }
            assertEquals(item.name, ControllerRemoteRouteError.INVALID_TRANSITION, error.reason)
        }
        fixture.mutationCases.forEach { item ->
            assertEquals(item.expected, item.state.mutationDisposition(item.completion))
        }
        fixture.switchCases.forEach { item ->
            if (item.expected != null) {
                assertEquals(
                    item.name,
                    item.expected,
                    item.initial.switchTo(item.target, item.platform, item.targetAvailable, item.confirmed),
                )
            } else {
                val error = assertThrows(ControllerRemoteRouteException::class.java) {
                    item.initial.switchTo(item.target, item.platform, item.targetAvailable, item.confirmed)
                }
                assertEquals(item.name, item.expectedError, errorName(error.reason))
            }
        }
    }

    private fun fixture(): Fixture {
        val stream = checkNotNull(javaClass.classLoader?.getResourceAsStream("route-selection-v1.json"))
        return stream.use { JSON.decodeFromString<Fixture>(it.reader().readText()) }
    }

    private fun errorName(error: ControllerRemoteRouteError) = when (error) {
        ControllerRemoteRouteError.INVALID_TRANSITION -> "InvalidTransition"
        ControllerRemoteRouteError.EXPLICIT_CONFIRMATION_REQUIRED -> "ExplicitConfirmationRequired"
        ControllerRemoteRouteError.SAME_ROUTE -> "SameRoute"
        ControllerRemoteRouteError.UNSUPPORTED_PLATFORM -> "UnsupportedPlatform"
        ControllerRemoteRouteError.TARGET_UNAVAILABLE -> "TargetUnavailable"
    }

    private companion object {
        val JSON = Json { ignoreUnknownKeys = false }
    }

    @Serializable
    private data class Fixture(
        @SerialName("schema_version") val schemaVersion: Int,
        val routes: List<ControllerRemoteRoutePolicy>,
        @SerialName("transition_cases") val transitionCases: List<TransitionCase>,
        @SerialName("invalid_transition_cases") val invalidTransitionCases: List<TransitionCase>,
        @SerialName("mutation_cases") val mutationCases: List<MutationCase>,
        @SerialName("switch_cases") val switchCases: List<SwitchCase>,
    )

    @Serializable
    private data class TransitionCase(
        val name: String,
        val initial: ControllerRemoteRouteState,
        val event: EventFixture,
        val expected: ControllerRemoteRouteTransition? = null,
    )

    @Serializable
    private data class EventFixture(
        val kind: String,
        val available: Boolean? = null,
        val retryable: Boolean? = null,
        @SerialName("mutation_in_flight") val mutationInFlight: Boolean? = null,
    ) {
        fun value(): ControllerRemoteRouteEvent = when (kind) {
            "enable" -> ControllerRemoteRouteEvent.Enable(checkNotNull(available))
            "connect" -> ControllerRemoteRouteEvent.Connect
            "transport_ready" -> ControllerRemoteRouteEvent.TransportReady
            "authenticated" -> ControllerRemoteRouteEvent.Authenticated
            "failure" -> ControllerRemoteRouteEvent.Failure(
                checkNotNull(retryable),
                checkNotNull(mutationInFlight),
            )
            "availability_lost" -> ControllerRemoteRouteEvent.AvailabilityLost
            "authorization_restored" -> ControllerRemoteRouteEvent.AuthorizationRestored(checkNotNull(available))
            "retry" -> ControllerRemoteRouteEvent.Retry
            "cancel" -> ControllerRemoteRouteEvent.Cancel
            "revoke" -> ControllerRemoteRouteEvent.Revoke
            "disable" -> ControllerRemoteRouteEvent.Disable
            else -> error("unsupported event $kind")
        }
    }

    @Serializable
    private data class MutationCase(
        val state: ControllerRemoteRouteState,
        val completion: ControllerRemoteMutationCompletion,
        val expected: ControllerRemoteMutationDisposition,
    )

    @Serializable
    private data class SwitchCase(
        val name: String,
        val initial: ControllerRemoteRouteState,
        val target: ControllerRemoteRouteKind,
        val platform: ControllerRemotePlatform,
        @SerialName("target_available") val targetAvailable: Boolean,
        val confirmed: Boolean,
        val expected: ControllerRemoteSwitchDecision? = null,
        @SerialName("expected_error") val expectedError: String? = null,
    )
}
