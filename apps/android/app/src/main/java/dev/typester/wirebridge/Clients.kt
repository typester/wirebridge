package dev.typester.wirebridge

import android.content.Context
import java.security.SecureRandom
import java.util.UUID
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import org.json.JSONArray
import org.json.JSONObject
import uniffi.wirebridge_core.ClientConfig
import uniffi.wirebridge_core.clientConfig
import uniffi.wirebridge_core.generatePrivateKey
import uniffi.wirebridge_core.publicKeyFromPrivate

/** A device allowed through the tunnel: its own key pair, and its own address inside the tunnel. */
data class Client(
    val id: String,
    val name: String,
    val privateKey: String,
    val publicKey: String,
    /** A plain IPv4 address in [Clients.NETWORK]. */
    val address: String,
    /** The unguessable part of the client's config URL on the group ([ConfigServer]). */
    val token: String,
    /** Where it was when the tunnel last stopped, so that the next start can handshake towards it ([Tunnel]). */
    val lastEndpoint: String? = null,
)

/**
 * The clients, made, renamed and deleted on the screen and kept in the app's private storage.
 *
 * Every client's key pair is made on this phone. Its private key leaves the phone only in its config ([config]): over
 * the Wi-Fi Direct group, in a QR code on the screen, or through Android's share sheet. So the group's passphrase is
 * what guards it, as any Wi-Fi's password would.
 *
 * A change reaches a running tunnel at once ([Tunnel.applyClients]).
 */
object Clients {
    /** Every client's address is `NETWORK.n`, n from 2 to 254. */
    const val NETWORK = "10.77.0"

    private const val PREFS = "clients"
    private const val LIST = "list"
    /**
     * Eight characters of [TOKEN_ALPHABET], about 8.5e11 tokens: no device on the group guesses one, and a Mac, which
     * has no camera to scan the QR code with, can still type the link in by hand.
     */
    private const val TOKEN_LENGTH = 8
    private const val TOKEN_ALPHABET = "abcdefghjkmnpqrstuvwxyz23456789"
    private val HOSTS = 2..254

    private val current = MutableStateFlow<List<Client>>(emptyList())
    val list: StateFlow<List<Client>> = current.asStateFlow()

    @Volatile
    private var loaded = false

    /** Reads the clients from storage, once. Everything else calls it first. */
    fun load(context: Context) {
        if (loaded) return
        synchronized(this) {
            if (!loaded) {
                current.value = read(context)
                loaded = true
            }
        }
    }

    /** Makes a client with a new key pair and the lowest free address. Throws if all 253 addresses are taken. */
    @Synchronized
    fun create(context: Context, name: String): Client {
        load(context)
        val taken = current.value.map { it.address }.toSet()
        val address = HOSTS.map { "$NETWORK.$it" }.firstOrNull { it !in taken }
            ?: throw IllegalStateException("every address in $NETWORK.0/24 is taken")
        val privateKey = generatePrivateKey()
        val client = Client(
            id = UUID.randomUUID().toString(),
            name = name.trim().ifEmpty { "Device ${current.value.size + 1}" },
            privateKey = privateKey,
            publicKey = publicKeyFromPrivate(privateKey),
            address = address,
            token = token(),
        )
        save(context, current.value + client)
        return client
    }

    @Synchronized
    fun rename(context: Context, id: String, name: String) {
        load(context)
        val trimmed = name.trim()
        if (trimmed.isEmpty()) return
        save(context, current.value.map { if (it.id == id) it.copy(name = trimmed) else it })
    }

    /** Deletes a client: a running tunnel stops answering it at once, and its config stops working. */
    @Synchronized
    fun delete(context: Context, id: String) {
        load(context)
        save(context, current.value.filterNot { it.id == id })
    }

    /** Keeps where each client was, by public key, as the tunnel stops. */
    @Synchronized
    fun rememberEndpoints(context: Context, endpoints: Map<String, String>) {
        load(context)
        if (endpoints.isEmpty()) return
        save(context, current.value.map { it.copy(lastEndpoint = endpoints[it.publicKey] ?: it.lastEndpoint) }, apply = false)
    }

    fun byId(id: String): Client? = current.value.firstOrNull { it.id == id }

    fun byToken(token: String): Client? = current.value.firstOrNull { it.token == token }

    /** What the tunnel is told about the clients. */
    fun configs(): List<ClientConfig> = current.value.map { ClientConfig(it.publicKey, it.address, it.lastEndpoint) }

    /** [client]'s WireGuard config, with the phone at [endpoint]: its address on the group, and the port. */
    fun config(context: Context, client: Client, endpoint: String): String =
        clientConfig(client.privateKey, client.address, Keys.phonePublicKey(context), endpoint)

    private fun save(context: Context, clients: List<Client>, apply: Boolean = true) {
        val array = JSONArray()
        clients.forEach { client ->
            array.put(
                JSONObject()
                    .put("id", client.id)
                    .put("name", client.name)
                    .put("privateKey", client.privateKey)
                    .put("publicKey", client.publicKey)
                    .put("address", client.address)
                    .put("token", client.token)
                    .put("lastEndpoint", client.lastEndpoint ?: JSONObject.NULL),
            )
        }
        prefs(context).edit().putString(LIST, array.toString()).commit()
        current.value = clients
        if (apply) Tunnel.applyClients()
    }

    private fun read(context: Context): List<Client> {
        val text = prefs(context).getString(LIST, null) ?: return emptyList()
        val array = JSONArray(text)
        return (0 until array.length()).map { i ->
            val o = array.getJSONObject(i)
            Client(
                id = o.getString("id"),
                name = o.getString("name"),
                privateKey = o.getString("privateKey"),
                publicKey = o.getString("publicKey"),
                address = o.getString("address"),
                token = o.getString("token"),
                lastEndpoint = if (o.isNull("lastEndpoint")) null else o.getString("lastEndpoint"),
            )
        }
    }

    private fun token(): String {
        val random = SecureRandom()
        return (0 until TOKEN_LENGTH).map { TOKEN_ALPHABET[random.nextInt(TOKEN_ALPHABET.length)] }.joinToString("")
    }

    private fun prefs(context: Context) = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
}
