package com.multiplex.mobile.controller

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
    const val MAX_HOST_ROUTES = 8
    const val MAX_REMEMBERED_NETWORKS = 8

    /**
     * Every capability bit this build understands: the five session ones, the three screen ones,
     * and starting a terminal. A record is rejected above this, never below it — a computer may
     * grant watching, control or terminal creation at any time, and a phone that called that
     * record invalid would refuse to talk to the computer at all, terminals included.
     *
     * [ControllerConnection.ALL_SUPPORTED_CAPABILITIES] spells out which bits these are, and a
     * test holds the two to the same value.
     */
    const val ALL_CAPABILITY_BITS = 0x1ff
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

/** The address that last connected on one network, named by the route planner's fingerprint. */
@Serializable
data class RememberedRouteRecord(
    val fingerprint: String,
    val address: String,
    val port: Int,
) {
    val route: HostRoute get() = HostRoute(address, port)

    fun validate() {
        // Building the route checks the address and port.
        require(ROUTE_FINGERPRINT_PATTERN.matches(fingerprint) && route.port == port)
    }
}

@Serializable
data class PairedHostRecord(
    @SerialName("schema_version") val schemaVersion: Int = 1,
    val id: String,
    @SerialName("display_name") val displayName: String,
    // The preferred route; always the first entry of `routes`, kept so older app builds can
    // still read the record.
    val route: HostRoute,
    @SerialName("host_static_public_key") val hostStaticPublicKey: String,
    @SerialName("device_static_key_id") val deviceStaticKeyId: String,
    @SerialName("device_id") val deviceId: String,
    @SerialName("identity_generation") val identityGeneration: Long,
    @SerialName("revocation_epoch") val revocationEpoch: Long,
    @SerialName("session_generation") val sessionGeneration: Long,
    @SerialName("capability_bits") val capabilityBits: Int,
    @SerialName("paired_at_millis") val pairedAtMillis: Long,
    // Records saved before multiple routes existed only carry `route`.
    val routes: List<HostRoute> = listOf(route),
    @SerialName("discovery_id") val discoveryId: String? = null,
    // Which address worked on each network the phone connected from, newest first.
    @SerialName("route_memory") val routeMemory: List<RememberedRouteRecord> = emptyList(),
) {
    fun validate() {
        require(schemaVersion == 1)
        require(id.isNotBlank() && id.toByteArray().size <= 256)
        require(displayName.codePointCount() in 1..ControllerLimits.MAX_TITLE_CODE_POINTS)
        require(deviceStaticKeyId.toByteArray().size in 1..128)
        require(identityGeneration > 0 && revocationEpoch >= 0 && sessionGeneration >= 0)
        require(capabilityBits in 0..ControllerLimits.ALL_CAPABILITY_BITS)
        require(routes.size in 1..ControllerLimits.MAX_HOST_ROUTES && routes.first() == route)
        require(routes.toSet().size == routes.size)
        require(discoveryId == null || DISCOVERY_ID_PATTERN.matches(discoveryId))
        require(routeMemory.size <= ControllerLimits.MAX_REMEMBERED_NETWORKS)
        routeMemory.forEach(RememberedRouteRecord::validate)
    }

    // Moves `preferred` to the front, adding it when new and dropping overflow from the end.
    fun preferringRoute(preferred: HostRoute): PairedHostRecord {
        val next = (listOf(preferred) + routes.filterNot { it == preferred })
            .take(ControllerLimits.MAX_HOST_ROUTES)
        return copy(route = preferred, routes = next)
    }
}

internal val DISCOVERY_ID_PATTERN = Regex("^[0-9a-f]{32}$")
internal val ROUTE_FINGERPRINT_PATTERN = Regex("^[0-9a-f]{32}$")

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
    /**
     * The host's revision, which is a 64-bit unsigned value on the wire.
     *
     * It was read into a signed Long, so a computer whose revision happened to land above
     * 2^63 — the host derives it by hashing, so about half of them do — failed every session
     * listing with a numeric overflow the phone reported as invalid_data.
     */
    val revision: ULong,
    @SerialName("update_sequence") val updateSequence: ULong,
    val sessions: List<ControllerSessionSummary>,
    @SerialName("capability_bits") val capabilityBits: Int = 0,
) {
    fun validate() {
        require(revision > 0uL && updateSequence > 0uL)
        require(capabilityBits in 0..ControllerLimits.ALL_CAPABILITY_BITS)
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

/**
 * What the phone can honestly say about a computer it is not connected to.
 *
 * Only one computer has a connection at a time, so every other row in the list is answered from
 * the cache: how many terminals were open when the phone last looked, and when that was. Nothing
 * here is live, and the row says so.
 */
data class HostGlance(
    val openTerminals: Int,
    val updatedAtMillis: Long,
)

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
    /** How the last connection went, when the route planner has something to say about it. */
    val routeAdvice: String? = null,
    /** What the cache remembers about each computer, keyed by host id. */
    val glances: Map<String, HostGlance> = emptyMap(),
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

sealed interface CodePairingTarget {
    data class Discovered(val computer: DiscoveredController) : CodePairingTarget
    data class Address(val text: String) : CodePairingTarget
}

private fun String.codePointCount(): Int = codePointCount(0, length)

private fun String.isUuid(): Boolean = runCatching { java.util.UUID.fromString(this) }.isSuccess
