package dev.typester.wirebridge.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.Autorenew
import androidx.compose.material3.Button
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import dev.typester.wirebridge.Direct
import dev.typester.wirebridge.Tunnel
import dev.typester.wirebridge.TunnelService

/** The Wi-Fi's name and passphrase. Saving while the tunnel runs makes the Wi-Fi again under the new ones. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun WifiScreen(onBack: () -> Unit) {
    val context = LocalContext.current
    val server by Tunnel.state.collectAsStateWithLifecycle()
    val savedSuffix = Direct.nameSuffix(context)
    val savedPassphrase = Direct.passphrase(context)
    var suffix by rememberSaveable { mutableStateOf(savedSuffix) }
    var passphrase by rememberSaveable { mutableStateOf(savedPassphrase) }
    val suffixError = Direct.checkNameSuffix(suffix)
    val passphraseError = Direct.checkPassphrase(passphrase)
    val changed = suffix != savedSuffix || passphrase != savedPassphrase

    Scaffold(topBar = { TopAppBar(title = { Text("Wi-Fi") }, navigationIcon = { BackButton(onBack) }) }) { padding ->
        Column(
            Modifier
                .padding(padding)
                .verticalScroll(rememberScrollState())
                .padding(16.dp),
            verticalArrangement = Arrangement.spacedBy(16.dp),
        ) {
            OutlinedTextField(
                value = suffix,
                onValueChange = { suffix = it.trim() },
                label = { Text("Network name") },
                prefix = { Text(Direct.NAME_PREFIX) },
                isError = suffixError != null,
                supportingText = {
                    Text(suffixError ?: "Up to ${Direct.MAX_NAME_SUFFIX} letters, digits, - and _. Android wants the start.")
                },
                singleLine = true,
                keyboardOptions = KeyboardOptions(capitalization = KeyboardCapitalization.None, autoCorrectEnabled = false),
                modifier = Modifier.fillMaxWidth(),
            )
            OutlinedTextField(
                value = passphrase,
                onValueChange = { passphrase = it },
                label = { Text("Passphrase") },
                isError = passphraseError != null,
                supportingText = { Text(passphraseError ?: "${Direct.MIN_PASSPHRASE} to ${Direct.MAX_PASSPHRASE} characters.") },
                singleLine = true,
                textStyle = MaterialTheme.typography.bodyLarge.copy(fontFamily = FontFamily.Monospace),
                keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password, autoCorrectEnabled = false),
                modifier = Modifier.fillMaxWidth(),
            )
            FilledTonalButton(onClick = { passphrase = Direct.generatePassphrase() }) {
                Icon(Icons.Outlined.Autorenew, contentDescription = null, modifier = Modifier.size(18.dp))
                Spacer(Modifier.width(8.dp))
                Text("Generate a passphrase")
            }
            Text(
                buildString {
                    append("Every device that joined before has to join again under the new name or passphrase.")
                    if (server != null) append(" Wirebridge is running: saving makes the Wi-Fi again now, and devices on it drop off for a moment.")
                },
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            Button(
                onClick = {
                    Direct.save(context, suffix, passphrase)
                    TunnelService.restartLink(context)
                    onBack()
                },
                enabled = changed && suffixError == null && passphraseError == null,
                modifier = Modifier.fillMaxWidth(),
            ) { Text("Save") }
        }
    }
}
