package com.termirust.mobile.replication

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.net.Uri
import android.os.PersistableBundle
import com.termirust.mobile.R
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

class EnrollmentTransfer(private val context: Context) {
    fun copy(request: ByteArray) {
        require(request.size in 1..4096)
        val clip = ClipData.newPlainText(context.getString(R.string.enrollment_title), request.toString(Charsets.UTF_8))
        clip.description.extras = PersistableBundle().apply { putBoolean("android.content.extra.IS_SENSITIVE", true) }
        (context.getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager).setPrimaryClip(clip)
    }

    suspend fun export(uri: Uri, request: ByteArray) = withContext(Dispatchers.IO) {
        require(request.size in 1..4096)
        // Stream the inert public request to the chosen URI. Never convert provider URIs to paths.
        requireNotNull(context.contentResolver.openOutputStream(uri, "wt")).use { it.write(request) }
    }
}
