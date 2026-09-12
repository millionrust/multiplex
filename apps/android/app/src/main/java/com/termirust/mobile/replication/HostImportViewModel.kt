package com.termirust.mobile.replication

import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import com.termirust.replication.security.MobileReplicationException
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch

data class ImportedHost(val recordId: String, val label: String, val host: String, val port: Int, val username: String)

class HostImportReview(bytes: ByteArray, token: ByteArray, val host: ImportedHost, val changesLocal: Boolean) {
    private val document = bytes.copyOf()
    private val reviewToken = token.copyOf()
    fun documentBytes() = document.copyOf()
    fun tokenBytes() = reviewToken.copyOf()
}

interface HostImportRepository {
    suspend fun importedHosts(): List<ImportedHost>
    suspend fun previewHosts(bytes: ByteArray): List<ImportedHost>
    suspend fun reviewHost(bytes: ByteArray, recordId: String): HostImportReview
    suspend fun importHost(review: HostImportReview)
}

enum class HostImportProblem { LOCKED, MISSING_KEY, CHANGED, RECOVERY, INVALID, UNAVAILABLE }
data class HostImportState(
    val hosts: List<ImportedHost> = emptyList(),
    val candidates: List<ImportedHost>? = null,
    val review: HostImportReview? = null,
    val loading: Boolean = true,
    val problem: HostImportProblem? = null,
)

class HostImportViewModel(private val repository: HostImportRepository) : ViewModel() {
    private val mutable = MutableStateFlow(HostImportState())
    val state = mutable.asStateFlow()
    private var document: ByteArray? = null
    private var generation = 0L
    private var running = false

    init { reload() }
    fun discard() {
        generation++
        document = null
        mutable.value = mutable.value.copy(candidates = null, review = null)
    }
    fun reload() {
        if (running) return
        discard()
        run { mutable.value = HostImportState(hosts = repository.importedHosts(), loading = true) }
    }
    fun preview(bytes: ByteArray) {
        if (running || mutable.value.problem != null) return
        discard()
        val captured = bytes.copyOf()
        run {
            val candidates = repository.previewHosts(captured)
            document = captured
            mutable.value = mutable.value.copy(candidates = candidates)
        }
    }
    fun select(recordId: String) {
        if (running || mutable.value.problem != null || mutable.value.candidates?.none { it.recordId == recordId } != false) return
        val bytes = document ?: return
        run {
            val review = repository.reviewHost(bytes.copyOf(), recordId)
            mutable.value = mutable.value.copy(review = review)
        }
    }
    fun apply() {
        if (running || mutable.value.problem != null) return
        val review = mutable.value.review ?: return
        discard()
        run {
            repository.importHost(review)
            mutable.value = HostImportState(hosts = repository.importedHosts(), loading = true)
        }
    }
    private fun run(action: suspend () -> Unit) {
        if (running) return
        running = true
        val expected = generation
        val before = mutable.value
        mutable.value = mutable.value.copy(loading = true, problem = null)
        viewModelScope.launch {
            try {
                action()
                if (generation != expected) {
                    document = null
                    mutable.value = before.copy(candidates = null, review = null)
                }
            } catch (cancelled: CancellationException) { throw cancelled }
            catch (error: Exception) {
                document = null
                mutable.value = before.copy(candidates = null, review = null, problem = if (generation != expected) null else when (error) {
                    is MobileReplicationException.Locked -> HostImportProblem.LOCKED
                    is MobileReplicationException.MissingSecret -> HostImportProblem.MISSING_KEY
                    is MobileReplicationException.StaleRequest -> HostImportProblem.CHANGED
                    is MobileReplicationException.RecoveryRequired -> HostImportProblem.RECOVERY
                    is MobileReplicationException.Invalid -> HostImportProblem.INVALID
                    else -> HostImportProblem.UNAVAILABLE
                })
            } finally {
                running = false
                mutable.value = mutable.value.copy(loading = false)
            }
        }
    }
}
