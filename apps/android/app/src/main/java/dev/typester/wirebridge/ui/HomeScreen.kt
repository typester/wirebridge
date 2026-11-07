package dev.typester.wirebridge.ui

import androidx.compose.foundation.clickable
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
import androidx.compose.material.icons.automirrored.filled.KeyboardArrowRight
import androidx.compose.material.icons.filled.PowerSettingsNew
import androidx.compose.material.icons.outlined.ExpandLess
import androidx.compose.material.icons.outlined.ExpandMore
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
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import dev.typester.wirebridge.Client
import dev.typester.wirebridge.Clients
import dev.typester.wirebridge.Direct
import dev.typester.wirebridge.Share
import dev.typester.wirebridge.Spot
import dev.typester.wirebridge.Tunnel
import dev.typester.wirebridge.TunnelService

/** What the app opens on: whether the tunnel runs and how busy it is, the Wi-Fi to join, and the clients. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun HomeScreen(onWifi: () -> Unit, onClients: () -> Unit, onClient: (String) -> Unit) {
    val server by Tunnel.state.collectAsStateWithLifecycle()
    val spot by Spot.state.collectAsStateWithLifecycle()
    val clients by Clients.list.collectAsStateWithLifecycle()
    val live = rememberLive(server)
    Scaffold(topBar = { TopAppBar(title = { Text("Wirebridge") }) }) { padding ->
        Column(
            Modifier
                .padding(padding)
                .verticalScroll(rememberScrollState())
                .padding(horizontal = 16.dp, vertical = 8.dp),
            verticalArrangement = Arrangement.spacedBy(16.dp),
        ) {
            StatusCard(running = server != null, spot = spot, live = live, clients = clients)
            WifiCard(running = server != null, spot = spot, onEdit = onWifi)
            ClientsCard(clients = clients, live = live, onManage = onClients, onClient = onClient)
            DetailsCard(live = live)
            Spacer(Modifier.height(8.dp))
        }
    }
}

@Composable
private fun StatusCard(running: Boolean, spot: Spot.State, live: Live, clients: List<Client>) {
    val context = LocalContext.current
    val error by Tunnel.error.collectAsStateWithLifecycle()
    SectionCard(title = if (running) "Running" else "Stopped") {
        if (running) {
            val connected = clients.count { live.client(it.publicKey).isConnected() }
            Text(
                when {
                    clients.isEmpty() -> "No clients yet"
                    else -> "$connected of ${clients.size} ${if (clients.size == 1) "client" else "clients"} connected"
                },
                style = MaterialTheme.typography.bodyLarge,
            )
            Row(horizontalArrangement = Arrangement.spacedBy(24.dp)) {
                Rate("To clients", live.toClients)
                Rate("From clients", live.fromClients)
            }
            when {
                spot.error != null -> Text("Wi-Fi: ${spot.error}", color = MaterialTheme.colorScheme.error)
                spot.network == null -> Text("Bringing up the Wi-Fi…", color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
        } else {
            Text(
                "Start brings up the Wi-Fi your devices join, and the tunnel they use through it.",
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
        error?.let { Text(it, color = MaterialTheme.colorScheme.error) }
        if (running) {
            OutlinedButton(onClick = { TunnelService.stop(context) }, modifier = Modifier.fillMaxWidth()) {
                Icon(Icons.Filled.PowerSettingsNew, contentDescription = null, modifier = Modifier.size(18.dp))
                Spacer(Modifier.width(8.dp))
                Text("Stop")
            }
        } else {
            Button(onClick = { TunnelService.start(context) }, modifier = Modifier.fillMaxWidth()) {
                Icon(Icons.Filled.PowerSettingsNew, contentDescription = null, modifier = Modifier.size(18.dp))
                Spacer(Modifier.width(8.dp))
                Text("Start")
            }
        }
    }
}

@Composable
private fun Rate(label: String, bytesPerSecond: Double) {
    Column {
        Text(label, style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
        Text(rate(bytesPerSecond), style = MaterialTheme.typography.titleMedium)
    }
}

/** The Wi-Fi as a QR code to scan, and as a name and passphrase to type. */
@Composable
private fun WifiCard(running: Boolean, spot: Spot.State, onEdit: () -> Unit) {
    val context = LocalContext.current
    // The group's own once it is up; the settings it will be made from before that.
    val network = spot.network ?: Direct.name(context)
    val passphrase = spot.passphrase ?: Direct.passphrase(context)
    SectionCard(title = "Wi-Fi", action = { TextButton(onClick = onEdit) { Text("Edit") } }) {
        // The code above, the name and passphrase below it at full width: a name can be 32 characters long.
        QrCode(
            Share.wifiQrText(network, passphrase),
            Modifier
                .size(176.dp)
                .align(Alignment.CenterHorizontally),
        )
        CopyLine("Network", network)
        CopyLine("Passphrase", passphrase, sensitive = true, mono = true)
        Text(
            if (running && spot.network != null) {
                "Scan the code with a phone's or tablet's camera to join; type it in on a Mac."
            } else {
                "The Wi-Fi is up while Wirebridge runs."
            },
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
    }
}

@Composable
private fun ClientsCard(clients: List<Client>, live: Live, onManage: () -> Unit, onClient: (String) -> Unit) {
    SectionCard(title = "Clients", action = { TextButton(onClick = onManage) { Text("Manage") } }) {
        if (clients.isEmpty()) {
            Text(
                "No clients yet. Each device gets its own key and address in the tunnel.",
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            FilledTonalButton(onClick = onManage) { Text("Add a client") }
        } else {
            Column {
                clients.forEach { client ->
                    val stats = live.client(client.publicKey)
                    Row(
                        Modifier
                            .fillMaxWidth()
                            .clickable { onClient(client.id) }
                            .padding(vertical = 8.dp),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        StatusDot(stats.isConnected())
                        Spacer(Modifier.width(12.dp))
                        Column(Modifier.weight(1f)) {
                            Text(client.name, style = MaterialTheme.typography.bodyLarge)
                            Text(
                                stats.describe(),
                                style = MaterialTheme.typography.bodySmall,
                                color = MaterialTheme.colorScheme.onSurfaceVariant,
                            )
                        }
                        Icon(
                            Icons.AutoMirrored.Filled.KeyboardArrowRight,
                            contentDescription = null,
                            tint = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                }
            }
        }
    }
}

/** The numbers for when something goes wrong, folded away until asked for. */
@Composable
private fun DetailsCard(live: Live) {
    var open by rememberSaveable { mutableStateOf(false) }
    SectionCard(
        title = "Details",
        action = {
            IconButton(onClick = { open = !open }) {
                Icon(
                    if (open) Icons.Outlined.ExpandLess else Icons.Outlined.ExpandMore,
                    contentDescription = if (open) "Hide details" else "Show details",
                )
            }
        },
    ) {
        if (!open) return@SectionCard
        Column(verticalArrangement = Arrangement.spacedBy(2.dp)) {
            live.stats?.let { s ->
                Mono("TCP  ${s.tcpActive} open, ${s.tcpTotal} total, ${s.tcpFailed} failed")
                Mono("UDP  ${s.udpActive} open, ${s.udpTotal} total, ${s.udpFailed} failed")
                Mono("Traffic   ${bytes(s.txBytes)} to clients, ${bytes(s.rxBytes)} from them")
                Mono("Not forwarded  ${s.otherDropped} (ICMP and others)")
                Mono("Lost inside    ${s.queueDropped}")
                Mono("Rejected       ${s.wgErrors} WireGuard datagrams")
                if (s.wgErrorKinds.isNotEmpty()) Mono("               ${s.wgErrorKinds}")
                Mono("Unknown        ${s.unknownPeer} from no client")
                Mono("Wrong source   ${s.wrongSource}")
                Mono("UDP port       ${s.listenPort}")
            } ?: Mono("Stopped")
            Spacer(Modifier.height(6.dp))
            live.addresses.forEach { Mono(it) }
            if (live.addresses.isEmpty()) Mono("No IPv4 address on any interface")
        }
    }
}
