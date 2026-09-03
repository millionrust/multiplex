package com.termirust.mobile.controller

import com.termirust.controller.security.ConnectionStartRequest
import com.termirust.controller.security.ControllerCapability
import com.termirust.controller.security.ControllerConnectionSession
import com.termirust.controller.security.ControllerFrameKind
import com.termirust.controller.security.ControllerPairingSession
import com.termirust.controller.security.ControllerSecurityEngine
import com.termirust.controller.security.PairingConfirmation
import com.termirust.controller.security.PairingRole
import com.termirust.controller.security.PairingStartRequest
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.encodeToString
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import java.io.DataInputStream
import java.io.DataOutputStream
import java.net.Inet4Address
import java.net.Inet6Address
import java.net.InetAddress
import java.net.InetSocketAddress
import java.net.Socket
import java.security.MessageDigest
import java.security.SecureRandom
import java.util.Base64
import java.util.UUID

class ControllerConnection(
    blobStore: ControllerSecureBlobStore,
    private val clockMillis: () -> Long = System::currentTimeMillis,
) : ControllerConnecting {
    private val engine = ControllerSecurityEngine(blobStore)
    private val mutex = Mutex()
    private val json = Json { ignoreUnknownKeys = false; encodeDefaults = true; explicitNulls = true }
    private val random = SecureRandom()
    @Volatile private var activeSocket: Socket? = null
    @Volatile private var activeTerminal: ActiveTerminalConnection? = null
    private var pendingPairing: PendingPairing? = null

    override suspend fun beginPairing(
        offerText: String,
        hostName: String,
        deviceName: String,
        deviceId: UUID,
    ): ControllerPairingChallenge = mutex.withLock {
        withContext(Dispatchers.IO) {
            cancelUnlocked(deleteCreatedKey = true)
            require(offerText.toByteArray().size in 1..MAX_OFFER_BYTES)
            require(hostName.codePointCount(0, hostName.length) in 1..256)
            require(deviceName.codePointCount(0, deviceName.length) in 1..64)
            require(deviceName.none(Char::isISOControl))
            val envelope = json.decodeFromString<PairingOfferEnvelope>(offerText)
            require(envelope.schemaVersion == 1 && envelope.offerBytes.size <= MAX_OFFER_BYTES)
            require(runCatching { UUID.fromString(envelope.offerId) }.isSuccess)
            require(envelope.offerBytes.all { it in 0..255 })
            val offer = envelope.offerBytes.map(Int::toByte).toByteArray()
            val summary = engine.decodeOfferSummary(offer)
            val nowSeconds = clockMillis() / 1_000
            require(summary.version.major.toInt() == 1 && summary.version.minor.toInt() == 0)
            require(summary.expiresAtUnixSeconds.toLong() > nowSeconds)
            require(summary.hostStaticPublicKey.size == 32 && envelope.identityGeneration > 0)
            val route = HostRoute(envelope.address, envelope.port)
            require(isPrivateRoute(route, envelope.addressFamily))
            val fingerprint = summary.hostStaticPublicKey.hex()
            val keyId = "controller.device.${deviceId.toString().lowercase()}.${fingerprint.take(16)}"
            val created = engine.secureBlobStatus(keyId).name == "MISSING"
            if (created) engine.storeSecureBlob(keyId, randomBytes(32))
            try {
                val socket = open(route)
                activeSocket = socket
                val input = DataInputStream(socket.getInputStream())
                val output = DataOutputStream(socket.getOutputStream())
                output.write(PAIRING_PREFACE)
                writeFrame(
                    output,
                    json.encodeToString(PairingConnectPayload(offerId = envelope.offerId)).encodeToByteArray(),
                    MAX_OFFER_BYTES,
                )
                val session = engine.pairingStart(
                    PairingStartRequest(
                        role = PairingRole.DEVICE_INITIATOR,
                        offerBytes = offer,
                        staticKeyId = keyId,
                        ephemeralPrivateKey = randomBytes(32),
                        nowMillis = uptimeMillis().toULong(),
                        nowUnixSeconds = nowSeconds.toULong(),
                    ),
                )
                val hello = session.pairingOutbound(uptimeMillis().toULong())
                writeFrame(output, hello, MAX_HANDSHAKE_BYTES)
                session.pairingReceive(readFrame(input, MAX_HANDSHAKE_BYTES), uptimeMillis().toULong())
                writeFrame(output, session.pairingOutbound(uptimeMillis().toULong()), MAX_HANDSHAKE_BYTES)
                val sas = session.sas().value
                pendingPairing = PendingPairing(
                    envelope = envelope,
                    route = route,
                    hostName = hostName,
                    deviceName = deviceName,
                    deviceId = deviceId,
                    keyId = keyId,
                    createdKey = created,
                    session = session,
                    socket = socket,
                    input = input,
                    output = output,
                    sas = sas,
                    hostKey = summary.hostStaticPublicKey,
                    capabilityBits = summary.capabilityBits.toInt(),
                )
                ControllerPairingChallenge(
                    sas = sas,
                    fingerprintSuffix = fingerprint.takeLast(12),
                    route = route,
                    expiresAtMillis = summary.expiresAtUnixSeconds.toLong() * 1_000,
                )
            } catch (error: Throwable) {
                if (created) runCatching { engine.deleteSecureBlob(keyId) }
                cancelUnlocked(deleteCreatedKey = false)
                throw error
            }
        }
    }

    override suspend fun finishPairing(matches: Boolean): PairedHostRecord = mutex.withLock {
        withContext(Dispatchers.IO) {
            val pending = checkNotNull(pendingPairing) { "No pairing is in progress." }
            if (!matches) {
                runCatching {
                    pending.session.confirmOrReject(
                        PairingConfirmation.REJECT,
                        pending.sas,
                        pending.envelope.revocationEpoch.toULong(),
                    )
                }
                cancelUnlocked(deleteCreatedKey = true)
                throw ControllerConnectionException.PairingRejected
            }
            var registrationSent = false
            try {
                val result = pending.session.confirmOrReject(
                    PairingConfirmation.CONFIRM,
                    pending.sas,
                    pending.envelope.revocationEpoch.toULong(),
                )
                require(result.hostStaticPublicKey.contentEquals(pending.hostKey))
                val registration = json.encodeToString(
                    PairingRegistrationPayload(
                        deviceId = pending.deviceId.toString(),
                        displayName = pending.deviceName,
                    ),
                ).encodeToByteArray()
                val sealed = pending.session.sealFrame(
                    ControllerFrameKind.CONTROL,
                    ControllerCapability.OBSERVE_SESSIONS,
                    pending.envelope.revocationEpoch.toULong(),
                    registration,
                )
                writeFrame(pending.output, sealed, MAX_SECURE_FRAME_BYTES)
                registrationSent = true
                val opened = pending.session.openFrame(readFrame(pending.input, MAX_SECURE_FRAME_BYTES))
                require(opened.kind == ControllerFrameKind.CONTROL)
                require(opened.capability == ControllerCapability.OBSERVE_SESSIONS)
                require(opened.revocationEpoch.toLong() == pending.envelope.revocationEpoch)
                val ack = json.decodeFromString<PairingHostAckPayload>(opened.payload.decodeToString())
                require(ack.schemaVersion == 1 && ack.deviceId == pending.deviceId.toString())
                require(ack.identityGeneration == pending.envelope.identityGeneration)
                require(ack.revocationEpoch == pending.envelope.revocationEpoch)
                require(ack.sessionGeneration == pending.envelope.sessionGeneration)
                require(ack.capabilityBits and pending.capabilityBits.inv() == 0)
                val record = PairedHostRecord(
                    id = pending.hostKey.hex(),
                    displayName = pending.hostName,
                    route = pending.route,
                    hostStaticPublicKey = Base64.getEncoder().encodeToString(pending.hostKey),
                    deviceStaticKeyId = pending.keyId,
                    deviceId = pending.deviceId.toString(),
                    identityGeneration = ack.identityGeneration,
                    revocationEpoch = ack.revocationEpoch,
                    sessionGeneration = ack.sessionGeneration,
                    capabilityBits = ack.capabilityBits,
                    pairedAtMillis = clockMillis(),
                )
                record.validate()
                cancelUnlocked(deleteCreatedKey = false)
                record
            } catch (error: Throwable) {
                cancelUnlocked(deleteCreatedKey = false)
                if (registrationSent) {
                    val provisional = PairedHostRecord(
                        id = pending.hostKey.hex(),
                        displayName = pending.hostName,
                        route = pending.route,
                        hostStaticPublicKey = Base64.getEncoder().encodeToString(pending.hostKey),
                        deviceStaticKeyId = pending.keyId,
                        deviceId = pending.deviceId.toString(),
                        identityGeneration = pending.envelope.identityGeneration,
                        revocationEpoch = pending.envelope.revocationEpoch,
                        sessionGeneration = pending.envelope.sessionGeneration,
                        capabilityBits = pending.capabilityBits,
                        pairedAtMillis = clockMillis(),
                    )
                    provisional.validate()
                    try {
                        fetchSessionsUnlocked(provisional)
                        return@withContext provisional
                    } catch (_: Throwable) {
                        cancelUnlocked(deleteCreatedKey = false)
                        throw ControllerConnectionException.AcknowledgementUncertain
                    }
                }
                throw error
            }
        }
    }

    override suspend fun fetchSessions(
        host: PairedHostRecord,
        progress: suspend (ControllerConnectionState) -> Unit,
    ): ControllerFleetSnapshot = mutex.withLock {
        withContext(Dispatchers.IO) { fetchSessionsUnlocked(host, progress) }
    }

    override suspend fun attachReadOnly(
        host: PairedHostRecord,
        cursor: TerminalStreamCursor,
        viewport: TerminalViewport,
        onEvent: suspend (ReadOnlyWireEvent) -> Unit,
    ) = mutex.withLock {
        withContext(Dispatchers.IO) {
            cancelUnlocked(deleteCreatedKey = true)
            host.validate()
            cursor.identity.validate()
            TerminalLimits().validate(viewport)
            require(host.capabilityBits and ATTACH_CAPABILITY == ATTACH_CAPABILITY)
            require(cursor.identity.hostId == host.id)
            val socket = open(host.route)
            activeSocket = socket
            try {
                val input = DataInputStream(socket.getInputStream())
                val output = DataOutputStream(socket.getOutputStream())
                val hostKey = Base64.getDecoder().decode(host.hostStaticPublicKey)
                val request = ConnectionStartRequest(
                    staticKeyId = host.deviceStaticKeyId,
                    ephemeralPrivateKey = randomBytes(32),
                    hostStaticPublicKey = hostKey,
                    identityGeneration = host.identityGeneration.toULong(),
                    revocationEpoch = host.revocationEpoch.toULong(),
                    requestedCapabilityBits = ATTACH_CAPABILITY.toUShort(),
                    clientNonce = randomBytes(32),
                    nowMillis = uptimeMillis().toULong(),
                )
                output.write(AUTH_PREFACE)
                output.write(engine.connectionPrelude(request))
                output.flush()
                val challenge = ByteArray(36).also(input::readFully)
                val session = engine.connectionStart(request, challenge)
                try {
                    writeFrame(output, session.handshakeOutbound(uptimeMillis().toULong()), MAX_HANDSHAKE_BYTES)
                    val publicResult = session.handshakeReceiveAccept(
                        readFrame(input, MAX_HANDSHAKE_BYTES),
                        uptimeMillis().toULong(),
                    )
                    require(publicResult.hostStaticPublicKey.contentEquals(hostKey))
                    require(publicResult.identityGeneration.toLong() == host.identityGeneration)
                    require(publicResult.revocationEpoch.toLong() == host.revocationEpoch)
                    require(publicResult.grantedCapabilityBits.toInt() == ATTACH_CAPABILITY)

                    val commandId = UUID.randomUUID()
                    val command = ControllerReadOnlyWireCodec.encodeAttach(
                        commandId = commandId,
                        sessionGeneration = host.sessionGeneration,
                        deadlineMillis = clockMillis() + READ_TIMEOUT_MILLIS,
                        cursor = cursor,
                        viewport = viewport,
                    )
                    val sealed = session.sealFrame(
                        ControllerFrameKind.CONTROL,
                        ControllerCapability.ATTACH_OUTPUT,
                        host.revocationEpoch.toULong(),
                        command,
                    )
                    writeFrame(output, sealed, MAX_SECURE_FRAME_BYTES)

                    var attached = false
                    while (true) {
                        val opened = session.openFrame(readFrame(input, MAX_TERMINAL_FRAME_BYTES))
                        require(opened.capability == ControllerCapability.ATTACH_OUTPUT)
                        require(opened.revocationEpoch.toLong() == host.revocationEpoch)
                        require(opened.kind == ControllerFrameKind.CONTROL || opened.kind == ControllerFrameKind.TERMINAL)
                        val event = ControllerReadOnlyWireCodec.decode(
                            opened.payload,
                            commandId,
                            cursor.identity,
                        )
                        when (event) {
                            is ReadOnlyWireEvent.Snapshot ->
                                require(!attached && opened.kind == ControllerFrameKind.TERMINAL)
                            is ReadOnlyWireEvent.Attached -> {
                                require(!attached && opened.kind == ControllerFrameKind.CONTROL)
                                attached = true
                            }
                            is ReadOnlyWireEvent.Output ->
                                require(attached && opened.kind == ControllerFrameKind.TERMINAL)
                            is ReadOnlyWireEvent.Completed ->
                                throw IllegalArgumentException("mutation response on read-only attach")
                            is ReadOnlyWireEvent.HostError -> {
                                require(event.commandId == commandId && opened.kind == ControllerFrameKind.CONTROL)
                                throw ControllerConnectionException.HostError(event.code)
                            }
                        }
                        onEvent(event)
                    }
                } finally {
                    runCatching { session.finish() }
                    session.close()
                }
            } finally {
                socket.close()
                if (activeSocket === socket) activeSocket = null
            }
        }
    }

    override suspend fun attachInteractive(
        host: PairedHostRecord,
        cursor: TerminalStreamCursor,
        viewport: TerminalViewport,
        onEvent: suspend (ReadOnlyWireEvent) -> Unit,
    ) = mutex.withLock {
        withContext(Dispatchers.IO) {
            cancelUnlocked(deleteCreatedKey = true)
            host.validate()
            cursor.identity.validate()
            TerminalLimits().validate(viewport)
            val required = ATTACH_CAPABILITY or INPUT_CAPABILITY
            require(host.capabilityBits and required == required && cursor.identity.hostId == host.id)
            val requested = host.capabilityBits and ALL_INTERACTIVE_CAPABILITIES
            val socket = open(host.route)
            activeSocket = socket
            try {
                val input = DataInputStream(socket.getInputStream())
                val output = DataOutputStream(socket.getOutputStream())
                val hostKey = Base64.getDecoder().decode(host.hostStaticPublicKey)
                val request = ConnectionStartRequest(
                    staticKeyId = host.deviceStaticKeyId,
                    ephemeralPrivateKey = randomBytes(32),
                    hostStaticPublicKey = hostKey,
                    identityGeneration = host.identityGeneration.toULong(),
                    revocationEpoch = host.revocationEpoch.toULong(),
                    requestedCapabilityBits = requested.toUShort(),
                    clientNonce = randomBytes(32),
                    nowMillis = uptimeMillis().toULong(),
                )
                output.write(AUTH_PREFACE)
                output.write(engine.connectionPrelude(request))
                output.flush()
                val challenge = ByteArray(36).also(input::readFully)
                val session = engine.connectionStart(request, challenge)
                val terminal = ActiveTerminalConnection(
                    hostId = host.id,
                    identity = cursor.identity,
                    grantedCapabilityBits = requested,
                    output = output,
                    session = session,
                )
                try {
                    writeFrame(output, session.handshakeOutbound(uptimeMillis().toULong()), MAX_HANDSHAKE_BYTES)
                    val publicResult = session.handshakeReceiveAccept(
                        readFrame(input, MAX_HANDSHAKE_BYTES),
                        uptimeMillis().toULong(),
                    )
                    require(publicResult.hostStaticPublicKey.contentEquals(hostKey))
                    require(publicResult.identityGeneration.toLong() == host.identityGeneration)
                    require(publicResult.revocationEpoch.toLong() == host.revocationEpoch)
                    require(publicResult.grantedCapabilityBits.toInt() == requested)

                    val attachCommandId = UUID.randomUUID()
                    terminal.attachCommandId = attachCommandId
                    activeTerminal = terminal
                    val command = ControllerReadOnlyWireCodec.encodeAttach(
                        attachCommandId,
                        host.sessionGeneration,
                        clockMillis() + READ_TIMEOUT_MILLIS,
                        cursor,
                        viewport,
                    )
                    val sealed = session.sealFrame(
                        ControllerFrameKind.CONTROL,
                        ControllerCapability.ATTACH_OUTPUT,
                        host.revocationEpoch.toULong(),
                        command,
                    )
                    writeFrame(output, sealed, MAX_SECURE_FRAME_BYTES)

                    var attached = false
                    while (true) {
                        val sealedResponse = readFrame(input, MAX_TERMINAL_FRAME_BYTES)
                        val opened = terminal.cryptoMutex.withLock { session.openFrame(sealedResponse) }
                        require(opened.revocationEpoch.toLong() == host.revocationEpoch)
                        require(opened.kind == ControllerFrameKind.CONTROL || opened.kind == ControllerFrameKind.TERMINAL)
                        val event = ControllerReadOnlyWireCodec.decode(
                            opened.payload,
                            attachCommandId,
                            cursor.identity,
                        )
                        when (event) {
                            is ReadOnlyWireEvent.Snapshot ->
                                require(!attached && opened.kind == ControllerFrameKind.TERMINAL &&
                                    opened.capability == ControllerCapability.ATTACH_OUTPUT)
                            is ReadOnlyWireEvent.Attached -> {
                                require(!attached && opened.kind == ControllerFrameKind.CONTROL &&
                                    opened.capability == ControllerCapability.ATTACH_OUTPUT)
                                attached = true
                            }
                            is ReadOnlyWireEvent.Output ->
                                require(attached && opened.kind == ControllerFrameKind.TERMINAL &&
                                    opened.capability == ControllerCapability.ATTACH_OUTPUT)
                            is ReadOnlyWireEvent.Completed -> {
                                require(attached && opened.kind == ControllerFrameKind.CONTROL)
                                val expected = terminal.pendingMutex.withLock {
                                    terminal.pendingCapabilities.remove(event.commandId)
                                }
                                require(expected != null && opened.capability == expected)
                            }
                            is ReadOnlyWireEvent.HostError -> {
                                if (event.commandId == attachCommandId) {
                                    throw ControllerConnectionException.HostError(event.code)
                                }
                                require(attached && opened.kind == ControllerFrameKind.CONTROL)
                                val expected = terminal.pendingMutex.withLock {
                                    terminal.pendingCapabilities.remove(event.commandId)
                                }
                                require(expected != null && opened.capability == expected)
                            }
                        }
                        onEvent(event)
                    }
                } finally {
                    if (activeTerminal === terminal) activeTerminal = null
                    runCatching { session.finish() }
                    session.close()
                }
            } finally {
                socket.close()
                if (activeSocket === socket) activeSocket = null
            }
        }
    }

    override suspend fun requestWriter(host: PairedHostRecord, identity: ReadOnlyAttachIdentity, commandId: UUID) {
        sendTerminalMutation(
            host,
            identity,
            commandId,
            ControllerCapability.SEND_INPUT,
            INPUT_CAPABILITY,
            ControllerWriterWireCodec.encodeAcquire(
                commandId,
                host.sessionGeneration,
                clockMillis() + READ_TIMEOUT_MILLIS,
                identity,
            ),
        )
    }

    override suspend fun releaseWriter(host: PairedHostRecord, identity: ReadOnlyAttachIdentity, commandId: UUID) {
        sendTerminalMutation(
            host,
            identity,
            commandId,
            ControllerCapability.SEND_INPUT,
            INPUT_CAPABILITY,
            ControllerWriterWireCodec.encodeRelease(
                commandId,
                host.sessionGeneration,
                clockMillis() + 2_000,
                identity,
            ),
        )
    }

    override suspend fun sendInput(
        host: PairedHostRecord,
        identity: ReadOnlyAttachIdentity,
        commandId: UUID,
        bytes: ByteArray,
    ) {
        sendTerminalMutation(
            host,
            identity,
            commandId,
            ControllerCapability.SEND_INPUT,
            INPUT_CAPABILITY,
            ControllerWriterWireCodec.encodeInput(
                commandId,
                host.sessionGeneration,
                clockMillis() + 10_000,
                identity,
                bytes,
            ),
        )
    }

    override suspend fun sendResize(
        host: PairedHostRecord,
        identity: ReadOnlyAttachIdentity,
        commandId: UUID,
        viewport: TerminalViewport,
    ) {
        sendTerminalMutation(
            host,
            identity,
            commandId,
            ControllerCapability.RESIZE,
            RESIZE_CAPABILITY,
            ControllerWriterWireCodec.encodeResize(
                commandId,
                host.sessionGeneration,
                clockMillis() + 10_000,
                identity,
                viewport,
            ),
        )
    }

    override suspend fun cancel() {
        // Socket.close is thread-safe and unblocks a pending read before the operation
        // coroutine can reacquire the serialization mutex.
        activeSocket?.close()
        mutex.withLock { withContext(Dispatchers.IO) { cancelUnlocked(true) } }
    }

    override fun close() {
        activeSocket?.close()
        activeSocket = null
        runCatching { pendingPairing?.session?.finish() }
        pendingPairing?.session?.close()
        pendingPairing = null
        engine.close()
    }

    private suspend fun sendTerminalMutation(
        host: PairedHostRecord,
        identity: ReadOnlyAttachIdentity,
        commandId: UUID,
        capability: ControllerCapability,
        capabilityBit: Int,
        payload: ByteArray,
    ) {
        require(payload.size <= MAX_SECURE_FRAME_BYTES)
        val terminal = activeTerminal
        require(terminal != null && terminal.hostId == host.id && terminal.identity == identity)
        require(terminal.grantedCapabilityBits and capabilityBit == capabilityBit)
        terminal.pendingMutex.withLock {
            require(terminal.pendingCapabilities.size < WriterControlReducer.MAX_QUEUED_CHUNKS)
            require(terminal.pendingCapabilities.putIfAbsent(commandId, capability) == null)
        }
        try {
            val sealed = terminal.cryptoMutex.withLock {
                terminal.session.sealFrame(
                    ControllerFrameKind.CONTROL,
                    capability,
                    host.revocationEpoch.toULong(),
                    payload,
                )
            }
            terminal.writeMutex.withLock { writeFrame(terminal.output, sealed, MAX_SECURE_FRAME_BYTES) }
        } catch (error: Throwable) {
            terminal.pendingMutex.withLock { terminal.pendingCapabilities.remove(commandId) }
            throw error
        }
    }

    private suspend fun fetchSessionsUnlocked(
        host: PairedHostRecord,
        progress: suspend (ControllerConnectionState) -> Unit = {},
    ): ControllerFleetSnapshot {
        cancelUnlocked(deleteCreatedKey = true)
        host.validate()
        require(host.capabilityBits and OBSERVE_CAPABILITY == OBSERVE_CAPABILITY)
        progress(ControllerConnectionState.Connecting)
        val socket = open(host.route)
        activeSocket = socket
        try {
            progress(ControllerConnectionState.Authenticating)
            val input = DataInputStream(socket.getInputStream())
            val output = DataOutputStream(socket.getOutputStream())
            val hostKey = Base64.getDecoder().decode(host.hostStaticPublicKey)
            val request = ConnectionStartRequest(
                staticKeyId = host.deviceStaticKeyId,
                ephemeralPrivateKey = randomBytes(32),
                hostStaticPublicKey = hostKey,
                identityGeneration = host.identityGeneration.toULong(),
                revocationEpoch = host.revocationEpoch.toULong(),
                requestedCapabilityBits = OBSERVE_CAPABILITY.toUShort(),
                clientNonce = randomBytes(32),
                nowMillis = uptimeMillis().toULong(),
            )
            output.write(AUTH_PREFACE)
            output.write(engine.connectionPrelude(request))
            output.flush()
            val challenge = ByteArray(36).also(input::readFully)
            val session = engine.connectionStart(request, challenge)
            try {
                writeFrame(output, session.handshakeOutbound(uptimeMillis().toULong()), MAX_HANDSHAKE_BYTES)
                val publicResult = session.handshakeReceiveAccept(
                    readFrame(input, MAX_HANDSHAKE_BYTES),
                    uptimeMillis().toULong(),
                )
                require(publicResult.hostStaticPublicKey.contentEquals(hostKey))
                require(publicResult.identityGeneration.toLong() == host.identityGeneration)
                require(publicResult.revocationEpoch.toLong() == host.revocationEpoch)
                require(publicResult.grantedCapabilityBits.toInt() == OBSERVE_CAPABILITY)
                progress(ControllerConnectionState.Syncing)
                return fetchStableSnapshot(host, session, input, output)
            } finally {
                runCatching { session.finish() }
                session.close()
            }
        } finally {
            socket.close()
            if (activeSocket === socket) activeSocket = null
        }
    }

    private fun fetchStableSnapshot(
        host: PairedHostRecord,
        session: ControllerConnectionSession,
        input: DataInputStream,
        output: DataOutputStream,
    ): ControllerFleetSnapshot {
        repeat(3) {
            var offset = 0
            var revision: Long? = null
            var updateSequence: Long? = null
            val summaries = mutableListOf<ControllerSessionSummary>()
            var restart = false
            do {
                val commandId = UUID.randomUUID().toString()
                val payload = json.encodeToString(
                    ListSessionsEnvelope(
                        commandId = commandId,
                        sessionGeneration = host.sessionGeneration,
                        deadlineMillis = clockMillis() + 30_000,
                        command = ListSessionsCommand(
                            offset = offset,
                            limit = ControllerLimits.MAX_PAGE_RECORDS,
                            expectedRevision = revision,
                        ),
                    ),
                ).encodeToByteArray()
                require(payload.size <= MAX_SECURE_FRAME_BYTES)
                val sealed = session.sealFrame(
                    ControllerFrameKind.CONTROL,
                    ControllerCapability.OBSERVE_SESSIONS,
                    host.revocationEpoch.toULong(),
                    payload,
                )
                writeFrame(output, sealed, MAX_SECURE_FRAME_BYTES)
                val opened = session.openFrame(readFrame(input, MAX_SECURE_FRAME_BYTES))
                require(opened.kind == ControllerFrameKind.CONTROL)
                require(opened.capability == ControllerCapability.OBSERVE_SESSIONS)
                require(opened.revocationEpoch.toLong() == host.revocationEpoch)
                val responseText = opened.payload.decodeToString()
                val responseKind = json.parseToJsonElement(responseText)
                    .jsonObject["kind"]?.jsonPrimitive?.content
                    ?: throw IllegalArgumentException("missing response kind")
                if (responseKind == "error") {
                    val error = json.decodeFromString<ErrorResponse>(responseText)
                    require(error.commandId == commandId)
                    if (error.code == "snapshot_changed" && !error.completionUnknown) {
                        restart = true
                        break
                    }
                    throw ControllerConnectionException.HostError(error.code)
                }
                val page = json.decodeFromString<SessionsResponse>(responseText)
                require(page.kind == "sessions" && page.commandId == commandId)
                require(page.revision > 0 && page.updateSequence > 0)
                require(page.sessions.size <= ControllerLimits.MAX_PAGE_RECORDS)
                require(revision == null || revision == page.revision)
                require(updateSequence == null || updateSequence == page.updateSequence)
                revision = page.revision
                updateSequence = page.updateSequence
                summaries += page.sessions.map(SessionSummaryPayload::toModel)
                require(summaries.size <= ControllerLimits.MAX_SESSIONS_PER_HOST)
                val next = page.nextOffset ?: break
                require(next > offset && next == summaries.size)
                offset = next
            } while (true)
            if (restart) return@repeat
            val snapshot = ControllerFleetSnapshot(
                revision = requireNotNull(revision),
                updateSequence = requireNotNull(updateSequence),
                sessions = summaries,
            )
            snapshot.validate()
            return snapshot
        }
        throw ControllerConnectionException.SequenceGap
    }

    private fun cancelUnlocked(deleteCreatedKey: Boolean) {
        val pending = pendingPairing
        if (deleteCreatedKey && pending?.createdKey == true) {
            runCatching { engine.deleteSecureBlob(pending.keyId) }
        }
        runCatching { pending?.session?.finish() }
        pending?.session?.close()
        pendingPairing = null
        val terminal = activeTerminal
        activeTerminal = null
        runCatching { terminal?.session?.finish() }
        terminal?.session?.close()
        activeSocket?.close()
        activeSocket = null
    }

    private fun open(route: HostRoute): Socket = Socket().apply {
        connect(InetSocketAddress(route.address, route.port), CONNECT_TIMEOUT_MILLIS)
        soTimeout = READ_TIMEOUT_MILLIS
        tcpNoDelay = true
    }

    private fun writeFrame(output: DataOutputStream, payload: ByteArray, maximum: Int) {
        require(payload.size in 1..maximum)
        output.writeInt(payload.size)
        output.write(payload)
        output.flush()
    }

    private fun readFrame(input: DataInputStream, maximum: Int): ByteArray {
        val length = input.readInt()
        require(length in 1..maximum)
        return ByteArray(length).also(input::readFully)
    }

    private fun randomBytes(size: Int): ByteArray = ByteArray(size).also(random::nextBytes)
    private fun uptimeMillis(): Long = android.os.SystemClock.elapsedRealtime()

    private fun isPrivateRoute(route: HostRoute, family: String): Boolean {
        val address = runCatching { InetAddress.getByName(route.address) }.getOrNull() ?: return false
        if (address.isLoopbackAddress || address.isAnyLocalAddress) return false
        val bytes = address.address.map(Byte::toInt).map { it and 0xff }
        return when {
            family == "ipv4" && address is Inet4Address ->
                bytes[0] == 10 || bytes[0] == 172 && bytes[1] in 16..31 ||
                    bytes[0] == 192 && bytes[1] == 168 || bytes[0] == 100 && bytes[1] in 64..127
            family == "ipv6" && address is Inet6Address -> bytes[0] and 0xfe == 0xfc
            else -> false
        }
    }

    private companion object {
        const val MAX_OFFER_BYTES = 4 * 1_024
        const val MAX_HANDSHAKE_BYTES = 1 * 1_024
        const val MAX_SECURE_FRAME_BYTES = 64 * 1_024
        const val MAX_TERMINAL_FRAME_BYTES = 1 * 1_024 * 1_024
        const val CONNECT_TIMEOUT_MILLIS = 10_000
        const val READ_TIMEOUT_MILLIS = 30_000
        const val OBSERVE_CAPABILITY = 1
        const val ATTACH_CAPABILITY = 1 shl 1
        const val INPUT_CAPABILITY = 1 shl 2
        const val RESIZE_CAPABILITY = 1 shl 3
        const val APPROVAL_CAPABILITY = 1 shl 4
        const val ALL_INTERACTIVE_CAPABILITIES = ATTACH_CAPABILITY or INPUT_CAPABILITY or
            RESIZE_CAPABILITY or APPROVAL_CAPABILITY
        val PAIRING_PREFACE = byteArrayOf(0x54, 0x52, 0x43, 0x4e, 0, 1, 2, 0)
        val AUTH_PREFACE = byteArrayOf(0x54, 0x52, 0x43, 0x4e, 0, 1, 1, 0)
    }
}

private data class PendingPairing(
    val envelope: PairingOfferEnvelope,
    val route: HostRoute,
    val hostName: String,
    val deviceName: String,
    val deviceId: UUID,
    val keyId: String,
    val createdKey: Boolean,
    val session: ControllerPairingSession,
    val socket: Socket,
    val input: DataInputStream,
    val output: DataOutputStream,
    val sas: String,
    val hostKey: ByteArray,
    val capabilityBits: Int,
)

private data class ActiveTerminalConnection(
    val hostId: String,
    val identity: ReadOnlyAttachIdentity,
    val grantedCapabilityBits: Int,
    val output: DataOutputStream,
    val session: ControllerConnectionSession,
    val cryptoMutex: Mutex = Mutex(),
    val writeMutex: Mutex = Mutex(),
    val pendingMutex: Mutex = Mutex(),
    val pendingCapabilities: MutableMap<UUID, ControllerCapability> = mutableMapOf(),
    var attachCommandId: UUID? = null,
)

@Serializable private data class PairingOfferEnvelope(
    @SerialName("schema_version") val schemaVersion: Int,
    @SerialName("offer_id") val offerId: String,
    @SerialName("identity_generation") val identityGeneration: Long,
    @SerialName("revocation_epoch") val revocationEpoch: Long,
    @SerialName("session_generation") val sessionGeneration: Long,
    @SerialName("address_family") val addressFamily: String,
    val address: String,
    val port: Int,
    @SerialName("offer_bytes") val offerBytes: List<Int>,
)

@Serializable private data class PairingConnectPayload(
    @SerialName("schema_version") val schemaVersion: Int = 1,
    @SerialName("offer_id") val offerId: String,
)

@Serializable private data class PairingRegistrationPayload(
    @SerialName("schema_version") val schemaVersion: Int = 1,
    @SerialName("device_id") val deviceId: String,
    @SerialName("display_name") val displayName: String,
)

@Serializable private data class PairingHostAckPayload(
    @SerialName("schema_version") val schemaVersion: Int,
    @SerialName("device_id") val deviceId: String,
    @SerialName("identity_generation") val identityGeneration: Long,
    @SerialName("revocation_epoch") val revocationEpoch: Long,
    @SerialName("session_generation") val sessionGeneration: Long,
    @SerialName("capability_bits") val capabilityBits: Int,
)

@Serializable private data class ListSessionsEnvelope(
    val version: Int = 1,
    @SerialName("command_id") val commandId: String,
    @SerialName("session_generation") val sessionGeneration: Long,
    @SerialName("deadline_millis") val deadlineMillis: Long,
    val command: ListSessionsCommand,
)

@Serializable private data class ListSessionsCommand(
    val kind: String = "list_sessions",
    val offset: Int,
    val limit: Int,
    @SerialName("expected_revision") val expectedRevision: Long?,
)

@Serializable private data class ErrorResponse(
    val kind: String,
    @SerialName("command_id") val commandId: String,
    val code: String,
    @SerialName("completion_unknown") val completionUnknown: Boolean,
)

@Serializable private data class SessionsResponse(
    val kind: String,
    @SerialName("command_id") val commandId: String,
    val revision: Long,
    @SerialName("update_sequence") val updateSequence: Long,
    val sessions: List<SessionSummaryPayload>,
    @SerialName("next_offset") val nextOffset: Int?,
)

@Serializable private data class SessionSummaryPayload(
    @SerialName("session_id") val sessionId: String,
    @SerialName("host_instance_id") val hostInstanceId: String? = null,
    val origin: ControllerSessionOrigin = ControllerSessionOrigin.UNKNOWN,
    val runtime: String? = null,
    val capabilities: List<ControllerSessionCapability> = emptyList(),
    val title: String,
    val project: String? = null,
    val group: String? = null,
    val lifecycle: String,
    val activity: String? = null,
    @SerialName("occupant_generation") val occupantGeneration: Long?,
    @SerialName("last_output_sequence") val lastOutputSequence: Long,
    @SerialName("has_writer") val hasWriter: Boolean,
    val unread: Boolean? = null,
) {
    fun toModel() = ControllerSessionSummary(
        id = sessionId,
        hostInstanceId = hostInstanceId,
        origin = origin,
        runtime = runtime,
        capabilities = capabilities,
        title = title,
        project = project,
        group = group,
        lifecycle = lifecycle,
        activity = activity,
        occupantGeneration = occupantGeneration,
        lastOutputSequence = lastOutputSequence,
        hasWriter = hasWriter,
        unreadCount = if (unread == true) 1 else 0,
    ).also(ControllerSessionSummary::validate)
}

sealed class ControllerConnectionException : Exception() {
    data object PairingRejected : ControllerConnectionException()
    data object AcknowledgementUncertain : ControllerConnectionException()
    data object SequenceGap : ControllerConnectionException()
    data class HostError(val code: String) : ControllerConnectionException()
}

private fun ByteArray.hex(): String = joinToString("") { "%02x".format(it) }
