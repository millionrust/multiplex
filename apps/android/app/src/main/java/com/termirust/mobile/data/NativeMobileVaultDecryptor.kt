package com.termirust.mobile.data

import java.nio.CharBuffer
import java.nio.charset.StandardCharsets

object NativeMobileVaultCrypto {
    init {
        System.loadLibrary("termirust_mobile_ffi")
    }

    external fun decryptVaultJson(encryptedVault: ByteArray, passphrase: ByteArray): ByteArray
}

class NativeMobileVaultDecryptor : MobileVaultDecryptor {
    override fun decrypt(encryptedVault: ByteArray, passphrase: CharArray): ByteArray {
        val passphraseBytes = passphraseUtf8Bytes(passphrase)
        return try {
            NativeMobileVaultCrypto.decryptVaultJson(encryptedVault, passphraseBytes)
        } finally {
            passphraseBytes.fill(0)
        }
    }
}

internal fun passphraseUtf8Bytes(passphrase: CharArray): ByteArray {
    val encoded = StandardCharsets.UTF_8.encode(CharBuffer.wrap(passphrase))
    val bytes = ByteArray(encoded.remaining())
    encoded.get(bytes)
    if (encoded.hasArray()) {
        encoded.array().fill(0)
    }
    return bytes
}
