package com.multiplex.mobile.controller

import com.multiplex.screens.ScreenRect
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The bookkeeping around the motion decoder: which references this phone may tell the computer it
 * holds, where a decoded picture goes, and what happens when there is no decoder.
 *
 * `MediaCodec` itself is not here — it needs a device — so the decoder is faked. What is being
 * tested is the half that can be got wrong silently: acknowledging a reference the decoder does
 * not hold would have the computer predict from a picture this phone never decoded, and nothing
 * would report it.
 */
class MotionViewTest {
    private val region = ScreenRect(x = 128u, y = 64u, width = 256u, height = 192u)

    /** A decoder that answers every frame with a flat picture. */
    private class Fake(private val answers: Boolean = true) : MotionDecoder {
        var decoded = 0
        var closed = false

        override fun decode(payload: ByteArray): MotionPicture? {
            decoded += 1
            if (!answers) return null
            return MotionPicture(4, 4, IntArray(16) { 0xFF112233.toInt() })
        }

        override fun close() {
            closed = true
        }
    }

    private class Fakes(private val decoder: MotionDecoder?) : MotionDecoders {
        val opened = mutableListOf<Pair<Int, Int>>()

        override fun open(parameterSets: ByteArray, width: Int, height: Int): MotionDecoder? {
            opened.add(width to height)
            return decoder
        }
    }

    @Test
    fun `a decoded frame reports the reference it now holds`() {
        val decoder = Fake()
        val view = MotionView(Fakes(decoder))
        view.configure(1u, region, byteArrayOf(0, 0, 0, 1, 0x40))

        val decoded = view.decode(1u, token = 7u, payload = byteArrayOf(0, 0, 0, 1, 0x26))
        assertEquals(1, decoder.decoded)
        assertEquals(listOf(7u), decoded.holding)
        assertEquals(region, decoded.rect)
        assertEquals(4, decoded.picture?.width)
    }

    @Test
    fun `a reference already held is not reported again`() {
        val view = MotionView(Fakes(Fake()))
        view.configure(1u, region, byteArrayOf(0, 0, 0, 1, 0x40))
        assertEquals(listOf(7u), view.decode(1u, 7u, ByteArray(4)).holding)
        assertNull(
            "the computer was already told about this one",
            view.decode(1u, 7u, ByteArray(4)).holding,
        )
        assertEquals(listOf(7u, 9u), view.decode(1u, 9u, ByteArray(4)).holding)
    }

    @Test
    fun `a frame that did not decode acknowledges nothing`() {
        val decoder = Fake(answers = false)
        val view = MotionView(Fakes(decoder))
        view.configure(1u, region, byteArrayOf(0, 0, 0, 1, 0x40))

        val decoded = view.decode(1u, token = 7u, payload = ByteArray(4))
        assertEquals(1, decoder.decoded)
        assertNull("nothing to draw", decoded.picture)
        assertNull(
            "acknowledging here would have the computer predict from a picture this phone " +
                "never decoded",
            decoded.holding,
        )
    }

    @Test
    fun `a phone with no decoder shows nothing and claims nothing`() {
        val decoders = Fakes(null)
        val view = MotionView(decoders)
        view.configure(1u, region, byteArrayOf(0, 0, 0, 1, 0x40))
        assertFalse(view.isDecoding(1u))
        assertEquals(listOf(256 to 192), decoders.opened)

        val decoded = view.decode(1u, 7u, ByteArray(4))
        assertNull(decoded.picture)
        assertNull(decoded.holding)
    }

    @Test
    fun `a frame for a surface that was never configured is ignored`() {
        val view = MotionView(Fakes(Fake()))
        val decoded = view.decode(9u, 1u, ByteArray(4))
        assertNull(decoded.picture)
        assertNull(decoded.holding)
    }

    @Test
    fun `a new configuration replaces the decoder and forgets its references`() {
        val first = Fake()
        val view = MotionView(Fakes(first))
        view.configure(1u, region, byteArrayOf(0, 0, 0, 1, 0x40))
        assertEquals(listOf(7u), view.decode(1u, 7u, ByteArray(4)).holding)

        view.configure(1u, region, byteArrayOf(0, 0, 0, 1, 0x41))
        assertTrue("the old decoder was released", first.closed)
        assertEquals(
            "references from the old encoder describe a stream that no longer exists",
            listOf(7u),
            view.decode(1u, 7u, ByteArray(4)).holding,
        )
    }

    @Test
    fun `clearing releases the decoder`() {
        val decoder = Fake()
        val view = MotionView(Fakes(decoder))
        view.configure(1u, region, ByteArray(4))
        view.clear(1u)
        assertTrue(decoder.closed)
        assertFalse(view.isDecoding(1u))
        assertNull(view.decode(1u, 1u, ByteArray(4)).picture)
    }

    @Test
    fun `ending the session releases every decoder`() {
        val decoder = Fake()
        val view = MotionView(Fakes(decoder))
        view.configure(1u, region, ByteArray(4))
        view.configure(2u, region, ByteArray(4))
        view.clearAll()
        assertTrue(decoder.closed)
        assertFalse(view.isDecoding(1u))
        assertFalse(view.isDecoding(2u))
    }

    @Test
    fun `what is held stays inside one acknowledgement message`() {
        val view = MotionView(Fakes(Fake()))
        view.configure(1u, region, ByteArray(4))
        var last: List<UInt>? = null
        for (token in 1u..40u) {
            last = view.decode(1u, token, ByteArray(4)).holding ?: last
        }
        assertEquals(32, last?.size)
        assertEquals(
            "the oldest references fall off, not the newest",
            listOf(40u),
            last?.takeLast(1),
        )
    }
}
