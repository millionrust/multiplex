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

    suspend fun cancel()
}

class AndroidControllerRouteConnections(
    val privateNetwork: ControllerConnecting?,
    val ssh: ControllerConnecting?,
    val selfHostedRelay: ControllerConnecting?,
) : AutoCloseable {
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

    override fun close() {
        listOfNotNull(privateNetwork, ssh, selfHostedRelay).distinctBy(System::identityHashCode).forEach {
            it.close()
        }
    }
}
