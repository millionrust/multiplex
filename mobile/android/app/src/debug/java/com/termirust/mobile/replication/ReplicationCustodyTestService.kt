package com.termirust.mobile.replication

import android.app.Service
import android.content.Intent
import android.os.Bundle
import android.os.Handler
import android.os.HandlerThread
import android.os.IBinder
import android.os.Message
import android.os.Messenger
import android.os.Process
import android.os.SystemClock
import com.termirust.replication.security.ReplicationStorageException

/** Debug-only, non-exported, same-UID process fixture. Never included in release. */
class ReplicationCustodyTestService : Service() {
    private val worker = HandlerThread("replication-custody-test")
    private lateinit var messenger: Messenger

    override fun onCreate() {
        super.onCreate()
        worker.start()
        messenger = Messenger(Handler(worker.looper) { request ->
            val namespace = request.data.getString("namespace").orEmpty()
            if (!namespace.matches(Regex("test-[a-f0-9-]{36}"))) return@Handler true
            val result = try {
                val start = request.data.getLong("start")
                while (SystemClock.elapsedRealtime() < start) Thread.sleep(5)
                val secret = "TRSC".toByteArray() + byteArrayOf(0, 1, 2) + ByteArray(8) + ByteArray(32) { 9 }
                ReplicationKeystoreStore(this, namespace).create("race", secret)
                "created"
            } catch (_: ReplicationStorageException.Collision) {
                "collision"
            } catch (_: Exception) {
                "failure"
            }
            request.replyTo.send(Message.obtain().apply {
                data = Bundle().apply { putString("result", result); putInt("pid", Process.myPid()) }
            })
            true
        })
    }

    override fun onBind(intent: Intent): IBinder = messenger.binder

    override fun onDestroy() {
        worker.quitSafely()
        super.onDestroy()
    }
}
