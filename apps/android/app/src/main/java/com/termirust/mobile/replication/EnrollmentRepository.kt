package com.termirust.mobile.replication

import android.content.Context
import android.util.AtomicFile
import com.termirust.replication.security.MobileReplicationProduct
import com.termirust.replication.security.MobileReplicationException
import com.termirust.replication.security.ReplicationSecureStore
import java.io.File
import java.nio.file.Files
import java.nio.file.NoSuchFileException
import java.nio.file.LinkOption
import java.nio.file.attribute.BasicFileAttributes
import java.nio.channels.FileChannel
import java.nio.channels.OverlappingFileLockException
import java.nio.file.StandardOpenOption
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

data class EnrollmentSnapshot(val request: ByteArray? = null, val cancellationPending: Boolean = false, val configured: Boolean = false)

class EnrollmentBundleReview(request: ByteArray, bytes: ByteArray, val workspace: String, val recipient: String, val code: String) {
    private val capturedRequest = request.copyOf()
    private val capturedBytes = bytes.copyOf()
    fun requestBytes(): ByteArray = capturedRequest.copyOf()
    fun bundleBytes(): ByteArray = capturedBytes.copyOf()
}

interface EnrollmentRepository {
    suspend fun load(): EnrollmentSnapshot
    suspend fun prepare(): EnrollmentSnapshot
    suspend fun cancel(request: ByteArray): EnrollmentSnapshot
    suspend fun reviewBundle(request: ByteArray, bytes: ByteArray): EnrollmentBundleReview
    suspend fun acceptBundle(review: EnrollmentBundleReview, verifiedCode: String): EnrollmentSnapshot
    suspend fun recover(): EnrollmentSnapshot
}

/** Only app-private local exchange staging. No provider transport or automatic enrollment. */
class NativeEnrollmentRepository(
    context: Context,
    private val directoryName: String = "replication-product-v1",
    private val custodyNamespace: String = "device",
    private val secureStore: ReplicationSecureStore? = null,
) : EnrollmentRepository, HostImportRepository {
    private val context = context.applicationContext
    private val root: File get() = File(context.noBackupFilesDir, directoryName).canonicalFile
    private fun journal() = AtomicFile(File(root, "reviewed-cancellation.json"))

    private fun facade(): MobileReplicationProduct {
        require(directoryName.matches(Regex("[a-zA-Z0-9-]{1,100}")))
        val directory = root
        if (!directory.isDirectory && !directory.mkdir()) throw MobileReplicationException.Unavailable()
        return MobileReplicationProduct(directory.path, secureStore ?: ReplicationKeystoreStore(context, custodyNamespace))
    }

    private fun <T> guarded(block: () -> T): T {
        require(directoryName.matches(Regex("[a-zA-Z0-9-]{1,100}")))
        val raw = File(context.noBackupFilesDir, directoryName)
        if (Files.isSymbolicLink(raw.toPath())) throw MobileReplicationException.Invalid()
        if (!root.isDirectory && !root.mkdir()) throw MobileReplicationException.Unavailable()
        FileChannel.open(File(root, "ui-operation.lock").toPath(), StandardOpenOption.CREATE,
            StandardOpenOption.WRITE, LinkOption.NOFOLLOW_LINKS).use { channel ->
            val lock = try { channel.tryLock() } catch (_: OverlappingFileLockException) { null }
            if (lock == null) throw MobileReplicationException.Busy()
            lock.use { return block() }
        }
    }

    private fun reviewed(): ByteArray? {
        val file = journal()
        val backup = File(file.baseFile.path + ".bak")
        val candidate = try {
            Files.readAttributes(backup.toPath(), BasicFileAttributes::class.java, LinkOption.NOFOLLOW_LINKS)
            backup
        } catch (_: NoSuchFileException) { file.baseFile }
        try {
            val attrs = Files.readAttributes(candidate.toPath(), BasicFileAttributes::class.java, LinkOption.NOFOLLOW_LINKS)
            if (!attrs.isRegularFile || attrs.size() !in 1..4096) throw MobileReplicationException.Invalid()
        } catch (_: NoSuchFileException) { return null }
        val bytes = file.openRead().use { input ->
            val buffer = ByteArray(4097)
            var length = 0
            while (length < buffer.size) {
                val count = input.read(buffer, length, buffer.size - length)
                if (count < 0) break
                length += count
            }
            buffer.copyOf(length)
        }
        if (bytes.size !in 1..4096) throw MobileReplicationException.Invalid()
        return bytes
    }

    override suspend fun load(): EnrollmentSnapshot = withContext(Dispatchers.IO) {
        guarded {
            facade().use { product ->
                val pendingCancel = reviewed()
                if (pendingCancel != null) EnrollmentSnapshot(pendingCancel, true)
                else try { EnrollmentSnapshot(product.pendingEnrollment()?.canonicalRequest) }
                catch (_: MobileReplicationException.AlreadyConfigured) { EnrollmentSnapshot(configured = true) }
            }
        }
    }

    override suspend fun reviewBundle(request: ByteArray, bytes: ByteArray): EnrollmentBundleReview = withContext(Dispatchers.IO) {
        guarded {
            if (reviewed() != null) throw MobileReplicationException.RecoveryRequired()
            if (bytes.size !in 1..(192 * 1024)) throw MobileReplicationException.Invalid()
            facade().use { product ->
                val review = product.reviewEnrollment(request, bytes)
                EnrollmentBundleReview(request, bytes, review.workspaceId, review.recipient, review.verificationCode)
            }
        }
    }

    override suspend fun acceptBundle(review: EnrollmentBundleReview, verifiedCode: String): EnrollmentSnapshot = withContext(Dispatchers.IO) {
        guarded {
            if (reviewed() != null) throw MobileReplicationException.RecoveryRequired()
            if (verifiedCode != review.code) throw MobileReplicationException.Invalid()
            facade().use { product ->
                // Rust owns recoverable activation. Never retry an uncertain mutation here.
                product.acceptEnrollment(review.requestBytes(), review.bundleBytes(), review.workspace, verifiedCode)
                EnrollmentSnapshot(configured = true)
            }
        }
    }

    override suspend fun prepare(): EnrollmentSnapshot = withContext(Dispatchers.IO) {
        guarded {
            facade().use { product ->
                if (reviewed() != null) throw MobileReplicationException.RecoveryRequired()
                val exchange = File(root, "local-exchange")
                if (!exchange.isDirectory && !exchange.mkdir()) throw MobileReplicationException.Unavailable()
                EnrollmentSnapshot(product.prepareEnrollment(exchange.canonicalPath).canonicalRequest)
            }
        }
    }

    override suspend fun recover(): EnrollmentSnapshot = withContext(Dispatchers.IO) {
        guarded {
            if (reviewed() != null) throw MobileReplicationException.RecoveryRequired()
            facade().use { product ->
                product.recoverPendingEnrollment()
                try { EnrollmentSnapshot(product.pendingEnrollment()?.canonicalRequest) }
                catch (_: MobileReplicationException.AlreadyConfigured) { EnrollmentSnapshot(configured = true) }
            }
        }
    }

    override suspend fun importedHosts(): List<ImportedHost> = withContext(Dispatchers.IO) {
        guarded {
            if (reviewed() != null) throw MobileReplicationException.RecoveryRequired()
            facade().use { product -> product.importedHosts().map { ImportedHost(it.recordId, it.label, it.host, it.port.toInt(), it.username) } }
        }
    }

    override suspend fun previewHosts(bytes: ByteArray): List<ImportedHost> = withContext(Dispatchers.IO) {
        guarded {
            if (reviewed() != null) throw MobileReplicationException.RecoveryRequired()
            facade().use { product -> product.previewHostTransfers(bytes).map { ImportedHost(it.recordId, it.label, it.host, it.port.toInt(), it.username) } }
        }
    }

    override suspend fun reviewHost(bytes: ByteArray, recordId: String): HostImportReview = withContext(Dispatchers.IO) {
        guarded {
            if (reviewed() != null) throw MobileReplicationException.RecoveryRequired()
            facade().use { product ->
                val review = product.reviewHostTransfer(bytes, recordId)
                HostImportReview(bytes, review.reviewToken,
                    ImportedHost(recordId, review.label, review.host, review.port.toInt(), review.username), review.changesLocal)
            }
        }
    }

    override suspend fun importHost(review: HostImportReview): Unit = withContext(Dispatchers.IO) {
        guarded {
            if (reviewed() != null) throw MobileReplicationException.RecoveryRequired()
            facade().use { it.applyHostTransfer(review.documentBytes(), review.host.recordId, review.tokenBytes()) }
        }
    }

    override suspend fun cancel(request: ByteArray): EnrollmentSnapshot = withContext(Dispatchers.IO) {
        guarded {
            if (request.size !in 1..4096) throw MobileReplicationException.Invalid()
            facade().use { product ->
                // Persist the reviewed public request before mutation, so failed deletion is recoverable.
                val file = journal()
                val previous = reviewed()
                if (previous != null && !previous.contentEquals(request)) throw MobileReplicationException.StaleRequest()
                val output = file.startWrite()
                try { output.write(request); file.finishWrite(output) }
                catch (error: Exception) { file.failWrite(output); throw error }
                try { product.cancelPendingEnrollment(request) }
                catch (error: MobileReplicationException.StaleRequest) { file.delete(); throw error }
                catch (error: MobileReplicationException.AlreadyConfigured) { file.delete(); throw error }
                file.delete()
                if (reviewed() != null) throw MobileReplicationException.Unavailable()
                EnrollmentSnapshot()
            }
        }
    }
}
