package com.termirust.mobile.data

import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable

const val MOBILE_VAULT_SCHEMA_VERSION = 1

@Serializable
data class EncryptedMobileVaultEnvelope(
    val version: Int,
    @SerialName("schema_version") val schemaVersion: Int,
    val cipher: String,
    val kdf: String,
    val salt: String,
    val nonce: String,
    val ciphertext: String,
)

@Serializable
data class MobileVaultExport(
    @SerialName("schema_version") val schemaVersion: Int,
    @SerialName("export_id") val exportId: String,
    @SerialName("created_at_millis") val createdAtMillis: ULong,
    @SerialName("updated_at_millis") val updatedAtMillis: ULong,
    @SerialName("source_device_id") val sourceDeviceId: String,
    val vaults: List<MobileVault> = emptyList(),
    val hosts: List<MobileHost> = emptyList(),
    val groups: List<MobileGroup> = emptyList(),
    val tags: List<String> = emptyList(),
    val identities: List<MobileIdentityMetadata> = emptyList(),
    @SerialName("known_hosts") val knownHosts: List<MobileKnownHost> = emptyList(),
    val sync: MobileSyncMetadata = MobileSyncMetadata(),
    val devices: List<MobileDeviceRecord> = emptyList(),
    @SerialName("device_keys") val deviceKeys: List<MobileDeviceVaultKey> = emptyList(),
) {
    fun sourceDeviceRecord(): MobileDeviceRecord? =
        devices.firstOrNull { it.deviceId == sourceDeviceId }

    fun sourceDeviceRevoked(): Boolean =
        sourceDeviceRecord()?.revokedAtMillis != null

    fun isOlderThan(other: MobileVaultExport): Boolean {
        val revision = sync.revision
        val otherRevision = other.sync.revision
        if (revision != null && otherRevision != null && revision != otherRevision) {
            return revision < otherRevision
        }
        return updatedAtMillis < other.updatedAtMillis
    }

    fun isDeviceRevoked(deviceId: String): Boolean {
        val trimmed = deviceId.trim()
        if (trimmed.isEmpty()) {
            return false
        }
        return devices.any { it.deviceId == trimmed && it.revokedAtMillis != null }
    }

    fun activeDeviceKey(deviceId: String): MobileDeviceVaultKey? {
        if (isDeviceRevoked(deviceId)) {
            return null
        }
        return deviceKeys.firstOrNull { it.deviceId == deviceId && it.revokedAtMillis == null }
    }
}

@Serializable
data class MobileVault(
    val id: String,
    val label: String,
    val description: String = "",
    val kind: String = "",
)

@Serializable
data class MobileHost(
    val id: String,
    val label: String,
    @SerialName("vault_id") val vaultId: String? = null,
    val group: String = "",
    val tags: List<String> = emptyList(),
    val host: String,
    val port: Int,
    val username: String,
    val auth: MobileAuthMetadata,
    @SerialName("jump_host_id") val jumpHostId: String? = null,
    @SerialName("startup_directory") val startupDirectory: String? = null,
    @SerialName("startup_command") val startupCommand: String? = null,
    @SerialName("start_in_files") val startInFiles: Boolean = false,
    @SerialName("persistent_session") val persistentSession: MobilePersistentSession = MobilePersistentSession(),
    @SerialName("terminal_scrollback_rows") val terminalScrollbackRows: UInt? = null,
    @SerialName("color_tag") val colorTag: String? = null,
    val environment: List<MobileEnvironmentVariable> = emptyList(),
    @SerialName("known_host_endpoint") val knownHostEndpoint: String? = null,
)

@Serializable
data class MobileAuthMetadata(
    val kind: MobileAuthKind,
    @SerialName("identity_id") val identityId: String? = null,
    @SerialName("secret_ref") val secretRef: String? = null,
)

@Serializable
enum class MobileAuthKind {
    @SerialName("password")
    Password,

    @SerialName("private_key")
    PrivateKey,
}

@Serializable
data class MobilePersistentSession(
    val enabled: Boolean = false,
    @SerialName("session_name") val sessionName: String? = null,
    @SerialName("detach_others") val detachOthers: Boolean = false,
)

@Serializable
data class MobileEnvironmentVariable(
    val name: String,
    val value: String,
)

@Serializable
data class MobileIdentityMetadata(
    val id: String,
    val label: String,
    @SerialName("vault_id") val vaultId: String? = null,
    val kind: String,
    @SerialName("public_key") val publicKey: String? = null,
    val fingerprint: String? = null,
    @SerialName("secret_ref") val secretRef: String? = null,
)

@Serializable
data class MobileKnownHost(
    val endpoint: String,
    @SerialName("public_key") val publicKey: String,
    val algorithm: String? = null,
    val fingerprint: String? = null,
)

@Serializable
data class MobileGroup(
    val id: String,
    val name: String,
)

@Serializable
data class MobileSyncMetadata(
    val revision: ULong? = null,
    @SerialName("last_synced_at_millis") val lastSyncedAtMillis: ULong? = null,
)

@Serializable
data class MobileDeviceRecord(
    @SerialName("device_id") val deviceId: String,
    val label: String,
    val platform: String? = null,
    @SerialName("public_key") val publicKey: String? = null,
    @SerialName("paired_at_millis") val pairedAtMillis: ULong? = null,
    @SerialName("last_seen_at_millis") val lastSeenAtMillis: ULong? = null,
    @SerialName("revoked_at_millis") val revokedAtMillis: ULong? = null,
)

@Serializable
data class MobileDeviceVaultKey(
    @SerialName("key_id") val keyId: String,
    @SerialName("device_id") val deviceId: String,
    @SerialName("wrapping_algorithm") val wrappingAlgorithm: String,
    @SerialName("encrypted_vault_key") val encryptedVaultKey: String,
    @SerialName("created_at_millis") val createdAtMillis: ULong? = null,
    @SerialName("revoked_at_millis") val revokedAtMillis: ULong? = null,
)

@Serializable
data class MobileDevicePairingRequest(
    @SerialName("schema_version") val schemaVersion: Int = MOBILE_VAULT_SCHEMA_VERSION,
    @SerialName("request_id") val requestId: String,
    @SerialName("device_id") val deviceId: String,
    val label: String,
    val platform: String,
    @SerialName("public_key") val publicKey: String? = null,
    @SerialName("created_at_millis") val createdAtMillis: ULong,
)
