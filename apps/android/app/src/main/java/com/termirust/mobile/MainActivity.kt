package com.termirust.mobile

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.platform.LocalContext
import androidx.lifecycle.ViewModel
import androidx.lifecycle.ViewModelProvider
import androidx.lifecycle.viewmodel.CreationExtras
import androidx.lifecycle.viewmodel.compose.viewModel
import com.termirust.mobile.controller.ControllerViewModel
import com.termirust.mobile.data.MobileVaultImporter
import com.termirust.mobile.data.NativeMobileVaultDecryptor
import com.termirust.mobile.data.SharedPreferencesDeviceIdentityStore
import com.termirust.mobile.data.SharedPreferencesEncryptedVaultStore
import com.termirust.mobile.security.KeystoreSecretStore
import com.termirust.mobile.ssh.DirectSshSessionClient
import com.termirust.mobile.ssh.MobileSshSecretProvider
import com.termirust.mobile.ui.UnifiedMobileApp

class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        setContent {
            val context = LocalContext.current.applicationContext
            val hostFactory = remember {
                val secretStore = KeystoreSecretStore(context)
                val encryptedVaultStore = SharedPreferencesEncryptedVaultStore(context)
                val identityStore = SharedPreferencesDeviceIdentityStore(context)
                MobileHostViewModelFactory(
                    vaultImporter = MobileVaultImporter(decryptor = NativeMobileVaultDecryptor()),
                    secretStore = secretStore,
                    encryptedVaultStore = encryptedVaultStore,
                    sshClient = DirectSshSessionClient(
                        secretProvider = MobileSshSecretProvider { reference ->
                            secretStore.readSecret(reference)?.toCharArray()
                        },
                    ),
                    localDeviceId = identityStore.deviceId(),
                )
            }
            val connections: MobileHostViewModel = viewModel(factory = hostFactory)
            val controller: ControllerViewModel = viewModel()
            var pendingVaultPassphrase by remember { mutableStateOf("") }
            val vaultPicker = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
                if (uri != null) {
                    runCatching {
                        requireNotNull(context.contentResolver.openInputStream(uri)).use { it.readBytes() }
                    }.onSuccess { bytes ->
                        connections.importEncryptedVault(bytes, pendingVaultPassphrase.toCharArray())
                        pendingVaultPassphrase = ""
                    }.onFailure { connections.reportStatus(it.message ?: "Unable to import vault.") }
                }
            }
            val keyPicker = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
                if (uri != null) {
                    runCatching {
                        requireNotNull(context.contentResolver.openInputStream(uri)).bufferedReader().use { it.readText() }
                    }.onSuccess(connections::saveCredentialForSelectedHost)
                        .onFailure { connections.reportStatus(it.message ?: "Unable to import key.") }
                }
            }
            UnifiedMobileApp(
                connections = connections,
                controller = controller,
                onImportVault = { passphrase ->
                    pendingVaultPassphrase = passphrase
                    vaultPicker.launch(arrayOf("application/json", "application/octet-stream", "text/*", "*/*"))
                },
                onImportCredentialFile = {
                    keyPicker.launch(arrayOf("application/x-pem-file", "application/octet-stream", "text/*", "*/*"))
                },
            )
        }
    }
}

private class MobileHostViewModelFactory(
    private val vaultImporter: MobileVaultImporter,
    private val secretStore: KeystoreSecretStore,
    private val encryptedVaultStore: SharedPreferencesEncryptedVaultStore,
    private val sshClient: DirectSshSessionClient,
    private val localDeviceId: String,
) : ViewModelProvider.Factory {
    @Suppress("UNCHECKED_CAST")
    override fun <T : ViewModel> create(modelClass: Class<T>, extras: CreationExtras): T {
        if (modelClass.isAssignableFrom(MobileHostViewModel::class.java)) {
            return MobileHostViewModel(
                importer = vaultImporter,
                sshClient = sshClient,
                secretStore = secretStore,
                encryptedVaultStore = encryptedVaultStore,
                localDeviceId = localDeviceId,
            ) as T
        }
        throw IllegalArgumentException("Unknown ViewModel class ${modelClass.name}")
    }
}
