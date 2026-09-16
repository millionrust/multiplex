package com.termirust.mobile.controller

import java.util.UUID
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Zoom, pan, the minimap and the two pointer modes: the arithmetic a finger depends on, and the
 * rules about which computers a phone may watch at all.
 *
 * These never open a session, so the model is built without a viewer: everything measured here is
 * geometry and permission, not pixels.
 */
class RemoteScreenModelTest {
    private val view = 500f

    private fun ticket(pointer: Boolean = true, keyboard: Boolean = true) =
        ControllerScreenTicket(
            commandId = UUID.randomUUID(),
            ticket = ByteArray(32) { 3 },
            canControlPointer = pointer,
            canControlKeyboard = keyboard,
        )

    /** A 1000x500 computer shown in a 500x500 view fits at half size. */
    private fun model(): RemoteScreenModel {
        val model = RemoteScreenModel(
            viewer = NoViewer,
            surface = 1u,
            ticket = ticket(),
        )
        model.setSurfaceSizeForTesting(1000f, 500f)
        return model
    }

    @Test
    fun `fit is the starting point and reads as fit`() {
        val model = model()
        assertEquals(1f, model.zoom)
        assertEquals("Fit", model.zoomLabel)
        assertEquals(0.5f, model.scale(view, view))
        assertEquals(125f, model.pictureOrigin(view, view).second)
    }

    @Test
    fun `zooming magnifies about the middle and says so`() {
        val model = model()
        model.setZoom(2f, view, view)
        assertEquals("200%", model.zoomLabel)
        assertEquals(1f, model.scale(view, view))
        assertEquals(-250f, model.pictureOrigin(view, view).first)
    }

    @Test
    fun `zoom stops at the limits rather than running away`() {
        val model = model()
        model.setZoom(0.1f, view, view)
        assertEquals(1f, model.zoom)
        model.setZoom(1000f, view, view)
        assertEquals(RemoteScreenModel.MAXIMUM_ZOOM, model.zoom)
    }

    @Test
    fun `the picture cannot be dragged off the view`() {
        val model = model()
        model.setZoom(2f, view, view)
        model.panBy(10_000f, 10_000f, view, view)
        assertEquals(250f, model.panX)
        assertEquals(0f, model.panY)
    }

    @Test
    fun `panning does nothing while the whole picture is shown`() {
        val model = model()
        model.panBy(100f, 100f, view, view)
        assertEquals(0f, model.panX)
        assertEquals(0f, model.panY)
    }

    @Test
    fun `a tap lands where the picture shows it after zooming and panning`() {
        val model = model()
        model.setZoom(2f, view, view)
        model.panBy(250f, 0f, view, view)
        val target = model.surfacePoint(0f, 250f, view, view)
        assertEquals(0u to 250u, target)
    }

    @Test
    fun `a touch outside the picture reaches nothing`() {
        val model = model()
        // Fitted, a 1000x500 computer leaves bars above and below in a square view.
        assertNull(model.surfacePoint(250f, 10f, view, view))
    }

    @Test
    fun `the minimap shows which part of the computer is on screen`() {
        val model = model()
        model.setZoom(2f, view, view)
        val visible = model.visibleRect(view, view)
        assertEquals(250f, visible[0], 0.5f)
        assertEquals(500f, visible[2], 0.5f)
        assertEquals(500f, visible[3], 0.5f)
    }

    @Test
    fun `the whole picture is visible at fit`() {
        val model = model()
        val visible = model.visibleRect(view, view)
        assertEquals(1000f, visible[2], 0.5f)
        assertEquals(500f, visible[3], 0.5f)
    }

    @Test
    fun `the trackpad pointer stays put without control`() {
        val model = model()
        model.pointerMode = RemotePointerMode.TRACKPAD
        assertEquals(500f, model.pointerX)
        model.movePointer(50f, 0f, view, view)
        assertEquals(500f, model.pointerX)
        assertFalse(model.isDriving)
    }

    @Test
    fun `accessory keys cover what a text field cannot type`() {
        assertEquals(
            listOf("esc", "tab", "ctrl", "←", "↑", "↓", "→", "|", "-"),
            RemoteScreenKey.ACCESSORY.map(RemoteScreenKey::label),
        )
        assertEquals(0x29u.toUShort(), RemoteScreenKey.ESCAPE.usage)
        assertEquals(1u.toUByte(), RemoteScreenKey.PIPE.modifiers, )
    }
}

/** Which computers the phone may watch, and what a ticket must look like. */
class ControllerScreenSessionTest {
    private fun host(capabilities: Int) = PairedHostRecord(
        id = "host-1",
        displayName = "Office Computer",
        route = HostRoute("192.168.1.10", 63322),
        hostStaticPublicKey = java.util.Base64.getEncoder().encodeToString(ByteArray(32) { 1 }),
        deviceStaticKeyId = "key-1",
        deviceId = UUID.randomUUID().toString(),
        identityGeneration = 1,
        revocationEpoch = 1,
        sessionGeneration = 1,
        capabilityBits = capabilities,
        pairedAtMillis = 1_000,
    )

    @Test
    fun `only a computer that granted screen access may be watched`() {
        assertFalse(ControllerScreenCoordinator.mayWatch(host(0b11)))
        assertTrue(ControllerScreenCoordinator.mayWatch(host(0b10_0011)))
    }

    @Test
    fun `a ticket is exactly thirty-two bytes`() {
        val commandId = UUID.randomUUID()
        val response = ScreenOpenedResponse(
            kind = "screen_opened",
            commandId = commandId.toString(),
            ticket = List(31) { 7 },
            canControlPointer = true,
            canControlKeyboard = false,
        )
        assertThrows(ControllerScreenException.InvalidTicket::class.java) {
            ControllerScreenCommands.ticket(response, commandId)
        }
    }

    @Test
    fun `a response to a different command is refused`() {
        val response = ScreenOpenedResponse(
            kind = "screen_opened",
            commandId = UUID.randomUUID().toString(),
            ticket = List(32) { 7 },
            canControlPointer = true,
            canControlKeyboard = false,
        )
        assertThrows(ControllerScreenException.MalformedResponse::class.java) {
            ControllerScreenCommands.ticket(response, UUID.randomUUID())
        }
    }

    @Test
    fun `a ticket carries what the computer granted`() {
        val commandId = UUID.randomUUID()
        val ticket = ControllerScreenCommands.ticket(
            ScreenOpenedResponse(
                kind = "screen_opened",
                commandId = commandId.toString(),
                ticket = List(32) { 7 },
                canControlPointer = true,
                canControlKeyboard = false,
            ),
            commandId,
        )
        assertEquals(32, ticket.ticket.size)
        assertTrue(ticket.canControlPointer)
        assertFalse(ticket.canControlKeyboard)
        assertTrue(ticket.canControl)
    }

    /** The pump refuses to send what the computer never granted. */
    @Test
    fun `input the computer never granted is never sent`() {
        val pump = ControllerScreenPump(
            ControllerScreenTicket(
                commandId = UUID.randomUUID(),
                ticket = ByteArray(32),
                canControlPointer = true,
                canControlKeyboard = false,
            ),
        )
        assertTrue(pump.allows(com.termirust.screens.ScreenCapability.OBSERVE))
        assertTrue(pump.allows(com.termirust.screens.ScreenCapability.POINTER))
        assertFalse(pump.allows(com.termirust.screens.ScreenCapability.KEYBOARD))
    }
}

/**
 * A viewer that answers nothing, so the geometry above can be measured without a live session.
 * The model never draws here: every test sets the surface size directly.
 */
private object NoViewer : com.termirust.screens.ScreenViewerInterface {
    override fun attachPanes(sessions: List<ByteArray>) = Unit
    override fun connect(ticket: ByteArray) = Unit
    override fun control() = com.termirust.screens.ScreenControlHolder.NOBODY
    override fun copyPixels(
        surface: UInt,
        preview: Boolean,
        rect: com.termirust.screens.ScreenRect,
    ): com.termirust.screens.ScreenPixels =
        com.termirust.screens.ScreenPixels(rect, ByteArray(0))

    override fun disconnected() = Unit
    override fun pollOutgoing(): com.termirust.screens.ScreenOutgoing? = null
    override fun receive(bytes: ByteArray): List<com.termirust.screens.ScreenEvent> = emptyList()
    override fun releaseControl() = Unit
    override fun reportVideo(surface: UInt, held: List<UInt>, lost: ULong?) = Unit
    override fun requestControl() = Unit
    override fun sendKey(usage: UShort, modifiers: UByte, pressed: Boolean) = Unit
    override fun sendPointerButton(
        surface: UInt,
        x: UInt,
        y: UInt,
        button: com.termirust.screens.ScreenPointerButton,
        pressed: Boolean,
    ) = Unit

    override fun sendPointerMove(surface: UInt, x: UInt, y: UInt) = Unit
    override fun sendScroll(surface: UInt, x: UInt, y: UInt, dx: Int, dy: Int) = Unit
    override fun sendText(surface: UInt, text: String) = Unit
    override fun setViewport(
        surface: UInt,
        rect: com.termirust.screens.ScreenRect,
        scaleMilli: UShort,
    ) = Unit

    override fun subscribe(surface: UInt, preview: Boolean) = Unit
    override fun surfaceSize(surface: UInt, preview: Boolean): com.termirust.screens.ScreenRect? =
        null

    override fun unsubscribe(surface: UInt) = Unit
}
