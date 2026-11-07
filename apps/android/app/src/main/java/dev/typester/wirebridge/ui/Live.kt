package dev.typester.wirebridge.ui

import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.lifecycle.repeatOnLifecycle
import java.net.Inet4Address
import java.net.NetworkInterface
import kotlinx.coroutines.delay
import uniffi.wirebridge_core.ClientStats
import uniffi.wirebridge_core.Server
import uniffi.wirebridge_core.ServerStats

/** What the screens show of a running tunnel, read once a second. */
data class Live(
    val stats: ServerStats? = null,
    /** Bytes per second to the clients, and from them, since the previous read. */
    val toClients: Double = 0.0,
    val fromClients: Double = 0.0,
    /** Every IPv4 address on an interface that is up, as "name  address". */
    val addresses: List<String> = emptyList(),
) {
    fun client(publicKey: String): ClientStats? = stats?.clients?.firstOrNull { it.publicKey == publicKey }
}

/**
 * [server]'s counters, read once a second while the screen is started. With the screen off there is nobody to show
 * them to, and a loop left running in the background is what read a destroyed server and crashed the app on
 * 2026-11-05. A read that fails (a server destroyed by some other path) is skipped rather than allowed to take the
 * screen down with it.
 */
@Composable
fun rememberLive(server: Server?): Live {
    var live by remember { mutableStateOf(Live()) }
    val lifecycle = LocalLifecycleOwner.current.lifecycle
    LaunchedEffect(server, lifecycle) {
        lifecycle.repeatOnLifecycle(Lifecycle.State.STARTED) {
            var previous: ServerStats? = null
            var previousAt = 0L
            while (true) {
                val current = try {
                    server?.stats()
                } catch (e: IllegalStateException) {
                    null
                }
                val now = System.nanoTime()
                live = if (current != null && previous != null) {
                    val seconds = (now - previousAt) / 1e9
                    Live(
                        current,
                        toClients = (current.txBytes - previous.txBytes).toDouble() / seconds,
                        fromClients = (current.rxBytes - previous.rxBytes).toDouble() / seconds,
                        addresses = interfaceAddresses(),
                    )
                } else {
                    Live(current, addresses = interfaceAddresses())
                }
                previous = current
                previousAt = now
                delay(1_000)
            }
        }
    }
    return live
}

private fun interfaceAddresses(): List<String> =
    try {
        NetworkInterface.getNetworkInterfaces().toList()
            .filter { it.isUp && !it.isLoopback }
            .flatMap { nic ->
                nic.inetAddresses.toList().filterIsInstance<Inet4Address>().map { "${nic.name}  ${it.hostAddress}" }
            }
            .sorted()
    } catch (e: Exception) {
        emptyList()
    }
