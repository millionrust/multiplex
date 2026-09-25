package com.multiplex.mobile.controller

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test

/** How old the list on screen is said to be. */
class ControllerFreshnessTests {
    @Test
    fun liveAndCachedListsBothSayWhenTheyWereTaken() {
        val nothingYet = ControllerUiState(cachedAtMillis = null)
        assertNull(freshnessLabelResource(nothingYet))

        val live = ControllerUiState(
            cachedAtMillis = 1_000,
            cachedReadOnly = false,
            connection = ControllerConnectionState.ReadyReadOnly,
        )
        assertEquals(com.multiplex.mobile.R.string.live_updated, freshnessLabelResource(live))

        val cached = ControllerUiState(cachedAtMillis = 1_000, cachedReadOnly = true)
        assertEquals(com.multiplex.mobile.R.string.cached_updated, freshnessLabelResource(cached))

        // Offline with a snapshot is not live, whatever the read-only flag says.
        val offline = ControllerUiState(
            cachedAtMillis = 1_000,
            cachedReadOnly = false,
            connection = ControllerConnectionState.PairedOffline,
        )
        assertEquals(com.multiplex.mobile.R.string.cached_updated, freshnessLabelResource(offline))
    }
}

/** What a pairing may store about how to reach a computer. */
class PairedRouteBoundsTests {
    private fun record(routes: List<HostRoute>) = PairedHostRecord(
        id = "host",
        displayName = "mac-studio",
        route = routes.first(),
        hostStaticPublicKey = java.util.Base64.getEncoder().encodeToString(ByteArray(32) { 3 }),
        deviceStaticKeyId = "key",
        deviceId = java.util.UUID.randomUUID().toString(),
        identityGeneration = 1,
        revocationEpoch = 1,
        sessionGeneration = 1,
        capabilityBits = 3,
        pairedAtMillis = 1,
        routes = routes,
    )

    /**
     * A laptop with Wi-Fi, a VPN and a tunnel or two announces more addresses than a record
     * holds, and can announce one of them twice. Both were stored as they arrived, and the
     * record they made was rejected by its own validation: the pairing looked successful, the
     * first connection failed, and the computer disappeared at the next launch.
     */
    @Test
    fun a_computer_with_many_addresses_still_pairs() {
        val many = (1..12).map { HostRoute("192.168.0.$it", 63322) }
        assertThrows(IllegalArgumentException::class.java) { record(many).validate() }
        val kept = many.distinct().take(ControllerLimits.MAX_HOST_ROUTES)
        record(kept).validate()
        assertEquals(ControllerLimits.MAX_HOST_ROUTES, kept.size)
        // The address pairing connected on stays at the front.
        assertEquals(many.first(), kept.first())

        val duplicated = listOf(
            HostRoute("192.168.0.9", 63322),
            HostRoute("192.168.0.9", 63322),
            HostRoute("100.81.17.202", 50198),
        )
        assertThrows(IllegalArgumentException::class.java) { record(duplicated).validate() }
        record(duplicated.distinct()).validate()
    }
}

/** The host's revision is a 64-bit unsigned number, and half of them do not fit a Long. */
class FleetRevisionRangeTests {
    @Test
    fun a_revision_above_two_to_the_sixty_three_is_still_read() {
        // The value a real computer sent when this was found, which overflowed a signed Long.
        val fromTheWire = 15130871412783078093uL
        assertTrue(fromTheWire > Long.MAX_VALUE.toULong())
        val snapshot = ControllerFleetSnapshot(
            revision = fromTheWire,
            updateSequence = fromTheWire,
            sessions = emptyList(),
            capabilityBits = 3,
        )
        snapshot.validate()
        assertEquals(fromTheWire, snapshot.revision)
    }
}

/** What a session is doing, in words rather than in the protocol's. */
class ControllerActivityLabelTests {
    @Test
    fun every_activity_a_computer_sends_reads_as_a_phrase() {
        // The codes in multiplex-controller-listener's `activity_code`.
        assertEquals(com.multiplex.mobile.R.string.activity_idle, activityLabelResource("idle"))
        assertEquals(com.multiplex.mobile.R.string.activity_busy, activityLabelResource("busy"))
        assertEquals(
            com.multiplex.mobile.R.string.activity_needs_input,
            activityLabelResource("needs_input"),
        )
        assertEquals(com.multiplex.mobile.R.string.activity_done, activityLabelResource("done"))
        assertEquals(com.multiplex.mobile.R.string.activity_failed, activityLabelResource("failed"))

        // "unknown" is what every session starts as, and it reached the screen as itself: a row
        // for a freshly opened shell said "unknown".
        assertEquals(
            com.multiplex.mobile.R.string.no_recent_activity,
            activityLabelResource("unknown"),
        )
        assertEquals(com.multiplex.mobile.R.string.no_recent_activity, activityLabelResource(null))
        // A code from a newer computer is not shown raw either.
        assertEquals(
            com.multiplex.mobile.R.string.no_recent_activity,
            activityLabelResource("compacting"),
        )
    }
}
