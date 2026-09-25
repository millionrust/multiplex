package com.multiplex.mobile.controller

import android.graphics.Bitmap
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlin.math.min

/** Why this phone is not showing a computer's screen. */
sealed class ControllerScreenUnavailable {
    /** The computer never gave this device screen access. */
    object NotGranted : ControllerScreenUnavailable()

    /** The computer is sharing nothing, or the session ended. */
    /** The computer is not sharing its screen at all; no grant makes a picture appear. */
    object SharingOff : ControllerScreenUnavailable()

    /** Watching stopped and reopening it did not help. */
    object Stopped : ControllerScreenUnavailable()
}

/**
 * Owns the phone's one screen session: the small preview on a computer's page, and the full
 * viewer it opens into.
 *
 * A phone holds one Controller connection at a time, so a preview and a viewer are the same
 * session in two shapes, and starting either ends the other. The last picture of each computer is
 * kept after its session ends, so the device list can show what a computer looked like without
 * holding a connection open to every computer at once.
 */
class ControllerScreenCoordinator(
    private val scope: CoroutineScope,
    /** How long to wait before opening a dropped session again. A test shortens it. */
    private val backoffMillis: (Int) -> Long = ::defaultBackoffMillis,
) {
    /** The small thumbnail on a computer's page, about one picture a second. */
    var preview: RemoteScreenModel? by mutableStateOf(null)
        private set

    /** The full-size screen, once someone opens it. */
    var viewer: RemoteScreenModel? by mutableStateOf(null)
        private set

    /** The last picture seen for each computer, keyed by host id. */
    var lastPictures: Map<String, Bitmap> by mutableStateOf(emptyMap())
        private set

    var unavailable: ControllerScreenUnavailable? by mutableStateOf(null)
        private set

    /** Set while a dropped session is being opened again. The last picture stays on screen. */
    var reconnecting: Boolean by mutableStateOf(false)
        private set

    var reconnectAttempt: Int by mutableStateOf(0)
        private set

    private var session: Job? = null
    private var watchingHost: PairedHostRecord? = null

    val isWatching: Boolean get() = session?.isActive == true

    /** Starts the one-picture-a-second preview for a computer's page. */
    fun startPreview(host: PairedHostRecord, connection: ControllerConnecting) {
        start(host, connection, wantsPreview = true)
    }

    /**
     * Opens the full screen, on `surface` when the person picked one from the computer's displays.
     * The preview, if any, ends: there is one connection.
     */
    fun openViewer(host: PairedHostRecord, connection: ControllerConnecting, surface: UInt? = null) {
        start(host, connection, wantsPreview = false, surface = surface)
    }

    /** Ends whatever session is running and keeps the last picture. */
    fun stop() {
        session?.cancel()
        session = null
        watchingHost = null
        preview = null
        viewer = null
        reconnecting = false
        reconnectAttempt = 0
    }

    private fun start(
        host: PairedHostRecord,
        connection: ControllerConnecting,
        wantsPreview: Boolean,
        surface: UInt? = null,
    ) {
        if (!mayWatch(host)) {
            unavailable = ControllerScreenUnavailable.NotGranted
            return
        }
        val previous = session
        session = null
        preview = null
        viewer = null
        unavailable = null
        reconnecting = false
        reconnectAttempt = 0
        watchingHost = host
        session = scope.launch {
            // The session being replaced holds the connection's serialization lock while it
            // blocks on a socket read, and cancelling its coroutine does not unblock that read.
            // Closing the socket does, which is what `cancel` is for. Without this, opening the
            // full screen from the running preview waited for a lock that was never released:
            // no picture, no error, nothing.
            if (previous != null) {
                previous.cancel()
                runCatching { connection.cancel() }
                previous.join()
            }
            // A screen session is a long-lived connection and a phone loses those: it changes
            // network, sleeps, or walks out of range. The last picture stays on screen while
            // this reopens it, because a frozen picture of the right computer says more than an
            // empty one.
            while (true) {
                try {
                    connection.watchScreen(
                        host = host,
                        surface = surface,
                        preview = wantsPreview,
                        onOpened = { ticket, screenViewer ->
                            val model = RemoteScreenModel(
                                viewer = screenViewer,
                                surface = surface,
                                ticket = ticket,
                                preview = wantsPreview,
                            )
                            if (wantsPreview) preview = model else viewer = model
                        },
                        onEvent = { events -> apply(events, host.id) },
                    )
                    return@launch
                } catch (cancelled: CancellationException) {
                    throw cancelled
                } catch (error: Throwable) {
                    if (!dropped(error)) return@launch
                }
                delay(backoffMillis(reconnectAttempt))
            }
        }
    }

    private fun apply(events: List<com.multiplex.screens.ScreenEvent>, hostId: String) {
        val model = viewer ?: preview ?: return
        model.apply(events)
        val picture = model.picture ?: return
        lastPictures = lastPictures + (hostId to picture)
        // Pictures are arriving again, so the session is back.
        reconnecting = false
        reconnectAttempt = 0
    }

    /** Records a dropped session. Returns whether it is worth opening again. */
    private fun dropped(error: Throwable): Boolean {
        // The computer answered by name: it is not sharing its screen. Nothing on this phone can
        // change that, and opening it again only asks the same question, so this says so instead
        // of retrying behind a spinner that never resolves.
        if (error is ControllerConnectionException.HostError && error.code == SHARING_OFF) {
            unavailable = ControllerScreenUnavailable.SharingOff
            reconnecting = false
            preview = null
            viewer = null
            return false
        }
        if (error is ControllerConnectionException.CapabilityDenied) {
            // The computer took screen access away; trying again would only be refused.
            unavailable = ControllerScreenUnavailable.NotGranted
            reconnecting = false
            preview = null
            viewer = null
            return false
        }
        reconnectAttempt += 1
        if (reconnectAttempt > MAXIMUM_RECONNECT_ATTEMPTS) {
            unavailable = ControllerScreenUnavailable.Stopped
            reconnecting = false
            preview = null
            viewer = null
            return false
        }
        // The models stay, so the last picture stays on screen while this reconnects.
        reconnecting = true
        return true
    }

    companion object {
        /** The capability bit a computer grants before this phone may watch it at all. */
        const val OBSERVE_SCREENS_CAPABILITY = 1 shl 5

        /**
         * After this many failures in a row the phone stops and says so, rather than draining
         * the battery against a computer that is not coming back.
         */
        const val MAXIMUM_RECONNECT_ATTEMPTS = 5

        /** What the listener answers `OpenScreen` with when this computer shares no screen. */
        const val SHARING_OFF = "screen_sharing_off"

        /** Whether [host] has given this phone screen access. */
        fun mayWatch(host: PairedHostRecord): Boolean =
            host.capabilityBits and OBSERVE_SCREENS_CAPABILITY == OBSERVE_SCREENS_CAPABILITY

        fun defaultBackoffMillis(attempt: Int): Long =
            min(500L * (1L shl maxOf(attempt - 1, 0)), 8_000L)
    }
}
