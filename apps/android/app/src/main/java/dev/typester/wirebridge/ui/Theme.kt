package dev.typester.wirebridge.ui

import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.dynamicDarkColorScheme
import androidx.compose.material3.dynamicLightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext

/** Material 3 in the wallpaper's colours (dynamic colour, which every phone from minSdk 33 has), light or dark. */
@Composable
fun WirebridgeTheme(content: @Composable () -> Unit) {
    val context = LocalContext.current
    val scheme = if (isSystemInDarkTheme()) dynamicDarkColorScheme(context) else dynamicLightColorScheme(context)
    MaterialTheme(colorScheme = scheme, content = content)
}

/** A client that has handshaken lately: green whatever the wallpaper, since that is what "connected" looks like. */
val Connected = Color(0xFF34A853)
