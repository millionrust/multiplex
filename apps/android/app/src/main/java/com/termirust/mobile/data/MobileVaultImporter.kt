package com.termirust.mobile.data

import kotlinx.serialization.json.Json

interface MobileVaultDecryptor {
    fun decrypt(encryptedVault: ByteArray, passphrase: CharArray): ByteArray
}

class MobileVaultImporter(
    private val json: Json = Json {
        ignoreUnknownKeys = true
        explicitNulls = false
    },
    private val decryptor: MobileVaultDecryptor? = null,
) {
    fun inspectEncryptedEnvelope(bytes: ByteArray): EncryptedMobileVaultEnvelope {
        val envelope = json.decodeFromString<EncryptedMobileVaultEnvelope>(bytes.decodeToString())
        require(envelope.schemaVersion == MOBILE_VAULT_SCHEMA_VERSION) {
            "Unsupported mobile vault schema version ${envelope.schemaVersion}."
        }
        return envelope
    }

    fun importPlaintextFixture(bytes: ByteArray): MobileVaultExport {
        val vault = json.decodeFromString<MobileVaultExport>(bytes.decodeToString())
        require(vault.schemaVersion == MOBILE_VAULT_SCHEMA_VERSION) {
            "Unsupported mobile vault schema version ${vault.schemaVersion}."
        }
        require(!vault.sourceDeviceRevoked()) {
            "This mobile vault was exported by a revoked device (${vault.sourceDeviceId}). Import blocked."
        }
        return vault
    }

    fun importEncryptedVault(bytes: ByteArray, passphrase: CharArray): MobileVaultExport {
        inspectEncryptedEnvelope(bytes)
        val plaintext = try {
            decryptor?.decrypt(bytes, passphrase)
                ?: error("This build is missing TermiRust shared vault crypto. Install the mobile crypto library before importing encrypted vaults.")
        } finally {
            passphrase.fill('\u0000')
        }
        return importPlaintextFixture(plaintext)
    }
}
