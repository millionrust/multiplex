package com.termirust.mobile.controller

import java.util.UUID

interface ControllerConnecting : AutoCloseable {
    suspend fun beginPairing(
        offerText: String,
        hostName: String,
        deviceName: String,
        deviceId: UUID,
    ): ControllerPairingChallenge

    suspend fun finishPairing(matches: Boolean): PairedHostRecord
    suspend fun pairWithCode(
        routes: List<HostRoute>,
        code: String,
        hostName: String?,
        expectedDiscoveryId: String?,
        deviceName: String,
        deviceId: UUID,
    ): PairedHostRecord

    /** The route the last connection to [hostId] used, when this connection picks routes. */
    fun connectedRoute(hostId: String): HostRoute?

    suspend fun fetchSessions(
        host: PairedHostRecord,
        progress: suspend (ControllerConnectionState) -> Unit = {},
    ): ControllerFleetSnapshot

    suspend fun attachReadOnly(
        host: PairedHostRecord,
        cursor: TerminalStreamCursor,
        viewport: TerminalViewport,
        onEvent: suspend (ReadOnlyWireEvent) -> Unit,
    )

    suspend fun attachInteractive(
        host: PairedHostRecord,
        cursor: TerminalStreamCursor,
        viewport: TerminalViewport,
        onEvent: suspend (ReadOnlyWireEvent) -> Unit,
    )

    suspend fun requestWriter(host: PairedHostRecord, identity: ReadOnlyAttachIdentity, commandId: UUID)
    suspend fun releaseWriter(host: PairedHostRecord, identity: ReadOnlyAttachIdentity, commandId: UUID)
    suspend fun sendInput(
        host: PairedHostRecord,
        identity: ReadOnlyAttachIdentity,
        commandId: UUID,
        bytes: ByteArray,
    )

    suspend fun sendResize(
        host: PairedHostRecord,
        identity: ReadOnlyAttachIdentity,
        commandId: UUID,
        viewport: TerminalViewport,
    )

    /**
     * Watches a computer's screen until [onEvent] throws or the coroutine is cancelled. A null
     * [surface] means the first display the computer offers, which is all a phone can ask for
     * before the welcome: a computer names its displays by their own ids.
     *
     * A transport that cannot carry screens says so, rather than every one of them having to.
     */
    suspend fun watchScreen(
        host: PairedHostRecord,
        surface: UInt?,
        preview: Boolean,
        onOpened: suspend (ControllerScreenTicket, com.termirust.screens.ScreenViewer) -> Unit,
        onEvent: suspend (List<com.termirust.screens.ScreenEvent>) -> Unit,
    ): Unit = throw ControllerConnectionException.CapabilityDenied

    suspend fun cancel()
}

class AndroidControllerRouteConnections(
    val privateNetwork: ControllerConnecting?,
    ssh: ControllerConnecting?,
    selfHostedRelay: ControllerConnecting?,
) : AutoCloseable {
    var ssh: ControllerConnecting? = ssh
        private set
    var selfHostedRelay: ControllerConnecting? = selfHostedRelay
        private set

    fun connection(route: ControllerRemoteRouteKind): ControllerConnecting? = when (route) {
        ControllerRemoteRouteKind.LOCAL_IPC -> null
        ControllerRemoteRouteKind.PRIVATE_NETWORK -> privateNetwork
        ControllerRemoteRouteKind.SSH -> ssh
        ControllerRemoteRouteKind.SELF_HOSTED_RELAY -> selfHostedRelay
    }

    fun availability() = AndroidControllerRouteAvailability(
        privateNetwork = privateNetwork != null,
        ssh = ssh != null,
        selfHostedRelay = selfHostedRelay != null,
    )

    suspend fun disconnect(route: ControllerRemoteRouteKind) {
        connection(route)?.cancel()
    }

    fun replaceSsh(connection: ControllerConnecting?) {
        if (ssh === connection) return
        ssh?.close()
        ssh = connection
    }

    fun replaceRelay(connection: ControllerConnecting?) {
        if (selfHostedRelay === connection) return
        selfHostedRelay?.close()
        selfHostedRelay = connection
    }

    override fun close() {
        listOfNotNull(privateNetwork, ssh, selfHostedRelay).distinctBy(System::identityHashCode).forEach {
            it.close()
        }
    }
}
