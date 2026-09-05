package com.termirust.mobile.controller

enum class MobileItemKind(val wireValue: String) {
    SAVED_CONNECTION("saved_connection"),
    PAIRED_DEVICE("paired_device"),
    DURABLE_DEVICE_SESSION("durable_device_session"),
}

enum class MobileCredentialOwner(val wireValue: String) {
    SSH_CREDENTIAL("ssh_credential"),
    DEVICE_PAIRING_IDENTITY("device_pairing_identity"),
}

enum class MobileContinuityOwner(val wireValue: String) {
    NONE("none"),
    REMOTE_TMUX_IF_ENABLED("remote_tmux_if_enabled"),
    HOST_SERVICE("host_service"),
}

enum class MobileRouteCapability(val wireValue: String) {
    LIST_DEVICE_SESSIONS("list_device_sessions"),
    TERMINAL_OUTPUT("terminal_output"),
    TERMINAL_INPUT("terminal_input"),
    TERMINAL_RESIZE("terminal_resize"),
    PERSISTENT_TMUX("persistent_tmux"),
    DURABLE_REPLAY("durable_replay"),
    AUTHORITATIVE_ACTIVITY("authoritative_activity"),
    SINGLE_WRITER("single_writer"),
}

enum class MobileRouteContractError(val wireValue: String) {
    UNKNOWN_ITEM_KIND("unknown_item_kind"),
    UNKNOWN_CAPABILITY("unknown_capability"),
    CREDENTIAL_OWNER_MISMATCH("credential_owner_mismatch"),
    CONTINUITY_OWNER_MISMATCH("continuity_owner_mismatch"),
    CAPABILITY_MISMATCH("capability_mismatch"),
    TERMINAL_OWNERSHIP_MISMATCH("terminal_ownership_mismatch"),
}

class MobileRouteContractException(val reason: MobileRouteContractError) : IllegalArgumentException(reason.wireValue)

data class MobileRouteProjection(
    val itemKind: MobileItemKind,
    val credentialOwner: MobileCredentialOwner,
    val continuityOwner: MobileContinuityOwner,
    val capabilities: Set<MobileRouteCapability>,
    val canOpenTerminal: Boolean,
) {
    companion object {
        fun validated(
            itemKind: String,
            credentialOwner: String,
            continuityOwner: String,
            capabilities: List<String>,
            canOpenTerminal: Boolean,
        ): MobileRouteProjection {
            val kind = MobileItemKind.entries.firstOrNull { it.wireValue == itemKind }
                ?: fail(MobileRouteContractError.UNKNOWN_ITEM_KIND)
            val credential = MobileCredentialOwner.entries.firstOrNull { it.wireValue == credentialOwner }
                ?: fail(MobileRouteContractError.CREDENTIAL_OWNER_MISMATCH)
            val continuity = MobileContinuityOwner.entries.firstOrNull { it.wireValue == continuityOwner }
                ?: fail(MobileRouteContractError.CONTINUITY_OWNER_MISMATCH)
            val parsed = capabilities.map { raw ->
                MobileRouteCapability.entries.firstOrNull { it.wireValue == raw }
                    ?: fail(MobileRouteContractError.UNKNOWN_CAPABILITY)
            }.toSet()
            if (parsed.size != capabilities.size) fail(MobileRouteContractError.CAPABILITY_MISMATCH)

            val expectedCredential: MobileCredentialOwner
            val expectedContinuity: MobileContinuityOwner
            val required: Set<MobileRouteCapability>
            val allowed: Set<MobileRouteCapability>
            val expectedTerminal: Boolean
            when (kind) {
                MobileItemKind.SAVED_CONNECTION -> {
                    expectedCredential = MobileCredentialOwner.SSH_CREDENTIAL
                    expectedContinuity = MobileContinuityOwner.REMOTE_TMUX_IF_ENABLED
                    required = setOf(MobileRouteCapability.TERMINAL_OUTPUT)
                    allowed = setOf(
                        MobileRouteCapability.TERMINAL_OUTPUT,
                        MobileRouteCapability.TERMINAL_INPUT,
                        MobileRouteCapability.TERMINAL_RESIZE,
                        MobileRouteCapability.PERSISTENT_TMUX,
                    )
                    expectedTerminal = true
                }
                MobileItemKind.PAIRED_DEVICE -> {
                    expectedCredential = MobileCredentialOwner.DEVICE_PAIRING_IDENTITY
                    expectedContinuity = MobileContinuityOwner.NONE
                    required = setOf(MobileRouteCapability.LIST_DEVICE_SESSIONS)
                    allowed = required
                    expectedTerminal = false
                }
                MobileItemKind.DURABLE_DEVICE_SESSION -> {
                    expectedCredential = MobileCredentialOwner.DEVICE_PAIRING_IDENTITY
                    expectedContinuity = MobileContinuityOwner.HOST_SERVICE
                    required = setOf(
                        MobileRouteCapability.TERMINAL_OUTPUT,
                        MobileRouteCapability.DURABLE_REPLAY,
                        MobileRouteCapability.AUTHORITATIVE_ACTIVITY,
                        MobileRouteCapability.SINGLE_WRITER,
                    )
                    allowed = setOf(
                        MobileRouteCapability.TERMINAL_OUTPUT,
                        MobileRouteCapability.TERMINAL_INPUT,
                        MobileRouteCapability.TERMINAL_RESIZE,
                        MobileRouteCapability.DURABLE_REPLAY,
                        MobileRouteCapability.AUTHORITATIVE_ACTIVITY,
                        MobileRouteCapability.SINGLE_WRITER,
                    )
                    expectedTerminal = true
                }
            }
            if (credential != expectedCredential) fail(MobileRouteContractError.CREDENTIAL_OWNER_MISMATCH)
            if (continuity != expectedContinuity) fail(MobileRouteContractError.CONTINUITY_OWNER_MISMATCH)
            if (!parsed.containsAll(required) || !allowed.containsAll(parsed)) {
                fail(MobileRouteContractError.CAPABILITY_MISMATCH)
            }
            if (canOpenTerminal != expectedTerminal) {
                fail(MobileRouteContractError.TERMINAL_OWNERSHIP_MISMATCH)
            }
            return MobileRouteProjection(kind, credential, continuity, parsed, canOpenTerminal)
        }

        private fun fail(error: MobileRouteContractError): Nothing = throw MobileRouteContractException(error)
    }
}

enum class MobileRootDestination { CONNECTIONS, DEVICES }

data class MobileTerminalDestination(
    val id: String,
    val title: String,
    val badge: String,
    val route: MobileRouteProjection,
) {
    init {
        if (!route.canOpenTerminal || route.itemKind == MobileItemKind.PAIRED_DEVICE) {
            throw MobileRouteContractException(MobileRouteContractError.TERMINAL_OWNERSHIP_MISMATCH)
        }
    }
}
