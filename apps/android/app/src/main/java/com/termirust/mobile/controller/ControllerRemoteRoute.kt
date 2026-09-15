package com.termirust.mobile.controller

import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable

@Serializable
enum class ControllerRemoteRouteKind {
    @SerialName("local_ipc") LOCAL_IPC,
    @SerialName("private_network") PRIVATE_NETWORK,
    @SerialName("ssh") SSH,
    @SerialName("self_hosted_relay") SELF_HOSTED_RELAY;

    companion object {
        val androidRoutes = listOf(PRIVATE_NETWORK, SSH, SELF_HOSTED_RELAY)
    }
}

@Serializable
enum class ControllerRemotePlatform {
    @SerialName("desktop") DESKTOP,
    @SerialName("apple_mobile") APPLE_MOBILE,
    @SerialName("android") ANDROID,
}

@Serializable
enum class ControllerRemoteTrustLayer {
    @SerialName("same_user_os_boundary") SAME_USER_OS_BOUNDARY,
    @SerialName("private_address") PRIVATE_ADDRESS,
    @SerialName("ssh_host_key") SSH_HOST_KEY,
    @SerialName("system_tls") SYSTEM_TLS,
    @SerialName("spki_pin") SPKI_PIN,
    @SerialName("relay_admission") RELAY_ADMISSION,
    @SerialName("controller_authentication") CONTROLLER_AUTHENTICATION,
}

@Serializable
enum class ControllerRemoteConfigurationRequirement {
    @SerialName("private_endpoint") PRIVATE_ENDPOINT,
    @SerialName("ssh_endpoint") SSH_ENDPOINT,
    @SerialName("ssh_credential") SSH_CREDENTIAL,
    @SerialName("relay_endpoint") RELAY_ENDPOINT,
    @SerialName("relay_spki_pin") RELAY_SPKI_PIN,
    @SerialName("relay_credential") RELAY_CREDENTIAL,
    @SerialName("paired_device") PAIRED_DEVICE,
}

@Serializable
enum class ControllerRemoteCapability {
    @SerialName("list_sessions") LIST_SESSIONS,
    @SerialName("attach_output") ATTACH_OUTPUT,
    @SerialName("send_input") SEND_INPUT,
    @SerialName("resize") RESIZE,
    @SerialName("respond_to_approval") RESPOND_TO_APPROVAL,
    @SerialName("detach") DETACH,
}

@Serializable
data class ControllerRemoteRoutePolicy(
    val kind: ControllerRemoteRouteKind,
    val platforms: List<ControllerRemotePlatform>,
    @SerialName("trust_layers") val trustLayers: List<ControllerRemoteTrustLayer>,
    val configuration: List<ControllerRemoteConfigurationRequirement>,
    val capabilities: List<ControllerRemoteCapability>,
    @SerialName("allows_automatic_switch") val allowsAutomaticSwitch: Boolean = false,
    @SerialName("allows_offline_mutations") val allowsOfflineMutations: Boolean = false,
) {
    fun supports(platform: ControllerRemotePlatform) = platform in platforms

    companion object {
        fun canonical(kind: ControllerRemoteRouteKind): ControllerRemoteRoutePolicy {
            val capabilities = ControllerRemoteCapability.entries
            return when (kind) {
                ControllerRemoteRouteKind.LOCAL_IPC -> ControllerRemoteRoutePolicy(
                    kind,
                    listOf(ControllerRemotePlatform.DESKTOP),
                    listOf(ControllerRemoteTrustLayer.SAME_USER_OS_BOUNDARY),
                    emptyList(),
                    capabilities,
                )
                ControllerRemoteRouteKind.PRIVATE_NETWORK -> ControllerRemoteRoutePolicy(
                    kind,
                    ControllerRemotePlatform.entries,
                    listOf(
                        ControllerRemoteTrustLayer.PRIVATE_ADDRESS,
                        ControllerRemoteTrustLayer.CONTROLLER_AUTHENTICATION,
                    ),
                    listOf(
                        ControllerRemoteConfigurationRequirement.PRIVATE_ENDPOINT,
                        ControllerRemoteConfigurationRequirement.PAIRED_DEVICE,
                    ),
                    capabilities,
                )
                ControllerRemoteRouteKind.SSH -> ControllerRemoteRoutePolicy(
                    kind,
                    ControllerRemotePlatform.entries,
                    listOf(
                        ControllerRemoteTrustLayer.SSH_HOST_KEY,
                        ControllerRemoteTrustLayer.CONTROLLER_AUTHENTICATION,
                    ),
                    listOf(
                        ControllerRemoteConfigurationRequirement.SSH_ENDPOINT,
                        ControllerRemoteConfigurationRequirement.SSH_CREDENTIAL,
                        ControllerRemoteConfigurationRequirement.PAIRED_DEVICE,
                    ),
                    capabilities,
                )
                ControllerRemoteRouteKind.SELF_HOSTED_RELAY -> ControllerRemoteRoutePolicy(
                    kind,
                    ControllerRemotePlatform.entries,
                    listOf(
                        ControllerRemoteTrustLayer.SYSTEM_TLS,
                        ControllerRemoteTrustLayer.SPKI_PIN,
                        ControllerRemoteTrustLayer.RELAY_ADMISSION,
                        ControllerRemoteTrustLayer.CONTROLLER_AUTHENTICATION,
                    ),
                    listOf(
                        ControllerRemoteConfigurationRequirement.RELAY_ENDPOINT,
                        ControllerRemoteConfigurationRequirement.RELAY_SPKI_PIN,
                        ControllerRemoteConfigurationRequirement.RELAY_CREDENTIAL,
                        ControllerRemoteConfigurationRequirement.PAIRED_DEVICE,
                    ),
                    capabilities,
                )
            }
        }
    }
}

@Serializable
enum class ControllerRemoteRoutePhase {
    @SerialName("disabled") DISABLED,
    @SerialName("unavailable") UNAVAILABLE,
    @SerialName("idle") IDLE,
    @SerialName("connecting") CONNECTING,
    @SerialName("authenticating") AUTHENTICATING,
    @SerialName("online") ONLINE,
    @SerialName("reconnecting") RECONNECTING,
    @SerialName("degraded") DEGRADED,
    @SerialName("revoked") REVOKED,
}

@Serializable
data class ControllerRemoteRouteState(
    val route: ControllerRemoteRouteKind,
    val phase: ControllerRemoteRoutePhase,
    @SerialName("writer_held") val writerHeld: Boolean = false,
) {
    fun transition(event: ControllerRemoteRouteEvent): ControllerRemoteRouteTransition = when (event) {
        is ControllerRemoteRouteEvent.Enable -> {
            requirePhase(phase == ControllerRemoteRoutePhase.DISABLED || phase == ControllerRemoteRoutePhase.UNAVAILABLE)
            neutral(if (event.available) ControllerRemoteRoutePhase.IDLE else ControllerRemoteRoutePhase.UNAVAILABLE)
        }
        ControllerRemoteRouteEvent.Connect -> {
            requirePhase(phase == ControllerRemoteRoutePhase.IDLE || phase == ControllerRemoteRoutePhase.DEGRADED)
            neutral(ControllerRemoteRoutePhase.CONNECTING)
        }
        ControllerRemoteRouteEvent.TransportReady -> {
            requirePhase(phase == ControllerRemoteRoutePhase.CONNECTING || phase == ControllerRemoteRoutePhase.RECONNECTING)
            neutral(ControllerRemoteRoutePhase.AUTHENTICATING)
        }
        ControllerRemoteRouteEvent.Authenticated -> {
            requirePhase(phase == ControllerRemoteRoutePhase.AUTHENTICATING)
            neutral(ControllerRemoteRoutePhase.ONLINE, writerHeld)
        }
        is ControllerRemoteRouteEvent.Failure -> {
            requirePhase(transportIsActive)
            ControllerRemoteRouteTransition(
                state = copy(
                    phase = if (event.retryable) ControllerRemoteRoutePhase.RECONNECTING else ControllerRemoteRoutePhase.DEGRADED,
                    writerHeld = false,
                ),
                disconnectTransport = true,
                clearPendingInput = true,
                releaseWriter = writerHeld,
                retryIdempotentReads = event.retryable,
                mutationDisposition = if (event.mutationInFlight) {
                    ControllerRemoteMutationDisposition.QUERY_BY_COMMAND_ID
                } else {
                    ControllerRemoteMutationDisposition.NONE
                },
                requiresExplicitAction = !event.retryable,
            )
        }
        ControllerRemoteRouteEvent.AvailabilityLost -> {
            requirePhase(phase != ControllerRemoteRoutePhase.DISABLED && phase != ControllerRemoteRoutePhase.REVOKED)
            cleanup(ControllerRemoteRoutePhase.UNAVAILABLE, explicit = true)
        }
        is ControllerRemoteRouteEvent.AuthorizationRestored -> {
            requirePhase(phase == ControllerRemoteRoutePhase.REVOKED)
            neutral(if (event.available) ControllerRemoteRoutePhase.IDLE else ControllerRemoteRoutePhase.UNAVAILABLE)
        }
        ControllerRemoteRouteEvent.Retry -> {
            requirePhase(phase == ControllerRemoteRoutePhase.DEGRADED)
            neutral(ControllerRemoteRoutePhase.CONNECTING)
        }
        ControllerRemoteRouteEvent.Cancel -> {
            requirePhase(phase in cancellablePhases)
            cleanup(ControllerRemoteRoutePhase.IDLE, explicit = false)
        }
        ControllerRemoteRouteEvent.Revoke -> cleanup(ControllerRemoteRoutePhase.REVOKED, explicit = true)
        ControllerRemoteRouteEvent.Disable -> cleanup(ControllerRemoteRoutePhase.DISABLED, explicit = true)
    }

    fun switchTo(
        target: ControllerRemoteRouteKind,
        platform: ControllerRemotePlatform,
        targetAvailable: Boolean,
        explicitlyConfirmed: Boolean,
    ): ControllerRemoteSwitchDecision {
        if (!explicitlyConfirmed) fail(ControllerRemoteRouteError.EXPLICIT_CONFIRMATION_REQUIRED)
        if (route == target) fail(ControllerRemoteRouteError.SAME_ROUTE)
        if (!ControllerRemoteRoutePolicy.canonical(target).supports(platform)) {
            fail(ControllerRemoteRouteError.UNSUPPORTED_PLATFORM)
        }
        if (!targetAvailable) fail(ControllerRemoteRouteError.TARGET_UNAVAILABLE)
        return ControllerRemoteSwitchDecision(
            from = route,
            to = target,
            disconnectSource = transportIsActive,
            clearPendingInput = true,
            releaseWriter = writerHeld,
        )
    }

    fun mutationDisposition(completion: ControllerRemoteMutationCompletion) = when (completion) {
        ControllerRemoteMutationCompletion.NOT_SENT -> if (phase == ControllerRemoteRoutePhase.ONLINE) {
            ControllerRemoteMutationDisposition.MAY_SEND
        } else {
            ControllerRemoteMutationDisposition.DO_NOT_REPLAY
        }
        ControllerRemoteMutationCompletion.UNKNOWN -> ControllerRemoteMutationDisposition.QUERY_BY_COMMAND_ID
        ControllerRemoteMutationCompletion.ACKNOWLEDGED,
        ControllerRemoteMutationCompletion.REJECTED,
        -> ControllerRemoteMutationDisposition.DO_NOT_REPLAY
    }

    private fun neutral(next: ControllerRemoteRoutePhase, held: Boolean = false): ControllerRemoteRouteTransition {
        val state = copy(phase = next, writerHeld = held)
        return ControllerRemoteRouteTransition(state, terminalAllowed = next == ControllerRemoteRoutePhase.ONLINE)
    }

    private fun cleanup(next: ControllerRemoteRoutePhase, explicit: Boolean) = ControllerRemoteRouteTransition(
        state = copy(phase = next, writerHeld = false),
        disconnectTransport = transportIsActive,
        clearPendingInput = true,
        releaseWriter = writerHeld,
        requiresExplicitAction = explicit,
    )

    private fun requirePhase(valid: Boolean) {
        if (!valid) fail(ControllerRemoteRouteError.INVALID_TRANSITION)
    }

    private val transportIsActive: Boolean
        get() = phase in setOf(
            ControllerRemoteRoutePhase.CONNECTING,
            ControllerRemoteRoutePhase.AUTHENTICATING,
            ControllerRemoteRoutePhase.ONLINE,
            ControllerRemoteRoutePhase.RECONNECTING,
        )

    private companion object {
        val cancellablePhases = setOf(
            ControllerRemoteRoutePhase.CONNECTING,
            ControllerRemoteRoutePhase.AUTHENTICATING,
            ControllerRemoteRoutePhase.ONLINE,
            ControllerRemoteRoutePhase.RECONNECTING,
            ControllerRemoteRoutePhase.DEGRADED,
        )
    }
}

sealed interface ControllerRemoteRouteEvent {
    data class Enable(val available: Boolean) : ControllerRemoteRouteEvent
    data object Connect : ControllerRemoteRouteEvent
    data object TransportReady : ControllerRemoteRouteEvent
    data object Authenticated : ControllerRemoteRouteEvent
    data class Failure(val retryable: Boolean, val mutationInFlight: Boolean) : ControllerRemoteRouteEvent
    data object AvailabilityLost : ControllerRemoteRouteEvent
    data class AuthorizationRestored(val available: Boolean) : ControllerRemoteRouteEvent
    data object Retry : ControllerRemoteRouteEvent
    data object Cancel : ControllerRemoteRouteEvent
    data object Revoke : ControllerRemoteRouteEvent
    data object Disable : ControllerRemoteRouteEvent
}

@Serializable
enum class ControllerRemoteMutationDisposition {
    @SerialName("none") NONE,
    @SerialName("may_send") MAY_SEND,
    @SerialName("do_not_replay") DO_NOT_REPLAY,
    @SerialName("query_by_command_id") QUERY_BY_COMMAND_ID,
}

@Serializable
enum class ControllerRemoteMutationCompletion {
    @SerialName("not_sent") NOT_SENT,
    @SerialName("acknowledged") ACKNOWLEDGED,
    @SerialName("unknown") UNKNOWN,
    @SerialName("rejected") REJECTED,
}

@Serializable
data class ControllerRemoteRouteTransition(
    val state: ControllerRemoteRouteState,
    @SerialName("terminal_allowed") val terminalAllowed: Boolean = false,
    @SerialName("disconnect_transport") val disconnectTransport: Boolean = false,
    @SerialName("clear_pending_input") val clearPendingInput: Boolean = false,
    @SerialName("release_writer") val releaseWriter: Boolean = false,
    @SerialName("retry_idempotent_reads") val retryIdempotentReads: Boolean = false,
    @SerialName("mutation_disposition") val mutationDisposition: ControllerRemoteMutationDisposition = ControllerRemoteMutationDisposition.NONE,
    @SerialName("requires_explicit_action") val requiresExplicitAction: Boolean = false,
)

@Serializable
data class ControllerRemoteSwitchDecision(
    val from: ControllerRemoteRouteKind,
    val to: ControllerRemoteRouteKind,
    @SerialName("disconnect_source") val disconnectSource: Boolean,
    @SerialName("clear_pending_input") val clearPendingInput: Boolean,
    @SerialName("release_writer") val releaseWriter: Boolean,
    val automatic: Boolean = false,
)

enum class ControllerRemoteRouteError {
    INVALID_TRANSITION,
    EXPLICIT_CONFIRMATION_REQUIRED,
    SAME_ROUTE,
    UNSUPPORTED_PLATFORM,
    TARGET_UNAVAILABLE,
}

class ControllerRemoteRouteException(val reason: ControllerRemoteRouteError) : IllegalStateException(reason.name)

private fun fail(error: ControllerRemoteRouteError): Nothing = throw ControllerRemoteRouteException(error)
