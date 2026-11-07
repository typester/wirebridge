package dev.typester.wirebridge.ui

import android.content.ClipData
import android.content.ClipDescription
import android.os.PersistableBundle
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.outlined.ContentCopy
import androidx.compose.material3.ElevatedCard
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.ClipEntry
import androidx.compose.ui.platform.LocalClipboard
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.launch

/** A filled dot when [on], a ring when not. */
@Composable
fun StatusDot(on: Boolean, modifier: Modifier = Modifier) {
    val shape = CircleShape
    Box(
        modifier
            .size(10.dp)
            .clip(shape)
            .then(
                if (on) {
                    Modifier.background(Connected)
                } else {
                    Modifier.border(1.5.dp, MaterialTheme.colorScheme.outline, shape)
                },
            ),
    )
}

/** A card with a title, an optional action at its right, and its content below. */
@Composable
fun SectionCard(
    title: String,
    modifier: Modifier = Modifier,
    action: (@Composable () -> Unit)? = null,
    content: @Composable ColumnScope.() -> Unit,
) {
    ElevatedCard(modifier.fillMaxWidth()) {
        Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(title, style = MaterialTheme.typography.titleMedium, modifier = Modifier.weight(1f))
                action?.invoke()
            }
            content()
        }
    }
}

/**
 * A labelled value with a button that copies it. A [sensitive] value is marked so, and Android keeps it out of the
 * clipboard's preview.
 */
@Composable
fun CopyLine(
    label: String,
    value: String,
    modifier: Modifier = Modifier,
    sensitive: Boolean = false,
    mono: Boolean = false,
    valueStyle: TextStyle = MaterialTheme.typography.bodyLarge,
) {
    val clipboard = LocalClipboard.current
    val scope = rememberCoroutineScope()
    Row(modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
        Column(Modifier.weight(1f)) {
            Text(label, style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
            Text(value, style = valueStyle, fontFamily = if (mono) FontFamily.Monospace else null)
        }
        Spacer(Modifier.width(8.dp))
        IconButton(
            onClick = {
                val clip = ClipData.newPlainText(label, value)
                if (sensitive) {
                    clip.description.extras = PersistableBundle().apply {
                        putBoolean(ClipDescription.EXTRA_IS_SENSITIVE, true)
                    }
                }
                scope.launch { clipboard.setClipEntry(ClipEntry(clip)) }
            },
        ) {
            Icon(Icons.Outlined.ContentCopy, contentDescription = "Copy $label")
        }
    }
}

/** A line of small monospaced text, for the numbers in Details. */
@Composable
fun Mono(text: String) {
    Text(text, fontFamily = FontFamily.Monospace, style = MaterialTheme.typography.bodySmall)
}

/** The back arrow of every screen but Home. */
@Composable
fun BackButton(onBack: () -> Unit) {
    IconButton(onClick = onBack) {
        Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = "Back")
    }
}
