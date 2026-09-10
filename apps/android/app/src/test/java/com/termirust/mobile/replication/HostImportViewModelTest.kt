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
class HostImportViewModelTest {
    private val dispatcher = StandardTestDispatcher()
    @Before fun before() { Dispatchers.setMain(dispatcher) }
    @After fun after() { Dispatchers.resetMain() }
    private class Repository : HostImportRepository {
        val host = ImportedHost("record", "Example", "example.test", 22, "demo")
        var hosts = emptyList<ImportedHost>()
        var gate: CompletableDeferred<Unit>? = null
        var failure: Exception? = null
        var calls = 0
        var selectedBytes: ByteArray? = null
        override suspend fun importedHosts() = hosts
        override suspend fun previewHosts(bytes: ByteArray): List<ImportedHost> { gate?.await(); return listOf(host) }
        override suspend fun reviewHost(bytes: ByteArray, recordId: String): HostImportReview {
            selectedBytes = bytes.copyOf(); return HostImportReview(bytes, byteArrayOf(3), host, true)
        }
        override suspend fun importHost(review: HostImportReview) {
            calls++; gate?.await(); failure?.let { throw it }; hosts = listOf(review.host)
        }
    }
    @Test fun discoveryDoesNotImportAndSelectionUsesCapturedBytes() = runTest(dispatcher) {
        val repo = Repository(); val model = HostImportViewModel(repo); advanceUntilIdle()
        val bytes = byteArrayOf(1); model.preview(bytes); bytes[0] = 9; advanceUntilIdle()
        assertEquals(0, repo.calls); assertTrue(model.state.value.hosts.isEmpty())
        model.select("unknown"); advanceUntilIdle(); assertNull(model.state.value.review)
        model.select("record"); advanceUntilIdle()
        assertArrayEquals(byteArrayOf(1), repo.selectedBytes)
        model.apply(); model.apply(); advanceUntilIdle()
        assertEquals(1, repo.calls); assertEquals(listOf(repo.host), model.state.value.hosts)
        assertNull(model.state.value.candidates); assertNull(model.state.value.review)
    }
    @Test fun abandonedPreviewCannotReappearOrImport() = runTest(dispatcher) {
        val repo = Repository(); val model = HostImportViewModel(repo); advanceUntilIdle()
        repo.gate = CompletableDeferred(); model.preview(byteArrayOf(1)); runCurrent()
        model.discard(); repo.gate!!.complete(Unit); advanceUntilIdle()
        assertNull(model.state.value.candidates); model.select("record"); model.apply(); advanceUntilIdle()
        assertEquals(0, repo.calls)
    }
    @Test fun uncertainImportDropsTokenAndRequiresReload() = runTest(dispatcher) {
        val repo = Repository(); val model = HostImportViewModel(repo); advanceUntilIdle()
        model.preview(byteArrayOf(1)); advanceUntilIdle(); model.select("record"); advanceUntilIdle()
        repo.failure = MobileReplicationException.RecoveryRequired(); model.apply(); advanceUntilIdle()
        assertEquals(HostImportProblem.RECOVERY, model.state.value.problem)
        assertNull(model.state.value.review); model.apply(); model.preview(byteArrayOf(2)); advanceUntilIdle()
        assertEquals(1, repo.calls); assertNull(model.state.value.candidates)
        model.reload(); advanceUntilIdle(); assertNull(model.state.value.problem)
    }
    @Test fun cancelledReviewNeverApplies() = runTest(dispatcher) {
        val repo = Repository(); val model = HostImportViewModel(repo); advanceUntilIdle()
        model.preview(byteArrayOf(1)); advanceUntilIdle(); model.select("record"); advanceUntilIdle()
        model.discard(); model.apply(); advanceUntilIdle(); assertEquals(0, repo.calls)
    }
    @Test fun reviewOwnsDocumentAndToken() {
        val bytes = byteArrayOf(1); val token = byteArrayOf(2)
        val review = HostImportReview(bytes, token, Repository().host, true)
        bytes[0] = 3; token[0] = 3; review.documentBytes()[0] = 4; review.tokenBytes()[0] = 4
        assertArrayEquals(byteArrayOf(1), review.documentBytes()); assertArrayEquals(byteArrayOf(2), review.tokenBytes())
    }
}
