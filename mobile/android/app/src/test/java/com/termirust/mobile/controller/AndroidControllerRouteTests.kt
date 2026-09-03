package com.termirust.mobile.controller

import com.termirust.mobile.security.MobileSecretStore
import kotlinx.serialization.encodeToString
import kotlinx.serialization.json.Json
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test
import java.util.UUID

class AndroidControllerRouteTests {
    @Test
    fun canonicalAndroidPoliciesRequireInnerAuthenticationAndNeverFallback() {
        ControllerRemoteRouteKind.androidRoutes.forEach { route ->
            val policy = ControllerRemoteRoutePolicy.canonical(route)
            assertTrue(policy.supports(ControllerRemotePlatform.ANDROID))
            assertTrue(ControllerRemoteTrustLayer.CONTROLLER_AUTHENTICATION in policy.trustLayers)
            assertEquals(ControllerRemoteCapability.entries, policy.capabilities)
            assertFalse(policy.allowsAutomaticSwitch)
            assertFalse(policy.allowsOfflineMutations)
        }
        assertFalse(
            ControllerRemoteRoutePolicy.canonical(ControllerRemoteRouteKind.LOCAL_IPC)
                .supports(ControllerRemotePlatform.ANDROID),
        )
    }

    @Test
    fun everyAndroidRoutePassesNormalDegradedReconnectCancelAndRevoke() {
        ControllerRemoteRouteKind.androidRoutes.forEach { route ->
            val coordinator = AndroidControllerRouteCoordinator(allAvailable())
            connectOnline(coordinator, route)
            coordinator.setWriterHeld(true)

            val reconnect = coordinator.failed(route, retryable = true, mutationInFlight = true)
            assertEquals(route, coordinator.selected)
            assertEquals(route, reconnect.disconnectTransport)
            assertTrue(reconnect.releaseWriter)
            assertTrue(reconnect.retryIdempotentReads)
            assertEquals(ControllerRemoteMutationDisposition.QUERY_BY_COMMAND_ID, reconnect.mutationDisposition)
            coordinator.transportReady(route)
            coordinator.authenticated(route)

            val degraded = coordinator.failed(route, retryable = false, mutationInFlight = false)
            assertEquals(route, degraded.disconnectTransport)
            assertFalse(degraded.retryIdempotentReads)
            assertEquals(AndroidControllerRouteRecovery.RETRY, projection(coordinator, route).recovery)
            assertEquals(route, coordinator.retrySelected().startTransport)

            val cancel = coordinator.cancelSelected()
            assertEquals(route, cancel.disconnectTransport)
            assertTrue(cancel.clearPendingInput)
            coordinator.connectSelected()
            coordinator.transportReady(route)
            coordinator.authenticated(route)
            coordinator.revokeSelected()
            assertEquals(ControllerRemoteRoutePhase.REVOKED, projection(coordinator, route).phase)
            assertEquals(AndroidControllerRouteRecovery.REAUTHORIZE, projection(coordinator, route).recovery)
        }
    }

    @Test
    fun everyDirectionalSwitchRequiresConfirmationAndDoesNotStartTarget() {
        ControllerRemoteRouteKind.androidRoutes.forEach { source ->
            ControllerRemoteRouteKind.androidRoutes.filter { it != source }.forEach { target ->
                val coordinator = AndroidControllerRouteCoordinator(allAvailable())
                connectOnline(coordinator, source)
                val error = assertThrows(AndroidControllerRouteCoordinatorException::class.java) {
                    coordinator.select(target, explicitlyConfirmed = false)
                }
                assertEquals(ControllerRemoteRouteError.EXPLICIT_CONFIRMATION_REQUIRED, error.transition)
                assertEquals(source, coordinator.selected)

                val plan = coordinator.select(target, explicitlyConfirmed = true)
                assertEquals(source, plan.disconnectTransport)
                assertNull(plan.startTransport)
                assertTrue(plan.clearPendingInput)
                assertEquals(target, coordinator.selected)
            }
        }
    }

    @Test
    fun unavailableTargetNeverFallsBackAndRepeatedLossIsIdempotent() {
        val coordinator = AndroidControllerRouteCoordinator(
            AndroidControllerRouteAvailability(privateNetwork = true, ssh = false, selfHostedRelay = false),
        )
        coordinator.select(ControllerRemoteRouteKind.PRIVATE_NETWORK, explicitlyConfirmed = true)
        val unavailable = assertThrows(AndroidControllerRouteCoordinatorException::class.java) {
            coordinator.select(ControllerRemoteRouteKind.SSH, explicitlyConfirmed = true)
        }
        assertEquals(ControllerRemoteRouteError.TARGET_UNAVAILABLE, unavailable.transition)
        assertEquals(ControllerRemoteRouteKind.PRIVATE_NETWORK, coordinator.selected)

        coordinator.connectSelected()
        assertEquals(
            ControllerRemoteRouteKind.PRIVATE_NETWORK,
            coordinator.setAvailable(ControllerRemoteRouteKind.PRIVATE_NETWORK, false)?.disconnectTransport,
        )
        assertNull(coordinator.setAvailable(ControllerRemoteRouteKind.PRIVATE_NETWORK, false))
        assertEquals(ControllerRemoteRouteKind.PRIVATE_NETWORK, coordinator.selected)
        assertEquals(
            ControllerRemoteRoutePhase.UNAVAILABLE,
            projection(coordinator, ControllerRemoteRouteKind.PRIVATE_NETWORK).phase,
        )
    }

    @Test
    fun persistedUnavailableSelectionStaysSelectedWithoutFallback() {
        val coordinator = AndroidControllerRouteCoordinator(
            AndroidControllerRouteAvailability(privateNetwork = true, ssh = false, selfHostedRelay = false),
        )
        coordinator.restorePersistedSelection(ControllerRemoteRouteKind.SSH)
        assertEquals(ControllerRemoteRouteKind.SSH, coordinator.selected)
        assertEquals(ControllerRemoteRoutePhase.UNAVAILABLE, projection(coordinator, ControllerRemoteRouteKind.SSH).phase)
        assertThrows(AndroidControllerRouteCoordinatorException::class.java) { coordinator.connectSelected() }
        assertEquals(
            ControllerRemoteRoutePhase.IDLE,
            projection(coordinator, ControllerRemoteRouteKind.PRIVATE_NETWORK).phase,
        )
    }

    @Test
    fun configurationChangesCannotEraseRevocationOrDisable() {
        val coordinator = AndroidControllerRouteCoordinator(allAvailable())
        connectOnline(coordinator, ControllerRemoteRouteKind.SSH)
        coordinator.revokeSelected()
        assertNull(coordinator.setAvailable(ControllerRemoteRouteKind.SSH, false))
        assertNull(coordinator.setAvailable(ControllerRemoteRouteKind.SSH, true))
        assertEquals(ControllerRemoteRoutePhase.REVOKED, projection(coordinator, ControllerRemoteRouteKind.SSH).phase)
        assertThrows(AndroidControllerRouteCoordinatorException::class.java) { coordinator.connectSelected() }

        coordinator.authorizationRestored(ControllerRemoteRouteKind.SSH)
        coordinator.disableSelected()
        coordinator.setAvailable(ControllerRemoteRouteKind.SSH, false)
        coordinator.setAvailable(ControllerRemoteRouteKind.SSH, true)
        assertEquals(ControllerRemoteRoutePhase.DISABLED, projection(coordinator, ControllerRemoteRouteKind.SSH).phase)
        coordinator.enableSelected()
        assertEquals(ControllerRemoteRouteKind.SSH, coordinator.connectSelected().startTransport)
    }

    @Test
    fun mutationCompletionNeverReplaysUnknownOrCompletedCommands() {
        val online = ControllerRemoteRouteState(
            ControllerRemoteRouteKind.PRIVATE_NETWORK,
            ControllerRemoteRoutePhase.ONLINE,
        )
        assertEquals(
            ControllerRemoteMutationDisposition.MAY_SEND,
            online.mutationDisposition(ControllerRemoteMutationCompletion.NOT_SENT),
        )
        assertEquals(
            ControllerRemoteMutationDisposition.QUERY_BY_COMMAND_ID,
            online.mutationDisposition(ControllerRemoteMutationCompletion.UNKNOWN),
        )
        listOf(
            ControllerRemoteMutationCompletion.ACKNOWLEDGED,
            ControllerRemoteMutationCompletion.REJECTED,
        ).forEach {
            assertEquals(ControllerRemoteMutationDisposition.DO_NOT_REPLAY, online.mutationDisposition(it))
        }
    }

    @Test
    fun configurationsContainReferencesButNeverCredentialMaterial() {
        val sshRef = ControllerRouteCredentialReference(
            "ssh-key-1",
            ControllerRemoteRouteKind.SSH,
            ControllerRouteCredentialPurpose.SSH_AUTHENTICATION,
        )
        val ssh = ControllerRemoteRouteConfiguration(
            ControllerRemoteRouteKind.SSH,
            "host.example",
            22,
            "deploy",
            "SHA256:host-key",
            sshRef,
        ).also(ControllerRemoteRouteConfiguration::validate)
        assertFalse(Json.encodeToString(ssh).contains("private-key-material"))

        val relayRef = ControllerRouteCredentialReference(
            "relay-token-1",
            ControllerRemoteRouteKind.SELF_HOSTED_RELAY,
            ControllerRouteCredentialPurpose.RELAY_ADMISSION,
        )
        ControllerRemoteRouteConfiguration(
            ControllerRemoteRouteKind.SELF_HOSTED_RELAY,
            "wss://relay.example/termirust",
            trustPin = "sha256/relay-spki",
            credential = relayRef,
        ).validate()

        assertThrows(IllegalArgumentException::class.java) {
            ControllerRemoteRouteConfiguration(
                ControllerRemoteRouteKind.SSH,
                "host.example",
                22,
                "deploy",
                "SHA256:host-key",
                relayRef,
            ).validate()
        }
        assertThrows(IllegalArgumentException::class.java) {
            ControllerRemoteRouteConfiguration(
                ControllerRemoteRouteKind.SELF_HOSTED_RELAY,
                "ws://relay.example/termirust",
                trustPin = "sha256/relay-spki",
                credential = relayRef,
            ).validate()
        }
    }

    @Test
    fun credentialStoreScopesSecretsByHostRoutePurposeAndReference() {
        val backing = MemorySecretStore()
        val store = ControllerRouteCredentialStore(backing)
        val ssh = ControllerRouteCredentialReference(
            "shared-id",
            ControllerRemoteRouteKind.SSH,
            ControllerRouteCredentialPurpose.SSH_AUTHENTICATION,
        )
        val relay = ControllerRouteCredentialReference(
            "shared-id",
            ControllerRemoteRouteKind.SELF_HOSTED_RELAY,
            ControllerRouteCredentialPurpose.RELAY_ADMISSION,
        )

        store.save("host-a", ssh, "ssh-secret")
        store.save("host-a", relay, "relay-secret")
        store.save("host-b", ssh, "other-host-secret")

        assertEquals("ssh-secret", store.read("host-a", ssh))
        assertEquals("relay-secret", store.read("host-a", relay))
        assertEquals("other-host-secret", store.read("host-b", ssh))
        assertEquals(3, backing.values.size)
        assertTrue(backing.values.keys.all { it.startsWith("controller-route-v1:") })
    }

    @Test
    fun switchAndLifecycleCancellationTouchOnlyTheOwnedTransport() = runTest {
        val lan = CountingControllerConnection()
        val ssh = CountingControllerConnection()
        val relay = CountingControllerConnection()
        val connections = AndroidControllerRouteConnections(lan, ssh, relay)
        val coordinator = AndroidControllerRouteCoordinator(connections.availability())

        connectOnline(coordinator, ControllerRemoteRouteKind.PRIVATE_NETWORK)
        val toSsh = coordinator.select(ControllerRemoteRouteKind.SSH, explicitlyConfirmed = true)
        toSsh.disconnectTransport?.let { connections.disconnect(it) }
        assertEquals(1, lan.cancellations)
        assertEquals(0, ssh.cancellations)
        assertEquals(0, relay.cancellations)

        coordinator.connectSelected()
        coordinator.transportReady(ControllerRemoteRouteKind.SSH)
        coordinator.authenticated(ControllerRemoteRouteKind.SSH)
        val toRelay = coordinator.select(ControllerRemoteRouteKind.SELF_HOSTED_RELAY, explicitlyConfirmed = true)
        toRelay.disconnectTransport?.let { connections.disconnect(it) }
        assertEquals(1, lan.cancellations)
        assertEquals(1, ssh.cancellations)
        assertEquals(0, relay.cancellations)

        coordinator.connectSelected()
        connections.disconnect(coordinator.selected!!)
        coordinator.cancelSelected()
        assertEquals(1, relay.cancellations)
    }

    private fun connectOnline(coordinator: AndroidControllerRouteCoordinator, route: ControllerRemoteRouteKind) {
        coordinator.select(route, explicitlyConfirmed = true)
        assertEquals(route, coordinator.connectSelected().startTransport)
        coordinator.transportReady(route)
        coordinator.authenticated(route)
        assertTrue(projection(coordinator, route).terminalAllowed)
    }

    private fun projection(coordinator: AndroidControllerRouteCoordinator, route: ControllerRemoteRouteKind) =
        coordinator.projections.first { it.route == route }

    private fun allAvailable() = AndroidControllerRouteAvailability(true, true, true)
}

private class MemorySecretStore : MobileSecretStore {
    val values = mutableMapOf<String, String>()
    override fun saveSecret(account: String, secret: String) { values[account] = secret }
    override fun readSecret(account: String) = values[account]
    override fun deleteSecret(account: String) { values.remove(account) }
}

private class CountingControllerConnection : ControllerConnecting {
    var cancellations = 0
    override suspend fun beginPairing(
        offerText: String,
        hostName: String,
        deviceName: String,
        deviceId: UUID,
    ): ControllerPairingChallenge = unsupported()

    override suspend fun finishPairing(matches: Boolean): PairedHostRecord = unsupported()
    override suspend fun fetchSessions(
        host: PairedHostRecord,
        progress: suspend (ControllerConnectionState) -> Unit,
    ): ControllerFleetSnapshot = unsupported()

    override suspend fun attachReadOnly(
        host: PairedHostRecord,
        cursor: TerminalStreamCursor,
        viewport: TerminalViewport,
        onEvent: suspend (ReadOnlyWireEvent) -> Unit,
    ) = unsupported<Unit>()

    override suspend fun attachInteractive(
        host: PairedHostRecord,
        cursor: TerminalStreamCursor,
        viewport: TerminalViewport,
        onEvent: suspend (ReadOnlyWireEvent) -> Unit,
    ) = unsupported<Unit>()

    override suspend fun requestWriter(host: PairedHostRecord, identity: ReadOnlyAttachIdentity, commandId: UUID) =
        unsupported<Unit>()

    override suspend fun releaseWriter(host: PairedHostRecord, identity: ReadOnlyAttachIdentity, commandId: UUID) =
        unsupported<Unit>()

    override suspend fun sendInput(
        host: PairedHostRecord,
        identity: ReadOnlyAttachIdentity,
        commandId: UUID,
        bytes: ByteArray,
    ) = unsupported<Unit>()

    override suspend fun sendResize(
        host: PairedHostRecord,
        identity: ReadOnlyAttachIdentity,
        commandId: UUID,
        viewport: TerminalViewport,
    ) = unsupported<Unit>()

    override suspend fun cancel() { cancellations += 1 }
    override fun close() = Unit

    private fun <T> unsupported(): T = throw UnsupportedOperationException()
}
