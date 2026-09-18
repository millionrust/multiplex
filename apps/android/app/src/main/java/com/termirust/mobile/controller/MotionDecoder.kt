package com.termirust.mobile.controller

import android.media.MediaCodec
import android.media.MediaCodecInfo
import android.media.MediaFormat
import android.os.Build
import java.nio.ByteBuffer

/**
 * Decoding the motion region on Android.
 *
 * When a part of the computer's screen is moving fast enough to be worth streaming, it arrives as
 * HEVC rather than as tiles. On Apple platforms the Rust library decodes it and the phone never
 * sees a video frame; Android has no decoder there, so the frames come up as
 * `ScreenEvent.VideoConfig` and `ScreenEvent.VideoFrame` and are decoded here with `MediaCodec`.
 *
 * The decoder sits behind [MotionDecoders] so the bookkeeping around it — which references the
 * decoder holds, where the picture goes, what happens when a frame cannot be decoded — is
 * testable without a device.
 */

/** One decoded picture of a motion region, as colours Android can draw. */
class MotionPicture(
    val width: Int,
    val height: Int,
    /** ARGB_8888, row by row, one entry per pixel. */
    val argb: IntArray,
)

/** A live decoder for one region. */
interface MotionDecoder {
    /**
     * Decodes one Annex B frame.
     *
     * Null means nothing came out, which is normal: the codec buffers, and a frame that predicts
     * from one that never arrived produces nothing at all. Neither is an error, and neither
     * should be drawn over the last good picture.
     */
    fun decode(payload: ByteArray): MotionPicture?

    fun close()
}

/** Opens decoders. */
interface MotionDecoders {
    /**
     * Null when this phone cannot decode the stream. The tile path still has the region, so the
     * screen stays correct — it just does not get the cheaper stream.
     */
    fun open(parameterSets: ByteArray, width: Int, height: Int): MotionDecoder?
}

/** Where a decoder cannot be had: every region falls back to tiles. */
object NoMotionDecoders : MotionDecoders {
    override fun open(parameterSets: ByteArray, width: Int, height: Int): MotionDecoder? = null
}

/** `MediaCodec`, which on any recent phone means the hardware HEVC decoder. */
object MediaCodecMotionDecoders : MotionDecoders {
    override fun open(parameterSets: ByteArray, width: Int, height: Int): MotionDecoder? =
        runCatching { MediaCodecMotionDecoder(parameterSets, width, height) }.getOrNull()
}

/**
 * One `MediaCodec` decoding one region.
 *
 * Output is taken as buffers rather than to a `Surface`. A `Surface` would be faster, but the
 * picture has to land in the same bitmap the tile path draws into, and reading pixels back out of
 * a `Surface` costs a `GL` round trip. The motion region is bounded to a fraction of the screen,
 * so converting it here is affordable.
 */
class MediaCodecMotionDecoder(
    parameterSets: ByteArray,
    private val width: Int,
    private val height: Int,
) : MotionDecoder {
    private val codec: MediaCodec = MediaCodec.createDecoderByType(MIME)
    private val info = MediaCodec.BufferInfo()
    private var running = true

    init {
        val format = MediaFormat.createVideoFormat(MIME, width, height)
        // The parameter sets go in as csd-0 in Annex B, which is exactly the form the computer
        // sends them in; that is why the host emits Annex B rather than length prefixes.
        format.setByteBuffer(CSD, ByteBuffer.wrap(parameterSets))
        format.setInteger(
            MediaFormat.KEY_COLOR_FORMAT,
            MediaCodecInfo.CodecCapabilities.COLOR_FormatYUV420Flexible,
        )
        // Low latency where the phone offers it: this is a remote screen, not a film, and a
        // decoder holding frames back to reorder them would add lag with nothing to gain. Android
        // only learned to ask for it in 30, and an older phone simply decodes a little behind.
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            format.setInteger(MediaFormat.KEY_LOW_LATENCY, 1)
        }
        codec.configure(format, null, null, 0)
        codec.start()
    }

    override fun decode(payload: ByteArray): MotionPicture? {
        if (!running) return null
        return runCatching { submit(payload) }.getOrElse {
            // A codec that has thrown is not worth retrying: it stays closed and the region falls
            // back to the tile path.
            close()
            null
        }
    }

    private fun submit(payload: ByteArray): MotionPicture? {
        val input = codec.dequeueInputBuffer(TIMEOUT_MICROS)
        if (input >= 0) {
            val buffer = codec.getInputBuffer(input) ?: return null
            buffer.clear()
            buffer.put(payload)
            codec.queueInputBuffer(input, 0, payload.size, 0, 0)
        }
        var picture: MotionPicture? = null
        while (true) {
            val output = codec.dequeueOutputBuffer(info, TIMEOUT_MICROS)
            if (output < 0) break
            // The newest picture wins: the phone draws the present, not a backlog.
            codec.getOutputImage(output)?.let { image ->
                picture = runCatching { image.toArgb(width, height) }.getOrNull() ?: picture
            }
            codec.releaseOutputBuffer(output, false)
        }
        return picture
    }

    override fun close() {
        if (!running) return
        running = false
        runCatching { codec.stop() }
        runCatching { codec.release() }
    }

    private companion object {
        const val MIME = MediaFormat.MIMETYPE_VIDEO_HEVC
        const val CSD = "csd-0"

        /** Short enough not to stall the frame loop, long enough for the codec to answer. */
        const val TIMEOUT_MICROS = 2_000L
    }
}

/**
 * Converts one YUV 4:2:0 image to ARGB.
 *
 * BT.709, limited range, which is what the computer's encoder produces for a region this size.
 * Getting the matrix wrong shifts the colours rather than breaking the picture, and the motion
 * region never contains text — the tile path takes that back before it could matter.
 */
internal fun android.media.Image.toArgb(width: Int, height: Int): MotionPicture {
    val w = minOf(width, this.width)
    val h = minOf(height, this.height)
    val y = planes[0]
    val u = planes[1]
    val v = planes[2]
    val argb = IntArray(w * h)
    val yBuffer = y.buffer
    val uBuffer = u.buffer
    val vBuffer = v.buffer
    for (row in 0 until h) {
        val yRow = row * y.rowStride
        val chromaRow = (row / 2) * u.rowStride
        for (column in 0 until w) {
            val luma = (yBuffer.get(yRow + column * y.pixelStride).toInt() and 0xFF) - 16
            val chromaAt = chromaRow + (column / 2) * u.pixelStride
            val cb = (uBuffer.get(chromaAt).toInt() and 0xFF) - 128
            val cr = (vBuffer.get(chromaAt).toInt() and 0xFF) - 128
            val scaled = 1192 * luma
            val red = (scaled + 1836 * cr) shr 10
            val green = (scaled - 218 * cb - 546 * cr) shr 10
            val blue = (scaled + 2163 * cb) shr 10
            argb[row * w + column] = (0xFF shl 24) or
                (red.clampToByte() shl 16) or
                (green.clampToByte() shl 8) or
                blue.clampToByte()
        }
    }
    return MotionPicture(w, h, argb)
}

private fun Int.clampToByte(): Int = if (this < 0) 0 else if (this > 255) 255 else this
