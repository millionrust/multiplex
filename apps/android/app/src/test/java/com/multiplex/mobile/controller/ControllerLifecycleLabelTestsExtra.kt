package com.multiplex.mobile.controller

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
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
