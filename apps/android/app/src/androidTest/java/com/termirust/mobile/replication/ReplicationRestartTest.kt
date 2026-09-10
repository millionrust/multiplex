package com.termirust.mobile.replication

import android.os.Process
import android.util.Base64
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import com.termirust.replication.security.ReplicationCustody
import java.io.File
import java.io.FileOutputStream
import java.nio.file.Files
import java.security.KeyStore
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import kotlinx.coroutines.runBlocking

/** Three runner-invoked stages, with an explicit app-process stop between them. */
@RunWith(AndroidJUnit4::class)
class ReplicationRestartTest {
    private val context = InstrumentationRegistry.getInstrumentation().targetContext
    private val token = InstrumentationRegistry.getArguments().getString("fixture").orEmpty().also {
        require(it.matches(Regex("[a-f0-9-]{36}"))) { "Runner fixture token required" }
    }
    private val store = ReplicationKeystoreStore(context, "test-$token")
    private val path = File(context.noBackupFilesDir, "replication-c01-$token.json")
    private val enrollmentName = "enrollment-$token"
    private fun enrollment() = NativeEnrollmentRepository(context, enrollmentName, "test-$token")

    @Test
    fun prepare() {
        assertFalse(path.exists())
        ReplicationCustody(store).use { custody ->
            val one = custody.createDeviceIdentity()
            val two = custody.createDeviceIdentity()
            val value = JSONObject().put("pid", Process.myPid())
                .put("oneRef", encode(one.secretReference)).put("onePublic", encode(one.publicKey))
                .put("twoRef", encode(two.secretReference)).put("twoPublic", encode(two.publicKey))
                .put("enrollment", encode(runBlocking { enrollment().prepare().request!! }))
            FileOutputStream(path).use { it.write(value.toString().toByteArray()); it.fd.sync() }
        }
    }

    @Test
    fun reopen() {
        val saved = JSONObject(path.readText())
        assertNotEquals(saved.getInt("pid"), Process.myPid())
        val expectedRequest = decode(saved.getString("enrollment"))
        assertArrayEquals(expectedRequest, runBlocking { enrollment().load().request })
        assertNull(runBlocking { enrollment().cancel(expectedRequest).request })
        ReplicationCustody(store).use { custody ->
            for (prefix in listOf("one", "two")) {
                assertArrayEquals(decode(saved.getString(prefix + "Public")), custody.devicePublicKey(decode(saved.getString(prefix + "Ref"))))
            }
            assertTrue(custody.deleteDeviceIdentity(decode(saved.getString("oneRef"))))
            assertFalse(custody.deleteDeviceIdentity(decode(saved.getString("oneRef"))))
            assertArrayEquals(decode(saved.getString("twoPublic")), custody.devicePublicKey(decode(saved.getString("twoRef"))))
        }
    }

    @Test
    fun cleanup() {
        val directory = File(context.noBackupFilesDir, enrollmentName)
        if (directory.exists()) Files.walk(directory.toPath()).use { entries ->
            entries.sorted(Comparator.reverseOrder()).forEach(Files::delete)
        }
        KeyStore.getInstance("AndroidKeyStore").apply { load(null) }.deleteEntry(store.alias)
        if (store.directory.exists()) Files.walk(store.directory.toPath()).use { entries ->
            entries.sorted(Comparator.reverseOrder()).forEach(Files::delete)
        }
        if (path.exists()) assertTrue(path.delete())
    }

    private fun encode(bytes: ByteArray) = Base64.encodeToString(bytes, Base64.NO_WRAP)
    private fun decode(text: String) = Base64.decode(text, Base64.NO_WRAP)
}
