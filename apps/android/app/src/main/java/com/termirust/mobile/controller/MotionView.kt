package com.termirust.mobile.controller

import com.termirust.screens.ScreenRect

/**
 * What decoding a frame produced, and what the computer should be told about it.
 *
 * This mirrors the Rust `MotionView` that Apple platforms use, because both sides of the
 * acknowledgement loop have to behave the same: a reference is named only once the frame carrying
 * it has really decoded. The computer predicts from what this reports, so claiming a reference
 * this phone does not hold produces a stream it cannot decode and no way to notice.
 */
class MotionDecoded(
    /** Pixels to draw, and where on the surface they go. */
    val picture: MotionPicture? = null,
    val rect: ScreenRect? = null,
    /** References the decoder now holds, when that changed. Null means nothing to report. */
    val holding: List<UInt>? = null,
)

/** Decoders for the motion regions this phone is watching. */
class MotionView(private val decoders: MotionDecoders = MediaCodecMotionDecoders) {
    private class Region(
        val rect: ScreenRect,
        val decoder: MotionDecoder?,
        val held: MutableList<UInt> = mutableListOf(),
    )

    private val regions = mutableMapOf<UInt, Region>()

    /**
     * Opens a decoder for a surface's motion region, replacing any decoder already there.
     *
     * A new configuration is a new encoder on the computer, so the old decoder's references
     * describe a stream that no longer exists and must not be acknowledged again.
     */
    fun configure(surface: UInt, rect: ScreenRect, parameterSets: ByteArray) {
        regions.remove(surface)?.decoder?.close()
        val decoder = decoders.open(parameterSets, rect.width.toInt(), rect.height.toInt())
        regions[surface] = Region(rect, decoder)
    }

    /** Decodes one frame into the region it belongs to. */
    fun decode(surface: UInt, token: UInt?, payload: ByteArray): MotionDecoded {
        val region = regions[surface] ?: return MotionDecoded()
        val decoder = region.decoder ?: return MotionDecoded()
        val picture = decoder.decode(payload) ?: return MotionDecoded()
        // Only now, having actually decoded it, may this reference be acknowledged.
        val holding = token?.takeIf { !region.held.contains(it) }?.let {
            region.held.add(it)
            while (region.held.size > MAX_TOKENS) region.held.removeAt(0)
            region.held.toList()
        }
        return MotionDecoded(picture = picture, rect = region.rect, holding = holding)
    }

    /** Whether this phone is decoding a surface's region, rather than waiting for tiles. */
    fun isDecoding(surface: UInt): Boolean = regions[surface]?.decoder != null

    /** Forgets a surface's region, because the computer demoted it. */
    fun clear(surface: UInt) {
        regions.remove(surface)?.decoder?.close()
    }

    /** Forgets everything, because the session ended. */
    fun clearAll() {
        for (region in regions.values) region.decoder?.close()
        regions.clear()
    }

    private companion object {
        /** What one acknowledgement message carries, matching `MAX_VIDEO_TOKENS` in the protocol. */
        const val MAX_TOKENS = 32
    }
}
