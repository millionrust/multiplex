package com.termirust.mobile.controller

import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import java.io.File
import java.security.KeyStore
import java.security.MessageDigest
import java.util.UUID
import org.junit.After
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class ControllerSecureBlobStoreInstrumentedTest {
    private val context = InstrumentationRegistry.getInstrumentation().targetContext
    private val keyId = "storage-test-${UUID.randomUUID()}"
    private val otherKeyId = "$keyId-other"
    private val alias = "termirust-storage-test-${UUID.randomUUID()}"
    private val store = ControllerSecureBlobStore(context, alias)

    @After
    fun cleanup() {
        store.delete(keyId)
        store.delete(otherKeyId)
        keyStore().deleteEntry(alias)
    }

    @Test
    fun committedBackupIsRecoveredWhenBaseFileIsMissing() {
        val value = byteArrayOf(3, 1, 4, 1, 5)
        store.store(keyId, value)
        val base = fileFor(keyId)
        val backup = File(base.path + ".bak")
        assertTrue(base.renameTo(backup))

        assertArrayEquals(value, ControllerSecureBlobStore(context, alias).load(keyId))
        assertTrue(base.exists())
        assertFalse(backup.exists())
    }

    @Test
    fun deleteRemovesRecoveryFilesWithoutRemovingAnotherSecret() {
        store.store(keyId, byteArrayOf(1, 2, 3))
        store.store(otherKeyId, byteArrayOf(4, 5, 6))
        val base = fileFor(keyId)
        val backup = File(base.path + ".bak")
        val pending = File(base.path + ".new")
        base.copyTo(backup)
        base.copyTo(pending)

        store.delete(keyId)
        store.delete(keyId)
        assertFalse(base.exists())
        assertFalse(backup.exists())
        assertFalse(pending.exists())
        assertNull(ControllerSecureBlobStore(context, alias).load(keyId))
        assertArrayEquals(byteArrayOf(4, 5, 6), store.load(otherKeyId))
    }

    @Test
    fun missingEncryptionKeyDoesNotCreateReplacementOnRead() {
        store.store(keyId, byteArrayOf(7, 8, 9))
        keyStore().deleteEntry(alias)

        assertThrows(ControllerSecretException.Unavailable::class.java) { store.load(keyId) }
        assertFalse(keyStore().containsAlias(alias))
    }

    @Test
    fun oversizedCiphertextIsRejected() {
        fileFor(keyId).writeBytes(ByteArray(64 * 1024))
        assertThrows(ControllerSecretException.Corrupt::class.java) { store.load(keyId) }
        assertFalse(keyStore().containsAlias(alias))
    }

    private fun keyStore(): KeyStore = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }

    private fun fileFor(id: String): File {
        val hash = MessageDigest.getInstance("SHA-256").digest(id.encodeToByteArray())
            .joinToString("") { "%02x".format(it) }
        return File(context.noBackupFilesDir, "controller-device-secrets/$hash.blob")
    }
}
