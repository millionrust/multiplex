package com.termirust.mobile.replication

import android.content.ClipboardManager
import android.content.Context
import android.net.Uri
import android.graphics.Bitmap
import androidx.compose.material3.MaterialTheme
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.setValue
import androidx.compose.ui.test.*
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.test.platform.app.InstrumentationRegistry
import com.termirust.replication.security.ReplicationSecureStore
import com.termirust.replication.security.ReplicationStorageException
import java.io.File
import java.nio.file.Files
import java.security.KeyStore
import java.util.UUID
import kotlinx.coroutines.runBlocking
import org.junit.After
import org.junit.Assert.*
import org.junit.Rule
import org.junit.Test

class EnrollmentWorkflowInstrumentedTest {
    @get:Rule val compose = createComposeRule()
    private val context = InstrumentationRegistry.getInstrumentation().targetContext
    private val id = "c04-${UUID.randomUUID()}"
    private val directory = File(context.noBackupFilesDir, id)
    private val store = ReplicationKeystoreStore(context, id)
    private fun repository(secureStore: ReplicationSecureStore? = null) = NativeEnrollmentRepository(context, id, id, secureStore)

    @After fun cleanup() {
        for (folder in listOf(directory, store.directory)) {
            if (folder.exists()) Files.walk(folder.toPath()).use { paths -> paths.sorted(Comparator.reverseOrder()).forEach(Files::delete) }
        }
        KeyStore.getInstance("AndroidKeyStore").apply { load(null) }.deleteEntry(store.alias)
    }

    @Test fun realWorkflowPrepareReloadCopyExportAndConfirmedCancel() {
        lateinit var model: EnrollmentViewModel
        compose.runOnUiThread { model = EnrollmentViewModel(repository()) }
        var copied: ByteArray? = null
        var exported: ByteArray? = null
        compose.setContent {
            val state by model.state.collectAsState()
            MaterialTheme { EnrollmentContent(state, {}, model::prepare, model::reload, model::cancel,
                onCopy = { copied = it; EnrollmentTransfer(context).copy(it) }, onExport = { exported = it }) }
        }
        compose.waitUntil(10_000) { !model.state.value.loading }
        compose.onNodeWithText("No enrollment request").assertIsDisplayed()
        compose.onNodeWithText("Prepare request").performClick()
        compose.waitUntil(10_000) { !model.state.value.loading }
        compose.onNodeWithText("Request pending").assertIsDisplayed()
        val saved = runBlocking { repository().load().request!! }
        compose.onNodeWithText("Copy request").performClick()
        compose.runOnIdle {
            assertArrayEquals(saved, copied)
            val clipboard = context.getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager
            assertEquals(saved.toString(Charsets.UTF_8), clipboard.primaryClip!!.getItemAt(0).text.toString())
        }
        compose.onNodeWithText("Export request").performClick()
        val file = File.createTempFile("c04-export-", ".json", context.cacheDir)
        try {
            runBlocking { EnrollmentTransfer(context).export(Uri.fromFile(file), exported!!) }
            assertArrayEquals(saved, file.readBytes())
        } finally { file.delete() }
        capture("pending")
        compose.onNodeWithText("Cancel request").performScrollTo().performClick()
        compose.onNodeWithText("Keep request").performClick()
        assertArrayEquals(saved, runBlocking { repository().load().request })
        compose.onNodeWithText("Cancel request").performScrollTo().performClick()
        compose.onNodeWithText("Confirm cancellation").performClick()
        compose.waitUntil(10_000) { !model.state.value.loading }
        compose.onNodeWithText("No enrollment request").assertIsDisplayed()
        assertNull(runBlocking { repository().load().request })
        capture("empty")
    }

    @Test fun failedCancellationSurvivesRepositoryRecreation() = runBlocking {
        val request = repository().prepare().request!!
        val locked = object : ReplicationSecureStore by store {
            override fun delete(account: String): Boolean { throw ReplicationStorageException.Locked() }
        }
        try { repository(locked).cancel(request); fail("Expected storage denial") }
        catch (_: com.termirust.replication.security.MobileReplicationException.Locked) { }
        val reopened = repository().load()
        assertTrue(reopened.cancellationPending)
        assertArrayEquals(request, reopened.request)
        try { repository().recover(); fail("Recovery must not bypass reviewed cancellation") }
        catch (_: com.termirust.replication.security.MobileReplicationException.RecoveryRequired) { }
        assertArrayEquals(request, repository().load().request)
        assertNull(repository().cancel(reopened.request!!).request)
        assertNull(repository().load().request)
    }

    @Test fun recoveryRequiresConfirmationAndCanBeDismissed() {
        var recovered = 0
        compose.setContent { MaterialTheme {
            EnrollmentContent(EnrollmentState(loading = false, problem = EnrollmentProblem.RECOVERY),
                {}, {}, {}, {}, {}, {}, onRecover = { recovered++ })
        } }
        compose.onNodeWithText("Review enrollment recovery").performScrollTo().performTouchInput { click() }
        compose.runOnIdle { assertEquals(0, recovered) }
        compose.onNodeWithText("Recover enrollment").assertIsDisplayed()
        capture("activation-recovery-review")
        compose.onNodeWithText(context.getString(com.termirust.mobile.R.string.enrollment_bundle_back)).performTouchInput { click() }
        compose.runOnIdle { assertEquals(0, recovered) }
        compose.onNodeWithText("Review enrollment recovery").performScrollTo().performTouchInput { click() }
        compose.onNodeWithText("Recover enrollment").performTouchInput { click() }
        compose.runOnIdle { assertEquals(1, recovered) }
    }

    @Test fun lockedStateDoesNotOfferPreparationOrExport() {
        compose.setContent { MaterialTheme {
            EnrollmentContent(EnrollmentState(loading = false, problem = EnrollmentProblem.LOCKED), {}, {}, {}, {}, {}, {})
        } }
        compose.onNodeWithText("Request status unavailable").assertIsDisplayed()
        compose.onNodeWithText("Prepare request").assertIsNotEnabled()
        compose.onNodeWithText("Device credentials").assertIsDisplayed()
        capture("locked")
    }

    private fun capture(name: String) {
        val out = File(context.getExternalFilesDir(null), "c04-evidence").apply { mkdirs() }
        val bitmap = InstrumentationRegistry.getInstrumentation().uiAutomation.takeScreenshot()
        val config = context.resources.configuration
        File(out, "${config.screenWidthDp}x${config.screenHeightDp}-$name.png").outputStream().use { bitmap.compress(Bitmap.CompressFormat.PNG, 100, it) }
        bitmap.recycle()
    }

    @Test fun desktopCodeIsRequiredBeforeAcceptance() {
        val review = EnrollmentBundleReview(byteArrayOf(1), byteArrayOf(2), "workspace", "recipient", "ABC123-DEF456")
        var accepted: String? = null
        compose.setContent { MaterialTheme {
            EnrollmentContent(EnrollmentState(snapshot = EnrollmentSnapshot(byteArrayOf(1)), loading = false, bundleReview = review),
                {}, {}, {}, {}, {}, {}, onAcceptBundle = { accepted = it })
        } }
        compose.onNodeWithText("Accept enrollment").assertIsNotEnabled()
        compose.onNodeWithText("Verification code shown on desktop").performTextInput("WRONG")
        compose.onNodeWithText("Accept enrollment").assertIsNotEnabled()
        compose.onNodeWithText("Verification code shown on desktop").performTextReplacement("ABC123-DEF456")
        compose.onNodeWithText("Accept enrollment").assertIsEnabled().assertIsDisplayed()
        compose.waitForIdle()
        compose.onNodeWithText("Verification code shown on desktop").assertIsDisplayed()
        capture("bundle-keyboard-review")
        compose.onNodeWithText("Accept enrollment").performTouchInput { click() }
        compose.runOnIdle { assertEquals("ABC123-DEF456", accepted) }
        capture("bundle-review")
    }

    @Test fun configuredStateDoesNotOfferNewIdentityOrRequestActions() {
        compose.setContent { MaterialTheme {
            EnrollmentContent(EnrollmentState(snapshot = EnrollmentSnapshot(configured = true), loading = false), {}, {}, {}, {}, {}, {})
        } }
        compose.onNodeWithText("Enrollment configured").assertIsDisplayed()
        compose.onNodeWithText("Prepare request").assertDoesNotExist()
        compose.onNodeWithText("Copy request").assertDoesNotExist()
        compose.onNodeWithText("Cancel request").assertDoesNotExist()
        capture("configured")
    }

    @Test fun hostImportRequiresSelectionAndExplicitConfirmation() {
        val host = ImportedHost("record", "Reviewed host", "example.test", 2222, "demo")
        var selected: String? = null
        var imported = 0
        var review by androidx.compose.runtime.mutableStateOf<HostImportReview?>(null)
        compose.setContent { MaterialTheme {
            HostImportContent(HostImportState(candidates = listOf(host), review = review, loading = false),
                {}, {}, {}, onSelect = { selected = it; review = HostImportReview(byteArrayOf(1), byteArrayOf(2), host, true) },
                onDiscard = { review = null }, onImport = { imported++ })
        } }
        compose.onNodeWithText("Reviewed host").assertIsDisplayed()
        compose.onNodeWithText("Review").performTouchInput { click() }
        compose.runOnIdle { assertEquals("record", selected); assertEquals(0, imported) }
        compose.onNodeWithText("Review host import").assertIsDisplayed()
        compose.onNodeWithText("Import host").assertIsDisplayed().performTouchInput { click() }
        compose.runOnIdle { assertEquals(1, imported) }
        capture("host-import-review")
    }

    @Test fun hostImportMissingKeyDisablesFileSelection() {
        compose.setContent { MaterialTheme {
            HostImportContent(HostImportState(loading = false, problem = HostImportProblem.MISSING_KEY), {}, {}, {}, {}, {}, {})
        } }
        compose.onNodeWithContentDescription("Choose encrypted host file").assertIsNotEnabled()
        compose.onNodeWithText("An enrollment key is missing. No replacement key was created.").assertIsDisplayed()
        capture("host-import-missing-key")
    }
}
