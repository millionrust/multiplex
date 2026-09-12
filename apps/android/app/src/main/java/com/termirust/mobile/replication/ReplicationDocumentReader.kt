package com.termirust.mobile.replication

import android.content.ContentResolver
import android.net.Uri
import android.os.CancellationSignal
import android.os.OperationCanceledException
import androidx.annotation.WorkerThread
import java.io.ByteArrayOutputStream
import java.io.IOException

enum class ReplicationTransferKind(val maximumBytes: Int) {
    ENROLLMENT_BUNDLE(192 * 1024), ENCRYPTED_REPLICA(8 * 1024 * 1024)
}

enum class ReplicationTransferFailure { INVALID_LOCATION, EMPTY, TOO_LARGE, DENIED, UNAVAILABLE, CANCELLED, UNSUPPORTED_PUBLICATION }
class ReplicationTransferException(val failure: ReplicationTransferFailure) : Exception(failure.name)

/** Reads untrusted transfer bytes; only Rust may authenticate or apply them. */
class ReplicationDocumentReader(private val resolver: ContentResolver) {
    @WorkerThread
    fun read(uri: Uri, kind: ReplicationTransferKind, cancellation: CancellationSignal): ByteArray {
        if (uri.scheme != "content" || uri.authority.isNullOrBlank()) {
            throw ReplicationTransferException(ReplicationTransferFailure.INVALID_LOCATION)
        }
        try {
            cancellation.throwIfCanceled()
            val descriptor = resolver.openAssetFileDescriptor(uri, "r", cancellation)
                ?: throw ReplicationTransferException(ReplicationTransferFailure.UNAVAILABLE)
            return descriptor.use {
                it.createInputStream().use { input ->
                    val output = ByteArrayOutputStream()
                    val buffer = ByteArray(8192)
                    while (true) {
                        cancellation.throwIfCanceled()
                        val count = input.read(buffer, 0, minOf(buffer.size, kind.maximumBytes + 1 - output.size()))
                        if (count == -1) break
                        if (count == 0) throw ReplicationTransferException(ReplicationTransferFailure.UNAVAILABLE)
                        output.write(buffer, 0, count)
                        if (output.size() > kind.maximumBytes) throw ReplicationTransferException(ReplicationTransferFailure.TOO_LARGE)
                    }
                    cancellation.throwIfCanceled()
                    if (output.size() == 0) throw ReplicationTransferException(ReplicationTransferFailure.EMPTY)
                    output.toByteArray()
                }
            }
        } catch (_: SecurityException) {
            throw ReplicationTransferException(ReplicationTransferFailure.DENIED)
        } catch (_: OperationCanceledException) {
            throw ReplicationTransferException(ReplicationTransferFailure.CANCELLED)
        } catch (_: IOException) {
            throw ReplicationTransferException(ReplicationTransferFailure.UNAVAILABLE)
        }
    }

    fun publish(): Nothing = throw ReplicationTransferException(ReplicationTransferFailure.UNSUPPORTED_PUBLICATION)
}
