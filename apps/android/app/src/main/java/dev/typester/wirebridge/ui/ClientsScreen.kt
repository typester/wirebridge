package dev.typester.wirebridge.ui

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.KeyboardArrowRight
import androidx.compose.material.icons.filled.Add
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ExtendedFloatingActionButton
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.ListItem
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
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
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import dev.typester.wirebridge.Clients
import dev.typester.wirebridge.Tunnel

/** Every client, with how it is doing; a new one is made here. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun ClientsScreen(onBack: () -> Unit, onClient: (String) -> Unit) {
    val context = LocalContext.current
    val server by Tunnel.state.collectAsStateWithLifecycle()
    val clients by Clients.list.collectAsStateWithLifecycle()
    val live = rememberLive(server)
    var adding by rememberSaveable { mutableStateOf(false) }
    Scaffold(
        topBar = { TopAppBar(title = { Text("Clients") }, navigationIcon = { BackButton(onBack) }) },
        floatingActionButton = {
            ExtendedFloatingActionButton(
                onClick = { adding = true },
                icon = { Icon(Icons.Filled.Add, contentDescription = null) },
                text = { Text("Add client") },
            )
        },
    ) { padding ->
        if (clients.isEmpty()) {
            Box(Modifier.padding(padding).fillMaxSize().padding(32.dp), contentAlignment = Alignment.Center) {
                Column(horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    Text("No clients yet", style = MaterialTheme.typography.titleMedium)
                    Text(
                        "Add one for each device: a Mac, a tablet. Each gets its own key and its own address in the " +
                            "tunnel, and they can all be connected at once.",
                        textAlign = TextAlign.Center,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
            }
        } else {
            LazyColumn(contentPadding = PaddingValues(top = padding.calculateTopPadding(), bottom = 96.dp)) {
                items(clients, key = { it.id }) { client ->
                    val stats = live.client(client.publicKey)
                    ListItem(
                        headlineContent = { Text(client.name) },
                        supportingContent = {
                            val traffic = stats?.let { " · ${bytes(it.txBytes)} down, ${bytes(it.rxBytes)} up" } ?: ""
                            Text("${client.address} · ${stats.describe()}$traffic")
                        },
                        leadingContent = { StatusDot(stats.isConnected()) },
                        trailingContent = { Icon(Icons.AutoMirrored.Filled.KeyboardArrowRight, contentDescription = null) },
                        modifier = Modifier.clickable { onClient(client.id) },
                    )
                    HorizontalDivider()
                }
            }
        }
    }
    if (adding) {
        NameDialog(
            title = "New client",
            confirm = "Create",
            initial = "",
            onDismiss = { adding = false },
            onConfirm = { name ->
                adding = false
                onClient(Clients.create(context, name).id)
            },
        )
    }
}

/** Asks for a client's name: for a new client, or to rename one. */
@Composable
fun NameDialog(title: String, confirm: String, initial: String, onDismiss: () -> Unit, onConfirm: (String) -> Unit) {
    var name by rememberSaveable { mutableStateOf(initial) }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(title) },
        text = {
            OutlinedTextField(
                value = name,
                onValueChange = { name = it },
                label = { Text("Name") },
                placeholder = { Text("Mac, Pixel Tablet, …") },
                singleLine = true,
            )
        },
        confirmButton = { TextButton(onClick = { onConfirm(name) }, enabled = name.isNotBlank()) { Text(confirm) } },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } },
    )
}
