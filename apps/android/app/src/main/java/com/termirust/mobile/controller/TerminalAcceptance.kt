package com.termirust.mobile.controller

import kotlin.math.floor

data class TerminalAcceptanceLayout(
    val fontSize: Double,
    val displayedFontSize: Double,
    val columns: Int,
    val rows: Int,
    val compactControls: Boolean,
)

data class TerminalBackgroundDecision(
    val releaseWriter: Boolean,
    val coverPrivacy: Boolean,
    val clearPendingInput: Boolean,
    val clearPendingResize: Boolean,
)

object TerminalAcceptance {
    const val MINIMUM_FONT_SIZE = 10.0
    const val MAXIMUM_FONT_SIZE = 32.0
    const val MAXIMUM_ACCESSIBILITY_CHARACTERS = 4_096
    const val MINIMUM_TOUCH_TARGET = 48.0

    fun layout(
        width: Double,
        height: Double,
        requestedFontSize: Double,
        textScale: Double,
    ): TerminalAcceptanceLayout {
        val fontSize = requestedFontSize.coerceIn(MINIMUM_FONT_SIZE, MAXIMUM_FONT_SIZE)
        val scale = textScale.coerceAtLeast(1.0)
        val displayedFontSize = fontSize * scale
        val usableWidth = (width - 24.0).coerceAtLeast(1.0)
        val usableHeight = (height - 24.0).coerceAtLeast(1.0)
        return TerminalAcceptanceLayout(
            fontSize = fontSize,
            displayedFontSize = displayedFontSize,
            columns = floor(usableWidth / (displayedFontSize * 0.62)).toInt().coerceIn(20, 400),
            rows = floor(usableHeight / (displayedFontSize * 1.35)).toInt().coerceIn(5, 200),
            compactControls = width < 600.0 || scale >= 1.6,
        )
    }

    fun accessibleOutput(
        lines: List<String>,
        maximumCharacters: Int = MAXIMUM_ACCESSIBILITY_CHARACTERS,
    ): String {
        if (maximumCharacters <= 0) return ""
        val output = lines.joinToString("\n").trim()
        return if (output.length <= maximumCharacters) output else output.takeLast(maximumCharacters)
    }

    fun backgroundDecision(writerHeld: Boolean) = TerminalBackgroundDecision(
        releaseWriter = writerHeld,
        coverPrivacy = true,
        clearPendingInput = true,
        clearPendingResize = true,
    )
}
