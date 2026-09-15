package com.termirust.mobile.security

import android.content.Context
import android.app.KeyguardManager
import android.os.Build
import android.security.keystore.KeyPermanentlyInvalidatedException
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.security.keystore.UserNotAuthenticatedException
import android.util.Base64
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

interface MobileSecretStore {
    fun saveSecret(account: String, secret: String)
    fun readSecret(account: String): String?
    fun deleteSecret(account: String)
}

class KeystoreSecretStore(
    private val context: Context,
    private val alias: String = "termirust-mobile-secrets",
) : MobileSecretStore {
    private val prefs by lazy {
        context.getSharedPreferences("termirust-mobile-secrets", Context.MODE_PRIVATE)
    }

    override fun saveSecret(account: String, secret: String) {
        requireSecureDeviceLock()
        runCatching {
            val cipher = Cipher.getInstance(TRANSFORMATION)
            cipher.init(Cipher.ENCRYPT_MODE, secretKey())
            val ciphertext = cipher.doFinal(secret.toByteArray(Charsets.UTF_8))
            val payload = "${Base64.encodeToString(cipher.iv, Base64.NO_WRAP)}:${Base64.encodeToString(ciphertext, Base64.NO_WRAP)}"
            prefs.edit().putString(account, payload).apply()
        }.getOrElse { throw userFacingKeystoreError(it) }
    }

    override fun readSecret(account: String): String? {
        requireSecureDeviceLock()
        return runCatching {
            val payload = prefs.getString(account, null) ?: return null
            val parts = payload.split(":")
            if (parts.size != 2) return null
            val iv = Base64.decode(parts[0], Base64.NO_WRAP)
            val ciphertext = Base64.decode(parts[1], Base64.NO_WRAP)
            val cipher = Cipher.getInstance(TRANSFORMATION)
            cipher.init(Cipher.DECRYPT_MODE, secretKey(), GCMParameterSpec(128, iv))
            cipher.doFinal(ciphertext).toString(Charsets.UTF_8)
        }.getOrElse { throw userFacingKeystoreError(it) }
    }

    override fun deleteSecret(account: String) {
        prefs.edit().remove(account).apply()
    }

    private fun secretKey(): SecretKey {
        val keyStore = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        val existing = keyStore.getEntry(alias, null) as? KeyStore.SecretKeyEntry
        if (existing != null) {
            return existing.secretKey
        }

        val keyGenerator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore")
        val spec = KeyGenParameterSpec.Builder(
            alias,
            KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT,
        )
            .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
            .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
            .setUserAuthenticationRequired(true)
            .apply {
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
                    setUserAuthenticationParameters(
                        AUTH_VALIDITY_SECONDS,
                        KeyProperties.AUTH_DEVICE_CREDENTIAL or KeyProperties.AUTH_BIOMETRIC_STRONG,
                    )
                } else {
                    @Suppress("DEPRECATION")
                    setUserAuthenticationValidityDurationSeconds(AUTH_VALIDITY_SECONDS)
                }
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
                    setUnlockedDeviceRequired(true)
                }
            }
            .build()
        keyGenerator.init(spec)
        return keyGenerator.generateKey()
    }

    private fun requireSecureDeviceLock() {
        val keyguard = context.getSystemService(KeyguardManager::class.java)
        check(keyguard?.isDeviceSecure == true) {
            "Set a device PIN, password, or pattern before storing TermiRust mobile SSH credentials."
        }
    }

    private fun userFacingKeystoreError(error: Throwable): Throwable =
        when (error) {
            is UserNotAuthenticatedException -> IllegalStateException(
                "Unlock this device with PIN, password, pattern, or biometrics before using TermiRust mobile SSH credentials.",
                error,
            )
            is KeyPermanentlyInvalidatedException -> IllegalStateException(
                "The Android secure lock changed. Remove and save this TermiRust mobile credential again.",
                error,
            )
            else -> error
        }

    private companion object {
        const val TRANSFORMATION = "AES/GCM/NoPadding"
        const val AUTH_VALIDITY_SECONDS = 300
    }
}
