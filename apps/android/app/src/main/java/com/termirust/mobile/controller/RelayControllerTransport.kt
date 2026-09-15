package com.termirust.mobile.controller

import okhttp3.CertificatePinner
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.Response
import okhttp3.WebSocket
import okhttp3.WebSocketListener
import okio.ByteString
import java.io.IOException
import java.io.InputStream
import java.io.OutputStream
import java.io.PipedInputStream
import java.io.PipedOutputStream
import java.net.URI
import java.util.Base64
import java.util.concurrent.ArrayBlockingQueue
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean

internal object RelayControllerTransportFactory {
    private const val ORIGIN = "termirust://relay-local"
    private const val SUBPROTOCOL = "termirust-relay-v1"
    private const val MAX_MESSAGE_BYTES = 1_048_640
    private const val MAX_PAYLOAD_BYTES = 1_048_576
    private const val STREAM_CHUNK_BYTES = 64 * 1_024
    private const val STREAM_BUFFER_BYTES = 256 * 1_024
    private const val EVENT_QUEUE_CAPACITY = 64
    private const val CONNECT_TIMEOUT_SECONDS = 10L

    fun create(
        hostId: String,
        configuration: ControllerRemoteRouteConfiguration,
        credentials: ControllerRouteCredentialStore,
    ): ControllerTransportFactory = create(
        hostId = hostId,
        configuration = configuration,
        credentials = credentials,
        clientBuilderFactory = RelayHttpClientBuilderFactory { OkHttpClient.Builder() },
    )

    internal fun create(
        hostId: String,
        configuration: ControllerRemoteRouteConfiguration,
        credentials: ControllerRouteCredentialStore,
        clientBuilderFactory: RelayHttpClientBuilderFactory,
    ): ControllerTransportFactory {
        configuration.validate()
        require(configuration.kind == ControllerRemoteRouteKind.SELF_HOSTED_RELAY)
        return ControllerTransportFactory {
            open(hostId, configuration, credentials, clientBuilderFactory)
        }
    }

    private fun open(
        hostId: String,
        configuration: ControllerRemoteRouteConfiguration,
        credentials: ControllerRouteCredentialStore,
        clientBuilderFactory: RelayHttpClientBuilderFactory,
    ): ControllerDuplexTransport {
        check(NativeRelayProtocol.loaded) { "The native relay protocol is unavailable." }
        val endpoint = URI(configuration.endpoint)
        val host = requireNotNull(endpoint.host)
        val pin = requireNotNull(configuration.trustPin)
        require(pin.startsWith("sha256/") && pin.length > "sha256/".length)
        val routeId = Base64.getDecoder().decode(requireNotNull(configuration.relayRouteId))
        require(routeId.size == 32)
        val reference = requireNotNull(configuration.credential)
        val secretText = requireNotNull(credentials.read(hostId, reference)) {
            "The relay admission credential is missing."
        }
        val credential = try {
            Base64.getDecoder().decode(secretText)
        } finally {
            // String storage is owned by the platform secret store; decoded bytes are wiped below.
        }
        require(credential.size == 32) { "The relay admission credential is invalid." }

        val client = clientBuilderFactory.create()
            .connectTimeout(CONNECT_TIMEOUT_SECONDS, TimeUnit.SECONDS)
            .readTimeout(0, TimeUnit.MILLISECONDS)
            .certificatePinner(CertificatePinner.Builder().add(host, pin).build())
            .build()
        val listener = RelayListener()
        val request = Request.Builder()
            .url(configuration.endpoint)
            .header("Origin", ORIGIN)
            .header("Sec-WebSocket-Protocol", SUBPROTOCOL)
            .build()
        val webSocket = client.newWebSocket(request, listener)
        try {
            val opened = listener.nextEvent()
            require(opened is RelayEvent.Opened && opened.response.header("Sec-WebSocket-Protocol") == SUBPROTOCOL) {
                "The relay rejected the required WebSocket subprotocol."
            }
            check(webSocket.send(ByteString.of(*NativeRelayProtocol.clientHello(routeId)))) {
                "The relay closed during admission."
            }
            val challenge = listener.nextBinary()
            val proof = NativeRelayProtocol.admissionProof(
                routeId,
                credential,
                requireNotNull(configuration.relayRevocationEpoch),
                System.currentTimeMillis() / 1_000,
                challenge,
            )
            check(webSocket.send(ByteString.of(*proof))) { "The relay closed during admission." }
            NativeRelayProtocol.admissionConnectionId(listener.nextBinary())
            return RelayDuplexTransport(
                client = client,
                webSocket = webSocket,
                listener = listener,
                routeId = routeId,
                streamBufferBytes = STREAM_BUFFER_BYTES,
                streamChunkBytes = STREAM_CHUNK_BYTES,
                maxPayloadBytes = MAX_PAYLOAD_BYTES,
            )
        } catch (error: Throwable) {
            listener.close()
            webSocket.cancel()
            client.dispatcher.executorService.shutdown()
            client.connectionPool.evictAll()
            throw error
        } finally {
            credential.fill(0)
        }
    }

    private sealed interface RelayEvent {
        data class Opened(val response: Response) : RelayEvent
        data class Binary(val bytes: ByteArray) : RelayEvent
        data class Failed(val error: Throwable) : RelayEvent
        data object Closed : RelayEvent
    }

    private class RelayListener : WebSocketListener() {
        private val events = ArrayBlockingQueue<RelayEvent>(EVENT_QUEUE_CAPACITY)
        private val closed = AtomicBoolean(false)

        override fun onOpen(webSocket: WebSocket, response: Response) = offer(RelayEvent.Opened(response))

        override fun onMessage(webSocket: WebSocket, bytes: ByteString) {
            if (bytes.size !in 1..MAX_MESSAGE_BYTES) {
                fail(IOException("Relay frame exceeded its bound."), webSocket)
            } else {
                offer(RelayEvent.Binary(bytes.toByteArray()), webSocket)
            }
        }

        override fun onMessage(webSocket: WebSocket, text: String) =
            fail(IOException("Relay sent a non-binary frame."), webSocket)

        override fun onClosing(webSocket: WebSocket, code: Int, reason: String) {
            close()
            webSocket.close(code, "")
        }

        override fun onClosed(webSocket: WebSocket, code: Int, reason: String) = close()

        override fun onFailure(webSocket: WebSocket, error: Throwable, response: Response?) =
            fail(error, webSocket)

        fun nextEvent(): RelayEvent {
            val event = events.poll(CONNECT_TIMEOUT_SECONDS, TimeUnit.SECONDS)
                ?: throw IOException("Relay operation timed out.")
            if (event is RelayEvent.Failed) throw IOException("Relay connection failed.", event.error)
            if (event is RelayEvent.Closed) throw IOException("Relay connection closed.")
            return event
        }

        fun nextBinary(): ByteArray {
            val event = nextEvent()
            return (event as? RelayEvent.Binary)?.bytes
                ?: throw IOException("Relay admission response was malformed.")
        }

        fun close() {
            if (closed.compareAndSet(false, true)) offer(RelayEvent.Closed)
        }

        private fun fail(error: Throwable, webSocket: WebSocket) {
            if (closed.compareAndSet(false, true)) offer(RelayEvent.Failed(error))
            webSocket.cancel()
        }

        private fun offer(event: RelayEvent, webSocket: WebSocket? = null) {
            if (!events.offer(event)) {
                closed.set(true)
                events.clear()
                events.offer(RelayEvent.Failed(IOException("Relay receive queue exceeded its bound.")))
                webSocket?.cancel()
            }
        }
    }

    private class RelayDuplexTransport(
        private val client: OkHttpClient,
        private val webSocket: WebSocket,
        private val listener: RelayListener,
        private val routeId: ByteArray,
        streamBufferBytes: Int,
        private val streamChunkBytes: Int,
        private val maxPayloadBytes: Int,
    ) : ControllerDuplexTransport {
        private val closed = AtomicBoolean(false)
        private val pipeOutput = PipedOutputStream()
        override val input = PipedInputStream(pipeOutput, streamBufferBytes)
        private var sendSequence = 0L
        private var receiveSequence = 0L
        private val pump = Thread(::pumpIncoming, "termirust-relay-receive").apply {
            isDaemon = true
            start()
        }
        override val output: OutputStream = object : OutputStream() {
            override fun write(value: Int) = write(byteArrayOf(value.toByte()))

            @Synchronized
            override fun write(bytes: ByteArray, offset: Int, length: Int) {
                require(offset >= 0 && length >= 0 && offset <= bytes.size - length)
                check(!closed.get()) { "Relay transport is closed." }
                var cursor = offset
                val end = offset + length
                while (cursor < end) {
                    val next = (cursor + streamChunkBytes).coerceAtMost(end)
                    val payload = bytes.copyOfRange(cursor, next)
                    require(payload.size <= maxPayloadBytes)
                    val envelope = NativeRelayProtocol.encodeEnvelope(routeId, sendSequence, payload)
                    check(webSocket.send(ByteString.of(*envelope))) { "Relay send queue is closed." }
                    sendSequence = Math.addExact(sendSequence, 1)
                    cursor = next
                }
            }

            override fun close() = this@RelayDuplexTransport.close()
        }

        private fun pumpIncoming() {
            try {
                while (!closed.get()) {
                    val envelope = listener.nextBinary()
                    val payload = NativeRelayProtocol.decodeEnvelope(routeId, receiveSequence, envelope)
                    receiveSequence = Math.addExact(receiveSequence, 1)
                    pipeOutput.write(payload)
                    pipeOutput.flush()
                }
            } catch (_: Throwable) {
                close()
            }
        }

        override fun close() {
            if (!closed.compareAndSet(false, true)) return
            listener.close()
            runCatching { pipeOutput.close() }
            runCatching { input.close() }
            webSocket.close(1000, "")
            webSocket.cancel()
            client.dispatcher.executorService.shutdown()
            client.connectionPool.evictAll()
            if (Thread.currentThread() !== pump) pump.interrupt()
        }
    }
}

internal fun interface RelayHttpClientBuilderFactory {
    fun create(): OkHttpClient.Builder
}
