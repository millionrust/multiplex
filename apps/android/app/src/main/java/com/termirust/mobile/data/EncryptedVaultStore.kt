package com.termirust.mobile.data

import android.content.Context
import android.util.Base64

interface EncryptedVaultStore {
    fun hasEncryptedVault(): Boolean
    fun saveEncryptedVault(bytes: ByteArray)
    fun readEncryptedVault(): ByteArray?
    fun clearEncryptedVault()
}

class SharedPreferencesEncryptedVaultStore(
    context: Context,
) : EncryptedVaultStore {
    private val prefs = context.getSharedPreferences("termirust-mobile-vault", Context.MODE_PRIVATE)

    override fun hasEncryptedVault(): Boolean =
        prefs.contains(KEY_ENCRYPTED_VAULT)

    override fun saveEncryptedVault(bytes: ByteArray) {
        prefs.edit()
            .putString(KEY_ENCRYPTED_VAULT, Base64.encodeToString(bytes, Base64.NO_WRAP))
            .apply()
    }

    override fun readEncryptedVault(): ByteArray? {
        val encoded = prefs.getString(KEY_ENCRYPTED_VAULT, null) ?: return null
        return runCatching { Base64.decode(encoded, Base64.NO_WRAP) }.getOrNull()
    }

    override fun clearEncryptedVault() {
        prefs.edit().remove(KEY_ENCRYPTED_VAULT).apply()
    }

    private companion object {
        const val KEY_ENCRYPTED_VAULT = "encrypted_vault"
    }
}
