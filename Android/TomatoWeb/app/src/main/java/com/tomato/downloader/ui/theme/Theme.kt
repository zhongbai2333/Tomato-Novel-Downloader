package com.tomato.downloader.ui.theme

import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.graphics.Color

private val DarkColors = darkColorScheme(
    primary = Green,
    onPrimary = Color(0xFF003314),
    primaryContainer = Color(0xFF14532D),
    onPrimaryContainer = Color(0xFFBBF7D0),
    secondary = Blue,
    onSecondary = Color(0xFF002B5D),
    background = TerminalBg,
    onBackground = TextPrimary,
    surface = TerminalBgElevated,
    onSurface = TextPrimary,
    surfaceVariant = TerminalSurface,
    onSurfaceVariant = TextSecondary,
    outline = TerminalBorder,
    error = Red,
    onError = Color(0xFF410000)
)

@Composable
fun TomatoWebTheme(content: @Composable () -> Unit) {
    MaterialTheme(colorScheme = DarkColors, content = content)
}
