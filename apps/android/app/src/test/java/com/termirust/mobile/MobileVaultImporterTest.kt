package com.termirust.mobile

import com.termirust.mobile.data.MobileAuthKind
import com.termirust.mobile.data.MobileAuthMetadata
import com.termirust.mobile.data.EncryptedVaultStore
import com.termirust.mobile.data.MobileHost
import com.termirust.mobile.data.MobilePersistentSession
import com.termirust.mobile.data.MobileVaultDecryptor
import com.termirust.mobile.data.MobileVaultImporter
import com.termirust.mobile.data.passphraseUtf8Bytes
import com.termirust.mobile.security.MobileSecretStore
import com.termirust.mobile.ssh.PinnedKnownHostVerifier
import com.termirust.mobile.ssh.TmuxBootstrap
import com.termirust.mobile.terminal.TerminalBuffer
import com.termirust.mobile.terminal.TerminalGrid
import com.termirust.mobile.terminal.encodeTerminalInput
import com.termirust.mobile.terminal.estimateTerminalGrid
import com.hierynomus.sshj.userauth.keyprovider.OpenSSHKeyV1KeyFile
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import java.security.KeyPairGenerator

class MobileVaultImporterTest {
    private val fixtureEd25519PrivateKey = """
        -----BEGIN OPENSSH PRIVATE KEY-----
        b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW
        QyNTUxOQAAACDXdpIxO9R+jxlETproGCGcxOE4SjX6r+jU8UIg2K5ULgAAAJgxZY5/MWWO
        fwAAAAtzc2gtZWQyNTUxOQAAACDXdpIxO9R+jxlETproGCGcxOE4SjX6r+jU8UIg2K5ULg
        AAAECEhzF5gu5ZhIg/P8PEq/33+dXN8wUMwtU7zIlgJLfC2Nd2kjE71H6PGUROmugYIZzE
        4ThKNfqv6NTxQiDYrlQuAAAAD2NsYXdAak1hYy5sb2NhbAECAwQFBg==
        -----END OPENSSH PRIVATE KEY-----
    """.trimIndent()

    private val encryptedEnvelope = """
        {
          "version": 1,
          "schema_version": 1,
          "cipher": "AES-256-GCM-SIV",
          "kdf": "Argon2id(m=19456,t=3,p=1)",
          "salt": "salt",
          "nonce": "nonce",
          "ciphertext": "ciphertext"
        }
    """.trimIndent().encodeToByteArray()

    private val plaintextVault = """
        {
          "schema_version": 1,
          "export_id": "export-1",
          "created_at_millis": 1,
          "updated_at_millis": 1,
          "source_device_id": "desktop-1",
          "vaults": [],
          "hosts": [{
            "id": "profile-1",
            "label": "Prod",
            "vault_id": null,
            "group": "Ops",
            "tags": ["prod"],
            "host": "prod.example.com",
            "port": 22,
            "username": "ubuntu",
            "auth": {"kind": "private_key", "identity_id": "identity-1", "secret_ref": "termirust-mobile://identity/identity-1/private-key"},
            "jump_host_id": null,
            "startup_directory": "/srv/app",
            "startup_command": "uptime",
            "start_in_files": false,
            "persistent_session": {"enabled": true, "session_name": "tr-prod", "detach_others": false},
            "terminal_scrollback_rows": 20000,
            "color_tag": null,
            "environment": [],
            "known_host_endpoint": "prod.example.com:22"
          }],
          "groups": [],
          "tags": [],
          "identities": [],
          "known_hosts": [{"endpoint": "prod.example.com:22", "public_key": "ssh-ed25519 AAAA", "algorithm": null, "fingerprint": null}],
          "sync": {"revision": null, "last_synced_at_millis": null},
          "devices": [{
            "device_id": "desktop-1",
            "label": "TermiRust Desktop",
            "platform": "desktop",
            "public_key": null,
            "paired_at_millis": 1,
            "last_seen_at_millis": 1,
            "revoked_at_millis": null
          }],
          "device_keys": [{
            "key_id": "vault-key-android-1",
            "device_id": "android-1",
            "wrapping_algorithm": "x25519-xsalsa20poly1305",
            "encrypted_vault_key": "base64-wrapped-key",
            "created_at_millis": 1,
            "revoked_at_millis": null
          }]
        }
    """.trimIndent().encodeToByteArray()

    @Test
    fun plaintextVaultDecodesPersistentTmuxHost() {
        val vault = MobileVaultImporter().importPlaintextFixture(plaintextVault)

        assertEquals(1, vault.schemaVersion)
        assertEquals("tr-prod", vault.hosts.first().persistentSession.sessionName)
        assertEquals(
            "termirust-mobile://identity/identity-1/private-key",
            vault.hosts.first().auth.secretRef,
        )
        assertEquals("prod.example.com:22", vault.knownHosts.first().endpoint)
        assertEquals("desktop-1", vault.devices.first().deviceId)
        assertEquals("desktop", vault.devices.first().platform)
        assertEquals(1UL, vault.devices.first().pairedAtMillis)
        assertEquals("android-1", vault.deviceKeys.first().deviceId)
        assertEquals("vault-key-android-1", vault.activeDeviceKey("android-1")?.keyId)
    }

    @Test
    fun plaintextVaultDefaultsMissingDeviceKeys() {
        val legacyVault = plaintextVault.decodeToString()
            .replace(
                Regex(""",\s*"device_keys"\s*:\s*\[\{[\s\S]*?\}\]"""),
                "",
            )
            .encodeToByteArray()

        val vault = MobileVaultImporter().importPlaintextFixture(legacyVault)

        assertTrue(vault.deviceKeys.isEmpty())
    }

    @Test
    fun viewModelGeneratesPairingRequestJson() {
        val viewModel = MobileHostViewModel(localDeviceId = "android-1")

        val request = viewModel.pairingRequestText(label = "Jacob Android", nowMillis = 42UL)
        val requestObject = Json.parseToJsonElement(request).jsonObject

        assertEquals("1", requestObject.getValue("schema_version").jsonPrimitive.content)
        assertEquals("pair-android-1-42", requestObject.getValue("request_id").jsonPrimitive.content)
        assertEquals("android-1", requestObject.getValue("device_id").jsonPrimitive.content)
        assertEquals("Jacob Android", requestObject.getValue("label").jsonPrimitive.content)
        assertEquals("android", requestObject.getValue("platform").jsonPrimitive.content)
        assertEquals("42", requestObject.getValue("created_at_millis").jsonPrimitive.content)
    }

    @Test
    fun viewModelRejectsVaultWhenLocalDeviceIsRevoked() {
        val revokedVault = plaintextVault.decodeToString()
            .let { json ->
                val match = requireNotNull(
                    Regex("""("devices"\s*:\s*\[\{[\s\S]*?"revoked_at_millis"\s*:\s*null\s*)\}""")
                        .find(json)
                )
                json.replaceRange(
                    match.range,
                    """
                    ${match.groupValues[1]}}, {
                        "device_id": "android-1",
                        "label": "Jacob Android",
                        "platform": "android",
                        "public_key": null,
                        "revoked_at_millis": 1719356789123
                    }
                    """.trimIndent(),
                )
            }
            .encodeToByteArray()
        val viewModel = MobileHostViewModel(localDeviceId = "android-1")

        viewModel.importPlaintextFixture(revokedVault)

        assertEquals(null, viewModel.selectedHost.value)
        assertEquals(
            "This device has been revoked for the imported mobile vault (android-1). Import blocked.",
            viewModel.status.value,
        )
    }

    @Test
    fun viewModelRejectsOlderVaultOverLoadedVault() {
        val viewModel = MobileHostViewModel()
        val currentVault = vaultFixture(updatedAtMillis = 10UL, revision = 2UL)
        val staleVault = vaultFixture(updatedAtMillis = 5UL, revision = 1UL)

        viewModel.importPlaintextFixture(currentVault)
        assertEquals(10UL, viewModel.vault.value?.updatedAtMillis)

        viewModel.importPlaintextFixture(staleVault)

        assertEquals(10UL, viewModel.vault.value?.updatedAtMillis)
        assertEquals(
            "Imported vault is older than the currently loaded vault. Import blocked to avoid overwriting newer mobile state.",
            viewModel.status.value,
        )
    }

    @Test
    fun viewModelResolvesSelectedHostKnownHostPin() {
        val viewModel = MobileHostViewModel()

        viewModel.importPlaintextFixture(plaintextVault)

        val host = viewModel.selectedHost.value!!
        assertEquals("prod.example.com:22", viewModel.knownHostFor(host)?.endpoint)
    }

    private fun vaultFixture(updatedAtMillis: ULong, revision: ULong): ByteArray =
        plaintextVault.decodeToString()
            .replace("\"updated_at_millis\": 1", "\"updated_at_millis\": $updatedAtMillis")
            .replace(
                "\"sync\": {\"revision\": null, \"last_synced_at_millis\": null}",
                "\"sync\": {\"revision\": $revision, \"last_synced_at_millis\": null}",
            )
            .encodeToByteArray()

    @Test
    fun plaintextVaultRejectsRevokedSourceDevice() {
        val revokedVault = plaintextVault.decodeToString()
            .replace("\"revoked_at_millis\": null", "\"revoked_at_millis\": 1719356789123")
            .encodeToByteArray()

        val error = kotlin.runCatching {
            MobileVaultImporter().importPlaintextFixture(revokedVault)
        }.exceptionOrNull()

        assertEquals(
            "This mobile vault was exported by a revoked device (desktop-1). Import blocked.",
            error?.message,
        )
    }

    @Test
    fun encryptedVaultRequiresSharedCryptoDecryptor() {
        val error = kotlin.runCatching {
            MobileVaultImporter().importEncryptedVault(encryptedEnvelope, "hunter2".toCharArray())
        }.exceptionOrNull()

        assertEquals(
            "This build is missing TermiRust shared vault crypto. Install the mobile crypto library before importing encrypted vaults.",
            error?.message,
        )
    }

    @Test
    fun encryptedVaultUsesInjectedDecryptorAndClearsPassphrase() {
        val passphrase = "hunter2".toCharArray()
        val importer = MobileVaultImporter(decryptor = FixtureDecryptor(plaintextVault))

        val vault = importer.importEncryptedVault(encryptedEnvelope, passphrase)

        assertEquals("prod.example.com", vault.hosts.first().host)
        assertEquals("tr-prod", vault.hosts.first().persistentSession.sessionName)
        assertTrue(passphrase.all { it == '\u0000' })
    }

    @Test
    fun nativeDecryptorPassphraseEncodingUsesUtf8Bytes() {
        assertEquals("hunter2".encodeToByteArray().toList(), passphraseUtf8Bytes("hunter2".toCharArray()).toList())
        assertEquals("päss 🔐".encodeToByteArray().toList(), passphraseUtf8Bytes("päss 🔐".toCharArray()).toList())
    }

    @Test
    fun pinnedKnownHostVerifierRejectsMismatchedHostKey() {
        val host = MobileHost(
            id = "profile-1",
            label = "Prod",
            host = "prod.example.com",
            port = 22,
            username = "ubuntu",
            auth = MobileAuthMetadata(kind = MobileAuthKind.PrivateKey),
        )
        val knownHost = com.termirust.mobile.data.MobileKnownHost(
            endpoint = "prod.example.com:22",
            publicKey = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAINd2kjE71H6PGUROmugYIZzE4ThKNfqv6NTxQiDYrlQu",
            algorithm = "ssh-ed25519",
        )
        val matchingKey = OpenSSHKeyV1KeyFile().apply {
            init(fixtureEd25519PrivateKey, null, null)
        }.public
        val mismatchedKey = KeyPairGenerator.getInstance("RSA").apply {
            initialize(2048)
        }.generateKeyPair().public
        val verifier = PinnedKnownHostVerifier(host, knownHost)

        assertTrue(verifier.verify("prod.example.com", 22, matchingKey))
        assertFalse(verifier.verify("prod.example.com", 22, mismatchedKey))
    }

    @Test
    fun encryptedVaultImportCachesEncryptedBytesForLaterUnlock() {
        val passphrase = "hunter2".toCharArray()
        val encryptedStore = FakeEncryptedVaultStore()
        val viewModel = MobileHostViewModel(
            importer = MobileVaultImporter(decryptor = FixtureDecryptor(plaintextVault)),
            encryptedVaultStore = encryptedStore,
        )

        viewModel.importEncryptedVault(encryptedEnvelope, passphrase)

        assertTrue(viewModel.hasStoredEncryptedVault.value)
        assertEquals(encryptedEnvelope.toList(), encryptedStore.saved?.toList())
        assertTrue(passphrase.all { it == '\u0000' })

        val unlockPassphrase = "hunter2".toCharArray()
        viewModel.unlockStoredEncryptedVault(unlockPassphrase)

        assertEquals("Prod", viewModel.selectedHost.value?.label)
        assertTrue(unlockPassphrase.all { it == '\u0000' })
    }

    @Test
    fun credentialSaveUsesExportedSecretReference() {
        val host = MobileHost(
            id = "profile-1",
            label = "Prod",
            host = "prod.example.com",
            port = 22,
            username = "ubuntu",
            auth = MobileAuthMetadata(
                kind = MobileAuthKind.Password,
                secretRef = "secret-prod-password",
            ),
            knownHostEndpoint = "prod.example.com:22",
        )
        val store = FakeSecretStore()
        val viewModel = MobileHostViewModel(secretStore = store)

        viewModel.selectHost(host)
        viewModel.saveCredentialForSelectedHost("super-secret")

        assertEquals("super-secret", store.saved["secret-prod-password"])
        assertEquals("Credential saved for Prod.", viewModel.status.value)
    }

    @Test
    fun privateKeyCredentialSaveUsesExportedSecretReference() {
        val host = MobileHost(
            id = "profile-1",
            label = "Prod",
            host = "prod.example.com",
            port = 22,
            username = "ubuntu",
            auth = MobileAuthMetadata(
                kind = MobileAuthKind.PrivateKey,
                secretRef = "secret-prod-key",
            ),
            knownHostEndpoint = "prod.example.com:22",
        )
        val privateKey = """
            -----BEGIN OPENSSH PRIVATE KEY-----
            key-body
            -----END OPENSSH PRIVATE KEY-----
        """.trimIndent()
        val store = FakeSecretStore()
        val viewModel = MobileHostViewModel(secretStore = store)

        viewModel.selectHost(host)
        viewModel.saveCredentialForSelectedHost(privateKey)

        assertEquals(privateKey, store.saved["secret-prod-key"])
        assertEquals("Credential saved for Prod.", viewModel.status.value)
    }

    @Test
    fun credentialSaveReportsSecureStoreFailure() {
        val host = MobileHost(
            id = "profile-1",
            label = "Prod",
            host = "prod.example.com",
            port = 22,
            username = "ubuntu",
            auth = MobileAuthMetadata(
                kind = MobileAuthKind.Password,
                secretRef = "secret-prod-password",
            ),
        )
        val viewModel = MobileHostViewModel(
            secretStore = FailingSecretStore("Unlock this device before using TermiRust mobile SSH credentials."),
        )

        viewModel.selectHost(host)
        viewModel.saveCredentialForSelectedHost("super-secret")

        assertEquals(
            "Unlock this device before using TermiRust mobile SSH credentials.",
            viewModel.status.value,
        )
    }

    @Test
    fun tmuxBootstrapDoesNotRunStartupCommandOnAttachPath() {
        val host = MobileHost(
            id = "profile-1",
            label = "Prod",
            host = "prod.example.com",
            port = 22,
            username = "ubuntu",
            auth = MobileAuthMetadata(kind = MobileAuthKind.PrivateKey),
            startupDirectory = "/srv/app",
            startupCommand = "uptime",
            persistentSession = MobilePersistentSession(
                enabled = true,
                sessionName = "tr-prod",
                detachOthers = true,
            ),
            knownHostEndpoint = "prod.example.com:22",
        )

        val script = TmuxBootstrap(host).startupCommand().orEmpty()

        assertTrue(script.contains("tmux has-session -t 'tr-prod'"))
        assertTrue(script.contains("exec tmux attach-session -d -t 'tr-prod'"))
        assertTrue(script.contains("exec tmux new-session -s 'tr-prod' -c '/srv/app' -- \"\${SHELL:-/bin/sh}\" -lc 'uptime; exec \"\${SHELL:-/bin/sh}\" -l'"))
        assertTrue(script.indexOf("exec tmux attach-session") < script.indexOf("uptime; exec"))
    }

    @Test
    fun terminalBufferHandlesCommonTerminalRedrawSequences() {
        val buffer = TerminalBuffer()

        buffer.append("progress 1\rprogress 2\r\n\u001B[31mred\u001B[0m\r\nabc\bZ")

        assertEquals(listOf("progress 2", "red", "abZ"), buffer.lines.value)
    }

    @Test
    fun terminalBufferClearsScreenOnAnsiClear() {
        val buffer = TerminalBuffer()

        buffer.append("before\n\u001B[2J\u001B[Hafter")

        assertEquals(listOf("after"), buffer.lines.value)
    }

    @Test
    fun terminalGridEstimationUsesTerminalSizeAndFont() {
        assertEquals(
            TerminalGrid(columns = 92, rows = 31),
            estimateTerminalGrid(widthPx = 800, heightPx = 600, fontSizeSp = 14, density = 1f),
        )
        assertEquals(
            TerminalGrid(columns = 46, rows = 15),
            estimateTerminalGrid(widthPx = 800, heightPx = 600, fontSizeSp = 28, density = 1f),
        )
        assertEquals(
            TerminalGrid(columns = 20, rows = 6),
            estimateTerminalGrid(widthPx = 1, heightPx = 1, fontSizeSp = 14, density = 1f),
        )
    }

    @Test
    fun terminalInputEncodingHandlesControlAndAltModifiers() {
        assertEquals("uptime\n".encodeToByteArray().toList(), encodeTerminalInput("uptime", control = false, alt = false).toList())
        assertEquals(listOf(0x03.toByte()), encodeTerminalInput("c", control = true, alt = false).toList())
        assertEquals(listOf(0x04.toByte()), encodeTerminalInput("D", control = true, alt = false).toList())
        assertEquals(listOf(0x1B.toByte()), encodeTerminalInput("[", control = true, alt = false).toList())
        assertEquals(listOf(0x1B.toByte(), 0x78.toByte()), encodeTerminalInput("x", control = false, alt = true).toList())
        assertEquals(listOf(0x1B.toByte(), 0x03.toByte()), encodeTerminalInput("c", control = true, alt = true).toList())
    }
}

private class FixtureDecryptor(
    private val plaintext: ByteArray,
) : MobileVaultDecryptor {
    override fun decrypt(encryptedVault: ByteArray, passphrase: CharArray): ByteArray {
        assertTrue(encryptedVault.isNotEmpty())
        assertEquals("hunter2", passphrase.concatToString())
        return plaintext
    }
}

private class FakeSecretStore : MobileSecretStore {
    val saved = mutableMapOf<String, String>()

    override fun saveSecret(account: String, secret: String) {
        saved[account] = secret
    }

    override fun readSecret(account: String): String? = saved[account]

    override fun deleteSecret(account: String) {
        saved.remove(account)
    }
}

private class FailingSecretStore(
    private val message: String,
) : MobileSecretStore {
    override fun saveSecret(account: String, secret: String) {
        throw IllegalStateException(message)
    }

    override fun readSecret(account: String): String? {
        throw IllegalStateException(message)
    }

    override fun deleteSecret(account: String) {
        throw IllegalStateException(message)
    }
}

private class FakeEncryptedVaultStore : EncryptedVaultStore {
    var saved: ByteArray? = null

    override fun hasEncryptedVault(): Boolean = saved != null

    override fun saveEncryptedVault(bytes: ByteArray) {
        saved = bytes.copyOf()
    }

    override fun readEncryptedVault(): ByteArray? = saved?.copyOf()

    override fun clearEncryptedVault() {
        saved = null
    }
}
