package com.termirust.mobile

import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import com.termirust.mobile.data.EncryptedVaultStore
import com.termirust.mobile.data.MobileHost
import com.termirust.mobile.data.MobileKnownHost
import com.termirust.mobile.data.MobileDevicePairingRequest
import com.termirust.mobile.data.MobileVaultExport
import com.termirust.mobile.data.MobileVaultImporter
import com.termirust.mobile.data.MOBILE_VAULT_SCHEMA_VERSION
import com.termirust.mobile.security.MobileSecretStore
import com.termirust.mobile.ssh.DirectSshSessionClient
import com.termirust.mobile.ssh.MobileSshSessionClient
import com.termirust.mobile.ssh.TerminalConnectionState
import com.termirust.mobile.terminal.TerminalBuffer
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Job
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.launch
import kotlinx.serialization.encodeToString
import kotlinx.serialization.json.Json

class MobileHostViewModel(
    private val importer: MobileVaultImporter = MobileVaultImporter(),
    private val sshClient: MobileSshSessionClient = DirectSshSessionClient(),
    private val secretStore: MobileSecretStore? = null,
    private val encryptedVaultStore: EncryptedVaultStore? = null,
    private val localDeviceId: String = "",
) : ViewModel() {
    private val _vault = MutableStateFlow<MobileVaultExport?>(null)
    val vault: StateFlow<MobileVaultExport?> = _vault

    private val _selectedHost = MutableStateFlow<MobileHost?>(null)
    val selectedHost: StateFlow<MobileHost?> = _selectedHost

    private val _connectionState = MutableStateFlow<TerminalConnectionState>(TerminalConnectionState.Disconnected)
    val connectionState: StateFlow<TerminalConnectionState> = _connectionState

    private val _status = MutableStateFlow<String?>(null)
    val status: StateFlow<String?> = _status

    private val _hasStoredEncryptedVault = MutableStateFlow(encryptedVaultStore?.hasEncryptedVault() == true)
    val hasStoredEncryptedVault: StateFlow<Boolean> = _hasStoredEncryptedVault

    private val _privacyCovered = MutableStateFlow(false)
    val privacyCovered: StateFlow<Boolean> = _privacyCovered

    val terminalBuffer = TerminalBuffer()
    private var connectionJob: Job? = null

    val localDeviceIdForDisplay: String
        get() = localDeviceId.ifBlank { "Unavailable" }

    fun pairingRequestText(
        label: String = "Android Device",
        nowMillis: ULong = currentUnixMillis(),
    ): String {
        require(localDeviceId.isNotBlank()) {
            "This mobile device does not have a pairing identity yet."
        }
        val request = MobileDevicePairingRequest(
            schemaVersion = MOBILE_VAULT_SCHEMA_VERSION,
            requestId = "pair-$localDeviceId-$nowMillis",
            deviceId = localDeviceId,
            label = label,
            platform = "android",
            publicKey = null,
            createdAtMillis = nowMillis,
        )
        return pairingJson.encodeToString(request)
    }

    fun reportStatus(message: String) {
        _status.value = message
    }

    fun importPlaintextFixture(bytes: ByteArray) {
        runCatching { importer.importPlaintextFixture(bytes) }
            .onSuccess { acceptImportedVault(it) }
            .onFailure { _status.value = it.message }
    }

    fun importEncryptedVault(bytes: ByteArray, passphrase: CharArray) {
        runCatching { importer.importEncryptedVault(bytes, passphrase) }
            .onSuccess {
                if (acceptImportedVault(it)) {
                    encryptedVaultStore?.saveEncryptedVault(bytes)
                    _hasStoredEncryptedVault.value = encryptedVaultStore?.hasEncryptedVault() == true
                }
            }
            .onFailure { _status.value = it.message }
    }

    fun unlockStoredEncryptedVault(passphrase: CharArray) {
        val bytes = encryptedVaultStore?.readEncryptedVault()
        if (bytes == null) {
            passphrase.fill('\u0000')
            _hasStoredEncryptedVault.value = false
            _status.value = "No stored encrypted vault is available."
            return
        }
        importEncryptedVault(bytes, passphrase)
    }

    fun forgetStoredEncryptedVault() {
        encryptedVaultStore?.clearEncryptedVault()
        _hasStoredEncryptedVault.value = false
        _vault.value = null
        _selectedHost.value = null
        _status.value = "Stored encrypted vault removed."
    }

    fun selectHost(host: MobileHost) {
        _selectedHost.value = host
    }

    private fun acceptImportedVault(imported: MobileVaultExport): Boolean {
        if (imported.isDeviceRevoked(localDeviceId)) {
            _vault.value = null
            _selectedHost.value = null
            _status.value = "This device has been revoked for the imported mobile vault ($localDeviceId). Import blocked."
            return false
        }
        if (_vault.value?.let { imported.isOlderThan(it) } == true) {
            _status.value = "Imported vault is older than the currently loaded vault. Import blocked to avoid overwriting newer mobile state."
            return false
        }
        _vault.value = imported
        _selectedHost.value = imported.hosts.firstOrNull()
        _status.value = null
        return true
    }

    fun knownHostFor(host: MobileHost): MobileKnownHost? =
        _vault.value?.knownHosts?.firstOrNull { it.endpoint == host.knownHostEndpoint }

    fun saveCredentialForSelectedHost(secret: String) {
        val host = _selectedHost.value ?: return
        val reference = host.auth.secretRef
        if (reference.isNullOrBlank()) {
            _status.value = "This host does not declare a mobile secret reference."
            return
        }
        if (secret.isBlank()) {
            _status.value = "Enter the SSH credential before saving."
            return
        }
        val store = secretStore
        if (store == null) {
            _status.value = "Secure credential storage is unavailable in this build."
            return
        }

        runCatching { store.saveSecret(reference, secret) }
            .onSuccess { _status.value = "Credential saved for ${host.label}." }
            .onFailure { _status.value = it.message }
    }

    fun deleteCredentialForSelectedHost() {
        val host = _selectedHost.value ?: return
        val reference = host.auth.secretRef
        if (reference.isNullOrBlank()) {
            _status.value = "This host does not declare a mobile secret reference."
            return
        }
        val store = secretStore
        if (store == null) {
            _status.value = "Secure credential storage is unavailable in this build."
            return
        }

        runCatching { store.deleteSecret(reference) }
            .onSuccess { _status.value = "Credential removed for ${host.label}." }
            .onFailure { _status.value = it.message }
    }

    fun connectSelectedHost() {
        if (_privacyCovered.value) return
        val host = _selectedHost.value ?: return
        val knownHost = knownHostFor(host)
        _connectionState.value = TerminalConnectionState.Connecting
        terminalBuffer.clear()
        terminalBuffer.append("Connecting to ${host.username}@${host.host}:${host.port}")

        connectionJob?.cancel()
        connectionJob = viewModelScope.launch {
            try {
                sshClient.connect(host, knownHost) { bytes ->
                    terminalBuffer.append(bytes)
                }
                _connectionState.value = TerminalConnectionState.Connected
            } catch (error: CancellationException) {
                throw error
            } catch (error: Throwable) {
                _connectionState.value = TerminalConnectionState.Failed(error.message ?: "Connection failed.")
                terminalBuffer.append(error.message ?: "Connection failed.")
            }
        }
    }

    fun sendTerminalInput(input: String) {
        if (_privacyCovered.value) return
        viewModelScope.launch {
            runCatching { sshClient.send("$input\n".encodeToByteArray()) }
                .onFailure { terminalBuffer.append(it.message ?: "Unable to send input.") }
        }
    }

    fun sendTerminalBytes(bytes: ByteArray) {
        if (_privacyCovered.value) return
        viewModelScope.launch {
            runCatching { sshClient.send(bytes) }
                .onFailure { terminalBuffer.append(it.message ?: "Unable to send input.") }
        }
    }

    fun resizeTerminal(columns: Int, rows: Int) {
        if (_privacyCovered.value) return
        terminalBuffer.resize(columns, rows)
        viewModelScope.launch {
            runCatching { sshClient.resize(columns, rows) }
                .onFailure { terminalBuffer.append(it.message ?: "Unable to resize terminal.") }
        }
    }

    fun disconnect() {
        connectionJob?.cancel()
        connectionJob = null
        viewModelScope.launch {
            sshClient.disconnect()
            _connectionState.value = TerminalConnectionState.Disconnected
        }
    }

    fun onBackground() {
        if (_privacyCovered.value) return
        _privacyCovered.value = true
        disconnect()
    }

    fun onForeground() {
        _privacyCovered.value = false
    }

    private companion object {
        val pairingJson = Json {
            prettyPrint = true
            encodeDefaults = true
            explicitNulls = false
        }

        fun currentUnixMillis(): ULong =
            System.currentTimeMillis().toULong()
    }
}
