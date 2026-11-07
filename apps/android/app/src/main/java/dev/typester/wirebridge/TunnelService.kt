package dev.typester.wirebridge

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.IBinder
import android.os.PowerManager
import kotlin.concurrent.thread
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.cancelAndJoin
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

/**
 * Keeps the process, and the tunnel in it, alive while the screen is off or another app is in front.
 *
 * A connectedDevice foreground service: the clients are the connected devices. The partial wake lock keeps the CPU
 * running while the screen is off, since every packet a client sends is decrypted and re-sent by this process.
 *
 * While it runs it also keeps the link the clients join ([Spot]): the Wi-Fi Direct group, and their configs served on
 * it. The notification says how many clients are connected, and on which Wi-Fi.
 *
 * Also the way to drive the app from a laptop while measuring:
 *
 *     adb shell am start-foreground-service -n dev.typester.wirebridge/.TunnelService
 *
 * Starting it while the endpoint runs does nothing.
 */
class TunnelService : Service() {
    private var wakeLock: PowerManager.WakeLock? = null
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
    private var spot: Job? = null
    private var counting: Job? = null

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        startForeground(NOTIFICATION_ID, notification(), ServiceInfo.FOREGROUND_SERVICE_TYPE_CONNECTED_DEVICE)
        if (intent?.action == ACTION_RESTART_LINK) {
            restartSpot()
            return START_NOT_STICKY
        }
        if (wakeLock == null) {
            wakeLock = getSystemService(PowerManager::class.java)
                .newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "wirebridge:tunnel")
                .apply { acquire() }
        }
        // Starting waits for a stop still in progress, which is not for the main thread.
        thread(name = "wirebridge-start") {
            if (Tunnel.start(applicationContext)) {
                startSpot()
                startCounting()
            } else {
                stopSelf()
            }
        }
        return START_NOT_STICKY
    }

    @Synchronized
    private fun startSpot() {
        if (spot?.isActive == true) return
        spot = scope.launch { Spot.run(applicationContext) { update() } }
    }

    /** Makes the group again, for a new name or passphrase. The tunnel keeps running; only the Wi-Fi goes and comes. */
    @Synchronized
    private fun restartSpot() {
        val old = spot ?: return
        spot = scope.launch {
            old.cancelAndJoin()
            Spot.run(applicationContext) { update() }
        }
    }

    /** Keeps the notification's count of connected clients current. */
    @Synchronized
    private fun startCounting() {
        if (counting?.isActive == true) return
        counting = scope.launch {
            var shown = -1
            while (true) {
                val connected = Tunnel.connectedClients()
                if (connected != shown) {
                    update()
                    shown = connected
                }
                delay(COUNT_INTERVAL_MS)
            }
        }
    }

    override fun onDestroy() {
        scope.cancel()
        thread(name = "wirebridge-stop") { Tunnel.stop(applicationContext) }
        wakeLock?.release()
        wakeLock = null
        super.onDestroy()
    }

    private fun update() {
        getSystemService(NotificationManager::class.java).notify(NOTIFICATION_ID, notification())
    }

    /** The Wi-Fi's name once the group is up, and how many clients have been heard from lately. */
    private fun notification(): Notification {
        val manager = getSystemService(NotificationManager::class.java)
        manager.createNotificationChannel(
            NotificationChannel(CHANNEL, "Tunnel", NotificationManager.IMPORTANCE_LOW),
        )
        val open = PendingIntent.getActivity(
            this,
            0,
            Intent(this, MainActivity::class.java),
            PendingIntent.FLAG_IMMUTABLE,
        )
        val network = Spot.state.value.network
        val connected = Tunnel.connectedClients()
        val clients = when (connected) {
            0 -> "No client connected"
            1 -> "1 client connected"
            else -> "$connected clients connected"
        }
        return Notification.Builder(this, CHANNEL)
            .setContentTitle("Wirebridge is running")
            .setContentText(network?.let { "$clients · Wi-Fi $it" } ?: "Bringing up the Wi-Fi…")
            .setSmallIcon(R.drawable.ic_stat_wirebridge)
            .setContentIntent(open)
            .setOngoing(true)
            .build()
    }

    companion object {
        private const val CHANNEL = "tunnel"
        private const val NOTIFICATION_ID = 1
        private const val COUNT_INTERVAL_MS = 10_000L
        private const val ACTION_RESTART_LINK = "dev.typester.wirebridge.RESTART_LINK"

        fun start(context: Context) {
            context.startForegroundService(Intent(context, TunnelService::class.java))
        }

        fun stop(context: Context) {
            context.stopService(Intent(context, TunnelService::class.java))
        }

        /** Remakes the group with the Wi-Fi settings as they are now, if the tunnel is running. */
        fun restartLink(context: Context) {
            if (Tunnel.state.value == null) return
            context.startForegroundService(Intent(context, TunnelService::class.java).setAction(ACTION_RESTART_LINK))
        }
    }
}
