package com.termirust.mobile.controller

import android.content.Context
import android.util.AtomicFile
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import kotlinx.serialization.SerializationException
import kotlinx.serialization.encodeToString
import kotlinx.serialization.json.Json
import java.io.File

private val ControllerJson = Json {
    ignoreUnknownKeys = false
    encodeDefaults = true
    explicitNulls = true
}

class PairedHostStore(context: Context) {
    private val file = AtomicFile(File(context.noBackupFilesDir, "controller-hosts-v1.json"))

    suspend fun load(): List<PairedHostRecord> = withContext(Dispatchers.IO) {
        val bytes = readBounded(file, ControllerLimits.MAX_CACHE_BYTES) ?: return@withContext emptyList()
        val document = runCatching {
            ControllerJson.decodeFromString<PairedHostDocument>(bytes.decodeToString())
        }.getOrElse { return@withContext emptyList() }
        if (document.schemaVersion != 1 || document.hosts.size > ControllerLimits.MAX_HOSTS) {
            return@withContext emptyList()
        }
        runCatching { document.hosts.forEach(PairedHostRecord::validate) }
            .getOrElse { return@withContext emptyList() }
        document.hosts
    }

    suspend fun upsert(host: PairedHostRecord): List<PairedHostRecord> = withContext(Dispatchers.IO) {
        host.validate()
        val current = load().associateBy { it.id }.toMutableMap()
        require(current.containsKey(host.id) || current.size < ControllerLimits.MAX_HOSTS)
        current[host.id] = host
        val next = current.values.sortedWith(compareBy({ it.displayName.lowercase() }, { it.id }))
        writeAtomic(file, ControllerJson.encodeToString(PairedHostDocument(hosts = next)).encodeToByteArray())
        next
    }

    suspend fun remove(hostId: String): List<PairedHostRecord> = withContext(Dispatchers.IO) {
        val next = load().filterNot { it.id == hostId }
        writeAtomic(file, ControllerJson.encodeToString(PairedHostDocument(hosts = next)).encodeToByteArray())
        next
    }
}

class ControllerFleetCacheStore(context: Context) {
    private val file = AtomicFile(File(context.noBackupFilesDir, "controller-fleet-cache-v1.json"))

    suspend fun load(): ControllerCacheDocument = withContext(Dispatchers.IO) {
        val bytes = readBounded(file, ControllerLimits.MAX_CACHE_BYTES) ?: return@withContext ControllerCacheDocument()
        val document = runCatching {
            ControllerJson.decodeFromString<ControllerCacheDocument>(bytes.decodeToString())
        }.getOrElse { return@withContext ControllerCacheDocument() }
        runCatching { validateDocument(document) }.getOrElse { ControllerCacheDocument() }
    }

    suspend fun saveSnapshot(
        current: ControllerCacheDocument,
        selectedHostId: String,
        host: PairedHostRecord,
        snapshot: ControllerFleetSnapshot,
        nowMillis: Long,
    ): ControllerCacheDocument = withContext(Dispatchers.IO) {
        host.validate()
        snapshot.validate()
        val next = ControllerCacheReducer.upsert(
            current = current,
            selectedHostId = selectedHostId,
            value = CachedHostFleet(host, snapshot, nowMillis, nowMillis),
            encodedSize = { document -> ControllerJson.encodeToString(document).encodeToByteArray().size },
        )
        writeAtomic(file, ControllerJson.encodeToString(next).encodeToByteArray())
        next
    }

    suspend fun remove(current: ControllerCacheDocument, hostId: String): ControllerCacheDocument =
        withContext(Dispatchers.IO) {
            val next = ControllerCacheDocument(hosts = current.hosts - hostId)
            writeAtomic(file, ControllerJson.encodeToString(next).encodeToByteArray())
            next
        }

    private fun fits(hosts: Map<String, CachedHostFleet>): Boolean {
        if (hosts.size > ControllerLimits.MAX_HOSTS) return false
        val sessions = hosts.values.sumOf { it.snapshot.sessions.size }
        if (sessions > ControllerLimits.MAX_SESSIONS_TOTAL) return false
        val bytes = ControllerJson.encodeToString(ControllerCacheDocument(hosts = hosts)).encodeToByteArray()
        return bytes.size <= ControllerLimits.MAX_CACHE_BYTES
    }

    private fun validateDocument(document: ControllerCacheDocument): ControllerCacheDocument {
        if (document.schemaVersion != 1 || !fits(document.hosts)) {
            throw SerializationException("invalid controller cache")
        }
        document.hosts.forEach { (id, value) ->
            require(id == value.host.id)
            value.host.validate()
            value.snapshot.validate()
            require(value.updatedAtMillis > 0 && value.lastViewedAtMillis > 0)
        }
        return document
    }
}

internal object ControllerCacheReducer {
    fun upsert(
        current: ControllerCacheDocument,
        selectedHostId: String,
        value: CachedHostFleet,
        encodedSize: (ControllerCacheDocument) -> Int,
    ): ControllerCacheDocument {
        val candidates = current.hosts.toMutableMap()
        candidates[value.host.id] = value
        while (!fits(candidates, encodedSize)) {
            val eviction = candidates.values
                .filter { it.host.id != selectedHostId }
                .minWithOrNull(compareBy<CachedHostFleet>({ it.lastViewedAtMillis }, { it.host.id }))
                ?: throw ControllerStoreException.ResourceLimit
            candidates.remove(eviction.host.id)
        }
        return ControllerCacheDocument(hosts = candidates.toSortedMap())
    }

    private fun fits(
        hosts: Map<String, CachedHostFleet>,
        encodedSize: (ControllerCacheDocument) -> Int,
    ): Boolean = hosts.size <= ControllerLimits.MAX_HOSTS &&
        hosts.values.sumOf { it.snapshot.sessions.size } <= ControllerLimits.MAX_SESSIONS_TOTAL &&
        encodedSize(ControllerCacheDocument(hosts = hosts)) <= ControllerLimits.MAX_CACHE_BYTES
}

@kotlinx.serialization.Serializable
private data class PairedHostDocument(
    @kotlinx.serialization.SerialName("schema_version") val schemaVersion: Int = 1,
    val hosts: List<PairedHostRecord> = emptyList(),
)

sealed class ControllerStoreException : Exception() {
    data object ResourceLimit : ControllerStoreException()
}

private fun readBounded(file: AtomicFile, maximum: Int): ByteArray? {
    if (!file.baseFile.exists()) return null
    if (file.baseFile.length() !in 1..maximum.toLong()) return null
    return runCatching { file.readFully() }.getOrNull()?.takeIf { it.size <= maximum }
}

private fun writeAtomic(file: AtomicFile, bytes: ByteArray) {
    require(bytes.isNotEmpty() && bytes.size <= ControllerLimits.MAX_CACHE_BYTES)
    val output = file.startWrite()
    try {
        output.write(bytes)
        output.fd.sync()
        file.finishWrite(output)
    } catch (error: Throwable) {
        file.failWrite(output)
        throw error
    }
}
