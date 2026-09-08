package com.termirust.mobile.replication

import android.app.KeyguardManager
import android.content.Context
import android.os.Build
import android.os.Looper
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyPermanentlyInvalidatedException
import android.security.keystore.KeyProperties
import android.security.keystore.UserNotAuthenticatedException
import android.system.ErrnoException
import android.system.Os
import android.system.OsConstants
import com.termirust.replication.security.ReplicationSecureStore
import com.termirust.replication.security.ReplicationStorageException
import java.io.File
import java.io.FileInputStream
import java.io.FileOutputStream
import java.nio.channels.OverlappingFileLockException
import java.nio.file.Files
import java.nio.file.AccessDeniedException
import java.security.KeyStore
import java.security.MessageDigest
import java.util.concurrent.TimeUnit
import java.util.concurrent.locks.ReentrantLock
import javax.crypto.AEADBadTagException
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/** Device-local replication only. No Controller accounts, aliases, or files are reused. */
class ReplicationKeystoreStore(
    context: Context,
    private val namespace: String = "device",
) : ReplicationSecureStore {
    private val context = context.applicationContext
    private val root = File(context.noBackupFilesDir.canonicalFile, "replication-secrets")
    internal val directory = File(root, namespace)
    internal val alias = "termirust-replication-$namespace-v1"

    init {
        if (!namespace.matches(Regex("[a-z0-9-]{1,80}"))) throw ReplicationStorageException.Invalid()
    }

    override fun create(account: String, value: ByteArray) {
        try {
            validate(account)
            if (value.size != SECRET_BYTES) throw ReplicationStorageException.Invalid()
            locked {
                val files = files(account)
                files.forEach(::regularOrAbsent)
                if (exists(files[0]) || exists(files[1])) throw ReplicationStorageException.Collision()
                val key = wrappingKey(create = true)
                val cipher = Cipher.getInstance(TRANSFORMATION)
                cipher.init(Cipher.ENCRYPT_MODE, key)
                cipher.updateAAD(aad(account))
                val ciphertext = byteArrayOf(VERSION) + cipher.iv + cipher.doFinal(value)
                if (ciphertext.size != PAYLOAD_BYTES) throw ReplicationStorageException.Invalid()
                if (Thread.currentThread().isInterrupted) throw InterruptedException()
                if (exists(files[2])) Files.delete(files[2].toPath())
                val fd = Os.open(files[2].path, OsConstants.O_WRONLY or OsConstants.O_CREAT or
                    OsConstants.O_EXCL or OsConstants.O_NOFOLLOW, 0x180)
                FileOutputStream(fd).use { output ->
                    output.write(ciphertext)
                    output.fd.sync()
                }
                checkDirectory()
                if (Thread.currentThread().isInterrupted) throw InterruptedException()
                if (exists(files[0]) || exists(files[1])) throw ReplicationStorageException.Collision()
                Os.rename(files[2].path, files[0].path)
                syncDirectory()
            }
        } finally {
            value.fill(0)
        }
    }

    override fun load(account: String): ByteArray {
        validate(account)
        return locked {
            val files = files(account)
            files.forEach(::regularOrAbsent)
            val source = when {
                exists(files[1]) -> files[1]
                exists(files[0]) -> files[0]
                else -> throw ReplicationStorageException.Missing()
            }
            val payload = readBounded(source)
            if (payload.size != PAYLOAD_BYTES || payload[0] != VERSION) {
                throw ReplicationStorageException.Invalid()
            }
            val cipher = Cipher.getInstance(TRANSFORMATION)
            cipher.init(Cipher.DECRYPT_MODE, wrappingKey(create = false),
                GCMParameterSpec(128, payload.copyOfRange(1, 13)))
            cipher.updateAAD(aad(account))
            val plaintext = cipher.doFinal(payload.copyOfRange(13, payload.size))
            try {
                if (plaintext.size != SECRET_BYTES) throw ReplicationStorageException.Invalid()
                if (source == files[1]) {
                    checkDirectory()
                    Os.rename(source.path, files[0].path)
                    syncDirectory()
                }
                plaintext.copyOf()
            } finally {
                plaintext.fill(0)
            }
        }
    }

    override fun delete(account: String): Boolean {
        validate(account)
        return locked {
            val files = files(account)
            files.forEach(::regularOrAbsent)
            val present = files.filter(::exists)
            present.forEach { Files.delete(it.toPath()) }
            if (present.isNotEmpty()) syncDirectory()
            if (files.any(::exists)) throw ReplicationStorageException.Unavailable()
            present.isNotEmpty()
        }
    }

    private fun <T> locked(operation: () -> T): T {
        if (Looper.myLooper() == Looper.getMainLooper()) throw ReplicationStorageException.Unavailable()
        val started = System.nanoTime()
        val mutex = locks[(directory.path.hashCode() and Int.MAX_VALUE) % locks.size]
        var held = false
        try {
            val keyguard = context.getSystemService(KeyguardManager::class.java)
                ?: throw ReplicationStorageException.Unavailable()
            if (!keyguard.isDeviceSecure || keyguard.isDeviceLocked) throw ReplicationStorageException.Locked()
            held = mutex.tryLock(LOCK_MILLIS, TimeUnit.MILLISECONDS)
            if (!held) throw ReplicationStorageException.Unavailable()
            checkedDirectory(root)
            checkedDirectory(directory)
            val before = Os.lstat(directory.path)
            val lockFile = File(directory, LOCK_FILE)
            regularOrAbsent(lockFile)
            val lockFd = Os.open(lockFile.path, OsConstants.O_RDWR or OsConstants.O_CREAT or OsConstants.O_NOFOLLOW, 0x180)
            FileOutputStream(lockFd).use { lockOutput ->
                val channel = lockOutput.channel
                while (TimeUnit.NANOSECONDS.toMillis(System.nanoTime() - started) < LOCK_MILLIS) {
                    if (Thread.currentThread().isInterrupted) throw InterruptedException()
                    val fileLock = try { channel.tryLock() } catch (_: OverlappingFileLockException) { null }
                    if (fileLock != null) {
                        fileLock.use {
                            checkDirectory()
                            val now = Os.lstat(directory.path)
                            val lockStat = Os.lstat(lockFile.path)
                            val heldStat = Os.fstat(lockFd)
                            if (before.st_ino != now.st_ino || before.st_dev != now.st_dev ||
                                lockStat.st_ino != heldStat.st_ino || lockStat.st_dev != heldStat.st_dev ||
                                !OsConstants.S_ISREG(lockStat.st_mode)) throw ReplicationStorageException.Invalid()
                            if (!keyguard.isDeviceSecure || keyguard.isDeviceLocked) throw ReplicationStorageException.Locked()
                            return operation()
                        }
                    }
                    Thread.sleep(10)
                }
                throw ReplicationStorageException.Unavailable()
            }
        } catch (error: InterruptedException) {
            Thread.currentThread().interrupt()
            throw ReplicationStorageException.Unavailable()
        } catch (error: Exception) {
            throw mapFailure(error)
        } finally {
            if (held) mutex.unlock()
        }
    }

    @Suppress("DEPRECATION")
    private fun wrappingKey(create: Boolean): SecretKey {
        val store = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        val entry = store.getEntry(alias, null)
        if (entry is KeyStore.SecretKeyEntry) return entry.secretKey
        if (entry != null || !create) throw ReplicationStorageException.Unavailable()
        // Never strand existing ciphertext by provisioning a replacement key.
        Files.newDirectoryStream(directory.toPath()).use { files ->
            if (files.any { it.fileName.toString().let { name -> name.endsWith(".blob") || name.endsWith(".blob.bak") } }) {
                throw ReplicationStorageException.Unavailable()
            }
        }
        val spec = KeyGenParameterSpec.Builder(alias, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
            .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
            .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
            .setKeySize(256)
            .setUserAuthenticationRequired(true)
        if (Build.VERSION.SDK_INT >= 30) {
            spec.setUserAuthenticationParameters(AUTH_SECONDS, KeyProperties.AUTH_DEVICE_CREDENTIAL)
        } else {
            spec.setUserAuthenticationValidityDurationSeconds(AUTH_SECONDS)
        }
        if (Build.VERSION.SDK_INT >= 28) spec.setUnlockedDeviceRequired(true)
        return KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore").run {
            init(spec.build())
            generateKey()
        }
    }

    internal fun fileFor(account: String): File {
        validate(account)
        val name = MessageDigest.getInstance("SHA-256").digest(account.toByteArray(Charsets.US_ASCII))
            .joinToString("") { "%02x".format(it) }
        return File(directory, "$name.blob")
    }

    private fun files(account: String): List<File> = fileFor(account).let {
        listOf(it, File(it.path + ".bak"), File(it.path + ".new"))
    }

    private fun aad(account: String) = "termirust.replication/$namespace/v1\u0000$account".toByteArray(Charsets.US_ASCII)

    private fun validate(account: String) {
        if (account.length !in 1..128 || account.any { it.code !in 33..126 }) throw ReplicationStorageException.Invalid()
    }

    private fun checkedDirectory(path: File) {
        if (!exists(path) && !path.mkdir() && !exists(path)) throw ReplicationStorageException.Unavailable()
        val stat = Os.lstat(path.path)
        if (!OsConstants.S_ISDIR(stat.st_mode) || path.canonicalFile != path.absoluteFile) {
            throw ReplicationStorageException.Invalid()
        }
    }

    private fun checkDirectory() {
        for (path in listOf(root, directory)) {
            if (!OsConstants.S_ISDIR(Os.lstat(path.path).st_mode) || path.canonicalFile != path.absoluteFile) {
                throw ReplicationStorageException.Invalid()
            }
        }
    }

    private fun regularOrAbsent(path: File) {
        if (exists(path) && !OsConstants.S_ISREG(Os.lstat(path.path).st_mode)) throw ReplicationStorageException.Invalid()
    }

    private fun exists(path: File): Boolean = try {
        Os.lstat(path.path)
        true
    } catch (error: ErrnoException) {
        if (error.errno == OsConstants.ENOENT) false else throw error
    }

    private fun readBounded(path: File): ByteArray {
        val fd = Os.open(path.path, OsConstants.O_RDONLY or OsConstants.O_NOFOLLOW, 0)
        return FileInputStream(fd).use { input ->
            val buffer = ByteArray(PAYLOAD_BYTES + 1)
            var count = 0
            while (count < buffer.size) {
                val read = input.read(buffer, count, buffer.size - count)
                if (read < 0) break
                count += read
            }
            buffer.copyOf(count)
        }
    }

    private fun syncDirectory() {
        val fd = Os.open(directory.path, OsConstants.O_RDONLY or OsConstants.O_NOFOLLOW, 0)
        try {
            if (!OsConstants.S_ISDIR(Os.fstat(fd).st_mode)) throw ReplicationStorageException.Invalid()
            Os.fsync(fd)
        } finally { Os.close(fd) }
    }

    internal companion object {
        const val SECRET_BYTES = 47
        const val PAYLOAD_BYTES = 76
        const val LOCK_FILE = "namespace.lock"
        const val LOCK_MILLIS = 2_000L
        const val AUTH_SECONDS = 300
        private const val VERSION: Byte = 1
        private const val TRANSFORMATION = "AES/GCM/NoPadding"
        private val locks = Array(64) { ReentrantLock() }

        fun mapFailure(error: Exception): ReplicationStorageException = when (error) {
            is ReplicationStorageException -> error
            is UserNotAuthenticatedException, is SecurityException, is AccessDeniedException -> ReplicationStorageException.Locked()
            is AEADBadTagException -> ReplicationStorageException.Invalid()
            is KeyPermanentlyInvalidatedException -> ReplicationStorageException.Unavailable()
            is ErrnoException -> if (error.errno == OsConstants.EACCES || error.errno == OsConstants.EPERM) {
                ReplicationStorageException.Locked()
            } else if (error.errno == OsConstants.ELOOP) { ReplicationStorageException.Invalid()
            } else { ReplicationStorageException.Unavailable() }
            else -> ReplicationStorageException.Unavailable()
        }
    }
}
