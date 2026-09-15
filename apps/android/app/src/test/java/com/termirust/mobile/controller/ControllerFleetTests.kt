package com.termirust.mobile.controller

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test
import java.util.Base64
import java.util.UUID

class ControllerFleetTests {
    @Test
    fun openTerminalRequiresLiveAttachableOccupant() {
        val open = ControllerSessionSummary(
            id = "00000000-0000-0000-0000-000000000020",
            origin = ControllerSessionOrigin.TERMINAL,
            runtime = "local_shell",
            capabilities = listOf(
                ControllerSessionCapability.OBSERVE_SESSIONS,
                ControllerSessionCapability.ATTACH_OUTPUT,
                ControllerSessionCapability.SEND_INPUT,
            ),
            title = "Local Terminal",
            lifecycle = "live",
            occupantGeneration = 1,
            lastOutputSequence = 2,
            hasWriter = false,
            unreadCount = 0,
        )

        assertTrue(open.isOpenTerminal())
        assertFalse(open.copy(lifecycle = "exited", occupantGeneration = null).isOpenTerminal())
        assertFalse(open.copy(capabilities = listOf(ControllerSessionCapability.OBSERVE_SESSIONS)).isOpenTerminal())
    }

    @Test
    fun sessionAndHostBoundsFailClosed() {
        val host = host("host-a")
        host.validate()
        assertThrows(IllegalArgumentException::class.java) {
            host.copy(displayName = "x".repeat(257)).validate()
        }
        val session = session(1)
        session.validate()
        assertThrows(IllegalArgumentException::class.java) {
            session.copy(title = "x".repeat(257)).validate()
        }
    }

    @Test
    fun fleetSnapshotCarriesOnlySupportedNegotiatedCapabilities() {
        val writable = ControllerFleetSnapshot(
            revision = 1,
            updateSequence = 1,
            sessions = listOf(session(1)),
            capabilityBits = 0b1_1111,
        )
        writable.validate()
        assertEquals(0b1_1111, writable.capabilityBits)
        assertThrows(IllegalArgumentException::class.java) {
            writable.copy(capabilityBits = 0b10_0000).validate()
        }
    }

    @Test
    fun cacheEvictsOldestThenFingerprintAndNeverSelectedHost() {
        val selected = cached("selected", viewed = 100)
        val sameTimeB = cached("host-b", viewed = 10)
        val sameTimeA = cached("host-a", viewed = 10)
        val current = ControllerCacheDocument(
            hosts = listOf(selected, sameTimeB, sameTimeA).associateBy { it.host.id },
        )

        val result = ControllerCacheReducer.upsert(
            current = current,
            selectedHostId = "selected",
            value = cached("host-c", viewed = 200),
            encodedSize = { document -> if (document.hosts.size > 3) Int.MAX_VALUE else 1 },
        )

        assertFalse(result.hosts.containsKey("host-a"))
        assertEquals(setOf("selected", "host-b", "host-c"), result.hosts.keys)
    }

    @Test
    fun selectedOnlyOversizeUpdatePreservesPriorDocument() {
        val current = ControllerCacheDocument(hosts = mapOf("selected" to cached("selected", 1)))
        assertThrows(ControllerStoreException.ResourceLimit::class.java) {
            ControllerCacheReducer.upsert(
                current = current,
                selectedHostId = "selected",
                value = cached("selected", 2),
                encodedSize = { ControllerLimits.MAX_CACHE_BYTES + 1 },
            )
        }
        assertEquals(1, current.hosts.size)
        assertEquals(1, current.hosts.getValue("selected").lastViewedAtMillis)
    }

    private fun cached(id: String, viewed: Long) = CachedHostFleet(
        host = host(id),
        snapshot = ControllerFleetSnapshot(1, 1, listOf(session(viewed.toInt()))),
        updatedAtMillis = viewed,
        lastViewedAtMillis = viewed,
    )

    private fun host(id: String) = PairedHostRecord(
        id = id,
        displayName = id,
        route = HostRoute("192.168.1.10", 22_222),
        hostStaticPublicKey = Base64.getEncoder().encodeToString(ByteArray(32) { 7 }),
        deviceStaticKeyId = "controller.device.$id",
        deviceId = UUID.randomUUID().toString(),
        identityGeneration = 1,
        revocationEpoch = 1,
        sessionGeneration = 1,
        capabilityBits = 3,
        pairedAtMillis = 1,
    )

    private fun session(index: Int) = ControllerSessionSummary(
        id = UUID.nameUUIDFromBytes("session-$index".encodeToByteArray()).toString(),
        title = "Session $index",
        lifecycle = "live",
        occupantGeneration = 1,
        lastOutputSequence = index.toLong().coerceAtLeast(0),
        hasWriter = false,
        unreadCount = 0,
    )
}
