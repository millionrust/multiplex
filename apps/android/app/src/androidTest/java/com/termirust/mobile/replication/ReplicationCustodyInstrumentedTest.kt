package com.termirust.mobile.replication

import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.content.ServiceConnection
import android.os.Bundle
import android.os.Handler
import android.os.HandlerThread
import android.os.IBinder
import android.os.Message
import android.os.Messenger
import android.os.Process
import android.os.SystemClock
import android.security.keystore.UserNotAuthenticatedException
import android.system.Os
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import com.termirust.mobile.controller.ControllerSecureBlobStore
import com.termirust.controller.security.ControllerSecurityEngine
import com.termirust.replication.security.ReplicationCustody
import com.termirust.replication.security.MobileReplicationProduct
import com.termirust.replication.security.MobileReplicationException
import com.termirust.replication.security.ReplicationStorageException
import java.io.File
import java.io.RandomAccessFile
import java.nio.file.Files
import java.security.KeyStore
import java.util.UUID
import java.util.concurrent.CountDownLatch
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicReference
import org.junit.After
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class ReplicationCustodyInstrumentedTest {
    private val context = InstrumentationRegistry.getInstrumentation().targetContext
    private val namespace = "test-${UUID.randomUUID()}"
    private val store = ReplicationKeystoreStore(context, namespace)
    private val controllerAlias = "c01-controller-$namespace"
    private val controllerAccount = "c01-$namespace"

    @Test
    fun productEnrollmentReopensAndCancelsThroughRealKeystore() {
        val folder = File(context.noBackupFilesDir, "product-$namespace").canonicalFile
        check(folder.mkdir())
        try {
            ReplicationCustody(store).use { custody ->
                val sentinel = custody.createDeviceIdentity()
                MobileReplicationProduct(folder.path, store).use { facade ->
                    assertNull(facade.pendingEnrollment())
                    val request = facade.prepareEnrollment(folder.path)
                    facade.recoverPendingEnrollment()
                    assertArrayEquals(request.canonicalRequest, facade.pendingEnrollment()!!.canonicalRequest)
                    val invalidJournal = File(folder, "enrollment/enrollment.transaction.json")
                    invalidJournal.writeText("invalid activation journal")
                    assertThrows(MobileReplicationException.Invalid::class.java) { facade.recoverPendingEnrollment() }
                    assertEquals("invalid activation journal", invalidJournal.readText())
                    assertArrayEquals(sentinel.publicKey, custody.devicePublicKey(sentinel.secretReference))
                    check(invalidJournal.delete())
                    assertTrue(request.canonicalRequest.isNotEmpty())
                    assertThrows(MobileReplicationException.Invalid::class.java) {
                        facade.reviewEnrollment(request.canonicalRequest, byteArrayOf())
                    }
                    assertThrows(MobileReplicationException.Invalid::class.java) {
                        facade.acceptEnrollment(request.canonicalRequest, byteArrayOf(), "wrong-workspace", "wrong-code")
                    }
                    assertThrows(MobileReplicationException.Invalid::class.java) {
                        facade.applyRecordTransfer(byteArrayOf(), "desktop-profiles", "fixture-host", ByteArray(31))
                    }
                    assertThrows(MobileReplicationException.Invalid::class.java) {
                        facade.reviewRecordTransfer(byteArrayOf(), "", "fixture-host")
                    }
                    assertThrows(MobileReplicationException.Invalid::class.java) {
                        facade.reviewHostTransfer(byteArrayOf(), "")
                    }
                    assertThrows(MobileReplicationException.Invalid::class.java) {
                        facade.applyHostTransfer(byteArrayOf(), "fixture-host", ByteArray(31))
                    }
                    assertThrows(MobileReplicationException.Invalid::class.java) {
                        facade.previewHostTransfers(byteArrayOf())
                    }
                    assertThrows(MobileReplicationException.Invalid::class.java) {
                        facade.importedHosts()
                    }
                    assertArrayEquals(request.canonicalRequest, facade.pendingEnrollment()!!.canonicalRequest)
                    assertThrows(MobileReplicationException.AlreadyConfigured::class.java) { facade.prepareEnrollment(folder.path) }
                    MobileReplicationProduct(folder.path, ReplicationKeystoreStore(context, namespace)).use { reopened ->
                        assertArrayEquals(request.canonicalRequest, reopened.pendingEnrollment()!!.canonicalRequest)
                        assertTrue(reopened.cancelPendingEnrollment(request.canonicalRequest))
                        assertFalse(reopened.cancelPendingEnrollment(request.canonicalRequest))
                        assertNull(reopened.pendingEnrollment())
                    }
                    assertArrayEquals(sentinel.publicKey, custody.devicePublicKey(sentinel.secretReference))
                    assertThrows(MobileReplicationException.Invalid::class.java) { facade.prepareEnrollment("content://provider/test") }
                }
            }
        } finally {
            Files.walk(folder.toPath()).use { paths -> paths.sorted(Comparator.reverseOrder()).forEach(Files::delete) }
        }
    }

    @After
    fun cleanup() {
        keyStore().deleteEntry(store.alias)
        // All process fixtures are unbound and finished before this namespace is removed.
        if (store.directory.exists()) Files.walk(store.directory.toPath()).use { paths ->
            paths.sorted(Comparator.reverseOrder()).forEach(Files::delete)
        }
        ControllerSecureBlobStore(context, controllerAlias).delete(controllerAccount)
        keyStore().deleteEntry(controllerAlias)
    }

    @Test
    fun realRustIdentitiesReopenAndDeleteWithoutAffectingController() {
        val controller = ControllerSecureBlobStore(context, controllerAlias)
        controller.store(controllerAccount, byteArrayOf(1, 2, 3))
        ControllerSecurityEngine(controller).use { assertEquals(1, it.protocolVersion().major.toInt()) }
        ReplicationCustody(store).use { engine ->
            val one = engine.createDeviceIdentity()
            val two = engine.createDeviceIdentity()
            ReplicationCustody(ReplicationKeystoreStore(context, namespace)).use { reopened ->
                assertArrayEquals(one.publicKey, reopened.devicePublicKey(one.secretReference))
                assertArrayEquals(two.publicKey, reopened.devicePublicKey(two.secretReference))
                assertTrue(reopened.deleteDeviceIdentity(one.secretReference))
                assertFalse(reopened.deleteDeviceIdentity(one.secretReference))
                assertThrows(ReplicationStorageException.Missing::class.java) { reopened.devicePublicKey(one.secretReference) }
                assertArrayEquals(two.publicKey, reopened.devicePublicKey(two.secretReference))
            }
        }
        assertArrayEquals(byteArrayOf(1, 2, 3), controller.load(controllerAccount))
        ControllerSecurityEngine(controller).use { assertEquals(1, it.protocolVersion().major.toInt()) }
    }

    @Test
    fun independentInstancesRaceWithoutOverwrite() {
        val gate = CountDownLatch(1)
        val executor = Executors.newFixedThreadPool(2)
        try {
            val futures = (1..2).map { byte -> executor.submit<String> {
                gate.await()
                try {
                    ReplicationKeystoreStore(context, namespace).create("race", secret(byte))
                    "created-$byte"
                } catch (_: ReplicationStorageException.Collision) { "collision" }
            } }
            gate.countDown()
            val results = futures.map { it.get(15, TimeUnit.SECONDS) }
            assertEquals(1, results.count { it == "collision" })
            val winner = results.single { it.startsWith("created") }.last().digitToInt()
            assertArrayEquals(secret(winner), store.load("race"))
        } finally { executor.shutdownNow() }
    }

    @Test
    fun separateProcessRaceUsesTheSameNamespaceLock() {
        val thread = HandlerThread("replication-test-replies").apply { start() }
        val connected = CountDownLatch(1)
        val finished = CountDownLatch(1)
        val remote = AtomicReference<Messenger>()
        val reply = AtomicReference<Bundle>()
        val connection = object : ServiceConnection {
            override fun onServiceConnected(name: ComponentName, service: IBinder) { remote.set(Messenger(service)); connected.countDown() }
            override fun onServiceDisconnected(name: ComponentName) = Unit
        }
        val intent = Intent().setClassName(context, "com.termirust.mobile.replication.ReplicationCustodyTestService")
        val bound = context.bindService(intent, connection, Context.BIND_AUTO_CREATE)
        try {
            assertTrue(bound)
            assertTrue(connected.await(10, TimeUnit.SECONDS))
            val start = SystemClock.elapsedRealtime() + 500
            remote.get().send(Message.obtain().apply {
                data = Bundle().apply { putString("namespace", namespace); putLong("start", start) }
                replyTo = Messenger(Handler(thread.looper) { reply.set(it.data); finished.countDown(); true })
            })
            while (SystemClock.elapsedRealtime() < start) Thread.sleep(5)
            val local = try { store.create("race", secret(7)); "created" }
                catch (_: ReplicationStorageException.Collision) { "collision" }
            assertTrue(finished.await(15, TimeUnit.SECONDS))
            assertNotEquals(Process.myPid(), reply.get().getInt("pid"))
            assertEquals(setOf("created", "collision"), setOf(local, reply.get().getString("result")))
            assertArrayEquals(secret(if (local == "created") 7 else 9), store.load("race"))
        } finally {
            if (bound) context.unbindService(connection)
            context.stopService(intent)
            thread.quitSafely()
            thread.join(2_000)
        }
    }

    @Test
    fun backupRecoveryAndPendingCreationDoNotReplaceCommittedData() {
        store.create("one", ByteArray(47) { 3 })
        val base = store.fileFor("one")
        val backup = File(base.path + ".bak")
        val pending = File(base.path + ".new")
        assertTrue(base.renameTo(backup))
        pending.writeBytes(ByteArray(76) { 9 })
        assertArrayEquals(ByteArray(47) { 3 }, store.load("one"))
        assertTrue(base.exists())
        assertFalse(backup.exists())
        assertThrows(ReplicationStorageException.Collision::class.java) { store.create("one", ByteArray(47) { 4 }) }
        base.copyTo(backup)
        assertTrue(store.delete("one"))
        assertFalse(store.delete("one"))
        assertFalse(base.exists() || backup.exists() || pending.exists())
    }

    @Test
    fun corruptAndOversizedCiphertextIsPreservedAndAccountBound() {
        store.create("one", ByteArray(47) { 3 })
        val base = store.fileFor("one")
        val valid = base.readBytes()
        val cases = listOf(byteArrayOf(), valid.copyOf(75), valid + byteArrayOf(0), ByteArray(65536),
            valid.clone().apply { this[0] = 2 }, valid.clone().apply { this[75] = (this[75].toInt() xor 1).toByte() })
        cases.forEach { damaged ->
            base.writeBytes(damaged)
            assertThrows(ReplicationStorageException.Invalid::class.java) { store.load("one") }
            assertArrayEquals(damaged, base.readBytes())
        }
        base.writeBytes(valid)
        base.copyTo(store.fileFor("two"))
        assertThrows(ReplicationStorageException.Invalid::class.java) { store.load("two") }
        assertArrayEquals(valid, store.fileFor("two").readBytes())
        assertArrayEquals(ByteArray(47) { 3 }, store.load("one"))
    }

    @Test
    fun missingKeyCannotBeReplacedWhenCiphertextExists() {
        store.create("one", ByteArray(47) { 3 })
        val original = store.fileFor("one").readBytes()
        keyStore().deleteEntry(store.alias)
        assertThrows(ReplicationStorageException.Unavailable::class.java) { store.load("one") }
        assertThrows(ReplicationStorageException.Unavailable::class.java) { store.create("two", ByteArray(47) { 4 }) }
        assertFalse(keyStore().containsAlias(store.alias))
        assertArrayEquals(original, store.fileFor("one").readBytes())
        assertFalse(store.fileFor("two").exists())
    }

    @Test
    fun invalidReferencesAndAccountsFailBeforeAccess() {
        InstrumentationRegistry.getInstrumentation().runOnMainSync {
            assertThrows(ReplicationStorageException.Unavailable::class.java) { store.load("main-thread") }
        }
        listOf("", "a b", "a\nb", "\u00e9", "a".repeat(129)).forEach { account ->
            assertThrows(ReplicationStorageException.Invalid::class.java) { store.load(account) }
            assertThrows(ReplicationStorageException.Invalid::class.java) { store.create(account, ByteArray(47)) }
            assertThrows(ReplicationStorageException.Invalid::class.java) { store.delete(account) }
        }
        ReplicationCustody(store).use { engine ->
            val wrongKind = "TRRF".toByteArray() + byteArrayOf(0, 1, 1) + ByteArray(8) + ByteArray(32) { 1 }
            assertThrows(ReplicationStorageException.Invalid::class.java) { engine.devicePublicKey(wrongKind) }
            assertThrows(ReplicationStorageException.Invalid::class.java) { engine.deleteDeviceIdentity(wrongKind) }
        }
        assertFalse(store.directory.exists())
        assertFalse(keyStore().containsAlias(store.alias))
    }

    @Test
    fun lockTimeoutAndInterruptionDoNotPublish() {
        store.create("one", ByteArray(47) { 3 })
        RandomAccessFile(File(store.directory, ReplicationKeystoreStore.LOCK_FILE), "rw").use { file ->
            file.channel.lock().use {
                val start = SystemClock.elapsedRealtime()
                assertThrows(ReplicationStorageException.Unavailable::class.java) { store.create("two", ByteArray(47) { 4 }) }
                assertTrue(SystemClock.elapsedRealtime() - start in 1800..4000)
                val result = AtomicReference<Throwable>()
                val worker = Thread {
                    try { store.create("three", ByteArray(47) { 5 }) } catch (error: Throwable) { result.set(error) }
                }.apply { start() }
                Thread.sleep(100)
                worker.interrupt()
                worker.join(3_000)
                assertFalse(worker.isAlive)
                assertTrue(result.get() is ReplicationStorageException.Unavailable)
            }
        }
        assertFalse(store.fileFor("two").exists())
        assertFalse(store.fileFor("three").exists())
        assertArrayEquals(ByteArray(47) { 3 }, store.load("one"))
    }

    @Test
    fun symlinkAndUndeletableRecoveryStateAreNotSilentlyAccepted() {
        store.create("one", ByteArray(47) { 3 })
        val base = store.fileFor("one")
        val backup = File(base.path + ".bak")
        assertTrue(backup.mkdir())
        assertThrows(ReplicationStorageException.Invalid::class.java) { store.delete("one") }
        assertTrue(base.exists())
        assertTrue(backup.delete())
        Os.symlink(base.path, store.fileFor("alias").path)
        assertThrows(ReplicationStorageException.Invalid::class.java) { store.load("alias") }
        assertThrows(ReplicationStorageException.Invalid::class.java) { store.create("alias", ByteArray(47)) }
        assertArrayEquals(ByteArray(47) { 3 }, store.load("one"))

        val moved = File(store.directory.path + "-original")
        assertTrue(store.directory.renameTo(moved))
        Os.symlink(moved.path, store.directory.path)
        try {
            assertThrows(ReplicationStorageException.Invalid::class.java) { store.load("one") }
            assertThrows(ReplicationStorageException.Invalid::class.java) { store.create("two", ByteArray(47)) }
        } finally {
            Files.delete(store.directory.toPath())
            assertTrue(moved.renameTo(store.directory))
        }
    }

    @Test
    fun simulatedAccessDenialRemainsLocked() {
        assertTrue(ReplicationKeystoreStore.mapFailure(SecurityException()) is ReplicationStorageException.Locked)
        assertTrue(ReplicationKeystoreStore.mapFailure(UserNotAuthenticatedException()) is ReplicationStorageException.Locked)
        assertFalse(store.directory.exists())
    }

    private fun keyStore(): KeyStore = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }

    private fun secret(value: Int) = "TRSC".toByteArray() + byteArrayOf(0, 1, 2) + ByteArray(8) + ByteArray(32) { value.toByte() }
}
