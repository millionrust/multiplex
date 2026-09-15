package com.termirust.mobile.replication

import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import com.termirust.replication.security.MobileReplicationException
import com.termirust.replication.security.ReplicationStorageException
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch

enum class EnrollmentProblem { LOCKED, BUSY, INVALID, CHANGED, RECOVERY, CONFIGURED, UNAVAILABLE }
data class EnrollmentState(
    val snapshot: EnrollmentSnapshot = EnrollmentSnapshot(),
    val loading: Boolean = true,
    val problem: EnrollmentProblem? = null,
    val bundleReview: EnrollmentBundleReview? = null,
)

class EnrollmentViewModel(private val repository: EnrollmentRepository) : ViewModel() {
    private val mutable = MutableStateFlow(EnrollmentState())
    val state = mutable.asStateFlow()
    private var running = false
    private var reviewGeneration = 0L

    init { reload() }
    fun reload() = operate { repository.load() }
    fun recover() {
        if (state.value.problem != EnrollmentProblem.RECOVERY || state.value.snapshot.cancellationPending) return
        operate { repository.recover() }
    }
    fun prepare() {
        if (state.value.problem != null || state.value.snapshot.request != null || state.value.snapshot.configured) return
        operate { repository.prepare() }
    }

    fun discardBundleReview() {
        reviewGeneration++
        mutable.value = mutable.value.copy(bundleReview = null)
    }

    fun reviewBundle(bytes: ByteArray) {
        val request = state.value.snapshot.request?.copyOf() ?: return
        if (running || state.value.snapshot.cancellationPending || state.value.problem != null) return
        val captured = bytes.copyOf()
        val generation = ++reviewGeneration
        running = true
        mutable.value = mutable.value.copy(loading = true, bundleReview = null)
        viewModelScope.launch {
            try {
                val review = repository.reviewBundle(request, captured)
                if (generation == reviewGeneration) mutable.value = mutable.value.copy(bundleReview = review)
            } catch (cancelled: CancellationException) { throw cancelled }
            catch (error: Exception) {
                if (generation == reviewGeneration) mutable.value = mutable.value.copy(problem = problemFor(error))
            } finally {
                running = false
                mutable.value = mutable.value.copy(loading = false)
            }
        }
    }

    fun acceptBundle(verifiedCode: String) {
        val review = state.value.bundleReview ?: return
        if (running || verifiedCode != review.code) return
        operate { repository.acceptBundle(review, verifiedCode) }
    }

    private fun problemFor(error: Exception) = when (error) {
        is MobileReplicationException.Locked, is ReplicationStorageException.Locked -> EnrollmentProblem.LOCKED
        is MobileReplicationException.Busy -> EnrollmentProblem.BUSY
        is MobileReplicationException.Invalid -> EnrollmentProblem.INVALID
        is MobileReplicationException.StaleRequest -> EnrollmentProblem.CHANGED
        is MobileReplicationException.RecoveryRequired -> EnrollmentProblem.RECOVERY
        is MobileReplicationException.AlreadyConfigured -> EnrollmentProblem.CONFIGURED
        else -> EnrollmentProblem.UNAVAILABLE
    }
    fun cancel(reviewedRequest: ByteArray) {
        if (running) return
        val captured = reviewedRequest.copyOf()
        mutable.value = mutable.value.copy(snapshot = EnrollmentSnapshot(captured, true))
        operate { repository.cancel(captured) }
    }

    private fun operate(action: suspend () -> EnrollmentSnapshot) {
        if (running) return
        running = true
        discardBundleReview()
        mutable.value = mutable.value.copy(loading = true, problem = null)
        viewModelScope.launch {
            try { mutable.value = EnrollmentState(action(), loading = false) }
            catch (cancelled: CancellationException) { throw cancelled }
            catch (error: Exception) {
                mutable.value = mutable.value.copy(loading = false, problem = problemFor(error))
            } finally { running = false }
        }
    }
}
