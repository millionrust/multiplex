package com.termirust.mobile.controller

import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable

enum class MobileTerminalRoute(val wireValue: String) {
    DIRECT_SSH("direct_ssh"),
    DEVICE_SESSION("device_session"),
}

enum class MobileRouteEvent(val wireValue: String) {
    CONNECT("connect"),
    FAILURE("failure"),
    CANCEL("cancel"),
    BACKGROUND("background"),
    RECONNECT("reconnect"),
    ROUTE_SWITCH("route_switch"),
    HOST_KEY_MISMATCH("host_key_mismatch"),
    MISSING_TMUX("missing_tmux"),
    AUTHORITY_REVOKED("authority_revoked"),
}

@Serializable
enum class MobileContinuityMode {
    @SerialName("none") NONE,
    @SerialName("normal_shell") NORMAL_SHELL,
    @SerialName("remote_tmux") REMOTE_TMUX,
    @SerialName("host_service") HOST_SERVICE,
}

@Serializable
enum class MobileAcceptanceStatus {
    @SerialName("connected") CONNECTED,
    @SerialName("fallback_shell") FALLBACK_SHELL,
    @SerialName("host_key_blocked") HOST_KEY_BLOCKED,
    @SerialName("cancelled") CANCELLED,
    @SerialName("backgrounded") BACKGROUNDED,
    @SerialName("host_offline") HOST_OFFLINE,
    @SerialName("authority_revoked") AUTHORITY_REVOKED,
    @SerialName("route_switched") ROUTE_SWITCHED,
}

@Serializable
data class MobileCrossRouteDecision(
    @SerialName("terminal_allowed") val terminalAllowed: Boolean,
    @SerialName("continuity_mode") val continuityMode: MobileContinuityMode,
    @SerialName("fallback_to_normal_shell") val fallbackToNormalShell: Boolean,
    @SerialName("cancel_connection") val cancelConnection: Boolean,
    @SerialName("disconnect_transport") val disconnectTransport: Boolean,
    @SerialName("cover_privacy") val coverPrivacy: Boolean,
    @SerialName("clear_pending_input") val clearPendingInput: Boolean,
    @SerialName("release_writer") val releaseWriter: Boolean,
    @SerialName("retain_terminal_output") val retainTerminalOutput: Boolean,
    @SerialName("replay_terminal_output") val replayTerminalOutput: Boolean,
    @SerialName("replay_terminal_input") val replayTerminalInput: Boolean,
    @SerialName("next_destination") val nextDestination: String?,
    val status: MobileAcceptanceStatus,
)

class MobileCrossRouteAcceptanceException : IllegalArgumentException("unsupported mobile route event")

object MobileCrossRouteAcceptance {
    fun decide(
        route: MobileTerminalRoute,
        event: MobileRouteEvent,
        tmuxEnabled: Boolean,
        tmuxAvailable: Boolean,
        hostKeyMatches: Boolean,
        authorityValid: Boolean,
        writerHeld: Boolean,
        @Suppress("UNUSED_PARAMETER") pendingInput: Boolean,
    ): MobileCrossRouteDecision {
        if (event == MobileRouteEvent.HOST_KEY_MISMATCH || event == MobileRouteEvent.MISSING_TMUX) {
            if (route != MobileTerminalRoute.DIRECT_SSH) throw MobileCrossRouteAcceptanceException()
        }
        if (event == MobileRouteEvent.AUTHORITY_REVOKED && route != MobileTerminalRoute.DEVICE_SESSION) {
            throw MobileCrossRouteAcceptanceException()
        }

        val continuity = continuityMode(route, tmuxEnabled, tmuxAvailable)
        return when (event) {
            MobileRouteEvent.CONNECT, MobileRouteEvent.RECONNECT -> {
                if (route == MobileTerminalRoute.DIRECT_SSH && !hostKeyMatches) return blockedHostKey()
                if (route == MobileTerminalRoute.DEVICE_SESSION && !authorityValid) return revoked(writerHeld)
                MobileCrossRouteDecision(
                    terminalAllowed = true,
                    continuityMode = continuity,
                    fallbackToNormalShell = false,
                    cancelConnection = false,
                    disconnectTransport = false,
                    coverPrivacy = false,
                    clearPendingInput = false,
                    releaseWriter = false,
                    retainTerminalOutput = true,
                    replayTerminalOutput = event == MobileRouteEvent.RECONNECT && route == MobileTerminalRoute.DEVICE_SESSION,
                    replayTerminalInput = false,
                    nextDestination = null,
                    status = MobileAcceptanceStatus.CONNECTED,
                )
            }
            MobileRouteEvent.MISSING_TMUX -> MobileCrossRouteDecision(
                terminalAllowed = true,
                continuityMode = MobileContinuityMode.NORMAL_SHELL,
                fallbackToNormalShell = true,
                cancelConnection = false,
                disconnectTransport = false,
                coverPrivacy = false,
                clearPendingInput = false,
                releaseWriter = false,
                retainTerminalOutput = true,
                replayTerminalOutput = false,
                replayTerminalInput = false,
                nextDestination = null,
                status = MobileAcceptanceStatus.FALLBACK_SHELL,
            )
            MobileRouteEvent.HOST_KEY_MISMATCH -> blockedHostKey()
            MobileRouteEvent.AUTHORITY_REVOKED -> revoked(writerHeld)
            MobileRouteEvent.FAILURE -> inactive(
                continuity = continuity,
                coverPrivacy = false,
                releaseWriter = false,
                retainOutput = true,
                nextDestination = null,
                status = MobileAcceptanceStatus.HOST_OFFLINE,
            )
            MobileRouteEvent.CANCEL -> inactive(
                continuity = if (route == MobileTerminalRoute.DIRECT_SSH) MobileContinuityMode.NONE else MobileContinuityMode.HOST_SERVICE,
                coverPrivacy = false,
                releaseWriter = false,
                retainOutput = true,
                nextDestination = null,
                status = MobileAcceptanceStatus.CANCELLED,
            )
            MobileRouteEvent.BACKGROUND -> inactive(
                continuity = continuity,
                coverPrivacy = true,
                releaseWriter = route == MobileTerminalRoute.DEVICE_SESSION && writerHeld,
                retainOutput = true,
                nextDestination = null,
                status = MobileAcceptanceStatus.BACKGROUNDED,
            )
            MobileRouteEvent.ROUTE_SWITCH -> inactive(
                continuity = continuity,
                coverPrivacy = true,
                releaseWriter = route == MobileTerminalRoute.DEVICE_SESSION && writerHeld,
                retainOutput = true,
                nextDestination = if (route == MobileTerminalRoute.DIRECT_SSH) "devices" else "connections",
                status = MobileAcceptanceStatus.ROUTE_SWITCHED,
            )
        }
    }

    private fun continuityMode(
        route: MobileTerminalRoute,
        tmuxEnabled: Boolean,
        tmuxAvailable: Boolean,
    ) = when (route) {
        MobileTerminalRoute.DEVICE_SESSION -> MobileContinuityMode.HOST_SERVICE
        MobileTerminalRoute.DIRECT_SSH -> {
            if (tmuxEnabled && tmuxAvailable) MobileContinuityMode.REMOTE_TMUX else MobileContinuityMode.NORMAL_SHELL
        }
    }

    private fun inactive(
        continuity: MobileContinuityMode,
        coverPrivacy: Boolean,
        releaseWriter: Boolean,
        retainOutput: Boolean,
        nextDestination: String?,
        status: MobileAcceptanceStatus,
    ) = MobileCrossRouteDecision(
        terminalAllowed = false,
        continuityMode = continuity,
        fallbackToNormalShell = false,
        cancelConnection = true,
        disconnectTransport = true,
        coverPrivacy = coverPrivacy,
        clearPendingInput = true,
        releaseWriter = releaseWriter,
        retainTerminalOutput = retainOutput,
        replayTerminalOutput = false,
        replayTerminalInput = false,
        nextDestination = nextDestination,
        status = status,
    )

    private fun blockedHostKey() = MobileCrossRouteDecision(
        terminalAllowed = false,
        continuityMode = MobileContinuityMode.NONE,
        fallbackToNormalShell = false,
        cancelConnection = true,
        disconnectTransport = true,
        coverPrivacy = false,
        clearPendingInput = true,
        releaseWriter = false,
        retainTerminalOutput = false,
        replayTerminalOutput = false,
        replayTerminalInput = false,
        nextDestination = null,
        status = MobileAcceptanceStatus.HOST_KEY_BLOCKED,
    )

    private fun revoked(writerHeld: Boolean) = MobileCrossRouteDecision(
        terminalAllowed = false,
        continuityMode = MobileContinuityMode.HOST_SERVICE,
        fallbackToNormalShell = false,
        cancelConnection = true,
        disconnectTransport = true,
        coverPrivacy = true,
        clearPendingInput = true,
        releaseWriter = writerHeld,
        retainTerminalOutput = false,
        replayTerminalOutput = false,
        replayTerminalInput = false,
        nextDestination = null,
        status = MobileAcceptanceStatus.AUTHORITY_REVOKED,
    )
}
