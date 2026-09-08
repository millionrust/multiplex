package com.termirust.mobile.replication

import android.os.CancellationSignal
import android.os.Process
import android.provider.DocumentsContract
import androidx.compose.material3.MaterialTheme
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.ui.test.*
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.test.platform.app.InstrumentationRegistry
import com.termirust.replication.security.MobileReplicationException
import kotlinx.coroutines.runBlocking
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Rule
import org.junit.Test
import java.io.File
import java.nio.file.Files
import java.security.KeyStore

/** Runner stages exchange only public requests and encrypted documents with desktop Rust. */
class EnrollmentAcceptanceTest {
    @get:Rule val compose = createComposeRule()
    private val context = InstrumentationRegistry.getInstrumentation().targetContext
    private val token = InstrumentationRegistry.getArguments().getString("fixture").orEmpty().also {
        require(it.matches(Regex("[a-f0-9-]{36}")))
    }
    private val name = "c07-$token"
    private val exchange = File(context.getExternalFilesDir(null), name)
    private val store = ReplicationKeystoreStore(context, name)
    private fun repository() = NativeEnrollmentRepository(context, name, name)
    private fun read(kind: ReplicationTransferKind, id: String): ByteArray =
        ReplicationDocumentReader(context.contentResolver).read(
            DocumentsContract.buildDocumentUri(context.packageName + ".replication-transfer-fixture", "$name:$id"), kind, CancellationSignal())
    private fun reviewInfo() = JSONObject(File(exchange, "review.json").readText())
    private fun capture(label: String) {
        compose.waitForIdle()
        val bitmap = InstrumentationRegistry.getInstrumentation().uiAutomation.takeScreenshot()
        try { File(exchange, "$label.png").outputStream().use { bitmap.compress(android.graphics.Bitmap.CompressFormat.PNG, 100, it) } }
        finally { bitmap.recycle() }
    }

    @Test fun prepare() = runBlocking {
        assertFalse(exchange.exists()); assertTrue(exchange.mkdir())
        assertNull(repository().load().request)
        val request = repository().prepare().request!!
        File(exchange, "request.json").writeBytes(request)
        File(exchange, "pid").writeText(Process.myPid().toString())
    }

    @Test fun accept() {
        assertNotEquals(File(exchange, "pid").readText().toInt(), Process.myPid())
        val bytes = read(ReplicationTransferKind.ENROLLMENT_BUNDLE, "bundle")
        val request = runBlocking { repository().load().request!! }
        assertArrayEquals(File(exchange, "request.json").readBytes(), request)
        val code = reviewInfo().getString("code")
        val originalReview = runBlocking { repository().reviewBundle(request, bytes) }
        runBlocking {
            val other = NativeEnrollmentRepository(context, "$name-other", name)
            val otherRequest = other.prepare().request!!
            try { other.reviewBundle(otherRequest, bytes); fail("Wrong recipient must fail") }
            catch (_: MobileReplicationException.Invalid) { }
            assertArrayEquals(otherRequest, other.load().request)
            other.cancel(otherRequest)
            val review = repository().reviewBundle(request, bytes)
            val wrongWorkspace = EnrollmentBundleReview(request, bytes, "wrong-workspace", review.recipient, review.code)
            try { repository().acceptBundle(wrongWorkspace, code); fail("Wrong reviewed workspace must fail") }
            catch (_: MobileReplicationException.Invalid) { }
            assertArrayEquals(request, repository().load().request)
        }
        lateinit var model: EnrollmentViewModel
        compose.runOnUiThread { model = EnrollmentViewModel(repository()) }
        compose.setContent { val state by model.state.collectAsState(); MaterialTheme {
            EnrollmentContent(state, {}, model::prepare, model::reload, model::cancel, {}, {},
                onSelectBundle = { model.reviewBundle(bytes) }, onDiscardBundle = model::discardBundleReview,
                onAcceptBundle = model::acceptBundle)
        } }
        compose.waitUntil(10_000) { !model.state.value.loading }
        compose.onNodeWithText("Select desktop enrollment bundle").performScrollTo().performTouchInput { click() }
        compose.waitUntil(10_000) { !model.state.value.loading }
        assertEquals(code, model.state.value.bundleReview!!.code)
        compose.onNodeWithText("Accept enrollment").assertIsNotEnabled()
        // Cancelling review leaves the exact pending request intact.
        compose.onNodeWithText("Back").performTouchInput { click() }
        assertArrayEquals(request, runBlocking { repository().load().request })
        compose.onNodeWithText("Select desktop enrollment bundle").performScrollTo().performTouchInput { click() }
        compose.waitUntil(10_000) { !model.state.value.loading }
        compose.onNodeWithText("Verification code shown on desktop").performTextInput("WRONG")
        compose.onNodeWithText("Accept enrollment").assertIsNotEnabled()
        compose.onNodeWithText("Verification code shown on desktop").performTextReplacement(code)
        compose.onNodeWithText("Accept enrollment").assertIsDisplayed().performTouchInput { click() }
        compose.waitUntil(10_000) { !model.state.value.loading }
        assertNull(model.state.value.problem); assertTrue(model.state.value.snapshot.configured)
        compose.onNodeWithText("Enrollment configured").assertIsDisplayed()
        assertTrue(runBlocking { repository().load().configured })
        runBlocking {
            try { repository().acceptBundle(originalReview, code); fail("Duplicate acceptance must fail") }
            catch (_: MobileReplicationException.AlreadyConfigured) { }
            assertTrue(repository().load().configured)
        }
        capture("accepted")
        File(exchange, "pid").writeText(Process.myPid().toString())
    }

    @Test fun importHost() {
        assertNotEquals(File(exchange, "pid").readText().toInt(), Process.myPid())
        assertTrue(runBlocking { repository().load().configured })
        assertTrue(runBlocking { repository().importedHosts().isEmpty() })
        val bytes = read(ReplicationTransferKind.ENCRYPTED_REPLICA, "replica")
        lateinit var model: HostImportViewModel
        compose.runOnUiThread { model = HostImportViewModel(repository()) }
        compose.setContent { val state by model.state.collectAsState(); MaterialTheme {
            HostImportContent(state, {}, model::reload, { model.preview(bytes) }, model::select, model::discard, model::apply)
        } }
        compose.waitUntil(10_000) { !model.state.value.loading }
        compose.onNodeWithContentDescription("Choose encrypted host file").performTouchInput { click() }
        compose.waitUntil(10_000) { !model.state.value.loading }
        assertNull(model.state.value.problem)
        compose.onNodeWithText("Desktop fixture host").assertIsDisplayed()
        assertTrue(runBlocking { repository().importedHosts().isEmpty() })
        compose.onNodeWithText("Review").performTouchInput { click() }
        compose.waitUntil(10_000) { !model.state.value.loading }
        assertTrue(runBlocking { repository().importedHosts().isEmpty() })
        runBlocking {
            val review = model.state.value.review!!
            val changed = review.documentBytes().also { it[0] = 0 }
            try { repository().importHost(HostImportReview(changed, review.tokenBytes(), review.host, review.changesLocal)); fail("Changed bytes must fail") }
            catch (_: MobileReplicationException.Invalid) { }
            assertTrue(repository().importedHosts().isEmpty())
        }
        compose.onNodeWithText("Import host").assertIsDisplayed().performTouchInput { click() }
        compose.waitUntil(10_000) { !model.state.value.loading }
        assertNull(model.state.value.problem)
        compose.onNodeWithText("Desktop fixture host").assertIsDisplayed()
        val hosts = runBlocking { repository().importedHosts() }
        assertEquals(1, hosts.size); assertEquals(reviewInfo().getString("record_id"), hosts.single().recordId)
        assertEquals("example.test", hosts.single().host); assertEquals(2222, hosts.single().port)
        // Duplicate import is reviewed against the current local record and is inert.
        runBlocking {
            val again = repository().reviewHost(bytes, hosts.single().recordId)
            assertFalse(again.changesLocal)
            repository().importHost(again)
            assertEquals(hosts, repository().importedHosts())
        }
        capture("imported")
        File(exchange, "pid").writeText(Process.myPid().toString())
    }

    @Test fun reopen() = runBlocking {
        assertNotEquals(File(exchange, "pid").readText().toInt(), Process.myPid())
        assertTrue(repository().load().configured)
        val host = repository().importedHosts().single()
        assertEquals("Desktop fixture host", host.label); assertEquals("demo", host.username)
        assertEquals(reviewInfo().getString("record_id"), host.recordId)
        // Removing only this fixture wrapping key must not silently regenerate it.
        KeyStore.getInstance("AndroidKeyStore").apply { load(null) }.deleteEntry(store.alias)
        try { repository().importedHosts(); fail("Lost wrapping key must fail") }
        catch (_: MobileReplicationException.Unavailable) { }
        assertFalse(KeyStore.getInstance("AndroidKeyStore").apply { load(null) }.containsAlias(store.alias))
    }

    @Test fun cleanup() {
        for (folder in listOf(File(context.noBackupFilesDir, name), File(context.noBackupFilesDir, "$name-other"), store.directory, exchange)) {
            if (folder.exists()) Files.walk(folder.toPath()).use { paths -> paths.sorted(Comparator.reverseOrder()).forEach(Files::delete) }
        }
        KeyStore.getInstance("AndroidKeyStore").apply { load(null) }.deleteEntry(store.alias)
    }
}
