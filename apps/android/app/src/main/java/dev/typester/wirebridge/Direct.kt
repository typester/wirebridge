package dev.typester.wirebridge

import android.Manifest
import android.annotation.SuppressLint
import android.content.Context
import android.content.pm.PackageManager
import android.net.wifi.p2p.WifiP2pConfig
import android.net.wifi.p2p.WifiP2pGroup
import android.net.wifi.p2p.WifiP2pManager
import android.os.Build
import android.os.Looper
import java.net.Inet4Address
import java.net.InterfaceAddress
import java.net.NetworkInterface
import java.security.SecureRandom
import kotlin.coroutines.resume
import kotlinx.coroutines.delay
import kotlinx.coroutines.suspendCancellableCoroutine
import kotlinx.coroutines.withTimeoutOrNull

/**
 * The phone's Wi-Fi Direct group, the link the clients join as an ordinary Wi-Fi network. The phone owns the group,
 * under a name and a passphrase that stay the same until changed on the screen, so that a device joined once by hand
 * joins again by itself whenever the group is up. WPA2 only, which every Mac speaks.
 *
 * Android serves the group as local-only: a device on it reaches the phone and nothing beyond, except through the
 * tunnel.
 */
object Direct {
    /** Android wants a group's name to start with "DIRECT-" and two letters or digits; "wb" is this app's. */
    const val NAME_PREFIX = "DIRECT-wb-"

    /** A group's name is 32 bytes at most, and the prefix takes ten. */
    const val MAX_NAME_SUFFIX = 32 - NAME_PREFIX.length

    const val MIN_PASSPHRASE = 8
    const val MAX_PASSPHRASE = 63

    private const val DEFAULT_NAME_SUFFIX = "wirebridge"
    private const val PREFS = "direct"
    private const val NAME_SUFFIX = "nameSuffix"
    private const val PASSPHRASE = "passphrase"

    /** Lower-case letters and digits, without those easy to misread (i, l, o, 0, 1): typed by hand on a Mac. */
    private const val ALPHABET = "abcdefghjkmnpqrstuvwxyz23456789"
    private val NAME_SUFFIX_CHARACTERS = Regex("[A-Za-z0-9_-]+")

    fun hasPermission(context: Context): Boolean =
        context.checkSelfPermission(Manifest.permission.NEARBY_WIFI_DEVICES) == PackageManager.PERMISSION_GRANTED

    /** What follows [NAME_PREFIX] in the group's name. */
    fun nameSuffix(context: Context): String = prefs(context).getString(NAME_SUFFIX, null) ?: DEFAULT_NAME_SUFFIX

    /** The group's name. */
    fun name(context: Context): String = NAME_PREFIX + nameSuffix(context)

    /** The group's passphrase: set on the screen, or else made the first time and kept from then on. */
    @Synchronized
    fun passphrase(context: Context): String {
        val prefs = prefs(context)
        prefs.getString(PASSPHRASE, null)?.let { return it }
        val passphrase = generatePassphrase()
        prefs.edit().putString(PASSPHRASE, passphrase).commit()
        return passphrase
    }

    /** Sixteen characters in four groups of four, from [ALPHABET]. */
    fun generatePassphrase(): String {
        val random = SecureRandom()
        return (0 until 4).joinToString("-") {
            (0 until 4).map { ALPHABET[random.nextInt(ALPHABET.length)] }.joinToString("")
        }
    }

    /** Why [suffix] cannot follow [NAME_PREFIX], or null when it can. */
    fun checkNameSuffix(suffix: String): String? =
        when {
            suffix.isEmpty() -> "Give the Wi-Fi a name."
            suffix.length > MAX_NAME_SUFFIX -> "At most $MAX_NAME_SUFFIX characters."
            !NAME_SUFFIX_CHARACTERS.matches(suffix) -> "Letters, digits, - and _ only."
            else -> null
        }

    /** Why [passphrase] cannot be the group's, or null when it can: WPA2 takes 8 to 63 printable ASCII characters. */
    fun checkPassphrase(passphrase: String): String? =
        when {
            passphrase.length < MIN_PASSPHRASE -> "At least $MIN_PASSPHRASE characters."
            passphrase.length > MAX_PASSPHRASE -> "At most $MAX_PASSPHRASE characters."
            passphrase.any { it < ' ' || it > '~' } -> "Letters, digits, spaces and ASCII symbols only."
            else -> null
        }

    /** Keeps a new name and passphrase. The caller has checked both; a running group takes them when it is remade. */
    @Synchronized
    fun save(context: Context, nameSuffix: String, passphrase: String) {
        require(checkNameSuffix(nameSuffix) == null && checkPassphrase(passphrase) == null)
        prefs(context).edit().putString(NAME_SUFFIX, nameSuffix).putString(PASSPHRASE, passphrase).commit()
    }

    private fun prefs(context: Context) = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)

    /** The group as the phone makes it. [band] is one of WifiP2pConfig's GROUP_OWNER_BAND_ values. */
    fun config(context: Context, band: Int = WifiP2pConfig.GROUP_OWNER_BAND_AUTO): WifiP2pConfig =
        WifiP2pConfig.Builder()
            .setNetworkName(name(context))
            .setPassphrase(passphrase(context))
            .setGroupOperatingBand(band)
            .apply {
                // Android 16 can also make a WPA3 group, and documents no default: ask for WPA2 alone.
                if (Build.VERSION.SDK_INT >= 36) {
                    setPccModeConnectionType(WifiP2pConfig.PCC_MODE_CONNECTION_TYPE_LEGACY_ONLY)
                }
            }
            .build()

    /** The phone's IPv4 address on the group's interface, with its prefix length, once Android has given it one. */
    fun ownerAddress(interfaceName: String): InterfaceAddress? =
        runCatching {
            NetworkInterface.getByName(interfaceName)?.interfaceAddresses?.firstOrNull { it.address is Inet4Address }
        }.getOrNull()

    /** [ownerAddress], waiting up to [timeoutMs] for it to appear. */
    suspend fun awaitOwnerAddress(interfaceName: String, timeoutMs: Long): InterfaceAddress? =
        withTimeoutOrNull(timeoutMs) {
            var address = ownerAddress(interfaceName)
            while (address == null) {
                delay(POLL_MS)
                address = ownerAddress(interfaceName)
            }
            address
        }

    fun describe(address: InterfaceAddress?): String =
        address?.let { "${it.address.hostAddress}/${it.networkPrefixLength}" } ?: "no address"

    /** The name of a WifiP2pManager failure reason. */
    fun reason(code: Int): String =
        when (code) {
            WifiP2pManager.ERROR -> "ERROR"
            WifiP2pManager.P2P_UNSUPPORTED -> "P2P_UNSUPPORTED"
            WifiP2pManager.BUSY -> "BUSY"
            WifiP2pManager.NO_SERVICE_REQUESTS -> "NO_SERVICE_REQUESTS"
            else -> "reason $code"
        }

    /** How a group is secured, as Android 16 reports it. */
    fun security(group: WifiP2pGroup): String {
        if (Build.VERSION.SDK_INT < 36) return "security not reported"
        return when (group.securityType) {
            WifiP2pGroup.SECURITY_TYPE_WPA2_PSK -> "WPA2"
            WifiP2pGroup.SECURITY_TYPE_WPA3_COMPATIBILITY -> "WPA3 compatibility (WPA2 and WPA3)"
            WifiP2pGroup.SECURITY_TYPE_WPA3_SAE -> "WPA3"
            else -> "security unknown"
        }
    }

    const val POLL_MS = 250L
}

class DirectException(message: String) : Exception(message)

/**
 * Wi-Fi Direct opened for this app: making the group, reading it, removing it. Every call answers on the main looper.
 * Close it when done; closing does not remove the group, [removeAndClose] does.
 */
class DirectLink private constructor(private val manager: WifiP2pManager) : AutoCloseable {
    private lateinit var channel: WifiP2pManager.Channel

    /** Closing the channel also calls its listener; only a drop this app did not ask for is reported. */
    @Volatile
    private var closed = false

    /** A group made afresh: the group, and whether one had to be removed first. */
    class Made(val group: WifiP2pGroup, val removedOld: Boolean)

    /**
     * Makes the group from [config]: waits for Wi-Fi Direct to be enabled, removes a group left from before, creates
     * this one and waits until the phone owns it. Throws [DirectException] saying which step failed and why.
     */
    suspend fun makeGroup(config: WifiP2pConfig): Made {
        if (!awaitEnabled(ENABLE_TIMEOUT_MS)) {
            throw DirectException("Wi-Fi Direct stayed disabled for ${ENABLE_TIMEOUT_MS / 1000} s (is Wi-Fi off?)")
        }
        val removedOld = group() != null
        if (removedOld) {
            remove()?.let { throw DirectException("could not remove the group left from before: ${Direct.reason(it)}") }
        }
        // Right after a removal Android can still be busy with it.
        var failure = create(config)
        var tries = 1
        while (failure == WifiP2pManager.BUSY && tries < CREATE_TRIES) {
            delay(BUSY_RETRY_MS)
            failure = create(config)
            tries++
        }
        failure?.let { throw DirectException("could not create the group: ${Direct.reason(it)} (after $tries tries)") }
        val group = withTimeoutOrNull(GROUP_TIMEOUT_MS) {
            var group = group()
            while (group == null || !group.isGroupOwner || group.`interface` == null) {
                delay(Direct.POLL_MS)
                group = group()
            }
            group
        } ?: throw DirectException("the group did not form within ${GROUP_TIMEOUT_MS / 1000} s")
        return Made(group, removedOld)
    }

    /** Polls until Wi-Fi Direct is enabled; Android enables it once an app opens it, while Wi-Fi is on. */
    suspend fun awaitEnabled(timeoutMs: Long): Boolean =
        withTimeoutOrNull(timeoutMs) {
            while (state() != WifiP2pManager.WIFI_P2P_STATE_ENABLED) delay(Direct.POLL_MS)
            true
        } ?: false

    private suspend fun state(): Int =
        suspendCancellableCoroutine { continuation ->
            manager.requestP2pState(channel) { state -> if (continuation.isActive) continuation.resume(state) }
        }

    /** The group this phone is in, or null. */
    @SuppressLint("MissingPermission")
    suspend fun group(): WifiP2pGroup? =
        suspendCancellableCoroutine { continuation ->
            manager.requestGroupInfo(channel) { group -> if (continuation.isActive) continuation.resume(group) }
        }

    /** Removes the group; null once removed, else the failure reason. */
    suspend fun remove(): Int? =
        suspendCancellableCoroutine { continuation ->
            manager.removeGroup(channel, listener { if (continuation.isActive) continuation.resume(it) })
        }

    @SuppressLint("MissingPermission")
    private suspend fun create(config: WifiP2pConfig): Int? =
        suspendCancellableCoroutine { continuation ->
            manager.createGroup(channel, config, listener { if (continuation.isActive) continuation.resume(it) })
        }

    /** Removes the group without waiting, then closes; [done] gets null once removed, else the failure reason. */
    fun removeAndClose(done: (Int?) -> Unit) {
        manager.removeGroup(
            channel,
            listener {
                close()
                done(it)
            },
        )
    }

    override fun close() {
        closed = true
        channel.close()
    }

    private fun listener(done: (Int?) -> Unit) =
        object : WifiP2pManager.ActionListener {
            override fun onSuccess() = done(null)

            override fun onFailure(reason: Int) = done(reason)
        }

    companion object {
        private const val ENABLE_TIMEOUT_MS = 10_000L
        private const val GROUP_TIMEOUT_MS = 15_000L
        private const val CREATE_TRIES = 10
        private const val BUSY_RETRY_MS = 500L

        /** Opens Wi-Fi Direct. [onLost] runs if Android drops this app's channel. */
        fun open(context: Context, onLost: () -> Unit): DirectLink {
            if (!Direct.hasPermission(context)) {
                throw DirectException("not allowed to use Wi-Fi Direct: NEARBY_WIFI_DEVICES is not granted")
            }
            val manager = context.getSystemService(WifiP2pManager::class.java)
                ?: throw DirectException("this phone has no Wi-Fi Direct")
            val link = DirectLink(manager)
            link.channel = manager.initialize(context, Looper.getMainLooper()) { if (!link.closed) onLost() }
                ?: throw DirectException("Wi-Fi Direct could not be opened")
            return link
        }
    }
}
