package com.termirust.mobile.controller

import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json

internal object NativeControllerTerminal {
    val loaded = runCatching { System.loadLibrary("termirust_mobile_ffi") }.isSuccess

    @JvmStatic external fun create(columns: Int, rows: Int, scrollbackRows: Int): Long
    @JvmStatic external fun process(handle: Long, input: ByteArray): ByteArray
    @JvmStatic external fun feed(handle: Long, input: ByteArray): Boolean
    @JvmStatic external fun resize(handle: Long, columns: Int, rows: Int): ByteArray
    @JvmStatic external fun snapshot(handle: Long): ByteArray
    @JvmStatic external fun destroy(handle: Long)
}

internal class NativeControllerTerminalSession private constructor(
    private var handle: Long,
) : AutoCloseable {
    fun process(bytes: ByteArray) = decode(NativeControllerTerminal.process(handle, bytes))

    fun feed(bytes: ByteArray) {
        check(handle != 0L) { "native terminal is closed" }
        check(NativeControllerTerminal.feed(handle, bytes)) { "native terminal rejected the frame" }
    }

    fun resize(viewport: TerminalViewport) = decode(
        NativeControllerTerminal.resize(handle, viewport.columns, viewport.rows),
    )

    fun snapshot() = decode(NativeControllerTerminal.snapshot(handle))

    override fun close() {
        if (handle == 0L) return
        NativeControllerTerminal.destroy(handle)
        handle = 0
    }

    private fun decode(bytes: ByteArray): BoundedTerminalSnapshot {
        check(handle != 0L) { "native terminal is closed" }
        return JSON.decodeFromString<NativeTerminalSnapshot>(bytes.decodeToString()).toModel()
    }

    companion object {
        private val JSON = Json { ignoreUnknownKeys = false }

        fun openOrNull(
            viewport: TerminalViewport,
            limits: TerminalLimits,
        ): NativeControllerTerminalSession? {
            if (!NativeControllerTerminal.loaded) return null
            val cellsPerRow = viewport.columns.coerceAtLeast(1)
            val cellBound = (limits.maxRetainedCells / cellsPerRow - viewport.rows).coerceAtLeast(0)
            val byteBound = (limits.maxModelBytes / (cellsPerRow * 64).coerceAtLeast(1) - viewport.rows)
                .coerceAtLeast(0)
            val scrollback = minOf(limits.maxScrollbackRows, cellBound, byteBound)
            return runCatching {
                NativeControllerTerminal.create(viewport.columns, viewport.rows, scrollback)
            }.getOrNull()?.takeIf { it != 0L }?.let(::NativeControllerTerminalSession)
        }
    }
}

@Serializable
private data class NativeTerminalSnapshot(
    @SerialName("schema_version") val schemaVersion: Int,
    val columns: Int,
    val rows: Int,
    val lines: List<String>,
    val cells: List<List<NativeTerminalCell>>,
    @SerialName("cursor_row") val cursorRow: Int,
    @SerialName("cursor_column") val cursorColumn: Int,
    @SerialName("cursor_visible") val cursorVisible: Boolean,
    @SerialName("application_cursor") val applicationCursor: Boolean,
    @SerialName("application_keypad") val applicationKeypad: Boolean,
    @SerialName("alternate_screen") val alternateScreen: Boolean,
    @SerialName("bracketed_paste") val bracketedPaste: Boolean,
    @SerialName("mouse_mode") val mouseMode: String,
    @SerialName("mouse_encoding") val mouseEncoding: String,
    @SerialName("scrollback_rows") val scrollbackRows: Int,
    @SerialName("retained_cells") val retainedCells: Int,
    @SerialName("accounted_bytes") val accountedBytes: Int,
) {
    fun toModel(): BoundedTerminalSnapshot {
        check(schemaVersion == 1) { "unsupported native terminal snapshot schema" }
        check(columns > 0 && rows > 0 && cells.size >= rows)
        val content = cells.map { row -> row.map(NativeTerminalCell::toModel) }
        check(content.all { it.size <= columns })
        val padded = content.map { row ->
            row + List(columns - row.size) { BoundedTerminalCell.blank() }
        }
        return BoundedTerminalSnapshot(
            lines = lines,
            cells = padded,
            contentCells = content,
            cursorRow = cursorRow,
            cursorColumn = cursorColumn,
            retainedCells = retainedCells,
            accountedBytes = accountedBytes,
            truncation = null,
            cursorVisible = cursorVisible,
            applicationCursor = applicationCursor,
            applicationKeypad = applicationKeypad,
            alternateScreen = alternateScreen,
            bracketedPaste = bracketedPaste,
            mouseMode = TerminalMouseMode.entries.first { it.wireName == mouseMode },
            mouseEncoding = TerminalMouseEncoding.entries.first { it.wireName == mouseEncoding },
            scrollbackRows = scrollbackRows,
        )
    }
}

@Serializable
private data class NativeTerminalCell(
    val text: String,
    val width: Int,
    val foreground: NativeTerminalColor,
    val background: NativeTerminalColor,
    val bold: Boolean,
    val dim: Boolean,
    val italic: Boolean,
    val underline: Boolean,
    val inverse: Boolean,
) {
    fun toModel() = BoundedTerminalCell(
        text = text,
        width = TerminalCellWidth.entries.first { it.columns == width },
        style = TerminalCellStyle(
            foreground = foreground.toModel(),
            background = background.toModel(),
            bold = bold,
            dim = dim,
            italic = italic,
            underline = underline,
            inverse = inverse,
        ),
    )
}

@Serializable
private data class NativeTerminalColor(
    val kind: String,
    val value: Int? = null,
    val red: Int? = null,
    val green: Int? = null,
    val blue: Int? = null,
) {
    fun toModel(): TerminalCellColor = when (kind) {
        "default" -> TerminalCellColor.Default
        "indexed" -> TerminalCellColor.Indexed(checkNotNull(value))
        "rgb" -> TerminalCellColor.Rgb(checkNotNull(red), checkNotNull(green), checkNotNull(blue))
        else -> error("unsupported native terminal color")
    }
}

private fun BoundedTerminalCell.isDefaultBlank() =
    text == " " && width == TerminalCellWidth.NARROW && style == TerminalCellStyle()
