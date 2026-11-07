package dev.typester.wirebridge.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.PowerSettingsNew
import androidx.compose.material.icons.outlined.Delete
import androidx.compose.material.icons.outlined.Edit
import androidx.compose.material.icons.outlined.QrCode2
import androidx.compose.material.icons.outlined.Share
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import dev.typester.wirebridge.Client
import dev.typester.wirebridge.Clients
import dev.typester.wirebridge.ConfigServer
import dev.typester.wirebridge.Share
import dev.typester.wirebridge.Spot
import dev.typester.wirebridge.Tunnel
import dev.typester.wirebridge.TunnelService
import uniffi.wirebridge_core.ClientStats

/** One client: how it is doing, and how to set its device up. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun ClientScreen(id: String, onBack: () -> Unit) {
    val context = LocalContext.current
    val server by Tunnel.state.collectAsStateWithLifecycle()
    val spot by Spot.state.collectAsStateWithLifecycle()
    val clients by Clients.list.collectAsStateWithLifecycle()
    val live = rememberLive(server)
    val client = clients.firstOrNull { it.id == id }
    if (client == null) {
        // Deleted, here or elsewhere.
        LaunchedEffect(Unit) { onBack() }
        return
    }
    var renaming by rememberSaveable { mutableStateOf(false) }
    var deleting by rememberSaveable { mutableStateOf(false) }

    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text(client.name) },
                navigationIcon = { BackButton(onBack) },
                actions = {
                    IconButton(onClick = { renaming = true }) { Icon(Icons.Outlined.Edit, contentDescription = "Rename") }
                    IconButton(onClick = { deleting = true }) { Icon(Icons.Outlined.Delete, contentDescription = "Delete") }
                },
            )
        },
    ) { padding ->
        Column(
            Modifier
                .padding(padding)
                .verticalScroll(rememberScrollState())
                .padding(horizontal = 16.dp, vertical = 8.dp),
            verticalArrangement = Arrangement.spacedBy(16.dp),
        ) {
            StateCard(client, live.client(client.publicKey))
            val endpoint = spot.endpoint
            val address = spot.address
            if (server != null && endpoint != null && address != null) {
                SetUpCard(client, endpoint, address)
            } else {
                SectionCard(title = "Set up the device") {
                    Text(
                        "Start Wirebridge first: the device's config names the phone's address on the Wi-Fi, which is " +
                            "only known once the Wi-Fi is up.",
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                    if (server == null) {
                        Button(onClick = { TunnelService.start(context) }) {
                            Icon(Icons.Filled.PowerSettingsNew, contentDescription = null, modifier = Modifier.size(18.dp))
                            Spacer(Modifier.width(8.dp))
                            Text("Start")
                        }
                    }
                }
            }
            Spacer(Modifier.height(8.dp))
        }
    }

    if (renaming) {
        NameDialog(
            title = "Rename",
            confirm = "Save",
            initial = client.name,
            onDismiss = { renaming = false },
            onConfirm = { name ->
                renaming = false
                Clients.rename(context, client.id, name)
            },
        )
    }
    if (deleting) {
        AlertDialog(
            onDismissRequest = { deleting = false },
            title = { Text("Delete ${client.name}?") },
            text = { Text("Its config stops working at once. A new client gets a new key and address.") },
            confirmButton = {
                TextButton(
                    onClick = {
                        deleting = false
                        Clients.delete(context, client.id)
                    },
                ) { Text("Delete", color = MaterialTheme.colorScheme.error) }
            },
            dismissButton = { TextButton(onClick = { deleting = false }) { Text("Cancel") } },
        )
    }
}

@Composable
private fun StateCard(client: Client, stats: ClientStats?) {
    SectionCard(title = "Connection") {
        Row(verticalAlignment = Alignment.CenterVertically) {
            StatusDot(stats.isConnected())
            Spacer(Modifier.width(12.dp))
            Text(stats.describe(), style = MaterialTheme.typography.bodyLarge)
        }
        Column(verticalArrangement = Arrangement.spacedBy(2.dp)) {
            Mono("Address    ${client.address}")
            stats?.secondsSinceHandshake?.let { Mono("Handshake  ${ago(it)}") }
            stats?.endpoint?.let { Mono("From       $it") }
            stats?.let { Mono("Traffic    ${bytes(it.txBytes)} down, ${bytes(it.rxBytes)} up") }
        }
    }
}

/**
 * The three ways a device gets its config: a QR code for the WireGuard app to scan, a link to open on the Wi-Fi, and
 * Android's share sheet. All three carry the client's private key.
 */
@Composable
private fun SetUpCard(client: Client, endpoint: String, address: String) {
    val context = LocalContext.current
    val config = remember(client, endpoint) { Clients.config(context, client, endpoint) }
    var showing by rememberSaveable { mutableStateOf(false) }
    SectionCard(title = "Set up the device") {
        Text(
            "Join the Wi-Fi on the device first (Home shows its code). Then, in the WireGuard app, scan this code: " +
                "+, Scan from QR code. Or open the link on the device and import the file.",
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        if (showing) {
            QrCode(config, Modifier.fillMaxWidth().padding(horizontal = 16.dp))
            Text(
                "The code holds this device's private key.",
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            TextButton(onClick = { showing = false }) { Text("Hide the code") }
        } else {
            FilledTonalButton(onClick = { showing = true }) {
                Icon(Icons.Outlined.QrCode2, contentDescription = null, modifier = Modifier.size(18.dp))
                Spacer(Modifier.width(8.dp))
                Text("Show the QR code")
            }
        }
        CopyLine(
            "Link",
            ConfigServer.url(address, client.token),
            sensitive = true,
            mono = true,
            valueStyle = MaterialTheme.typography.bodyMedium,
        )
        OutlinedButton(onClick = { Share.config(context, client, config) }) {
            Icon(Icons.Outlined.Share, contentDescription = null, modifier = Modifier.size(18.dp))
            Spacer(Modifier.width(8.dp))
            Text("Share the config")
        }
    }
}
