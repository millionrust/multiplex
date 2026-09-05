package com.termirust.mobile

import com.termirust.controller.security.AuthorizationDecision
import com.termirust.controller.security.ControllerBindingException
import com.termirust.controller.security.ControllerCapability
import com.termirust.controller.security.ControllerFrameKind
import com.termirust.controller.security.ControllerSecurityEngine
import com.termirust.controller.security.PairingConfirmation
import com.termirust.controller.security.PairingRole
import com.termirust.controller.security.PairingStartRequest
import com.termirust.controller.security.SecureBlobException
import com.termirust.controller.security.SecureBlobStore
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertThrows
import org.junit.Test

class ControllerBindingConformanceTest {
    @Test
    fun controllerV1GoldenVectorCrossesGeneratedBindingExactly() {
        val vector = GoldenVector.load()
        val device = ControllerSecurityEngine(MemorySecureBlobStore())
        val host = ControllerSecurityEngine(MemorySecureBlobStore())
        try {
            device.storeSecureBlob("fixture-device", vector.bytes("device_static_private_hex"))
            host.storeSecureBlob("fixture-host", vector.bytes("host_static_private_hex"))
            val offer = vector.bytes("offer_hex")
            val summary = device.decodeOfferSummary(offer)
            assertEquals(1.toUShort(), summary.version.major)
            assertEquals(0.toUShort(), summary.version.minor)
            assertEquals(7.toUShort(), summary.capabilityBits)

            val deviceSession = device.pairingStart(
                PairingStartRequest(
                    PairingRole.DEVICE_INITIATOR,
                    offer,
                    "fixture-device",
                    vector.bytes("device_ephemeral_private_hex"),
                    1_000UL,
                    1_000UL,
                ),
            )
            val hostSession = host.pairingStart(
                PairingStartRequest(
                    PairingRole.HOST_RESPONDER,
                    offer,
                    "fixture-host",
                    vector.bytes("host_ephemeral_private_hex"),
                    1_000UL,
                    1_000UL,
                ),
            )
            try {
                val message1 = deviceSession.pairingOutbound(1_001UL)
                assertArrayEquals(vector.bytes("message_1_hex"), message1)
                hostSession.pairingReceive(message1, 1_002UL)

                val message2 = hostSession.pairingOutbound(1_003UL)
                assertArrayEquals(vector.bytes("message_2_hex"), message2)
                deviceSession.pairingReceive(message2, 1_004UL)

                val message3 = deviceSession.pairingOutbound(1_005UL)
                assertArrayEquals(vector.bytes("message_3_hex"), message3)
                hostSession.pairingReceive(message3, 1_006UL)

                listOf(deviceSession, hostSession).forEach { session ->
                    assertEquals(vector.string("sas_display"), session.sas().value)
                    assertArrayEquals(vector.bytes("handshake_hash_hex"), session.handshakeHash())
                    session.confirmOrReject(PairingConfirmation.CONFIRM, vector.string("sas_display"), 4UL)
                    assertEquals(
                        AuthorizationDecision.ALLOW,
                        session.authorize(ControllerCapability.OBSERVE_SESSIONS, 4UL),
                    )
                    assertEquals(
                        AuthorizationDecision.DENY,
                        session.authorize(ControllerCapability.RESIZE, 4UL),
                    )
                }

                val frame = deviceSession.sealFrame(
                    ControllerFrameKind.CONTROL,
                    ControllerCapability.OBSERVE_SESSIONS,
                    4UL,
                    "controller-v1-first".encodeToByteArray(),
                )
                assertArrayEquals(vector.bytes("first_frame_hex"), frame)
                val opened = hostSession.openFrame(frame)
                assertEquals(0UL, opened.sequence)
                assertArrayEquals("controller-v1-first".encodeToByteArray(), opened.payload)
            } finally {
                deviceSession.close()
                hostSession.close()
            }
        } finally {
            device.close()
            host.close()
        }
    }

    @Test
    fun callbackSizeCancellationAndDisposalFailuresAreTyped() {
        val store = MemorySecureBlobStore()
        val engine = ControllerSecurityEngine(store)
        try {
            store.failure = SecureBlobException.Locked()
            assertThrows(ControllerBindingException.SecureBlobLocked::class.java) {
                engine.secureBlobStatus("device")
            }

            store.failure = null
            assertThrows(ControllerBindingException.SecureBlobInvalid::class.java) {
                engine.storeSecureBlob("device", ByteArray(4_097))
            }

            val vector = GoldenVector.load()
            engine.storeSecureBlob("device", vector.bytes("device_static_private_hex"))
            val session = engine.pairingStart(
                PairingStartRequest(
                    PairingRole.DEVICE_INITIATOR,
                    vector.bytes("offer_hex"),
                    "device",
                    vector.bytes("device_ephemeral_private_hex"),
                    1_000UL,
                    1_000UL,
                ),
            )
            session.cancel()
            assertThrows(ControllerBindingException.Disposed::class.java) { session.sas() }
            session.finish()
            session.finish()
            session.close()
            session.close()
        } finally {
            engine.close()
        }
    }
}

private class MemorySecureBlobStore : SecureBlobStore {
    private val values = mutableMapOf<String, ByteArray>()
    var failure: SecureBlobException? = null

    @Synchronized
    override fun load(keyId: String): ByteArray? {
        failure?.let { throw it }
        return values[keyId]?.copyOf()
    }

    @Synchronized
    override fun store(keyId: String, value: ByteArray) {
        failure?.let { throw it }
        values[keyId] = value.copyOf()
    }

    @Synchronized
    override fun delete(keyId: String) {
        failure?.let { throw it }
        values.remove(keyId)
    }
}

private class GoldenVector private constructor(private val objectValue: JsonObject) {
    fun string(key: String): String = objectValue.getValue(key).jsonPrimitive.content

    fun bytes(key: String): ByteArray {
        val value = string(key)
        require(value.length % 2 == 0)
        return ByteArray(value.length / 2) { index ->
            val offset = index * 2
            value.substring(offset, offset + 2).toInt(16).toByte()
        }
    }

    companion object {
        fun load(): GoldenVector {
            val stream = requireNotNull(
                ControllerBindingConformanceTest::class.java.classLoader
                    ?.getResourceAsStream("controller-v1.json"),
            )
            return stream.bufferedReader().use { reader ->
                GoldenVector(Json.parseToJsonElement(reader.readText()).jsonObject)
            }
        }
    }
}
