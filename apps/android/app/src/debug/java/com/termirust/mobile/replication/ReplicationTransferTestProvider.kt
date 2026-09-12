package com.termirust.mobile.replication

import android.database.Cursor
import android.database.MatrixCursor
import android.os.CancellationSignal
import android.os.ParcelFileDescriptor
import android.provider.DocumentsContract.Document
import android.provider.DocumentsContract.Root
import android.provider.DocumentsProvider
import java.io.File
import java.io.FileNotFoundException

/** Debug-only synthetic documents. No user documents or persistent grants. */
class ReplicationTransferTestProvider : DocumentsProvider() {
    override fun onCreate() = true
    override fun queryRoots(projection: Array<out String>?): Cursor {
        val columns = projection ?: arrayOf(Root.COLUMN_ROOT_ID, Root.COLUMN_DOCUMENT_ID, Root.COLUMN_TITLE, Root.COLUMN_FLAGS, Root.COLUMN_MIME_TYPES)
        return MatrixCursor(columns).apply {
            context!!.getExternalFilesDir(null)?.listFiles().orEmpty().filter {
                it.isDirectory && it.name.matches(Regex("c07-[a-f0-9-]{36}")) && File(it, "bundle.json").isFile
            }.forEach { folder -> addRow(columns.map { column -> when (column) {
                Root.COLUMN_ROOT_ID -> folder.name
                Root.COLUMN_DOCUMENT_ID -> folder.name + ":root"
                Root.COLUMN_TITLE -> "TermiRust fixture"
                Root.COLUMN_FLAGS -> Root.FLAG_LOCAL_ONLY
                Root.COLUMN_MIME_TYPES -> "application/json"
                else -> null
            } }) }
        }
    }
    override fun queryChildDocuments(parentDocumentId: String?, projection: Array<out String>?, sortOrder: String?): Cursor {
        val cursor = documentCursor(projection)
        if (parentDocumentId?.matches(Regex("c07-[a-f0-9-]{36}:root")) == true) {
            val name = parentDocumentId.substringBefore(':')
            for (kind in listOf("bundle", "replica")) {
                val file = File(File(context!!.getExternalFilesDir(null), name), "$kind.json")
                if (file.isFile) addDocument(cursor, "$name:$kind")
            }
        }
        return cursor
    }
    private fun documentCursor(projection: Array<out String>?) = MatrixCursor(projection ?: arrayOf(
        Document.COLUMN_DOCUMENT_ID, Document.COLUMN_MIME_TYPE, Document.COLUMN_DISPLAY_NAME, Document.COLUMN_FLAGS))
    private fun addDocument(cursor: MatrixCursor, id: String) {
        val directory = id.endsWith(":root")
        cursor.addRow(cursor.columnNames.map { column -> when (column) {
            Document.COLUMN_DOCUMENT_ID -> id
            Document.COLUMN_MIME_TYPE -> if (directory) Document.MIME_TYPE_DIR else "application/json"
            Document.COLUMN_DISPLAY_NAME -> when {
                directory -> "TermiRust fixture"
                id.endsWith(":bundle") -> "Enrollment bundle.json"
                id.endsWith(":replica") -> "Encrypted hosts.json"
                else -> id
            }
            Document.COLUMN_FLAGS -> 0
            else -> null
        } })
    }
    override fun queryDocument(documentId: String, projection: Array<out String>?): Cursor {
        return documentCursor(projection).apply { addDocument(this, documentId) }
    }
    override fun openDocument(documentId: String, mode: String, signal: CancellationSignal?): ParcelFileDescriptor {
        signal?.throwIfCanceled()
        if (mode != "r" || documentId == "denied") throw SecurityException("fixture denial")
        if (documentId.matches(Regex("c07-[a-f0-9-]{36}:(bundle|replica)"))) {
            val parts = documentId.split(':')
            val file = File(File(context!!.getExternalFilesDir(null), parts[0]), parts[1] + ".json")
            if (java.nio.file.Files.isSymbolicLink(file.toPath()) || !file.isFile) throw FileNotFoundException("fixture missing")
            return ParcelFileDescriptor.open(file, ParcelFileDescriptor.MODE_READ_ONLY)
        }
        if (documentId == "broken") {
            val pipe = ParcelFileDescriptor.createReliablePipe()
            try {
                ParcelFileDescriptor.AutoCloseOutputStream(pipe[1]).use { output ->
                    output.write(byteArrayOf(1, 2, 3))
                    pipe[1].closeWithError("fixture transfer failed")
                }
                return pipe[0]
            } catch (error: Exception) {
                pipe[0].close()
                pipe[1].close()
                throw error
            }
        }
        val size = when (documentId) {
            "small" -> 17
            "empty" -> 0
            "limit" -> 192 * 1024
            "oversized" -> 192 * 1024 + 1
            "replica-limit" -> 8 * 1024 * 1024
            "replica-oversized" -> 8 * 1024 * 1024 + 1
            else -> throw FileNotFoundException("fixture missing")
        }
        val file = File.createTempFile("replication-transfer-", ".fixture", context!!.cacheDir)
        try {
            file.writeBytes(ByteArray(size) { (it % 251).toByte() })
            return ParcelFileDescriptor.open(file, ParcelFileDescriptor.MODE_READ_ONLY)
        } finally { file.delete() }
    }
}
