package dev.typester.wirebridge

import android.content.Context
import android.util.Log
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.awaitCancellation
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow

/**
 * The link the clients use while [TunnelService] runs: the Wi-Fi Direct group ([Direct]), and the clients' configs
 * served on the phone's address in it ([ConfigServer]). The address is read from the group once it is up, never
 * assumed, and every config names it as the endpoint, so a config can only be made while the group is up.
 */
object Spot {
    private const val TAG = "wirebridge"
    private const val ADDRESS_TIMEOUT_MS = 10_000L

    /** What the screen shows: the group once it is up, or why it is not. */
    data class State(
        val network: String? = null,
        val passphrase: String? = null,
        /** The phone's address on the group. */
        val address: String? = null,
        val error: String? = null,
    ) {
        /** Where the clients reach the phone's WireGuard: its address on the group, and the port. */
        val endpoint: String? get() = address?.let { "$it:${Tunnel.PORT}" }
    }

    private val current = MutableStateFlow(State())
    val state: StateFlow<State> = current.asStateFlow()

    /**
     * Brings the group and the config server up, calls [onUp] with what they are, and keeps them until cancelled;
     * then takes both down. A failure is left in [state] for the screen.
     */
    suspend fun run(context: Context, onUp: (State) -> Unit) {
        var link: DirectLink? = null
        var server: ConfigServer? = null
        try {
            current.value = State()
            link = DirectLink.open(context) { fail("Android dropped the Wi-Fi Direct channel") }
            val group = link.makeGroup(Direct.config(context)).group
            val name: String = group.`interface` ?: throw DirectException("the group has no interface")
            val owner = Direct.awaitOwnerAddress(name, ADDRESS_TIMEOUT_MS)?.address
                ?: throw DirectException("$name got no IPv4 address within ${ADDRESS_TIMEOUT_MS / 1000} s")
            val address = owner.hostAddress ?: throw DirectException("$name has an address with no text form")
            val up = State(group.networkName, group.passphrase, address)
            val endpoint = up.endpoint!!
            server = ConfigServer.start(
                owner,
                { token -> Clients.byToken(token)?.let { it.name to Clients.config(context, it, endpoint) } },
            ) { Log.i(TAG, it) }
            current.value = up
            Log.i(TAG, "spot: ${group.networkName} on $name, ${group.frequency} MHz, ${Direct.security(group)}, " +
                "configs at http://$address:${ConfigServer.PORT}/c/")
            onUp(up)
            awaitCancellation()
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            fail(e.message ?: e.toString())
        } finally {
            server?.close()
            link?.removeAndClose { failure ->
                if (failure != null) Log.w(TAG, "spot: removing the group failed, ${Direct.reason(failure)}")
            }
            if (current.value.error == null) current.value = State()
        }
    }

    private fun fail(error: String) {
        Log.w(TAG, "spot: $error")
        current.value = State(error = error)
    }
}
