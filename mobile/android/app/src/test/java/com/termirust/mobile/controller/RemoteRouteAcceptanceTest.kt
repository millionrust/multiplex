package com.termirust.mobile.controller

import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test

class RemoteRouteAcceptanceTest {
    @Test
    fun sharedLifecycleCorpusPassesEveryAndroidRoute() {
        val fixture = fixture()
        assertEquals(1, fixture.schemaVersion)
        assertEquals(ControllerRemoteRouteKind.androidRoutes, fixture.routes)

        fixture.routes.forEach { route ->
            fixture.lifecycleCases.forEach { item ->
                val coordinator = AndroidControllerRouteCoordinator(allAvailable())
                val actual = Metrics()
                item.steps.forEach { step ->
                    val plan = when (step.kind) {
                        "select" -> coordinator.select(route, explicitlyConfirmed = true)
                        "connect" -> coordinator.connectSelected()
                        "transport_ready" -> coordinator.transportReady(route)
                        "authenticated" -> coordinator.authenticated(route)
                        "set_writer" -> {
                            coordinator.setWriterHeld(checkNotNull(step.held))
                            return@forEach
                        }
                        "failure" -> coordinator.failed(
                            route,
                            retryable = checkNotNull(step.retryable),
                            mutationInFlight = checkNotNull(step.mutationInFlight),
                        )
                        "retry" -> coordinator.retrySelected()
                        "cancel" -> coordinator.cancelSelected()
                        "revoke" -> coordinator.revokeSelected()
                        "set_available" -> {
                            coordinator.setAvailable(route, checkNotNull(step.available))?.let(actual::observe)
                            return@forEach
                        }
                        "authorization_restored" -> coordinator.authorizationRestored(route)
                        else -> error("${item.name}: unsupported step ${step.kind}")
                    }
                    actual.observe(plan)
                }
                val projection = coordinator.projections.first { it.route == route }
                actual.phase = projection.phase
                actual.terminalAllowed = projection.terminalAllowed
                assertEquals("${item.name} on ${route.name}", item.expected, actual)
                assertEquals(0, actual.mutationReplays)
                assertEquals(0, actual.automaticSwitches)
            }
        }
    }

    @Test
    fun sharedSwitchMatrixIsExplicitAndSourceOwned() {
        val fixture = fixture()
        val expected = fixture.switchMatrix.confirmed
        assertEquals(ControllerRemoteRoutePhase.ONLINE, expected.sourcePhase)
        assertTrue(expected.writerHeld)

        fixture.routes.forEach { source ->
            fixture.routes.filter { it != source }.forEach { target ->
                val coordinator = AndroidControllerRouteCoordinator(allAvailable())
                connectOnline(coordinator, source)
                coordinator.setWriterHeld(expected.writerHeld)

                val unconfirmed = assertThrows(AndroidControllerRouteCoordinatorException::class.java) {
                    coordinator.select(target, explicitlyConfirmed = false)
                }
                assertEquals(fixture.switchMatrix.unconfirmedError, errorCode(unconfirmed))
                assertEquals(source, coordinator.selected)

                val plan = coordinator.select(target, explicitlyConfirmed = true)
                assertEquals(expected.sourceDisconnects, if (plan.disconnectTransport == source) 1 else 0)
                assertEquals(expected.targetStarts, if (plan.startTransport == target) 1 else 0)
                assertEquals(expected.inputClears, if (plan.clearPendingInput) 1 else 0)
                assertEquals(expected.writerReleases, if (plan.releaseWriter) 1 else 0)
                assertEquals(0, expected.automaticSwitches)
                assertFalse(plan.requiresExplicitAction)
                assertEquals(
                    expected.targetPhase,
                    coordinator.projections.first { it.route == target }.phase,
                )

                val blocked = AndroidControllerRouteCoordinator(availabilityExcluding(target))
                connectOnline(blocked, source)
                val unavailable = assertThrows(AndroidControllerRouteCoordinatorException::class.java) {
                    blocked.select(target, explicitlyConfirmed = true)
                }
                assertEquals(fixture.switchMatrix.unavailableError, errorCode(unavailable))
                assertEquals(source, blocked.selected)
            }
        }
    }

    private fun connectOnline(
        coordinator: AndroidControllerRouteCoordinator,
        route: ControllerRemoteRouteKind,
    ) {
        coordinator.select(route, explicitlyConfirmed = true)
        coordinator.connectSelected()
        coordinator.transportReady(route)
        coordinator.authenticated(route)
    }

    private fun errorCode(error: AndroidControllerRouteCoordinatorException) = when (error.transition) {
        ControllerRemoteRouteError.EXPLICIT_CONFIRMATION_REQUIRED -> "explicit_confirmation_required"
        ControllerRemoteRouteError.TARGET_UNAVAILABLE -> "target_unavailable"
        else -> "unexpected"
    }

    private fun allAvailable() = AndroidControllerRouteAvailability(true, true, true)

    private fun availabilityExcluding(route: ControllerRemoteRouteKind) =
        AndroidControllerRouteAvailability(
            privateNetwork = route != ControllerRemoteRouteKind.PRIVATE_NETWORK,
            ssh = route != ControllerRemoteRouteKind.SSH,
            selfHostedRelay = route != ControllerRemoteRouteKind.SELF_HOSTED_RELAY,
        )

    private fun fixture(): Fixture {
        val stream = checkNotNull(javaClass.classLoader?.getResourceAsStream("remote-route-acceptance-v1.json"))
        return stream.use { JSON.decodeFromString<Fixture>(it.reader().readText()) }
    }

    private companion object {
        val JSON = Json { ignoreUnknownKeys = false }
    }

    @Serializable
    private data class Fixture(
        @SerialName("schema_version") val schemaVersion: Int,
        val routes: List<ControllerRemoteRouteKind>,
        @SerialName("lifecycle_cases") val lifecycleCases: List<LifecycleCase>,
        @SerialName("switch_matrix") val switchMatrix: SwitchMatrix,
    )

    @Serializable
    private data class LifecycleCase(
        val name: String,
        val steps: List<Step>,
        val expected: Metrics,
    )

    @Serializable
    private data class Step(
        val kind: String,
        val held: Boolean? = null,
        val retryable: Boolean? = null,
        @SerialName("mutation_in_flight") val mutationInFlight: Boolean? = null,
        val available: Boolean? = null,
    )

    @Serializable
    private data class Metrics(
        var phase: ControllerRemoteRoutePhase? = null,
        @SerialName("transport_starts") var transportStarts: Int = 0,
        @SerialName("transport_disconnects") var transportDisconnects: Int = 0,
        @SerialName("input_clears") var inputClears: Int = 0,
        @SerialName("writer_releases") var writerReleases: Int = 0,
        @SerialName("idempotent_read_retries") var idempotentReadRetries: Int = 0,
        @SerialName("mutation_queries") var mutationQueries: Int = 0,
        @SerialName("mutation_replays") var mutationReplays: Int = 0,
        @SerialName("automatic_switches") var automaticSwitches: Int = 0,
        @SerialName("explicit_actions") var explicitActions: Int = 0,
        @SerialName("terminal_allowed") var terminalAllowed: Boolean? = null,
    ) {
        fun observe(plan: AndroidControllerRoutePlan) {
            transportStarts += if (plan.startTransport != null) 1 else 0
            transportDisconnects += if (plan.disconnectTransport != null) 1 else 0
            inputClears += if (plan.clearPendingInput) 1 else 0
            writerReleases += if (plan.releaseWriter) 1 else 0
            idempotentReadRetries += if (plan.retryIdempotentReads) 1 else 0
            mutationQueries += if (plan.mutationDisposition == ControllerRemoteMutationDisposition.QUERY_BY_COMMAND_ID) 1 else 0
            explicitActions += if (plan.requiresExplicitAction) 1 else 0
        }
    }

    @Serializable
    private data class SwitchMatrix(
        val confirmed: ConfirmedSwitch,
        @SerialName("unconfirmed_error") val unconfirmedError: String,
        @SerialName("unavailable_error") val unavailableError: String,
    )

    @Serializable
    private data class ConfirmedSwitch(
        @SerialName("source_phase") val sourcePhase: ControllerRemoteRoutePhase,
        @SerialName("writer_held") val writerHeld: Boolean,
        @SerialName("source_disconnects") val sourceDisconnects: Int,
        @SerialName("target_starts") val targetStarts: Int,
        @SerialName("input_clears") val inputClears: Int,
        @SerialName("writer_releases") val writerReleases: Int,
        @SerialName("automatic_switches") val automaticSwitches: Int,
        @SerialName("target_phase") val targetPhase: ControllerRemoteRoutePhase,
    )
}
