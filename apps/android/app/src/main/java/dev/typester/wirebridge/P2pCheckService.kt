package dev.typester.wirebridge

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.Service
import android.content.Intent
import android.content.pm.ServiceInfo
import android.net.ConnectivityManager
import android.net.NetworkCapabilities
import android.net.wifi.p2p.WifiP2pConfig
import android.os.IBinder
import android.os.PowerManager
import android.os.SystemClock
import android.util.Log
import java.io.File
import java.net.InetSocketAddress
import java.net.NetworkInterface
import java.net.Socket
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/**
 * Checks the Wi-Fi Direct group on its own: makes it ([Direct]), reports how it came up, and then, until stopped,
 * reports every change in the group, the screen, Doze and the phone's default network, with a test connection out
 * to the internet every minute. Driven from the laptop:
 *
 *     adb shell am start-foreground-service -n dev.typester.wirebridge/.P2pCheckService \
 *         [--es action start|recreate] [--es band 2|5]
 *     adb shell am stopservice -n dev.typester.wirebridge/.P2pCheckService
 *
 * `recreate` removes the group and makes it again with the same name and passphrase; `band` keeps it to one band.
 * Stopping removes the group. Every line goes to logcat (tag `wirebridge`) and, with the time, to the app's
 * `files/p2p-check.log`, which outlasts logcat's turnover:
 *
 *     adb shell run-as dev.typester.wirebridge cat files/p2p-check.log
 *
 * The passphrase is written out in both: the same passphrase is on the app's screen anyway.
 *
 * Not while [TunnelService] runs: it makes the same group through [Spot], and each would remove the other's.
 */
class P2pCheckService : Service() {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)

    @Volatile
    private var link: DirectLink? = null
    private var making: Job? = null
    private var watching: Job? = null
    private var wakeLock: PowerManager.WakeLock? = null

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        startForeground(NOTIFICATION_ID, notification(null), ServiceInfo.FOREGROUND_SERVICE_TYPE_CONNECTED_DEVICE)
        if (wakeLock == null) {
            wakeLock = getSystemService(PowerManager::class.java)
                .newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "wirebridge:p2p-check")
                .apply { acquire() }
        }
        val action = intent?.getStringExtra("action") ?: "start"
        val band = when (intent?.getStringExtra("band")) {
            "2" -> WifiP2pConfig.GROUP_OWNER_BAND_2GHZ
            "5" -> WifiP2pConfig.GROUP_OWNER_BAND_5GHZ
            else -> WifiP2pConfig.GROUP_OWNER_BAND_AUTO
        }
        when {
            making?.isActive == true -> log("$action ignored: still making the group")
            action == "start" && link != null -> log("start ignored: already running (recreate makes it afresh)")
            action == "start" || action == "recreate" -> making = scope.launch { make(action, band) }
            else -> log("unknown action $action")
        }
        return START_NOT_STICKY
    }

    override fun onDestroy() {
        scope.cancel()
        val link = link
        this.link = null
        if (link == null) {
            log("stopped")
        } else {
            link.removeAndClose { failure ->
                log(
                    if (failure == null) {
                        "stopped: group removed"
                    } else {
                        "stopped: removing the group failed, ${Direct.reason(failure)}"
                    },
                )
            }
        }
        wakeLock?.release()
        wakeLock = null
        super.onDestroy()
    }

    private suspend fun make(action: String, band: Int) {
        try {
            val link = this.link
                ?: DirectLink.open(applicationContext) { log("Android dropped the Wi-Fi Direct channel") }
                    .also { this.link = it }
            log("$action: making the group${if (band == WifiP2pConfig.GROUP_OWNER_BAND_AUTO) "" else " (band $band)"}")
            val start = SystemClock.elapsedRealtime()
            val made = link.makeGroup(Direct.config(applicationContext, band))
            val formedAfter = SystemClock.elapsedRealtime() - start
            val group = made.group
            val address = Direct.awaitOwnerAddress(group.`interface`, ADDRESS_TIMEOUT_MS)
            val addressedAfter = SystemClock.elapsedRealtime() - start
            log(
                "$action: group up in $formedAfter ms${if (made.removedOld) " (the old one removed first)" else ""}: " +
                    "${group.networkName}, passphrase ${group.passphrase}, ${group.`interface`}, " +
                    "${group.frequency} MHz, ${Direct.security(group)}, owner ${Direct.describe(address)} " +
                    "after $addressedAfter ms",
            )
            getSystemService(NotificationManager::class.java)
                .notify(NOTIFICATION_ID, notification("${group.networkName}  ${group.passphrase}"))
            if (watching?.isActive != true) watching = scope.launch { watch() }
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            log("$action: failed: ${e.message ?: e}")
        }
    }

    /** Every [WATCH_MS]: one line whenever something changed, and a test connection every [DIAL_MS]. */
    private suspend fun watch() {
        val power = getSystemService(PowerManager::class.java)
        var last = ""
        var lastNetwork = ""
        var lastDial = 0L
        while (true) {
            val group = try {
                link?.group()
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                null
            }
            val groupText = group?.let {
                val name: String? = it.`interface`
                "group ${it.networkName} on $name with ${it.clientList.size} client(s), " +
                    "owner ${Direct.describe(name?.let(Direct::ownerAddress))}"
            } ?: "no group"
            val screen = if (power.isInteractive) "screen on" else "screen off"
            val idle = if (power.isDeviceIdleMode) "dozing" else "not dozing"
            val network = defaultNetwork()
            val line = "$groupText; $screen, $idle; default network $network"
            if (line != last) {
                log(line)
                last = line
            }
            val now = SystemClock.elapsedRealtime()
            if (network != lastNetwork || now - lastDial >= DIAL_MS) {
                log(dial())
                lastNetwork = network
                lastDial = now
            }
            delay(WATCH_MS)
        }
    }

    private fun defaultNetwork(): String {
        val connectivity = getSystemService(ConnectivityManager::class.java)
        val network = connectivity.activeNetwork ?: return "none"
        val capabilities = connectivity.getNetworkCapabilities(network)
        val kind = when {
            capabilities == null -> "unknown"
            capabilities.hasTransport(NetworkCapabilities.TRANSPORT_WIFI) -> "Wi-Fi"
            capabilities.hasTransport(NetworkCapabilities.TRANSPORT_CELLULAR) -> "cellular"
            else -> "other"
        }
        return "$kind (${connectivity.getLinkProperties(network)?.interfaceName ?: "no interface"})"
    }

    /** A TCP connection to 1.1.1.1:443 from the app's own socket, as the tunnel's are, and the interface it took. */
    private suspend fun dial(): String =
        withContext(Dispatchers.IO) {
            val start = SystemClock.elapsedRealtime()
            try {
                Socket().use { socket ->
                    socket.connect(InetSocketAddress(DIAL_HOST, DIAL_PORT), DIAL_TIMEOUT_MS)
                    val local = socket.localAddress
                    val name = runCatching { NetworkInterface.getByInetAddress(local)?.name }.getOrNull() ?: "?"
                    "dial $DIAL_HOST:$DIAL_PORT from ${local.hostAddress} ($name) in " +
                        "${SystemClock.elapsedRealtime() - start} ms"
                }
            } catch (e: Exception) {
                "dial $DIAL_HOST:$DIAL_PORT failed after ${SystemClock.elapsedRealtime() - start} ms: ${e.message ?: e}"
            }
        }

    @Synchronized
    private fun log(line: String) {
        Log.i(TAG, "p2p-check: $line")
        val stamp = SimpleDateFormat("yyyy-MM-dd HH:mm:ss.SSS", Locale.US).format(Date())
        runCatching { File(filesDir, LOG_FILE).appendText("$stamp $line\n") }
    }

    private fun notification(text: String?): Notification {
        val manager = getSystemService(NotificationManager::class.java)
        manager.createNotificationChannel(
            NotificationChannel(CHANNEL, "Wi-Fi Direct check", NotificationManager.IMPORTANCE_LOW),
        )
        return Notification.Builder(this, CHANNEL)
            .setContentTitle("Wirebridge is checking Wi-Fi Direct")
            .apply { if (text != null) setContentText(text) }
            .setSmallIcon(android.R.drawable.stat_sys_upload)
            .setOngoing(true)
            .build()
    }

    companion object {
        private const val TAG = "wirebridge"
        private const val CHANNEL = "p2p-check"
        private const val NOTIFICATION_ID = 3
        private const val LOG_FILE = "p2p-check.log"
        private const val ADDRESS_TIMEOUT_MS = 10_000L
        private const val WATCH_MS = 2_000L
        private const val DIAL_MS = 60_000L
        private const val DIAL_HOST = "1.1.1.1"
        private const val DIAL_PORT = 443
        private const val DIAL_TIMEOUT_MS = 5_000
    }
}
