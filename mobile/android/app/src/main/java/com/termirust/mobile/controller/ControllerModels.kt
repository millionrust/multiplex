package com.termirust.mobile.controller

import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable

object ControllerLimits {
    const val MAX_HOSTS = 16
    const val MAX_SESSIONS_PER_HOST = 5_000
    const val MAX_SESSIONS_TOTAL = 10_000
    const val MAX_CACHE_BYTES = 8 * 1_024 * 1_024
    const val MAX_PAGE_RECORDS = 1_000
    const val MAX_PAGE_BYTES = 1 * 1_024 * 1_024
    const val MAX_TITLE_CODE_POINTS = 256
}

@Serializable
data class HostRoute(
    val address: String,
    val port: Int,
) {
    init {
        require(address.isNotBlank() && address.toByteArray().size <= 255)
        require(port in 1..65_535)
    }
}

@Serializable
data class PairedHostRecord(
    @SerialName("schema_version") val schemaVersion: Int = 1,
    val id: String,
    @SerialName("display_name") val displayName: String,
    val route: HostRoute,
    @SerialName("host_static_public_key") val hostStaticPublicKey: String,
    @SerialName("device_static_key_id") val deviceStaticKeyId: String,
    @SerialName("device_id") val deviceId: String,
    @SerialName("identity_generation") val identityGeneration: Long,
    @SerialName("revocation_epoch") val revocationEpoch: Long,
    @SerialName("session_generation") val sessionGeneration: Long,
    @SerialName("capability_bits") val capabilityBits: Int,
    @SerialName("paired_at_millis") val pairedAtMillis: Long,
) {
    fun validate() {
        require(schemaVersion == 1)
        require(id.isNotBlank() && id.toByteArray().size <= 256)
        require(displayName.codePointCount() in 1..ControllerLimits.MAX_TITLE_CODE_POINTS)
        require(deviceStaticKeyId.toByteArray().size in 1..128)
        require(identityGeneration > 0 && revocationEpoch >= 0 && sessionGeneration >= 0)
        require(capabilityBits in 0..0x1f)
    }
}

@Serializable
enum class ControllerSessionOrigin {
    @SerialName("terminal") TERMINAL,
    @SerialName("managed_agent") MANAGED_AGENT,
    @SerialName("observed_agent") OBSERVED_AGENT,
    @SerialName("unknown") UNKNOWN,
}

@Serializable
enum class ControllerSessionCapability {
    @SerialName("observe_sessions") OBSERVE_SESSIONS,
    @SerialName("attach_output") ATTACH_OUTPUT,
    @SerialName("send_input") SEND_INPUT,
    @SerialName("resize") RESIZE,
    @SerialName("respond_to_approval") RESPOND_TO_APPROVAL,
}

@Serializable
data class ControllerSessionSummary(
    val id: String,
    @SerialName("host_instance_id") val hostInstanceId: String? = null,
    val origin: ControllerSessionOrigin = ControllerSessionOrigin.UNKNOWN,
    val runtime: String? = null,
    val capabilities: List<ControllerSessionCapability> = emptyList(),
    val title: String,
    val project: String? = null,
    val group: String? = null,
    val lifecycle: String,
    val activity: String? = null,
    @SerialName("occupant_generation") val occupantGeneration: Long? = null,
    @SerialName("last_output_sequence") val lastOutputSequence: Long,
    @SerialName("has_writer") val hasWriter: Boolean,
    @SerialName("unread_count") val unreadCount: Int,
) {
    fun validate() {
        require(id.isUuid())
        require(hostInstanceId == null || hostInstanceId.isUuid())
        require(runtime == null || runtime.toByteArray().size in 1..128)
        require(capabilities.size <= 5 && capabilities.toSet().size == capabilities.size)
        require(title.codePointCount() in 1..ControllerLimits.MAX_TITLE_CODE_POINTS)
        require(project == null || project.codePointCount() <= ControllerLimits.MAX_TITLE_CODE_POINTS)
        require(group == null || group.codePointCount() <= ControllerLimits.MAX_TITLE_CODE_POINTS)
        require(lifecycle.toByteArray().size in 1..64)
        require(activity == null || activity.toByteArray().size <= 64)
        require(occupantGeneration == null || occupantGeneration > 0)
        require(lastOutputSequence >= 0 && unreadCount >= 0)
    }
}

fun ControllerSessionSummary.isOpenTerminal(): Boolean {
    val canAttach = capabilities.isEmpty() || ControllerSessionCapability.ATTACH_OUTPUT in capabilities
    return lifecycle in setOf("live", "running", "running_app_attached") &&
        occupantGeneration != null &&
        canAttach
}

@Serializable
data class ControllerFleetSnapshot(
    val revision: Long,
    @SerialName("update_sequence") val updateSequence: Long,
    val sessions: List<ControllerSessionSummary>,
) {
    fun validate() {
        require(revision > 0 && updateSequence > 0)
        require(sessions.size <= ControllerLimits.MAX_SESSIONS_PER_HOST)
        sessions.forEach(ControllerSessionSummary::validate)
        require(sessions.map { it.id }.toSet().size == sessions.size)
    }
}

@Serializable
data class CachedHostFleet(
    val host: PairedHostRecord,
    val snapshot: ControllerFleetSnapshot,
    @SerialName("updated_at_millis") val updatedAtMillis: Long,
    @SerialName("last_viewed_at_millis") val lastViewedAtMillis: Long,
)

@Serializable
data class ControllerCacheDocument(
    @SerialName("schema_version") val schemaVersion: Int = 1,
    val hosts: Map<String, CachedHostFleet> = emptyMap(),
)

sealed interface ControllerConnectionState {
    data object Unpaired : ControllerConnectionState
    data object Pairing : ControllerConnectionState
    data class SasReady(val sas: String, val fingerprintSuffix: String) : ControllerConnectionState
    data object PairedOffline : ControllerConnectionState
    data object Connecting : ControllerConnectionState
    data object Authenticating : ControllerConnectionState
    data object Syncing : ControllerConnectionState
    data object ReadyReadOnly : ControllerConnectionState
    data object Revoked : ControllerConnectionState
    data object Incompatible : ControllerConnectionState
    data class Failed(val code: String) : ControllerConnectionState
}

data class ControllerUiState(
    val hosts: List<PairedHostRecord> = emptyList(),
    val selectedHostId: String? = null,
    val sessions: List<ControllerSessionSummary> = emptyList(),
    val connection: ControllerConnectionState = ControllerConnectionState.Unpaired,
    val cachedAtMillis: Long? = null,
    val cachedReadOnly: Boolean = false,
    val activeTerminal: ControllerTerminalUiState? = null,
    val selectedRoute: ControllerRemoteRouteKind = ControllerRemoteRouteKind.PRIVATE_NETWORK,
    val routeProjections: List<AndroidControllerRouteProjection> = emptyList(),
    val routeError: String? = null,
)

data class ControllerTerminalUiState(
    val hostTitle: String,
    val sessionTitle: String,
    val attachState: ReadOnlyAttachState,
    val screen: BoundedTerminalSnapshot,
    val outputSequence: Long,
    val hasWriterElsewhere: Boolean = false,
    val writerLease: WriterLeaseState = WriterLeaseState.None,
    val writerMessage: String? = null,
    val pendingPasteBytes: Int = 0,
    val supportsWriter: Boolean = false,
    val supportsResize: Boolean = false,
    val privacyCovered: Boolean = false,
)

data class ControllerPairingChallenge(
    val sas: String,
    val fingerprintSuffix: String,
    val route: HostRoute,
    val expiresAtMillis: Long,
)

private fun String.codePointCount(): Int = codePointCount(0, length)

private fun String.isUuid(): Boolean = runCatching { java.util.UUID.fromString(this) }.isSuccess
