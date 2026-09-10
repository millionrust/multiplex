package com.termirust.mobile.controller

import android.content.Context
import android.os.Build
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyPermanentlyInvalidatedException
import android.security.keystore.KeyProperties
import android.security.keystore.StrongBoxUnavailableException
import android.util.AtomicFile
import androidx.annotation.RequiresApi
import com.termirust.controller.security.SecureBlobStore
import java.io.File
import java.io.FileNotFoundException
import java.security.KeyStore
import java.security.MessageDigest
import javax.crypto.AEADBadTagException
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

class ControllerSecureBlobStore(
    context: Context,
    private val alias: String = "termirust-controller-device-v1",
) : SecureBlobStore {
    private val directory = File(context.noBackupFilesDir, "controller-device-secrets").apply {
        check(mkdirs() || isDirectory)
    }

    @Synchronized
    override fun load(keyId: String): ByteArray? {
        validateKeyId(keyId)
        val file = AtomicFile(fileFor(keyId))
        val payload = try {
            // openRead restores a committed backup before opening the base file.
            file.openRead().use { input ->
                val buffer = ByteArray(MAX_PAYLOAD_BYTES + 1)
                var count = 0
                while (count < buffer.size) {
                    val read = input.read(buffer, count, buffer.size - count)
                    if (read < 0) break
                    count += read
                }
                buffer.copyOf(count)
            }
        } catch (error: FileNotFoundException) {
            if (!file.baseFile.exists() && !File(file.baseFile.path + ".bak").exists()) {
                return null
            }
            throw ControllerSecretException.Unavailable(error)
        } catch (error: Exception) {
            throw ControllerSecretException.Unavailable(error)
        }
        if (payload.size !in MIN_PAYLOAD_BYTES..MAX_PAYLOAD_BYTES || payload[0] != FORMAT_VERSION) {
            throw ControllerSecretException.Corrupt
        }
        return try {
            val iv = payload.copyOfRange(1, 1 + IV_BYTES)
            val ciphertext = payload.copyOfRange(1 + IV_BYTES, payload.size)
            val cipher = Cipher.getInstance(TRANSFORMATION)
            cipher.init(Cipher.DECRYPT_MODE, secretKey(createIfMissing = false), GCMParameterSpec(128, iv))
            cipher.updateAAD(keyId.encodeToByteArray())
            cipher.doFinal(ciphertext)
        } catch (error: KeyPermanentlyInvalidatedException) {
            throw ControllerSecretException.Invalidated(error)
        } catch (error: AEADBadTagException) {
            throw ControllerSecretException.Corrupt
        } catch (error: Throwable) {
            throw ControllerSecretException.Unavailable(error)
        }
    }

    @Synchronized
    override fun store(keyId: String, value: ByteArray) {
        validateKeyId(keyId)
        require(value.size in 1..MAX_SECRET_BYTES)
        val encrypted = try {
            val cipher = Cipher.getInstance(TRANSFORMATION)
            cipher.init(Cipher.ENCRYPT_MODE, secretKey())
            cipher.updateAAD(keyId.encodeToByteArray())
            byteArrayOf(FORMAT_VERSION) + cipher.iv + cipher.doFinal(value)
        } catch (error: KeyPermanentlyInvalidatedException) {
            throw ControllerSecretException.Invalidated(error)
        } catch (error: Throwable) {
            throw ControllerSecretException.Unavailable(error)
        }
        val file = AtomicFile(fileFor(keyId))
        val output = file.startWrite()
        try {
            output.write(encrypted)
            output.fd.sync()
            file.finishWrite(output)
        } catch (error: Throwable) {
            file.failWrite(output)
            throw ControllerSecretException.Unavailable(error)
        }
    }

    @Synchronized
    override fun delete(keyId: String) {
        validateKeyId(keyId)
        val file = fileFor(keyId)
        AtomicFile(file).delete()
        if (file.exists() || File(file.path + ".bak").exists() || File(file.path + ".new").exists()) {
            throw ControllerSecretException.Unavailable(null)
        }
    }

    private fun secretKey(createIfMissing: Boolean = true): SecretKey {
        val keyStore = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        (keyStore.getEntry(alias, null) as? KeyStore.SecretKeyEntry)?.let { return it.secretKey }
        if (!createIfMissing) throw ControllerSecretException.Unavailable(null)
        return if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
            generateStrongBoxKey()
        } else {
            generateKey()
        }
    }

    private fun generateKey(): SecretKey {
        val generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore")
        val builder = KeyGenParameterSpec.Builder(
            alias,
            KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT,
        )
            .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
            .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
            .setKeySize(256)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
            builder.setUnlockedDeviceRequired(true)
        }
        generator.init(builder.build())
        return generator.generateKey()
    }

    @RequiresApi(Build.VERSION_CODES.P)
    private fun generateStrongBoxKey(): SecretKey = try {
        val generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore")
        val spec = KeyGenParameterSpec.Builder(
            alias,
            KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT,
        )
            .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
            .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
            .setKeySize(256)
            .setUnlockedDeviceRequired(true)
            .setIsStrongBoxBacked(true)
            .build()
        generator.init(spec)
        generator.generateKey()
    } catch (_: StrongBoxUnavailableException) {
        generateKey()
    }

    private fun fileFor(keyId: String): File {
        val digest = MessageDigest.getInstance("SHA-256").digest(keyId.encodeToByteArray())
        val name = digest.joinToString("") { "%02x".format(it) }
        return File(directory, "$name.blob")
    }

    private fun validateKeyId(keyId: String) {
        require(keyId.toByteArray().size in 1..128)
        require(keyId.none(Char::isISOControl))
    }

    private companion object {
        const val TRANSFORMATION = "AES/GCM/NoPadding"
        const val FORMAT_VERSION: Byte = 1
        const val IV_BYTES = 12
        const val MAX_SECRET_BYTES = 4 * 1_024
        const val MIN_PAYLOAD_BYTES = 1 + IV_BYTES + 16 + 1
        const val MAX_PAYLOAD_BYTES = 1 + IV_BYTES + 16 + MAX_SECRET_BYTES
    }
}

sealed class ControllerSecretException : Exception() {
    data class Invalidated(override val cause: Throwable?) : ControllerSecretException()
    data class Unavailable(override val cause: Throwable?) : ControllerSecretException()
    data object Corrupt : ControllerSecretException()
}
