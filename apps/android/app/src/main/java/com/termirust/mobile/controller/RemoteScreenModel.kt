package com.termirust.mobile.controller

import android.graphics.Bitmap
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import com.termirust.screens.ScreenControlHolder
import com.termirust.screens.ScreenEvent
import com.termirust.screens.ScreenPointerButton
import com.termirust.screens.ScreenRect
import com.termirust.screens.ScreenSurface
import com.termirust.screens.ScreenViewerInterface
import kotlin.math.floor
import kotlin.math.max
import kotlin.math.min
import kotlin.math.roundToInt

/** What a phone shows while watching a computer's screen. */
sealed class RemoteScreenState {
    /** Waiting for the first pixels. */
    object Opening : RemoteScreenState()
    object Watching : RemoteScreenState()

    /** The computer stopped sharing, or the session ended. */
    data class Closed(val reason: String) : RemoteScreenState()
}

/** How a touch on the picture reaches the computer's pointer. */
enum class RemotePointerMode(val title: String) {
    /** The pointer goes where the finger lands. Direct, but a finger hides what it touches. */
    TOUCH("Touch"),

    /** The finger drags the pointer from where it was, as a trackpad does. */
    TRACKPAD("Trackpad"),
}

/** A key the phone can send that a text field cannot type. */
data class RemoteScreenKey(
    val id: String,
    val label: String,
    /** USB HID usage on the keyboard page; layouts stay the computer's business. */
    val usage: UShort,
    val modifiers: UByte = 0u,
) {
    companion object {
        val ESCAPE = RemoteScreenKey("escape", "esc", 0x29u)
        val TAB = RemoteScreenKey("tab", "tab", 0x2Bu)
        val CONTROL = RemoteScreenKey("control", "ctrl", 0xE0u)
        val LEFT = RemoteScreenKey("left", "←", 0x50u)
        val UP = RemoteScreenKey("up", "↑", 0x52u)
        val DOWN = RemoteScreenKey("down", "↓", 0x51u)
        val RIGHT = RemoteScreenKey("right", "→", 0x4Fu)
        val PIPE = RemoteScreenKey("pipe", "|", 0x31u, 1u)
        val MINUS = RemoteScreenKey("minus", "-", 0x2Du)

        /** The row above the keyboard, in the order the design shows them. */
        val ACCESSORY = listOf(ESCAPE, TAB, CONTROL, LEFT, UP, DOWN, RIGHT, PIPE, MINUS)
    }
}

/**
 * Draws one shared screen and turns touches on it into pointer and keyboard input.
 *
 * Pixels arrive as damaged rectangles, so the picture is kept in one bitmap and only the
 * rectangles that changed are redrawn. A phone screen is a fraction of a desktop, so the picture
 * is fitted, may be zoomed and panned, and touches are mapped back through the same transform.
 */
class RemoteScreenModel(
    private val viewer: ScreenViewerInterface,
    surface: UInt?,
    ticket: ControllerScreenTicket,
    /** A preview is the computer's thumbnail profile: about one small picture a second. */
    private val preview: Boolean = false,
) {
    var state: RemoteScreenState by mutableStateOf(RemoteScreenState.Opening)
        private set
    var picture: Bitmap? by mutableStateOf(null)
        private set
    var control: ScreenControlHolder by mutableStateOf(ScreenControlHolder.NOBODY)
        private set
    var displayName: String? by mutableStateOf(null)
        private set
    var displays: List<ScreenSurface> by mutableStateOf(emptyList())
        private set
    var zoom: Float by mutableStateOf(1f)
        private set
    var panX: Float by mutableStateOf(0f)
        private set
    var panY: Float by mutableStateOf(0f)
        private set
    var pointerMode: RemotePointerMode by mutableStateOf(RemotePointerMode.TOUCH)
    var pointerX: Float by mutableStateOf(0f)
        private set
    var pointerY: Float by mutableStateOf(0f)
        private set
    var picturesDrawn: Int by mutableStateOf(0)
        private set
    var lastPictureAtMillis: Long? by mutableStateOf(null)
        private set

    val canControlPointer: Boolean = ticket.canControlPointer
    val canControlKeyboard: Boolean = ticket.canControlKeyboard

    private var watching: UInt? = surface
    private var surfaceWidth = 0f
    private var surfaceHeight = 0f

    val surfaceSize: Pair<Float, Float> get() = surfaceWidth to surfaceHeight

    /** Whether this device may send anything at all right now. */
    val isDriving: Boolean
        get() = !preview && canControlPointer && control == ScreenControlHolder.YOU

    /** Lets a test check the fit, zoom and pan arithmetic without a live session's pixels. */
    fun setSurfaceSizeForTesting(width: Float, height: Float) {
        surfaceWidth = width
        surfaceHeight = height
        pointerX = width / 2
        pointerY = height / 2
    }

    /** Applies what arrived in one screen frame and redraws the parts that changed. */
    fun apply(events: List<ScreenEvent>, nowMillis: Long = System.currentTimeMillis()) {
        for (event in events) {
            when (event) {
                is ScreenEvent.Welcomed -> {
                    displays = event.surfaces
                    val display = watching?.let { id -> event.surfaces.firstOrNull { it.id == id } }
                        ?: event.surfaces.firstOrNull()
                        ?: continue
                    watching = display.id
                    displayName = display.name
                }
                is ScreenEvent.Updated -> {
                    if (event.surface != watching || event.preview != preview) continue
                    redraw(event.damaged, event.reset, nowMillis)
                }
                is ScreenEvent.Control -> control = event.holder
                is ScreenEvent.Closed -> state = RemoteScreenState.Closed(event.reason)
                is ScreenEvent.MotionRegion, is ScreenEvent.Panes -> Unit
            }
        }
    }

    /** How many view pixels one of the computer's pixels takes, at the current zoom. */
    fun scale(viewWidth: Float, viewHeight: Float): Float {
        if (surfaceWidth <= 0f || surfaceHeight <= 0f || viewWidth <= 0f || viewHeight <= 0f) {
            return 0f
        }
        return min(viewWidth / surfaceWidth, viewHeight / surfaceHeight) * zoom
    }

    /** The top-left of the drawn picture in the view, once fitted, zoomed and dragged. */
    fun pictureOrigin(viewWidth: Float, viewHeight: Float): Pair<Float, Float> {
        val scale = scale(viewWidth, viewHeight)
        if (scale <= 0f) return 0f to 0f
        return ((viewWidth - surfaceWidth * scale) / 2 + panX) to
            ((viewHeight - surfaceHeight * scale) / 2 + panY)
    }

    /** Where a touch lands on the computer's screen, or null when it misses the picture. */
    fun surfacePoint(x: Float, y: Float, viewWidth: Float, viewHeight: Float): Pair<UInt, UInt>? {
        val scale = scale(viewWidth, viewHeight)
        if (scale <= 0f) return null
        val (originX, originY) = pictureOrigin(viewWidth, viewHeight)
        val onPicture = ((x - originX) / scale) to ((y - originY) / scale)
        if (onPicture.first < 0f || onPicture.second < 0f ||
            onPicture.first >= surfaceWidth || onPicture.second >= surfaceHeight
        ) {
            return null
        }
        return floor(onPicture.first).toUInt() to floor(onPicture.second).toUInt()
    }

    /** Zooms to [factor], keeping the middle of the view on the same part of the picture. */
    fun setZoom(factor: Float, viewWidth: Float, viewHeight: Float) {
        val clamped = min(max(factor, 1f), MAXIMUM_ZOOM)
        if (clamped == zoom) return
        val before = scale(viewWidth, viewHeight)
        zoom = clamped
        val after = scale(viewWidth, viewHeight)
        if (before > 0f && after > 0f) {
            panX = panX * after / before
            panY = panY * after / before
        }
        clampPan(viewWidth, viewHeight)
    }

    /** Drags the magnified picture. At fit there is nothing to drag. */
    fun panBy(dx: Float, dy: Float, viewWidth: Float, viewHeight: Float) {
        if (zoom <= 1f) return
        panX += dx
        panY += dy
        clampPan(viewWidth, viewHeight)
    }

    /** Back to the whole picture. */
    fun fit() {
        zoom = 1f
        panX = 0f
        panY = 0f
    }

    /** What the zoom chip reads. */
    val zoomLabel: String
        get() = if (zoom <= 1.01f) "Fit" else "${(zoom * 100).roundToInt()}%"

    /** The part of the computer's screen the view is showing, for the minimap. */
    fun visibleRect(viewWidth: Float, viewHeight: Float): FloatArray {
        val scale = scale(viewWidth, viewHeight)
        if (scale <= 0f) return floatArrayOf(0f, 0f, 0f, 0f)
        val (originX, originY) = pictureOrigin(viewWidth, viewHeight)
        val left = max(-originX / scale, 0f)
        val top = max(-originY / scale, 0f)
        val right = min(left + viewWidth / scale, surfaceWidth)
        val bottom = min(top + viewHeight / scale, surfaceHeight)
        return floatArrayOf(left, top, max(right - left, 0f), max(bottom - top, 0f))
    }

    /** Asks the computer for the writer lease. It answers with who holds control. */
    fun requestControl() {
        if (preview || !(canControlPointer || canControlKeyboard)) return
        viewer.requestControl()
    }

    fun releaseControl() {
        if (preview) return
        viewer.releaseControl()
    }

    /**
     * Sends a tap as a press and release. In trackpad mode the finger has already moved the
     * pointer, so a tap clicks where the pointer is rather than where the finger landed.
     */
    fun tap(x: Float, y: Float, viewWidth: Float, viewHeight: Float) {
        if (!isDriving) return
        val surface = watching ?: return
        val target = when (pointerMode) {
            RemotePointerMode.TOUCH -> surfacePoint(x, y, viewWidth, viewHeight)?.also {
                pointerX = it.first.toFloat()
                pointerY = it.second.toFloat()
            }
            RemotePointerMode.TRACKPAD ->
                floor(pointerX).toUInt() to floor(pointerY).toUInt()
        } ?: return
        viewer.sendPointerMove(surface, target.first, target.second)
        viewer.sendPointerButton(
            surface, target.first, target.second, ScreenPointerButton.PRIMARY, true,
        )
        viewer.sendPointerButton(
            surface, target.first, target.second, ScreenPointerButton.PRIMARY, false,
        )
    }

    /** Moves the pointer the way a trackpad does: by how far the finger went. */
    fun movePointer(dx: Float, dy: Float, viewWidth: Float, viewHeight: Float) {
        if (!isDriving || pointerMode != RemotePointerMode.TRACKPAD) return
        val surface = watching ?: return
        val scale = scale(viewWidth, viewHeight)
        if (scale <= 0f) return
        pointerX = min(max(pointerX + dx / scale, 0f), max(surfaceWidth - 1f, 0f))
        pointerY = min(max(pointerY + dy / scale, 0f), max(surfaceHeight - 1f, 0f))
        viewer.sendPointerMove(surface, floor(pointerX).toUInt(), floor(pointerY).toUInt())
    }

    /** Scrolls the computer under the pointer, in its own pixels. */
    fun scroll(dx: Float, dy: Float, x: Float, y: Float, viewWidth: Float, viewHeight: Float) {
        if (!isDriving) return
        val surface = watching ?: return
        val scale = scale(viewWidth, viewHeight)
        if (scale <= 0f) return
        val target = if (pointerMode == RemotePointerMode.TRACKPAD) {
            floor(pointerX).toUInt() to floor(pointerY).toUInt()
        } else {
            surfacePoint(x, y, viewWidth, viewHeight)
        } ?: return
        viewer.sendScroll(
            surface,
            target.first,
            target.second,
            (dx / scale).roundToInt(),
            (dy / scale).roundToInt(),
        )
    }

    /** Types text the computer will insert as typed characters. */
    fun sendText(text: String) {
        if (preview || !canControlKeyboard || control != ScreenControlHolder.YOU) return
        val surface = watching ?: return
        if (text.isEmpty()) return
        viewer.sendText(surface, text)
    }

    /** Presses and releases one key a text field cannot type. */
    fun sendKey(key: RemoteScreenKey) {
        if (preview || !canControlKeyboard || control != ScreenControlHolder.YOU) return
        runCatching { viewer.sendKey(key.usage, key.modifiers, true) }
        runCatching { viewer.sendKey(key.usage, key.modifiers, false) }
    }

    /** Watches a different display of the same computer. */
    fun watch(display: ScreenSurface) {
        if (display.id == watching) return
        watching?.let(viewer::unsubscribe)
        watching = display.id
        displayName = display.name
        picture = null
        surfaceWidth = 0f
        surfaceHeight = 0f
        state = RemoteScreenState.Opening
        fit()
        viewer.subscribe(display.id, preview)
    }

    private fun clampPan(viewWidth: Float, viewHeight: Float) {
        val scale = scale(viewWidth, viewHeight)
        if (scale <= 0f) return
        // Once the picture is wider than the view, its edges may not come inside it.
        val slackX = max((surfaceWidth * scale - viewWidth) / 2, 0f)
        val slackY = max((surfaceHeight * scale - viewHeight) / 2, 0f)
        panX = min(max(panX, -slackX), slackX)
        panY = min(max(panY, -slackY), slackY)
    }

    private fun redraw(damaged: List<ScreenRect>, reset: Boolean, nowMillis: Long) {
        val surface = watching ?: return
        val whole = viewer.surfaceSize(surface, preview) ?: return
        var canvas = picture
        if (canvas == null ||
            canvas.width != whole.width.toInt() ||
            canvas.height != whole.height.toInt()
        ) {
            canvas = Bitmap.createBitmap(
                whole.width.toInt(),
                whole.height.toInt(),
                Bitmap.Config.ARGB_8888,
            )
            surfaceWidth = whole.width.toFloat()
            surfaceHeight = whole.height.toFloat()
            // A trackpad pointer starts in the middle, where a person can find it.
            pointerX = surfaceWidth / 2
            pointerY = surfaceHeight / 2
        }
        val rects = if (reset) listOf(whole) else damaged
        for (rect in rects) {
            val pixels = runCatching { viewer.copyPixels(surface, preview, rect) }.getOrNull()
                ?: continue
            draw(pixels.bgra, pixels.rect, canvas)
        }
        picture = canvas
        picturesDrawn += 1
        lastPictureAtMillis = nowMillis
        if (state is RemoteScreenState.Opening) state = RemoteScreenState.Watching
    }

    /** BGRA from the computer into the ARGB_8888 bitmap Android draws. */
    private fun draw(bgra: ByteArray, rect: ScreenRect, canvas: Bitmap) {
        val width = rect.width.toInt()
        val height = rect.height.toInt()
        if (width <= 0 || height <= 0 || bgra.size != width * height * 4) return
        val colours = IntArray(width * height)
        for (index in colours.indices) {
            val offset = index * 4
            val blue = bgra[offset].toInt() and 0xFF
            val green = bgra[offset + 1].toInt() and 0xFF
            val red = bgra[offset + 2].toInt() and 0xFF
            colours[index] = (0xFF shl 24) or (red shl 16) or (green shl 8) or blue
        }
        canvas.setPixels(colours, 0, width, rect.x.toInt(), rect.y.toInt(), width, height)
    }

    companion object {
        /** As far in as a finger may zoom. Past this a phone is magnifying its own blur. */
        const val MAXIMUM_ZOOM = 6f

        /** How long without a picture before the interface says the connection is struggling. */
        const val WEAK_AFTER_MILLIS = 4_000L
    }
}
