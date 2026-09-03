package com.termirust.mobile.controller

import android.content.Context
import android.text.InputType
import android.view.KeyEvent
import android.view.View
import android.view.inputmethod.BaseInputConnection
import android.view.inputmethod.EditorInfo
import android.view.inputmethod.InputConnection
import android.view.inputmethod.InputMethodManager
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.viewinterop.AndroidView

private class ControllerTerminalInputBridge(context: Context) : View(context) {
    var acceptsInput: Boolean = false
    var applicationCursor: Boolean = false
    var onBytes: (ByteArray) -> Unit = {}
    private var ime = TerminalImeState()

    init {
        isFocusable = true
        isFocusableInTouchMode = true
        importantForAutofill = IMPORTANT_FOR_AUTOFILL_NO_EXCLUDE_DESCENDANTS
    }

    override fun onCheckIsTextEditor(): Boolean = true

    override fun onCreateInputConnection(outAttrs: EditorInfo): InputConnection {
        outAttrs.inputType = InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS or
            InputType.TYPE_TEXT_FLAG_MULTI_LINE
        outAttrs.imeOptions = EditorInfo.IME_FLAG_NO_EXTRACT_UI or EditorInfo.IME_ACTION_NONE
        return object : BaseInputConnection(this@ControllerTerminalInputBridge, false) {
            override fun commitText(text: CharSequence?, newCursorPosition: Int): Boolean {
                ime.commit(text?.toString().orEmpty())?.let(::send)
                return true
            }

            override fun setComposingText(text: CharSequence?, newCursorPosition: Int): Boolean {
                ime.update(text?.toString().orEmpty())
                return true
            }

            override fun finishComposingText(): Boolean {
                ime.finish()?.let(::send)
                return true
            }

            override fun deleteSurroundingText(beforeLength: Int, afterLength: Int): Boolean {
                repeat(beforeLength.coerceIn(0, 64)) { sendKey(TerminalInputKey.BACKSPACE) }
                return true
            }

            override fun closeConnection() {
                ime.cancel()
                super.closeConnection()
            }

            override fun sendKeyEvent(event: KeyEvent): Boolean = handleKey(event) || super.sendKeyEvent(event)
        }
    }

    override fun onKeyDown(keyCode: Int, event: KeyEvent): Boolean = handleKey(event) || super.onKeyDown(keyCode, event)

    fun showKeyboard() {
        if (!acceptsInput) return
        requestFocus()
        (context.getSystemService(Context.INPUT_METHOD_SERVICE) as InputMethodManager)
            .showSoftInput(this, InputMethodManager.SHOW_IMPLICIT)
    }

    fun hideKeyboard() {
        ime.cancel()
        (context.getSystemService(Context.INPUT_METHOD_SERVICE) as InputMethodManager)
            .hideSoftInputFromWindow(windowToken, 0)
        clearFocus()
    }

    private fun handleKey(event: KeyEvent): Boolean {
        if (!acceptsInput || event.action != KeyEvent.ACTION_DOWN) return false
        val fixed = when (event.keyCode) {
            KeyEvent.KEYCODE_ENTER -> TerminalInputKey.ENTER
            KeyEvent.KEYCODE_DEL -> TerminalInputKey.BACKSPACE
            KeyEvent.KEYCODE_FORWARD_DEL -> TerminalInputKey.DELETE
            KeyEvent.KEYCODE_TAB -> TerminalInputKey.TAB
            KeyEvent.KEYCODE_ESCAPE -> TerminalInputKey.ESCAPE
            KeyEvent.KEYCODE_DPAD_UP -> TerminalInputKey.UP
            KeyEvent.KEYCODE_DPAD_DOWN -> TerminalInputKey.DOWN
            KeyEvent.KEYCODE_DPAD_RIGHT -> TerminalInputKey.RIGHT
            KeyEvent.KEYCODE_DPAD_LEFT -> TerminalInputKey.LEFT
            KeyEvent.KEYCODE_MOVE_HOME -> TerminalInputKey.HOME
            KeyEvent.KEYCODE_MOVE_END -> TerminalInputKey.END
            KeyEvent.KEYCODE_INSERT -> TerminalInputKey.INSERT
            KeyEvent.KEYCODE_PAGE_UP -> TerminalInputKey.PAGEUP
            KeyEvent.KEYCODE_PAGE_DOWN -> TerminalInputKey.PAGEDOWN
            else -> null
        }
        if (fixed != null) {
            sendKey(
                fixed,
                TerminalInputModifiers(
                    shift = event.isShiftPressed,
                    control = event.isCtrlPressed,
                    alt = event.isAltPressed,
                ),
            )
            return true
        }
        val unicode = event.unicodeChar
        if (unicode == 0) return false
        val text = String(Character.toChars(unicode))
        TerminalInteraction.encode(
            if (text == " ") TerminalInputKey.SPACE else TerminalInputKey.TEXT,
            text,
            TerminalInputModifiers(
                shift = event.isShiftPressed,
                control = event.isCtrlPressed,
                alt = event.isAltPressed,
            ),
            applicationCursor,
        )?.let(::send)
        return true
    }

    private fun sendKey(
        key: TerminalInputKey,
        modifiers: TerminalInputModifiers = TerminalInputModifiers(),
    ) {
        TerminalInteraction.encode(key, modifiers = modifiers, applicationCursor = applicationCursor)
            ?.let(::send)
    }
    private fun send(bytes: ByteArray) {
        if (acceptsInput && bytes.isNotEmpty()) onBytes(bytes)
    }
}

@Composable
fun ControllerTerminalInputView(
    enabled: Boolean,
    keyboardRequest: Long,
    showKeyboard: Boolean,
    applicationCursor: Boolean,
    onBytes: (ByteArray) -> Unit,
    modifier: Modifier = Modifier,
) {
    val bridge = remember { arrayOfNulls<ControllerTerminalInputBridge>(1) }
    AndroidView(
        modifier = modifier,
        factory = { context -> ControllerTerminalInputBridge(context).also { bridge[0] = it } },
        update = {
            bridge[0] = it
            it.acceptsInput = enabled
            it.applicationCursor = applicationCursor
            it.onBytes = onBytes
            if (!enabled) it.hideKeyboard()
        },
    )
    LaunchedEffect(keyboardRequest, enabled) {
        if (keyboardRequest <= 0) return@LaunchedEffect
        if (enabled && showKeyboard) {
            bridge[0]?.showKeyboard()
        } else {
            bridge[0]?.hideKeyboard()
        }
    }
}
