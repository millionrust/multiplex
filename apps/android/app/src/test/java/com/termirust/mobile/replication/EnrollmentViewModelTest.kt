package com.termirust.mobile.replication

import com.termirust.replication.security.MobileReplicationException
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.*
import org.junit.After
import org.junit.Before
import org.junit.Test
import org.junit.Assert.*

@OptIn(ExperimentalCoroutinesApi::class)
class EnrollmentViewModelTest {
    private val dispatcher = StandardTestDispatcher()
    @Before fun before() { Dispatchers.setMain(dispatcher) }
    @After fun after() { Dispatchers.resetMain() }
    private class Repository : EnrollmentRepository {
        var snapshot = EnrollmentSnapshot()
        var failure: Exception? = null
        var prepareCount = 0
        var cancelBytes: ByteArray? = null
        var gate: CompletableDeferred<Unit>? = null
        var acceptCount = 0
        var recoveryCount = 0
        var reviewGate: CompletableDeferred<Unit>? = null
        override suspend fun load(): EnrollmentSnapshot { failure?.let { throw it }; return snapshot }
        override suspend fun prepare(): EnrollmentSnapshot {
            prepareCount++; gate?.await(); failure?.let { throw it }
            snapshot = EnrollmentSnapshot(byteArrayOf(1, 2)); return snapshot
        }
        override suspend fun cancel(request: ByteArray): EnrollmentSnapshot {
            cancelBytes = request; failure?.let { throw it }; snapshot = EnrollmentSnapshot(); return snapshot
        }
        override suspend fun reviewBundle(request: ByteArray, bytes: ByteArray): EnrollmentBundleReview {
            reviewGate?.await(); failure?.let { throw it }
            return EnrollmentBundleReview(request, bytes, "workspace", "recipient", "ABC123-DEF456")
        }
        override suspend fun acceptBundle(review: EnrollmentBundleReview, verifiedCode: String): EnrollmentSnapshot {
            acceptCount++; failure?.let { throw it }
            snapshot = EnrollmentSnapshot(configured = true); return snapshot
        }
        override suspend fun recover(): EnrollmentSnapshot {
            recoveryCount++; gate?.await(); failure?.let { throw it }; return snapshot
        }
    }
    @Test fun openingOnlyLoadsAndNeverCreatesIdentity() = runTest(dispatcher) {
        val repository = Repository(); val model = EnrollmentViewModel(repository)
        advanceUntilIdle()
        assertFalse(model.state.value.loading); assertNull(model.state.value.snapshot.request)
        assertEquals(0, repository.prepareCount)
    }
    @Test fun recoveryIsExplicitSingleFlightAndNeverAcceptsOrPrepares() = runTest(dispatcher) {
        val repository = Repository(); repository.failure = MobileReplicationException.RecoveryRequired()
        val model = EnrollmentViewModel(repository); advanceUntilIdle()
        assertEquals(0, repository.recoveryCount)
        repository.failure = null; repository.snapshot = EnrollmentSnapshot(byteArrayOf(1))
        repository.gate = CompletableDeferred()
        model.recover(); model.recover(); runCurrent(); assertEquals(1, repository.recoveryCount)
        repository.gate!!.complete(Unit); advanceUntilIdle()
        assertArrayEquals(byteArrayOf(1), model.state.value.snapshot.request)
        assertNull(model.state.value.problem)
        model.recover(); advanceUntilIdle(); assertEquals(1, repository.recoveryCount)
        assertEquals(0, repository.acceptCount); assertEquals(0, repository.prepareCount)
    }
    @Test fun failedRecoveryNeedsReloadAndDoesNotDiscardCancellation() = runTest(dispatcher) {
        val repository = Repository(); repository.failure = MobileReplicationException.RecoveryRequired()
        val model = EnrollmentViewModel(repository); advanceUntilIdle()
        repository.failure = MobileReplicationException.Locked()
        model.recover(); advanceUntilIdle(); model.recover(); advanceUntilIdle()
        assertEquals(1, repository.recoveryCount); assertEquals(EnrollmentProblem.LOCKED, model.state.value.problem)
        repository.failure = null; repository.snapshot = EnrollmentSnapshot(byteArrayOf(3), cancellationPending = true)
        model.reload(); advanceUntilIdle(); model.recover(); advanceUntilIdle()
        assertEquals(1, repository.recoveryCount); assertTrue(model.state.value.snapshot.cancellationPending)
    }
    @Test fun duplicatePrepareIsIgnoredWhileBusy() = runTest(dispatcher) {
        val repository = Repository(); val model = EnrollmentViewModel(repository)
        advanceUntilIdle(); repository.gate = CompletableDeferred()
        model.prepare(); model.prepare(); runCurrent()
        assertEquals(1, repository.prepareCount); assertTrue(model.state.value.loading)
        repository.gate!!.complete(Unit); advanceUntilIdle()
        assertArrayEquals(byteArrayOf(1, 2), model.state.value.snapshot.request)
    }
    @Test fun uncertainPrepareNeedsReloadBeforeAnotherMutation() = runTest(dispatcher) {
        val repository = Repository(); val model = EnrollmentViewModel(repository)
        advanceUntilIdle(); repository.failure = MobileReplicationException.Unavailable()
        model.prepare(); advanceUntilIdle(); model.prepare(); advanceUntilIdle()
        assertEquals(1, repository.prepareCount)
        repository.failure = null; repository.snapshot = EnrollmentSnapshot(byteArrayOf(4))
        model.reload(); advanceUntilIdle(); assertArrayEquals(byteArrayOf(4), model.state.value.snapshot.request)
    }
    @Test fun lockedCancellationPreservesReviewedRequestForRetry() = runTest(dispatcher) {
        val repository = Repository(); val model = EnrollmentViewModel(repository)
        advanceUntilIdle(); repository.failure = MobileReplicationException.Locked()
        val request = byteArrayOf(8); model.cancel(request); request[0] = 7; advanceUntilIdle()
        assertEquals(EnrollmentProblem.LOCKED, model.state.value.problem)
        assertTrue(model.state.value.snapshot.cancellationPending)
        assertArrayEquals(byteArrayOf(8), repository.cancelBytes)
        repository.failure = null; model.cancel(byteArrayOf(8)); advanceUntilIdle()
        assertNull(model.state.value.snapshot.request)
    }
    @Test fun recoveryAndStalenessRemainDistinct() = runTest(dispatcher) {
        val repository = Repository(); repository.failure = MobileReplicationException.RecoveryRequired()
        val model = EnrollmentViewModel(repository); advanceUntilIdle()
        assertEquals(EnrollmentProblem.RECOVERY, model.state.value.problem)
        repository.failure = MobileReplicationException.StaleRequest()
        model.cancel(byteArrayOf(3)); advanceUntilIdle()
        assertEquals(EnrollmentProblem.CHANGED, model.state.value.problem)
    }

    @Test fun bundleReviewRequiresExactDesktopCodeAndAcceptsOnce() = runTest(dispatcher) {
        val repository = Repository(); repository.snapshot = EnrollmentSnapshot(byteArrayOf(1))
        val model = EnrollmentViewModel(repository); advanceUntilIdle()
        model.reviewBundle(byteArrayOf(2)); advanceUntilIdle()
        model.acceptBundle("wrong"); advanceUntilIdle(); assertEquals(0, repository.acceptCount)
        model.acceptBundle("ABC123-DEF456"); model.acceptBundle("ABC123-DEF456"); advanceUntilIdle()
        assertEquals(1, repository.acceptCount); assertTrue(model.state.value.snapshot.configured)
        model.prepare(); advanceUntilIdle(); assertEquals(0, repository.prepareCount)
    }

    @Test fun abandonedReadCannotRestoreReviewOrCancelRequest() = runTest(dispatcher) {
        val repository = Repository(); repository.snapshot = EnrollmentSnapshot(byteArrayOf(1))
        val model = EnrollmentViewModel(repository); advanceUntilIdle()
        repository.reviewGate = CompletableDeferred()
        model.reviewBundle(byteArrayOf(2)); runCurrent(); model.discardBundleReview()
        repository.reviewGate!!.complete(Unit); advanceUntilIdle()
        assertNull(model.state.value.bundleReview); assertNull(repository.cancelBytes)
        assertArrayEquals(byteArrayOf(1), model.state.value.snapshot.request)
    }

    @Test fun failedAcceptanceDropsReviewAndNeverRetries() = runTest(dispatcher) {
        val repository = Repository(); repository.snapshot = EnrollmentSnapshot(byteArrayOf(1))
        val model = EnrollmentViewModel(repository); advanceUntilIdle()
        model.reviewBundle(byteArrayOf(2)); advanceUntilIdle()
        repository.failure = MobileReplicationException.RecoveryRequired()
        model.acceptBundle("ABC123-DEF456"); advanceUntilIdle()
        assertEquals(EnrollmentProblem.RECOVERY, model.state.value.problem)
        assertNull(model.state.value.bundleReview)
        model.acceptBundle("ABC123-DEF456"); advanceUntilIdle(); assertEquals(1, repository.acceptCount)
    }

    @Test fun reviewOwnsItsBytes() {
        val request = byteArrayOf(1); val bytes = byteArrayOf(2)
        val review = EnrollmentBundleReview(request, bytes, "workspace", "recipient", "code")
        request[0] = 9; bytes[0] = 9; review.bundleBytes()[0] = 8
        assertArrayEquals(byteArrayOf(1), review.requestBytes())
        assertArrayEquals(byteArrayOf(2), review.bundleBytes())
    }
}
