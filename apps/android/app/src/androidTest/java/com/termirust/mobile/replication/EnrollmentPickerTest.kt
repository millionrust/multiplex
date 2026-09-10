package com.termirust.mobile.replication

import android.os.Process
import android.os.SystemClock
import android.view.KeyEvent
import android.view.MotionEvent
import android.graphics.Rect
import android.view.accessibility.AccessibilityNodeInfo
import androidx.compose.material3.MaterialTheme
import androidx.compose.ui.Modifier
import androidx.compose.ui.test.*
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.test.platform.app.InstrumentationRegistry
import kotlinx.coroutines.runBlocking
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Rule
import org.junit.Test
import java.io.File

/** Uses real ActivityResult launchers and the system DocumentsUI, never URI injection. */
class EnrollmentPickerTest {
    @get:Rule val compose = createComposeRule()
    private val instrumentation = InstrumentationRegistry.getInstrumentation()
    private val context = instrumentation.targetContext
    private val token = InstrumentationRegistry.getArguments().getString("fixture").orEmpty().also {
        require(it.matches(Regex("[a-f0-9-]{36}")))
    }
    private val name = "c07-$token"
    private val exchange = File(context.getExternalFilesDir(null), name)
    private val repository = NativeEnrollmentRepository(context, name, name)

    private fun awaitText(text: String) {
        compose.waitUntil(15_000) { compose.onAllNodesWithText(text).fetchSemanticsNodes().isNotEmpty() }
        compose.onNodeWithText(text).assertIsDisplayed()
    }
    private fun awaitPicker() {
        val deadline = SystemClock.uptimeMillis() + 10_000
        while (SystemClock.uptimeMillis() < deadline) {
            if (instrumentation.uiAutomation.rootInActiveWindow?.packageName?.contains("documentsui") == true) return
            SystemClock.sleep(100)
        }
        fail("System document picker did not open")
    }
    private fun tapSystem(text: String) {
        val deadline = SystemClock.uptimeMillis() + 10_000
        while (SystemClock.uptimeMillis() < deadline) {
            val root = instrumentation.uiAutomation.rootInActiveWindow
            val nodes = root?.findAccessibilityNodeInfosByText(text).orEmpty()
            val matches = nodes.filter { it.isVisibleToUser &&
                (it.text?.toString() == text || it.contentDescription?.toString() == text) }
            for (match in matches) {
                var node: AccessibilityNodeInfo? = match
                repeat(32) {
                    val current = node
                    if (current != null && current.isClickable && current.performAction(AccessibilityNodeInfo.ACTION_CLICK)) return
                    node = current?.parent
                }
            }
            // Some DocumentsUI rows expose a visible label without an accessible click action.
            if (text.endsWith(".json")) matches.firstOrNull()?.let { match ->
                val bounds = Rect().also(match::getBoundsInScreen)
                if (!bounds.isEmpty) {
                    val now = SystemClock.uptimeMillis()
                    for (action in listOf(MotionEvent.ACTION_DOWN, MotionEvent.ACTION_UP)) {
                        val event = MotionEvent.obtain(now, SystemClock.uptimeMillis(), action,
                            bounds.exactCenterX(), bounds.exactCenterY(), 0)
                        try { instrumentation.sendPointerSync(event) } finally { event.recycle() }
                    }
                    return
                }
            }
            SystemClock.sleep(100)
        }
        fail("System picker item not selectable: $text")
    }
    private fun choose(filename: String) {
        awaitPicker()
        tapSystem("Show roots")
        tapSystem("TermiRust fixture")
        tapSystem(filename)
    }
    private fun capture(label: String) {
        compose.waitForIdle()
        val bitmap = instrumentation.uiAutomation.takeScreenshot()
        try { File(exchange, "$label.png").outputStream().use { bitmap.compress(android.graphics.Bitmap.CompressFormat.PNG, 100, it) } }
        finally { bitmap.recycle() }
    }

    @Test fun accept() {
        assertNotEquals(File(exchange, "pid").readText().toInt(), Process.myPid())
        val request = runBlocking { repository.load().request!! }
        compose.setContent { MaterialTheme { EnrollmentScreen({}, Modifier, repository) } }
        awaitText("Request pending")
        compose.onNodeWithText("Select desktop enrollment bundle").performScrollTo().performTouchInput { click() }
        awaitPicker()
        instrumentation.sendKeyDownUpSync(KeyEvent.KEYCODE_BACK)
        awaitText("Request pending")
        assertArrayEquals(request, runBlocking { repository.load().request })
        compose.onNodeWithText("Select desktop enrollment bundle").performScrollTo().performTouchInput { click() }
        choose("Enrollment bundle.json")
        awaitText("Review desktop enrollment")
        compose.onNodeWithText("Accept enrollment").assertIsNotEnabled()
        val code = JSONObject(File(exchange, "review.json").readText()).getString("code")
        compose.onNodeWithText("Verification code shown on desktop").performTextInput(code)
        compose.onNodeWithText("Accept enrollment").assertIsDisplayed().performTouchInput { click() }
        awaitText("Enrollment configured")
        assertTrue(runBlocking { repository.load().configured })
        capture("accepted")
        File(exchange, "pid").writeText(Process.myPid().toString())
    }

    @Test fun importHost() {
        assertNotEquals(File(exchange, "pid").readText().toInt(), Process.myPid())
        compose.setContent { MaterialTheme { EnrollmentScreen({}, Modifier, repository) } }
        awaitText("Enrollment configured")
        compose.onNodeWithText("Imported hosts").performTouchInput { click() }
        awaitText("No imported hosts")
        compose.onNodeWithContentDescription("Choose encrypted host file").performTouchInput { click() }
        awaitPicker()
        instrumentation.sendKeyDownUpSync(KeyEvent.KEYCODE_BACK)
        awaitText("No imported hosts")
        compose.onNodeWithContentDescription("Choose encrypted host file").assertIsEnabled().performTouchInput { click() }
        choose("Encrypted hosts.json")
        awaitText("Desktop fixture host")
        assertTrue(runBlocking { repository.importedHosts().isEmpty() })
        compose.onNodeWithText("Review").performTouchInput { click() }
        awaitText("Review host import")
        compose.onNodeWithText("Import host").performTouchInput { click() }
        compose.waitUntil(10_000) { compose.onAllNodesWithText("Review host import").fetchSemanticsNodes().isEmpty() }
        compose.waitUntil(10_000) {
            compose.onAllNodes(hasContentDescription("Choose encrypted host file") and isEnabled())
                .fetchSemanticsNodes().isNotEmpty()
        }
        compose.onNodeWithContentDescription("Choose encrypted host file").assertIsEnabled()
        assertEquals("Desktop fixture host", runBlocking { repository.importedHosts().single().label })
        capture("imported")
        File(exchange, "pid").writeText(Process.myPid().toString())
    }
}
