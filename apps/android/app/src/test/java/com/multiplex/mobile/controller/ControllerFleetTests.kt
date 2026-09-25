package com.multiplex.mobile.controller

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
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
            revision = 1uL,
            updateSequence = 1uL,
            sessions = listOf(session(1)),
            capabilityBits = 0b1_1111,
        )
        writable.validate()
        assertEquals(0b1_1111, writable.capabilityBits)
        // The three screen bits and starting a terminal are ours to understand too.
        writable.copy(capabilityBits = 0b1_1111_1111).validate()
        assertThrows(IllegalArgumentException::class.java) {
            writable.copy(capabilityBits = 0b10_0000_0000).validate()
        }
    }

    /**
     * A computer that grants watching or control must stay usable for everything else.
     *
     * The bits above the five session ones were rejected outright, so pairing with a computer
     * that shared its screen wrote a record the app then called invalid — and every path that
     * checks the record first (listing sessions, attaching a terminal, opening the screen) threw
     * before it got as far as the capability it was looking for.
     */
    @Test
    fun aComputerThatGrantsScreenAccessIsStillAValidRecord() {
        val watching = host("host-screens").copy(
            capabilityBits = ControllerConnection.OBSERVE_SCREENS_CAPABILITY or
                ControllerConnection.OBSERVE_CAPABILITY,
        )
        watching.validate()
        val controlling = host("host-control").copy(
            capabilityBits = ControllerConnection.ALL_SUPPORTED_CAPABILITIES,
        )
        controlling.validate()
        assertThrows(IllegalArgumentException::class.java) {
            controlling.copy(capabilityBits = ControllerLimits.ALL_CAPABILITY_BITS + 1).validate()
        }
    }

    /** The mask the records accept is the one the connection negotiates, not a second opinion. */
    @Test
    fun theRecordAndTheConnectionAgreeOnWhichBitsExist() {
        assertEquals(
            ControllerConnection.ALL_SUPPORTED_CAPABILITIES,
            ControllerLimits.ALL_CAPABILITY_BITS,
        )
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
        snapshot = ControllerFleetSnapshot(1uL, 1uL, listOf(session(viewed.toInt()))),
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

/** What a session's state is called on screen. */
class ControllerLifecycleLabelTests {
    @Test
    fun everyWireStateHasAWordAndUnknownOnesDoNotLeak() {
        // The three the host uses for a terminal that is running all read the same way.
        val live = lifecycleLabelResource("live")
        assertEquals(live, lifecycleLabelResource("running"))
        assertEquals(live, lifecycleLabelResource("running_app_attached"))
        assertEquals(
            lifecycleLabelResource("exited"),
            lifecycleLabelResource("stopped"),
        )
        // A state this build has never heard of reads as Unknown rather than as itself.
        assertEquals(
            com.multiplex.mobile.R.string.lifecycle_unknown,
            lifecycleLabelResource("something_the_host_added_later"),
        )
        assertEquals(
            com.multiplex.mobile.R.string.lifecycle_provisioning,
            lifecycleLabelResource("provisioning"),
        )
    }
}

/** The colours both halves of the app draw with. */
class SlateColorSchemeTests {
    @Test
    fun bothHalvesDrawInSlateRatherThanMaterialsOwnPalette() {
        for (dark in listOf(false, true)) {
            val theme = if (dark) {
                com.multiplex.mobile.ui.SlateTheme.Dark
            } else {
                com.multiplex.mobile.ui.SlateTheme.Light
            }
            val scheme = com.multiplex.mobile.ui.slateColorScheme(dark)
            assertEquals(
                androidx.compose.ui.graphics.Color(
                    com.multiplex.mobile.ui.SlateTokens.colorActionPrimary(theme),
                ),
                scheme.primary,
            )
            assertEquals(
                androidx.compose.ui.graphics.Color(
                    com.multiplex.mobile.ui.SlateTokens.colorBgCanvas(theme),
                ),
                scheme.background,
            )
            assertEquals(
                androidx.compose.ui.graphics.Color(
                    com.multiplex.mobile.ui.SlateTokens.colorTextMuted(theme),
                ),
                scheme.onSurfaceVariant,
            )
            // Material's stock seed purple must not survive anywhere the app draws with it.
            val stock = if (dark) {
                androidx.compose.material3.darkColorScheme()
            } else {
                androidx.compose.material3.lightColorScheme()
            }
            assertNotEquals(stock.primary, scheme.primary)
            assertNotEquals(stock.secondaryContainer, scheme.secondaryContainer)
        }
    }
}
