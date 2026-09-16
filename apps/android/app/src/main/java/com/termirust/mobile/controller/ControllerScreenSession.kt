package com.termirust.mobile.controller

import com.termirust.controller.security.ControllerCapability
import com.termirust.screens.ScreenCapability
import com.termirust.screens.ScreenViewer
import java.util.UUID
import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable

/**
 * Watching a paired computer's screen over the Controller channel.
 *
 * The Controller connection carries the bytes; this owns what they mean. A screen session starts
 * with an `open_screen` command, which answers with a one-time ticket, and then rides screen
 * frames: watching traffic under `ObserveScreens`, pointer input under `ControlPointer`, typing
 * under `ControlKeyboard`. The computer checks that claim again on its side, so a frame labelled
 * as watching can never carry a keystroke.
 */
@Serializable
internal data class OpenScreenEnvelope(
    val version: Int = 1,
    @SerialName("command_id") val commandId: String,
    @SerialName("session_generation") val sessionGeneration: Long,
    @SerialName("deadline_millis") val deadlineMillis: Long,
    val command: ScreenCommand,
)

@Serializable
internal data class ScreenCommand(val kind: String)

@Serializable
internal data class ScreenOpenedResponse(
    val kind: String,
    @SerialName("command_id") val commandId: String,
    val ticket: List<Int>,
    @SerialName("can_control_pointer") val canControlPointer: Boolean,
    @SerialName("can_control_keyboard") val canControlKeyboard: Boolean,
)

/** What the computer answered when a screen session was opened. */
data class ControllerScreenTicket(
    val commandId: UUID,
    val ticket: ByteArray,
    val canControlPointer: Boolean,
    val canControlKeyboard: Boolean,
) {
    val canControl: Boolean get() = canControlPointer || canControlKeyboard

    // A ticket is bytes, so identity has to compare them rather than the array reference.
    override fun equals(other: Any?): Boolean {
        if (this === other) return true
        if (other !is ControllerScreenTicket) return false
        return commandId == other.commandId &&
            ticket.contentEquals(other.ticket) &&
            canControlPointer == other.canControlPointer &&
            canControlKeyboard == other.canControlKeyboard
    }

    override fun hashCode(): Int {
        var result = commandId.hashCode()
        result = 31 * result + ticket.contentHashCode()
        result = 31 * result + canControlPointer.hashCode()
        result = 31 * result + canControlKeyboard.hashCode()
        return result
    }
}

sealed class ControllerScreenException(message: String) : Exception(message) {
    /** The response was not the one this command asked for, or carried unexpected fields. */
    object MalformedResponse : ControllerScreenException("malformed screen response")

    /** A ticket is exactly 32 bytes. */
    object InvalidTicket : ControllerScreenException("a screen ticket is 32 bytes")

    /** The device may watch but was never granted what this input needs. */
    object NotGranted : ControllerScreenException("this capability was never granted")
}

object ControllerScreenCommands {
    const val OPEN = "open_screen"
    const val CLOSE = "close_screen"
    const val TICKET_BYTES = 32

    /** Turns a `screen_opened` response into a ticket, rejecting anything that is not one. */
    internal fun ticket(response: ScreenOpenedResponse, expecting: UUID): ControllerScreenTicket {
        if (response.kind != "screen_opened") throw ControllerScreenException.MalformedResponse
        val commandId = runCatching { UUID.fromString(response.commandId) }.getOrNull()
            ?: throw ControllerScreenException.MalformedResponse
        if (commandId != expecting) throw ControllerScreenException.MalformedResponse
        if (response.ticket.size != TICKET_BYTES) throw ControllerScreenException.InvalidTicket
        val bytes = ByteArray(TICKET_BYTES)
        response.ticket.forEachIndexed { index, value ->
            if (value !in 0..255) throw ControllerScreenException.InvalidTicket
            bytes[index] = value.toByte()
        }
        return ControllerScreenTicket(
            commandId = commandId,
            ticket = bytes,
            canControlPointer = response.canControlPointer,
            canControlKeyboard = response.canControlKeyboard,
        )
    }
}

/**
 * Moves bytes between the screen viewer and the Controller connection.
 *
 * The viewer says which capability each outgoing chunk needs; this refuses to send one the device
 * was never granted, so the phone does not ask the computer to close the session on its behalf.
 */
class ControllerScreenPump(private val ticket: ControllerScreenTicket) {
    /** Whether this device may send a chunk that claims [capability]. */
    fun allows(capability: ScreenCapability): Boolean = when (capability) {
        ScreenCapability.OBSERVE -> true
        ScreenCapability.POINTER -> ticket.canControlPointer
        ScreenCapability.KEYBOARD -> ticket.canControlKeyboard
    }

    /** Everything the viewer has queued, ready to seal, in order. */
    fun drain(viewer: ScreenViewer): List<Pair<ControllerCapability, ByteArray>> {
        val frames = mutableListOf<Pair<ControllerCapability, ByteArray>>()
        while (true) {
            val outgoing = viewer.pollOutgoing() ?: return frames
            if (!allows(outgoing.capability)) throw ControllerScreenException.NotGranted
            frames += capability(outgoing.capability) to outgoing.bytes
        }
    }

    companion object {
        /** The Controller capability a screen frame must claim for this chunk. */
        fun capability(outgoing: ScreenCapability): ControllerCapability = when (outgoing) {
            ScreenCapability.OBSERVE -> ControllerCapability.OBSERVE_SCREENS
            ScreenCapability.POINTER -> ControllerCapability.CONTROL_POINTER
            ScreenCapability.KEYBOARD -> ControllerCapability.CONTROL_KEYBOARD
        }
    }
}
