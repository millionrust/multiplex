package com.termirust.mobile.terminal

import kotlin.math.floor
import kotlin.math.max

data class TerminalGrid(
    val columns: Int,
    val rows: Int,
)

fun estimateTerminalGrid(
    widthPx: Int,
    heightPx: Int,
    fontSizeSp: Int,
    density: Float,
): TerminalGrid {
    val fontPx = fontSizeSp.coerceAtLeast(1) * density.coerceAtLeast(0.1f)
    val charWidth = (fontPx * 0.62f).coerceAtLeast(1f)
    val lineHeight = (fontPx * 1.35f).coerceAtLeast(1f)

    return TerminalGrid(
        columns = max(20, floor(widthPx / charWidth).toInt()),
        rows = max(6, floor(heightPx / lineHeight).toInt()),
    )
}
