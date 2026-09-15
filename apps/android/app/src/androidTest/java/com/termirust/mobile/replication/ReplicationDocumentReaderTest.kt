package com.termirust.mobile.replication

import android.net.Uri
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
import android.os.CancellationSignal
import android.provider.DocumentsContract
import androidx.test.platform.app.InstrumentationRegistry
import org.junit.Assert.*
import org.junit.Test
import java.util.concurrent.ArrayBlockingQueue
import java.util.concurrent.TimeUnit

class ReplicationDocumentReaderTest {
    private val context = InstrumentationRegistry.getInstrumentation().targetContext
    private val reader = ReplicationDocumentReader(context.contentResolver)
    private fun uri(id: String) = DocumentsContract.buildDocumentUri(context.packageName + ".replication-transfer-fixture", id)
    private fun read(id: String) = reader.read(uri(id), ReplicationTransferKind.ENROLLMENT_BUNDLE, CancellationSignal())
    private fun fails(expected: ReplicationTransferFailure, action: () -> Unit) {
        try { action(); fail("Expected $expected") }
        catch (error: ReplicationTransferException) { assertEquals(expected, error.failure) }
    }

    @Test fun realProviderReadAndReopenPreserveBytes() {
        val expected = ByteArray(17) { it.toByte() }
        assertArrayEquals(expected, read("small"))
        assertArrayEquals(expected, ReplicationDocumentReader(context.contentResolver)
            .read(uri("small"), ReplicationTransferKind.ENROLLMENT_BUNDLE, CancellationSignal()))
    }
    @Test fun exactLimitAllowedAndOverflowRejected() {
        assertEquals(192 * 1024, read("limit").size)
        fails(ReplicationTransferFailure.TOO_LARGE) { read("oversized") }
        fails(ReplicationTransferFailure.EMPTY) { read("empty") }
        assertEquals(8 * 1024 * 1024, reader.read(uri("replica-limit"), ReplicationTransferKind.ENCRYPTED_REPLICA, CancellationSignal()).size)
        fails(ReplicationTransferFailure.TOO_LARGE) {
            reader.read(uri("replica-oversized"), ReplicationTransferKind.ENCRYPTED_REPLICA, CancellationSignal())
        }
    }
    @Test fun accessFailuresAreNotEmptySnapshots() {
        fails(ReplicationTransferFailure.DENIED) { read("denied") }
        fails(ReplicationTransferFailure.UNAVAILABLE) { read("missing") }
    }
    @Test fun providerFailureDiscardsPartialBytes() {
        fails(ReplicationTransferFailure.UNAVAILABLE) { read("broken") }
    }

    @Test fun externalUidGrantAndRevocationAreEnforced() {
        val testPackage = InstrumentationRegistry.getInstrumentation().context.packageName
        val replies = ArrayBlockingQueue<Bundle>(1)
        val connections = ArrayBlockingQueue<Messenger>(1)
        val thread = HandlerThread("transfer-permission-fixture").apply { start() }
        val reply = Messenger(Handler(thread.looper) { replies.offer(it.data); true })
        val connection = object : ServiceConnection {
            override fun onServiceConnected(name: ComponentName, binder: IBinder) { connections.offer(Messenger(binder)) }
            override fun onServiceDisconnected(name: ComponentName) {}
        }
        val selected = uri("small")
        var bound = false
        try {
            val intent = Intent().setComponent(ComponentName(testPackage, TransferPermissionProbeService::class.java.name))
            bound = context.bindService(intent, connection, Context.BIND_AUTO_CREATE)
            assertTrue(bound)
            val remote = requireNotNull(connections.poll(10, TimeUnit.SECONDS))
            fun probe(): Bundle {
                remote.send(Message.obtain().apply {
                    replyTo = reply
                    data = Bundle().apply { putString("uri", selected.toString()) }
                })
                val result = requireNotNull(replies.poll(10, TimeUnit.SECONDS))
                assertNotEquals(Process.myUid(), result.getInt("uid"))
                return result
            }
            assertEquals("denied", probe().getString("status"))
            context.grantUriPermission(testPackage, selected, Intent.FLAG_GRANT_READ_URI_PERMISSION)
            val granted = probe()
            assertEquals("read", granted.getString("status"))
            assertArrayEquals(ByteArray(17) { it.toByte() }, granted.getByteArray("bytes"))
            context.revokeUriPermission(testPackage, selected, Intent.FLAG_GRANT_READ_URI_PERMISSION)
            assertEquals("denied", probe().getString("status"))
            assertEquals(17, read("small").size)
        } finally {
            context.revokeUriPermission(testPackage, selected, Intent.FLAG_GRANT_READ_URI_PERMISSION)
            if (bound) context.unbindService(connection)
            thread.quitSafely()
            thread.join(5000)
        }
    }
    @Test fun cancelledReadAndFilesystemUriRejected() {
        fails(ReplicationTransferFailure.CANCELLED) {
            reader.read(uri("small"), ReplicationTransferKind.ENROLLMENT_BUNDLE, CancellationSignal().apply { cancel() })
        }
        fails(ReplicationTransferFailure.INVALID_LOCATION) {
            reader.read(Uri.parse("file:///private/replica.json"), ReplicationTransferKind.ENCRYPTED_REPLICA, CancellationSignal())
        }
    }
    @Test fun publicationIsAlwaysUnsupported() {
        fails(ReplicationTransferFailure.UNSUPPORTED_PUBLICATION) { reader.publish() }
        assertEquals(17, read("small").size)
    }
}
