package com.multiplex.mobile.controller

import com.multiplex.controller.security.CodePairingFinishRequest
import com.multiplex.controller.security.CodePairingStartRequest
import com.multiplex.controller.security.ConnectionStartRequest
import com.multiplex.controller.security.ControllerCapability
import com.multiplex.controller.security.ControllerConnectionSession
import com.multiplex.controller.security.ControllerFrameKind
import com.multiplex.controller.security.ControllerPairingSession
import com.multiplex.controller.security.ControllerSecurityEngine
import com.multiplex.controller.security.PairingConfirmation
import com.multiplex.controller.security.PairingRole
import com.multiplex.controller.security.PairingStartRequest
import com.multiplex.controller.security.PhoneNetwork
import com.multiplex.controller.security.RememberedRoute
import com.multiplex.controller.security.RouteAdvice
import com.multiplex.controller.security.planRoutes
import com.multiplex.controller.security.rememberRoute
import com.multiplex.controller.security.routeAdvice as adviseOnRoutes
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
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
import java.io.IOException
import java.security.SecureRandom
import java.util.Base64
import java.util.UUID
import java.util.concurrent.ConcurrentHashMap

class ControllerConnection internal constructor(
    blobStore: ControllerSecureBlobStore,
    private val clockMillis: () -> Long = System::currentTimeMillis,
    private val transportFactory: ControllerTransportFactory = TcpControllerTransportFactory,
    private val phoneNetwork: ControllerPhoneNetworkSource = ControllerPhoneNetworkSource.Unknown,
) : ControllerConnecting {
    private val engine = ControllerSecurityEngine(blobStore)
    private val mutex = Mutex()
    private val json = Json { ignoreUnknownKeys = false; encodeDefaults = true; explicitNulls = true }
    private val random = SecureRandom()
    @Volatile private var activeTransport: ControllerDuplexTransport? = null
    @Volatile private var activeTerminal: ActiveTerminalConnection? = null
    private var pendingPairing: PendingPairing? = null
    private val connectedRoutes = ConcurrentHashMap<String, HostRoute>()
    private val connectedMemory = ConcurrentHashMap<String, List<RememberedRouteRecord>>()
    private val adviceByHost = ConcurrentHashMap<String, RouteAdvice>()
    private val discoveredRoutes = ConcurrentHashMap<String, List<HostRoute>>()

    override fun connectedRoute(hostId: String): HostRoute? = connectedRoutes[hostId]

    override fun connectedRouteMemory(hostId: String): List<RememberedRouteRecord>? = connectedMemory[hostId]

    override fun routeAdvice(hostId: String): RouteAdvice? = adviceByHost[hostId]

    override fun noteDiscoveredRoutes(hostId: String, routes: List<HostRoute>) {
        if (routes.isEmpty()) discoveredRoutes.remove(hostId) else discoveredRoutes[hostId] = routes
    }

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
            require(envelope.schemaVersion == 2 && envelope.offerBytes.size <= MAX_OFFER_BYTES)
            require(runCatching { UUID.fromString(envelope.offerId) }.isSuccess)
            require(envelope.offerBytes.all { it in 0..255 })
            require(envelope.routes.size in 1..ControllerLimits.MAX_HOST_ROUTES)
            val offerRoutes = envelope.routes.map { HostRoute(it.address, it.port) }.distinct()
            require(offerRoutes.all(ControllerNetworkAddresses::isPrivateLiteralRoute))
            val offer = envelope.offerBytes.map(Int::toByte).toByteArray()
            val summary = engine.decodeOfferSummary(offer)
            val nowSeconds = clockMillis() / 1_000
            require(summary.version.major.toInt() == 2 && summary.version.minor.toInt() == 0)
            require(summary.expiresAtUnixSeconds.toLong() > nowSeconds)
            require(summary.hostStaticPublicKey.size == 32 && envelope.identityGeneration > 0)
            val fingerprint = summary.hostStaticPublicKey.hex()
            val keyId = "controller.device.${deviceId.toString().lowercase()}.${fingerprint.take(16)}"
            val created = engine.secureBlobStatus(keyId).name == "MISSING"
            if (created) engine.storeSecureBlob(keyId, randomBytes(32))
            try {
                val (transport, route) = openFirstReachable(offerRoutes)
                activeTransport = transport
                val input = DataInputStream(transport.input)
                val output = DataOutputStream(transport.output)
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
                    routes = listOf(route) + offerRoutes.filterNot { it == route },
                    hostName = hostName,
                    deviceName = deviceName,
                    deviceId = deviceId,
                    keyId = keyId,
                    createdKey = created,
                    session = session,
                    transport = transport,
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
            try {
                val result = pending.session.confirmOrReject(
                    PairingConfirmation.CONFIRM,
                    pending.sas,
                    pending.envelope.revocationEpoch.toULong(),
                )
                require(result.hostStaticPublicKey.contentEquals(pending.hostKey))
            } catch (error: Throwable) {
                cancelUnlocked(deleteCreatedKey = false)
                throw error
            }
            registerDevice(
                ConfirmedPairing(
                    session = pending.session,
                    input = pending.input,
                    output = pending.output,
                    hostKey = pending.hostKey,
                    capabilityBits = pending.capabilityBits,
                    identityGeneration = pending.envelope.identityGeneration,
                    revocationEpoch = pending.envelope.revocationEpoch,
                    sessionGeneration = pending.envelope.sessionGeneration,
                    routes = pending.routes,
                    hostName = pending.hostName,
                    deviceName = pending.deviceName,
                    deviceId = pending.deviceId,
                    keyId = pending.keyId,
                ),
            )
        }
    }

    override suspend fun pairWithCode(
        routes: List<HostRoute>,
        code: String,
        hostName: String?,
        expectedDiscoveryId: String?,
        deviceName: String,
        deviceId: UUID,
    ): PairedHostRecord = mutex.withLock {
        withContext(Dispatchers.IO) {
            cancelUnlocked(deleteCreatedKey = true)
            require(code.length == 6 && code.all { it in '0'..'9' })
            require(routes.size in 1..ControllerLimits.MAX_HOST_ROUTES && routes.toSet().size == routes.size)
            require(routes.all(ControllerNetworkAddresses::isPrivateLiteralRoute))
            require(hostName == null || hostName.codePointCount(0, hostName.length) in 1..256)
            require(deviceName.codePointCount(0, deviceName.length) in 1..64)
            require(deviceName.none(Char::isISOControl))
            require(expectedDiscoveryId == null || DISCOVERY_ID_PATTERN.matches(expectedDiscoveryId))
            val (transport, route) = openFirstReachable(routes)
            activeTransport = transport
            coroutineScope {
                // Blocking reads ignore coroutine cancellation, so closing the socket is what
                // bounds the whole exchange.
                val deadline = launch {
                    delay(CODE_PAIRING_TIMEOUT_MILLIS)
                    transport.close()
                }
                try {
                    val confirmed = startCodePairing(
                        transport = transport,
                        routes = listOf(route) + routes.filterNot { it == route },
                        code = code,
                        hostName = hostName,
                        expectedDiscoveryId = expectedDiscoveryId,
                        deviceName = deviceName,
                        deviceId = deviceId,
                    )
                    registerDevice(confirmed)
                } finally {
                    deadline.cancel()
                }
            }
        }
    }

    // Runs the code exchange and the pairing handshake bound to it. Every failure here reads
    // as a wrong code: the Host closes the connection without saying why, and it counts the
    // attempts itself.
    private fun startCodePairing(
        transport: ControllerDuplexTransport,
        routes: List<HostRoute>,
        code: String,
        hostName: String?,
        expectedDiscoveryId: String?,
        deviceName: String,
        deviceId: UUID,
    ): ConfirmedPairing {
        var createdKeyId: String? = null
        try {
            val input = DataInputStream(transport.input)
            val output = DataOutputStream(transport.output)
            output.write(PAIRING_CODE_PREFACE)
            val deviceNonce = randomBytes(32)
            writeFrame(
                output,
                json.encodeToString(
                    CodePairingHelloPayload(deviceNonce = deviceNonce.map { it.toInt() and 0xff }),
                ).encodeToByteArray(),
                MAX_OFFER_BYTES,
            )
            val envelope = json.decodeFromString<CodePairingOfferEnvelope>(
                readFrame(input, MAX_OFFER_BYTES).decodeToString(),
            )
            require(envelope.schemaVersion == 1 && envelope.offerBytes.size == OFFER_CORE_BYTES)
            require(runCatching { UUID.fromString(envelope.offerId) }.isSuccess)
            require(envelope.offerBytes.all { it in 0..255 })
            // A new Host starts at session generation zero.
            require(envelope.identityGeneration > 0 && envelope.revocationEpoch >= 0 && envelope.sessionGeneration >= 0)
            val offer = envelope.offerBytes.map(Int::toByte).toByteArray()
            val summary = engine.decodeOfferSummary(offer)
            require(summary.version.major.toInt() == 2 && summary.version.minor.toInt() == 0)
            require(summary.expiresAtUnixSeconds.toLong() > clockMillis() / 1_000)
            require(summary.hostStaticPublicKey.size == 32)
            val discoveryId = ControllerNetworkAddresses.discoveryId(summary.hostStaticPublicKey)
            require(expectedDiscoveryId == null || expectedDiscoveryId == discoveryId)
            val fingerprint = summary.hostStaticPublicKey.hex()
            val keyId = "controller.device.${deviceId.toString().lowercase()}.${fingerprint.take(16)}"
            if (engine.secureBlobStatus(keyId).name == "MISSING") {
                engine.storeSecureBlob(keyId, randomBytes(32))
                createdKeyId = keyId
            }
            val exchange = engine.codePairingStart(
                CodePairingStartRequest(
                    code = code,
                    offerBytes = offer,
                    deviceNonce = deviceNonce,
                    scalarEntropy = randomBytes(64),
                ),
            )
            val session = try {
                writeFrame(output, exchange.share(), CODE_SHARE_BYTES)
                val hostShare = readFrame(input, CODE_SHARE_BYTES)
                require(hostShare.size == CODE_SHARE_BYTES)
                exchange.finish(
                    CodePairingFinishRequest(
                        hostShare = hostShare,
                        staticKeyId = keyId,
                        ephemeralPrivateKey = randomBytes(32),
                        nowMillis = uptimeMillis().toULong(),
                        nowUnixSeconds = (clockMillis() / 1_000).toULong(),
                    ),
                )
            } finally {
                exchange.close()
            }
            try {
                writeFrame(output, session.pairingOutbound(uptimeMillis().toULong()), MAX_HANDSHAKE_BYTES)
                session.pairingReceive(readFrame(input, MAX_HANDSHAKE_BYTES), uptimeMillis().toULong())
                writeFrame(output, session.pairingOutbound(uptimeMillis().toULong()), MAX_HANDSHAKE_BYTES)
                val result = session.confirmCodePairing(envelope.revocationEpoch.toULong())
                require(result.hostStaticPublicKey.contentEquals(summary.hostStaticPublicKey))
            } catch (error: Throwable) {
                closePairingSession(session)
                throw error
            }
            return ConfirmedPairing(
                session = session,
                input = input,
                output = output,
                hostKey = summary.hostStaticPublicKey,
                capabilityBits = summary.capabilityBits.toInt(),
                identityGeneration = envelope.identityGeneration,
                revocationEpoch = envelope.revocationEpoch,
                sessionGeneration = envelope.sessionGeneration,
                routes = routes,
                hostName = hostName ?: ControllerNetworkAddresses.defaultHostName(discoveryId),
                deviceName = deviceName,
                deviceId = deviceId,
                keyId = keyId,
            )
        } catch (error: Throwable) {
            createdKeyId?.let { runCatching { engine.deleteSecureBlob(it) } }
            cancelUnlocked(deleteCreatedKey = false)
            throw when (error) {
                is CancellationException, is ControllerSecretException -> error
                else -> ControllerConnectionException.CodeRejected
            }
        }
    }

    // Sends the device registration over a confirmed pairing and saves nothing until the Host
    // acknowledges it. When the acknowledgement is lost, an authenticated session list proves
    // the Host kept the device.
    private suspend fun registerDevice(pairing: ConfirmedPairing): PairedHostRecord {
        var registrationSent = false
        try {
            val registration = json.encodeToString(
                PairingRegistrationPayload(
                    deviceId = pairing.deviceId.toString(),
                    displayName = pairing.deviceName,
                ),
            ).encodeToByteArray()
            val sealed = pairing.session.sealFrame(
                ControllerFrameKind.CONTROL,
                ControllerCapability.OBSERVE_SESSIONS,
                pairing.revocationEpoch.toULong(),
                registration,
            )
            writeFrame(pairing.output, sealed, MAX_SECURE_FRAME_BYTES)
            registrationSent = true
            val opened = pairing.session.openFrame(readFrame(pairing.input, MAX_SECURE_FRAME_BYTES))
            require(opened.kind == ControllerFrameKind.CONTROL)
            require(opened.capability == ControllerCapability.OBSERVE_SESSIONS)
            require(opened.revocationEpoch.toLong() == pairing.revocationEpoch)
            val ack = json.decodeFromString<PairingHostAckPayload>(opened.payload.decodeToString())
            require(ack.schemaVersion == 1 && ack.deviceId == pairing.deviceId.toString())
            require(ack.identityGeneration == pairing.identityGeneration)
            require(ack.revocationEpoch == pairing.revocationEpoch)
            require(ack.sessionGeneration == pairing.sessionGeneration)
            require(ack.capabilityBits and pairing.capabilityBits.inv() == 0)
            val record = pairedHostRecord(
                pairing,
                identityGeneration = ack.identityGeneration,
                revocationEpoch = ack.revocationEpoch,
                sessionGeneration = ack.sessionGeneration,
                capabilityBits = ack.capabilityBits,
            )
            record.validate()
            closePairingSession(pairing.session)
            cancelUnlocked(deleteCreatedKey = false)
            return record
        } catch (error: Throwable) {
            closePairingSession(pairing.session)
            cancelUnlocked(deleteCreatedKey = false)
            if (registrationSent) {
                val provisional = pairedHostRecord(
                    pairing,
                    identityGeneration = pairing.identityGeneration,
                    revocationEpoch = pairing.revocationEpoch,
                    sessionGeneration = pairing.sessionGeneration,
                    capabilityBits = pairing.capabilityBits,
                )
                provisional.validate()
                try {
                    fetchSessionsUnlocked(provisional)
                    return connectedRoutes[provisional.id]?.let(provisional::preferringRoute) ?: provisional
                } catch (_: Throwable) {
                    cancelUnlocked(deleteCreatedKey = false)
                    throw ControllerConnectionException.AcknowledgementUncertain
                }
            }
            throw error
        }
    }

    private fun pairedHostRecord(
        pairing: ConfirmedPairing,
        identityGeneration: Long,
        revocationEpoch: Long,
        sessionGeneration: Long,
        capabilityBits: Int,
    ) = PairedHostRecord(
        id = pairing.hostKey.hex(),
        displayName = pairing.hostName,
        route = pairing.routes.first(),
        hostStaticPublicKey = Base64.getEncoder().encodeToString(pairing.hostKey),
        deviceStaticKeyId = pairing.keyId,
        deviceId = pairing.deviceId.toString(),
        identityGeneration = identityGeneration,
        revocationEpoch = revocationEpoch,
        sessionGeneration = sessionGeneration,
        capabilityBits = capabilityBits,
        pairedAtMillis = clockMillis(),
        routes = pairing.routes,
        discoveryId = ControllerNetworkAddresses.discoveryId(pairing.hostKey),
    )

    private fun closePairingSession(session: ControllerPairingSession) {
        runCatching { session.finish() }
        session.close()
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
            val socket = openHost(host)
            activeTransport = socket
            try {
                val input = DataInputStream(socket.input)
                val output = DataOutputStream(socket.output)
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
                if (activeTransport === socket) activeTransport = null
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
            val socket = openHost(host)
            activeTransport = socket
            try {
                val input = DataInputStream(socket.input)
                val output = DataOutputStream(socket.output)
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
                if (activeTransport === socket) activeTransport = null
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

    /**
     * Watches a computer's screen until [onEvent] throws or the coroutine is cancelled.
     *
     * The session starts with an `open_screen` command, whose one-time ticket the screen
     * protocol's hello proves. After that every frame is a screen frame claiming the capability
     * its contents need, which the computer checks again before acting on it.
     */
    override suspend fun watchScreen(
        host: PairedHostRecord,
        surface: UInt?,
        preview: Boolean,
        onOpened: suspend (ControllerScreenTicket, com.multiplex.screens.ScreenViewer) -> Unit,
        onEvent: suspend (List<com.multiplex.screens.ScreenEvent>) -> Unit,
    ) = mutex.withLock {
        withContext(Dispatchers.IO) {
            cancelUnlocked(deleteCreatedKey = true)
            host.validate()
            if (host.capabilityBits and OBSERVE_SCREENS_CAPABILITY != OBSERVE_SCREENS_CAPABILITY) {
                throw ControllerConnectionException.CapabilityDenied
            }
            val socket = openHost(host)
            activeTransport = socket
            try {
                val input = DataInputStream(socket.input)
                val output = DataOutputStream(socket.output)
                val hostKey = Base64.getDecoder().decode(host.hostStaticPublicKey)
                val requested = host.capabilityBits and ALL_SUPPORTED_CAPABILITIES
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
                try {
                    writeFrame(output, session.handshakeOutbound(uptimeMillis().toULong()), MAX_HANDSHAKE_BYTES)
                    val publicResult = session.handshakeReceiveAccept(
                        readFrame(input, MAX_HANDSHAKE_BYTES),
                        uptimeMillis().toULong(),
                    )
                    require(publicResult.hostStaticPublicKey.contentEquals(hostKey))
                    require(publicResult.identityGeneration.toLong() == host.identityGeneration)
                    require(publicResult.revocationEpoch.toLong() == host.revocationEpoch)
                    val granted = publicResult.grantedCapabilityBits.toInt()
                    require(granted and OBSERVE_SCREENS_CAPABILITY == OBSERVE_SCREENS_CAPABILITY)
                    require(granted and ALL_SUPPORTED_CAPABILITIES.inv() == 0)

                    val ticket = openScreenSession(host, session, input, output)
                    // This app decodes the motion region itself, with MediaCodec, so it asks the
                    // computer for it. A preview is one small picture a second and never worth a
                    // video stream.
                    val viewer = com.multiplex.screens.ScreenViewer.withMotion(
                        SCREEN_CACHE_BYTES,
                        !preview,
                    )
                    try {
                        viewer.connect(ticket.ticket)
                        var watching = surface
                        watching?.let { viewer.subscribe(it, preview) }
                        onOpened(ticket, viewer)
                        val pump = ControllerScreenPump(ticket)
                        while (true) {
                            for ((capability, bytes) in pump.drain(viewer)) {
                                val sealed = session.sealFrame(
                                    ControllerFrameKind.SCREEN,
                                    capability,
                                    host.revocationEpoch.toULong(),
                                    bytes,
                                )
                                writeFrame(output, sealed, MAX_TERMINAL_FRAME_BYTES)
                            }
                            val opened = session.openFrame(readFrame(input, MAX_TERMINAL_FRAME_BYTES))
                            require(opened.kind == ControllerFrameKind.SCREEN)
                            require(opened.capability == ControllerCapability.OBSERVE_SCREENS)
                            require(opened.revocationEpoch.toLong() == host.revocationEpoch)
                            val events = viewer.receive(opened.payload)
                            // The welcome is the first thing that names what this computer
                            // shares, so a phone that asked for "whatever you have" subscribes
                            // here rather than guessing an id.
                            if (watching == null) {
                                val first = events.filterIsInstance<com.multiplex.screens.ScreenEvent.Welcomed>()
                                    .firstOrNull()?.surfaces?.firstOrNull()
                                if (first != null) {
                                    watching = first.id
                                    viewer.subscribe(first.id, preview)
                                }
                            }
                            onEvent(events)
                        }
                    } finally {
                        viewer.close()
                    }
                } finally {
                    runCatching { session.finish() }
                    session.close()
                }
            } finally {
                socket.close()
                if (activeTransport === socket) activeTransport = null
            }
        }
    }

    /** Asks for a screen session and reads the one-time ticket it answers with. */
    private fun openScreenSession(
        host: PairedHostRecord,
        session: ControllerConnectionSession,
        input: DataInputStream,
        output: DataOutputStream,
    ): ControllerScreenTicket {
        val commandId = UUID.randomUUID()
        val envelope = OpenScreenEnvelope(
            commandId = commandId.toString(),
            sessionGeneration = host.sessionGeneration,
            deadlineMillis = clockMillis() + READ_TIMEOUT_MILLIS,
            command = ScreenCommand(ControllerScreenCommands.OPEN),
        )
        val sealed = session.sealFrame(
            ControllerFrameKind.CONTROL,
            ControllerCapability.OBSERVE_SCREENS,
            host.revocationEpoch.toULong(),
            json.encodeToString(envelope).encodeToByteArray(),
        )
        writeFrame(output, sealed, MAX_SECURE_FRAME_BYTES)
        val opened = session.openFrame(readFrame(input, MAX_SECURE_FRAME_BYTES))
        require(opened.kind == ControllerFrameKind.CONTROL)
        require(opened.capability == ControllerCapability.OBSERVE_SCREENS)
        require(opened.revocationEpoch.toLong() == host.revocationEpoch)
        val response = json.decodeFromString<ScreenOpenedResponse>(opened.payload.decodeToString())
        return ControllerScreenCommands.ticket(response, commandId)
    }

    override suspend fun cancel() {
        // Socket.close is thread-safe and unblocks a pending read before the operation
        // coroutine can reacquire the serialization mutex.
        activeTransport?.close()
        mutex.withLock { withContext(Dispatchers.IO) { cancelUnlocked(true) } }
    }

    override fun close() {
        activeTransport?.close()
        activeTransport = null
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
        val socket = openHost(host)
        activeTransport = socket
        try {
            progress(ControllerConnectionState.Authenticating)
            val input = DataInputStream(socket.input)
            val output = DataOutputStream(socket.output)
            val hostKey = Base64.getDecoder().decode(host.hostStaticPublicKey)
            val request = ConnectionStartRequest(
                staticKeyId = host.deviceStaticKeyId,
                ephemeralPrivateKey = randomBytes(32),
                hostStaticPublicKey = hostKey,
                identityGeneration = host.identityGeneration.toULong(),
                revocationEpoch = host.revocationEpoch.toULong(),
                requestedCapabilityBits = ALL_SUPPORTED_CAPABILITIES.toUShort(),
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
                val grantedCapabilityBits = publicResult.grantedCapabilityBits.toInt()
                require(grantedCapabilityBits and OBSERVE_CAPABILITY == OBSERVE_CAPABILITY)
                require(grantedCapabilityBits and ALL_SUPPORTED_CAPABILITIES.inv() == 0)
                progress(ControllerConnectionState.Syncing)
                return fetchStableSnapshot(host, session, input, output, grantedCapabilityBits)
            } finally {
                runCatching { session.finish() }
                session.close()
            }
        } finally {
            socket.close()
            if (activeTransport === socket) activeTransport = null
        }
    }

    private fun fetchStableSnapshot(
        host: PairedHostRecord,
        session: ControllerConnectionSession,
        input: DataInputStream,
        output: DataOutputStream,
        capabilityBits: Int,
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
                capabilityBits = capabilityBits,
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
        activeTransport?.close()
        activeTransport = null
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

    // Races the routes as the shared planner orders and times them, and returns the first that
    // accepts a connection. Nothing has been sent to any other route, and none is left open.
    private suspend fun openFirstReachable(
        routes: List<HostRoute>,
        discovered: List<HostRoute> = emptyList(),
        remembered: List<RememberedRouteRecord> = emptyList(),
        onRaced: (PhoneNetwork, ControllerRouteRaceResult) -> Unit = { _, _ -> },
    ): Pair<ControllerDuplexTransport, HostRoute> {
        val network = phoneNetwork.current()
        val plan = planRoutes(
            routes.map(HostRoute::toRouteAddress),
            discovered.map(HostRoute::toRouteAddress),
            network,
            remembered.map { RememberedRoute(it.fingerprint, it.route.toRouteAddress()) },
        )
        val result = ControllerRouteRace.race(plan, transportFactory)
        onRaced(network, result)
        val transport = result.transport ?: throw checkNotNull(result.failure)
        return transport to checkNotNull(result.route)
    }

    // SSH and relay transports ignore the route, so only the direct TCP transport races the list.
    private suspend fun openHost(host: PairedHostRecord): ControllerDuplexTransport {
        if (transportFactory !== TcpControllerTransportFactory) {
            val transport = transportFactory.open(host.route)
            connectedRoutes[host.id] = host.route
            return transport
        }
        val discovered = discoveredRoutes.remove(host.id).orEmpty()
        val (transport, route) = openFirstReachable(host.routes, discovered, host.routeMemory) { network, result ->
            val saved = (host.routes + discovered).distinct().map(HostRoute::toRouteAddress)
            adviceByHost[host.id] = adviseOnRoutes(saved, network, result.outcomes)
            val fingerprint = network.fingerprint
            val won = result.route
            if (fingerprint != null && won != null) {
                connectedMemory[host.id] = rememberRoute(
                    host.routeMemory.map { RememberedRoute(it.fingerprint, it.route.toRouteAddress()) },
                    fingerprint,
                    won.toRouteAddress(),
                ).map { RememberedRouteRecord(it.fingerprint, it.route.address, it.route.port.toInt()) }
            }
        }
        connectedRoutes[host.id] = route
        return transport
    }

    private companion object {
        const val MAX_OFFER_BYTES = 4 * 1_024
        const val MAX_HANDSHAKE_BYTES = 1 * 1_024
        const val MAX_SECURE_FRAME_BYTES = 64 * 1_024
        const val MAX_TERMINAL_FRAME_BYTES = 1 * 1_024 * 1_024
        const val READ_TIMEOUT_MILLIS = 30_000
        val SCREEN_CACHE_BYTES: ULong = 32UL * 1_024UL * 1_024UL
        const val OBSERVE_CAPABILITY = 1
        const val ATTACH_CAPABILITY = 1 shl 1
        const val INPUT_CAPABILITY = 1 shl 2
        const val RESIZE_CAPABILITY = 1 shl 3
        const val APPROVAL_CAPABILITY = 1 shl 4
        const val ALL_INTERACTIVE_CAPABILITIES = ATTACH_CAPABILITY or INPUT_CAPABILITY or
            RESIZE_CAPABILITY or APPROVAL_CAPABILITY
        const val OBSERVE_SCREENS_CAPABILITY = 1 shl 5
        const val CONTROL_POINTER_CAPABILITY = 1 shl 6
        const val CONTROL_KEYBOARD_CAPABILITY = 1 shl 7
        const val ALL_SCREEN_CAPABILITIES = OBSERVE_SCREENS_CAPABILITY or
            CONTROL_POINTER_CAPABILITY or CONTROL_KEYBOARD_CAPABILITY
        // Every bit this build understands. A computer may grant a screen capability at any time,
        // and a phone that refused to recognise one would break its own terminal connection.
        const val ALL_SUPPORTED_CAPABILITIES = OBSERVE_CAPABILITY or ALL_INTERACTIVE_CAPABILITIES or
            ALL_SCREEN_CAPABILITIES
        const val OFFER_CORE_BYTES = 84
        const val CODE_SHARE_BYTES = 32
        const val CODE_PAIRING_TIMEOUT_MILLIS = 60_000L
        val PAIRING_PREFACE = byteArrayOf(0x54, 0x52, 0x43, 0x4e, 0, 1, 2, 0)
        val PAIRING_CODE_PREFACE = byteArrayOf(0x54, 0x52, 0x43, 0x4e, 0, 1, 3, 0)
        val AUTH_PREFACE = byteArrayOf(0x54, 0x52, 0x43, 0x4e, 0, 1, 1, 0)
    }
}

private data class PendingPairing(
    val envelope: PairingOfferEnvelope,
    // The route that connected comes first.
    val routes: List<HostRoute>,
    val hostName: String,
    val deviceName: String,
    val deviceId: UUID,
    val keyId: String,
    val createdKey: Boolean,
    val session: ControllerPairingSession,
    val transport: ControllerDuplexTransport,
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
    val routes: List<PairingRoutePayload>,
    @SerialName("offer_bytes") val offerBytes: List<Int>,
)

@Serializable private data class PairingRoutePayload(
    val address: String,
    val port: Int,
)

private class ConfirmedPairing(
    val session: ControllerPairingSession,
    val input: DataInputStream,
    val output: DataOutputStream,
    val hostKey: ByteArray,
    val capabilityBits: Int,
    val identityGeneration: Long,
    val revocationEpoch: Long,
    val sessionGeneration: Long,
    val routes: List<HostRoute>,
    val hostName: String,
    val deviceName: String,
    val deviceId: UUID,
    val keyId: String,
)

@Serializable private data class CodePairingHelloPayload(
    @SerialName("schema_version") val schemaVersion: Int = 1,
    @SerialName("device_nonce") val deviceNonce: List<Int>,
)

// The Host's offer on a code connection carries no routes: the phone already reached it.
@Serializable private data class CodePairingOfferEnvelope(
    @SerialName("schema_version") val schemaVersion: Int,
    @SerialName("offer_id") val offerId: String,
    @SerialName("identity_generation") val identityGeneration: Long,
    @SerialName("revocation_epoch") val revocationEpoch: Long,
    @SerialName("session_generation") val sessionGeneration: Long,
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
    data object CodeRejected : ControllerConnectionException()
    data object AcknowledgementUncertain : ControllerConnectionException()
    data object SequenceGap : ControllerConnectionException()

    /** This device may not do what it asked for, or the transport cannot carry it. */
    data object CapabilityDenied : ControllerConnectionException()
    data class HostError(val code: String) : ControllerConnectionException()
}

private fun ByteArray.hex(): String = joinToString("") { "%02x".format(it) }
