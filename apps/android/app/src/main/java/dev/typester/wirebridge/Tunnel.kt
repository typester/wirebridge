package dev.typester.wirebridge

import android.content.Context
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import uniffi.wirebridge_core.CoreException
import uniffi.wirebridge_core.Server
import uniffi.wirebridge_core.ServerConfig
import uniffi.wirebridge_core.initLogging

/**
 * The one tunnel endpoint this process runs. [TunnelService] starts and stops it; the screen only reads it, so it
 * survives the activity being recreated. Its key is this phone's ([Keys]); its clients are [Clients], and a change to
 * them reaches it while it runs.
 */
object Tunnel {
    /** The UDP port the phone listens on, which every client's config names (`wirebridge_core::defaults::PORT`). */
    const val PORT: UShort = 51820u

    /** A client counts as connected for this long after its last handshake: WireGuard's own limit on a session. */
    const val CONNECTED_SECONDS = 180uL

    /** The running endpoint, or null. */
    private val running = MutableStateFlow<Server?>(null)
    val state: StateFlow<Server?> = running.asStateFlow()

    private val lastError = MutableStateFlow<String?>(null)
    val error: StateFlow<String?> = lastError.asStateFlow()

    /**
     * Starts the endpoint unless it is running already. Returns whether it is running afterwards. Waits for a [stop]
     * still in progress, up to about three seconds, so this must not run on the main thread.
     *
     * Each client's last address is handed over, so that the new endpoint starts the handshake itself rather than
     * waiting the fifteen or so seconds a client takes to notice.
     */
    @Synchronized
    fun start(context: Context): Boolean {
        if (running.value != null) return true
        initLogging()
        Clients.load(context)
        return try {
            running.value = Server.start(
                ServerConfig(
                    privateKey = Keys.phonePrivateKey(context),
                    listenPort = PORT,
                    clients = Clients.configs(),
                ),
            )
            lastError.value = null
            true
        } catch (e: CoreException) {
            lastError.value = describe(e)
            false
        }
    }

    /** Hands the clients, as they are now, to the running endpoint. Does nothing while it is stopped. */
    fun applyClients() {
        val server = running.value ?: return
        try {
            server.setClients(Clients.configs())
        } catch (e: CoreException) {
            lastError.value = describe(e)
        }
    }

    /**
     * Stops the endpoint and waits until its port is free. Blocks for up to about three seconds.
     *
     * Stops without `close()`. A stopped server is still safe to read, and the screen may hold it for a moment longer:
     * closing it here crashed the app when a stack was switched with the screen off, because the screen's polling
     * loop read the destroyed object (2026-11-05). The Rust side is freed by UniFFI's cleaner once nothing refers to
     * the Kotlin object.
     */
    @Synchronized
    fun stop(context: Context) {
        val server = running.value ?: return
        running.value = null
        val endpoints = server.stats().clients.mapNotNull { client -> client.endpoint?.let { client.publicKey to it } }
        Clients.rememberEndpoints(context, endpoints.toMap())
        server.stop()
    }

    private fun describe(e: CoreException): String =
        when (e) {
            is CoreException.InvalidKey -> "Invalid key: ${e.reason}"
            is CoreException.Start -> "Could not start: ${e.reason}"
            is CoreException.InvalidEndpoint -> "Invalid endpoint: ${e.reason}"
            is CoreException.InvalidAddress -> "Invalid address: ${e.reason}"
        }
}
