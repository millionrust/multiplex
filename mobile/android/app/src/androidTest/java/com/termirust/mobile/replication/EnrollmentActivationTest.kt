package com.termirust.mobile.replication

import android.os.Process
import androidx.test.platform.app.InstrumentationRegistry
import com.termirust.replication.security.MobileReplicationException
import com.termirust.replication.security.ReplicationSecureStore
import com.termirust.replication.security.ReplicationStorageException
import kotlinx.coroutines.runBlocking
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import java.io.File
import java.security.MessageDigest

/** Real JNI and Keystore with a fixture-owned publication obstruction, across process stops. */
class EnrollmentActivationTest {
    private val context = InstrumentationRegistry.getInstrumentation().targetContext
    private val token = InstrumentationRegistry.getArguments().getString("fixture").orEmpty().also {
        require(it.matches(Regex("[a-f0-9-]{36}")))
    }
    private val name = "c07-$token"
    private val root = File(context.noBackupFilesDir, name)
    private val enrollment = File(root, "enrollment")
    private val exchange = File(context.getExternalFilesDir(null), name)
    private val store = ReplicationKeystoreStore(context, name)
    private val journal = File(enrollment, "enrollment.transaction.json")
    private val pending = File(enrollment, "pending-enrollment.json")
    private val obstruction = File(enrollment, "profile.json")
    private fun repository(backend: ReplicationSecureStore = store) =
        NativeEnrollmentRepository(context, name, name, backend)

    private fun custodyDigests(): Map<String, String> = store.directory.listFiles().orEmpty()
        .filter { it.isFile }.associate { file ->
            file.name to MessageDigest.getInstance("SHA-256").digest(file.readBytes())
                .joinToString("") { "%02x".format(it.toInt() and 255) }
        }

    @Test fun failPreparedCreation() = runBlocking {
        assertNotEquals(File(exchange, "pid").readText().toInt(), Process.myPid())
        val request = repository().load().request!!
        val originalPending = pending.readBytes()
        val bundle = File(exchange, "bundle.json").readBytes()
        val code = JSONObject(File(exchange, "review.json").readText()).getString("code")
        for (committed in listOf(false, true)) {
            var attempted: String? = null
            val failed = object : ReplicationSecureStore by store {
                override fun create(account: String, value: ByteArray) {
                    check(JSONObject(journal.readText()).getBoolean("custody_prepared"))
                    attempted = account
                    if (committed) store.create(account, value) else value.fill(0)
                    throw ReplicationStorageException.Unavailable()
                }
            }
            val review = repository().reviewBundle(request, bundle)
            try { repository(failed).acceptBundle(review, code); fail("Injected creation failure must surface") }
            catch (_: MobileReplicationException) { }
            val account = requireNotNull(attempted)
            assertTrue(JSONObject(journal.readText()).getBoolean("custody_prepared"))
            assertArrayEquals(originalPending, pending.readBytes())
            if (committed) {
                store.load(account).fill(0)
                File(root, "uncertain-epoch-account").writeText(account)
                File(root, "uncertain-journal-before").writeBytes(journal.readBytes())
                File(root, "uncertain-pending-before").writeBytes(originalPending)
            } else {
                assertThrows(ReplicationStorageException.Missing::class.java) { store.load(account) }
                val originalJournal = journal.readBytes()
                val originalCustody = custodyDigests()
                for (failure in listOf(
                    ReplicationStorageException.Locked(),
                    ReplicationStorageException.Missing(),
                    ReplicationStorageException.Invalid(),
                )) {
                    var deniedLoads = 0
                    val inaccessibleIdentity = object : ReplicationSecureStore by store {
                        override fun load(candidate: String): ByteArray {
                            if (candidate == account) return store.load(candidate)
                            deniedLoads++
                            throw failure
                        }
                        override fun create(candidate: String, value: ByteArray) {
                            value.fill(0)
                            fail("Recovery must not replace identity custody")
                        }
                        override fun delete(candidate: String): Boolean {
                            fail("An unused intent has no custody to delete")
                            return false
                        }
                    }
                    try {
                        repository(inaccessibleIdentity).recover()
                        fail("Unavailable identity must preserve the unused intent")
                    } catch (_: MobileReplicationException) { }
                    assertTrue(deniedLoads > 0)
                    assertArrayEquals(originalJournal, journal.readBytes())
                    assertArrayEquals(originalPending, pending.readBytes())
                    assertEquals(originalCustody, custodyDigests())
                }
                assertArrayEquals(request, repository().recover().request)
                assertFalse(journal.exists())
                assertArrayEquals(originalPending, pending.readBytes())
            }
        }
        File(exchange, "pid").writeText(Process.myPid().toString())
    }

    @Test fun recoverPreparedCreation() = runBlocking {
        assertNotEquals(File(exchange, "pid").readText().toInt(), Process.myPid())
        val account = File(root, "uncertain-epoch-account").readText()
        val before = custodyDigests()
        try { repository().recover(); fail("Unconfirmed creation must not be adopted or deleted") }
        catch (_: MobileReplicationException.RecoveryRequired) { }
        assertEquals(before, custodyDigests())
        assertArrayEquals(File(root, "uncertain-journal-before").readBytes(), journal.readBytes())
        assertArrayEquals(File(root, "uncertain-pending-before").readBytes(), pending.readBytes())
        store.load(account).fill(0)
        // The fixture owns this injected write. This is test cleanup, not a product recovery action.
        assertTrue(store.delete(account))
        assertArrayEquals(File(exchange, "request.json").readBytes(), repository().recover().request)
        assertFalse(journal.exists())
        File(exchange, "pid").writeText(Process.myPid().toString())
    }

    @Test fun failActivation() = runBlocking {
        assertNotEquals(File(exchange, "pid").readText().toInt(), Process.myPid())
        val request = repository().load().request!!
        File(root, "pending-before-activation").writeBytes(pending.readBytes())
        var createdAccount: String? = null
        val blocked = object : ReplicationSecureStore by store {
            override fun create(account: String, value: ByteArray) {
                store.create(account, value)
                check(createdAccount == null)
                createdAccount = account
                check(obstruction.mkdir())
            }
        }
        val bytes = File(exchange, "bundle.json").readBytes()
        val code = JSONObject(File(exchange, "review.json").readText()).getString("code")
        val review = repository().reviewBundle(request, bytes)
        try { repository(blocked).acceptBundle(review, code); fail("Profile obstruction must reject activation") }
        catch (_: MobileReplicationException) { }
        assertTrue(journal.isFile)
        assertTrue(obstruction.isDirectory)
        assertArrayEquals(File(root, "pending-before-activation").readBytes(), pending.readBytes())
        val account = requireNotNull(createdAccount)
        store.load(account).fill(0)
        File(root, "activation-epoch-account").writeText(account)
        File(root, "activation-journal-before-recovery").writeBytes(journal.readBytes())
        File(exchange, "pid").writeText(Process.myPid().toString())
    }

    @Test fun recoverActivation() = runBlocking {
        assertNotEquals(File(exchange, "pid").readText().toInt(), Process.myPid())
        val originalPending = File(root, "pending-before-activation").readBytes()
        val originalJournal = File(root, "activation-journal-before-recovery").readBytes()
        val account = File(root, "activation-epoch-account").readText()
        fun assertPreserved() {
            assertArrayEquals(originalPending, pending.readBytes())
            assertArrayEquals(originalJournal, journal.readBytes())
            store.load(account).fill(0)
        }
        assertPreserved()
        try { repository().recover(); fail("Unsafe profile must prevent recovery") }
        catch (_: MobileReplicationException) { }
        assertPreserved()
        assertTrue(obstruction.isDirectory)
        assertTrue(obstruction.delete())
        val deletionDenied = object : ReplicationSecureStore by store {
            override fun delete(candidate: String): Boolean {
                check(candidate == account)
                throw ReplicationStorageException.Unavailable()
            }
        }
        try { repository(deletionDenied).recover(); fail("Failed key deletion must preserve recovery") }
        catch (_: MobileReplicationException) { }
        assertPreserved()
        val recovered = repository().recover()
        assertFalse(recovered.configured)
        assertArrayEquals(File(exchange, "request.json").readBytes(), recovered.request)
        assertArrayEquals(originalPending, pending.readBytes())
        assertFalse(journal.exists())
        assertFalse(File(enrollment, "repository").exists())
        assertThrows(ReplicationStorageException.Missing::class.java) { store.load(account) }
        assertArrayEquals(recovered.request, repository().recover().request)
        File(exchange, "pid").writeText(Process.myPid().toString())
    }

    @Test fun prepareCommittedRecovery() = runBlocking {
        assertNotEquals(File(exchange, "pid").readText().toInt(), Process.myPid())
        assertTrue(repository().load().configured)
        assertFalse(journal.exists())
        assertFalse(pending.exists())
        val metadata = File(enrollment, "repository/replica.json").readBytes()
        val epochs = JSONObject(metadata.toString(Charsets.UTF_8))
            .getJSONObject("custody").getJSONArray("epoch_references")
        assertEquals(1, epochs.length())
        val reference = epochs.getJSONArray(0)
        val hex = (0 until reference.length()).joinToString("") { index ->
            val byte = reference.getInt(index)
            require(byte in 0..255)
            "%02x".format(byte)
        }
        // Reconstruct only unfinished cleanup; keep the actual committed profile and custody.
        pending.writeBytes(File(root, "pending-before-activation").readBytes())
        journal.writeText(JSONObject().put("format_version", 1).put("epoch_reference_hex", hex).toString())
        File(root, "committed-profile-before").writeBytes(obstruction.readBytes())
        File(root, "committed-repository-before").writeBytes(metadata)
        File(root, "committed-journal-before").writeBytes(journal.readBytes())
        File(root, "committed-custody-digests").writeText(JSONObject(custodyDigests()).toString())
        File(exchange, "pid").writeText(Process.myPid().toString())
    }

    @Test fun finishCommittedRecovery() = runBlocking {
        assertNotEquals(File(exchange, "pid").readText().toInt(), Process.myPid())
        val before = JSONObject(File(root, "committed-custody-digests").readText()).let { json ->
            json.keys().asSequence().associateWith { json.getString(it) }
        }
        fun assertCommittedUnchanged() {
            assertArrayEquals(File(root, "committed-profile-before").readBytes(), obstruction.readBytes())
            assertArrayEquals(File(root, "committed-repository-before").readBytes(),
                File(enrollment, "repository/replica.json").readBytes())
            assertEquals(before, custodyDigests())
        }
        var mutations = 0
        var deniedLoads = 0
        val immutable = object : ReplicationSecureStore by store {
            override fun create(account: String, value: ByteArray) {
                value.fill(0)
                mutations++
                throw ReplicationStorageException.Unavailable()
            }
            override fun delete(account: String): Boolean {
                mutations++
                throw ReplicationStorageException.Unavailable()
            }
        }
        val unavailable = object : ReplicationSecureStore by immutable {
            override fun load(account: String): ByteArray {
                deniedLoads++
                throw ReplicationStorageException.Unavailable()
            }
        }
        try { repository(unavailable).recover(); fail("Unavailable custody must prevent finalization") }
        catch (_: MobileReplicationException) { }
        assertTrue(deniedLoads > 0)
        assertEquals(0, mutations)
        assertCommittedUnchanged()
        assertArrayEquals(File(root, "committed-journal-before").readBytes(), journal.readBytes())
        assertArrayEquals(File(root, "pending-before-activation").readBytes(), pending.readBytes())
        assertTrue(repository(immutable).recover().configured)
        assertTrue(repository(immutable).recover().configured)
        assertEquals(0, mutations)
        assertFalse(pending.exists())
        assertFalse(journal.exists())
        assertCommittedUnchanged()
        File(exchange, "pid").writeText(Process.myPid().toString())
    }
}
