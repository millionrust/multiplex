package com.multiplex.mobile.ui

import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.material3.ColorScheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.graphics.Color

/**
 * The Material colours this app draws with, taken from the Slate tokens.
 *
 * Both halves of the app use this. Connections had a scheme of its own that set three roles and
 * left the rest to Material's defaults, and Devices set none at all, so a phone showed Slate's
 * greys beside Material's stock purple depending on which tab was open. The roles named here are
 * the ones the two halves actually draw with; anything not named still falls back to Material,
 * which is fine for surfaces nothing uses.
 */
@Composable
fun MultiplexMaterialTheme(
    dark: Boolean = isSystemInDarkTheme(),
    content: @Composable () -> Unit,
) {
    MaterialTheme(colorScheme = slateColorScheme(dark), content = content)
}

/** The scheme on its own, for a preview or a test that wants it without the theme wrapper. */
fun slateColorScheme(dark: Boolean): ColorScheme {
    val theme = if (dark) SlateTheme.Dark else SlateTheme.Light
    fun token(value: Long) = Color(value)
    val accent = token(SlateTokens.colorActionPrimary(theme))
    val onAccent = token(SlateTokens.colorActionPrimaryText(theme))
    val canvas = token(SlateTokens.colorBgCanvas(theme))
    val elevated = token(SlateTokens.colorBgElevated(theme))
    val surface = token(SlateTokens.colorBgSurface(theme))
    val selected = token(SlateTokens.colorBgSelected(theme))
    val primaryText = token(SlateTokens.colorTextPrimary(theme))
    val mutedText = token(SlateTokens.colorTextMuted(theme))
    val error = token(SlateTokens.colorStatusError(theme))
    val outline = token(SlateTokens.colorBorderDefault(theme))
    val base = if (dark) darkColorScheme() else lightColorScheme()
    return base.copy(
        primary = accent,
        onPrimary = onAccent,
        primaryContainer = selected,
        onPrimaryContainer = primaryText,
        secondary = accent,
        onSecondary = onAccent,
        secondaryContainer = selected,
        onSecondaryContainer = primaryText,
        tertiary = accent,
        onTertiary = onAccent,
        background = canvas,
        onBackground = primaryText,
        surface = elevated,
        onSurface = primaryText,
        surfaceVariant = surface,
        onSurfaceVariant = mutedText,
        surfaceContainer = surface,
        surfaceContainerHigh = elevated,
        surfaceContainerHighest = elevated,
        surfaceContainerLow = canvas,
        surfaceContainerLowest = canvas,
        error = error,
        onError = onAccent,
        outline = outline,
        outlineVariant = token(SlateTokens.colorBorderSubtle(theme)),
    )
}
