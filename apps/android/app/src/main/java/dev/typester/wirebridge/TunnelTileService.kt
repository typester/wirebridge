package dev.typester.wirebridge

import android.Manifest
import android.app.ForegroundServiceStartNotAllowedException
import android.app.PendingIntent
import android.content.Intent
import android.content.pm.PackageManager
import android.graphics.drawable.Icon
import android.os.Build
import android.service.quicksettings.Tile
import android.service.quicksettings.TileService
import android.util.Log
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.collectLatest
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.launch

/**
 * The Quick Settings tile: a tap starts or stops [TunnelService], as Start and Stop on Home do. It is lit while the
 * tunnel runs, with how many clients are connected under it, the same count as the notification's.
 *
 * Shaped after VPNHotspot's RepeaterTileService, which starts its own Wi-Fi Direct group's foreground service from a
 * tap in the same way. What a tap cannot do, it leaves to the app: without "Nearby devices" the group cannot be made,
 * and the app is where that permission is asked for; if Android refuses to start the service from here, the app opens
 * instead. Start and Stop work on the lock screen too, since the tile shows nothing private.
 */
class TunnelTileService : TileService() {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    private var listening: Job? = null

    override fun onStartListening() {
        super.onStartListening()
        listening?.cancel()
        listening = scope.launch {
            val states = combine(Tunnel.state, Spot.state) { server, spot -> (server != null) to spot }
            states.collectLatest { (running, spot) ->
                // The count changes with every handshake, which no flow announces: read it again while the panel is
                // open.
                while (true) {
                    show(running, spot)
                    if (!running) break
                    delay(REFRESH_MS)
                }
            }
        }
    }

    override fun onStopListening() {
        listening?.cancel()
        listening = null
        super.onStopListening()
    }

    override fun onDestroy() {
        scope.cancel()
        super.onDestroy()
    }

    override fun onClick() {
        if (Tunnel.state.value != null) {
            TunnelService.stop(this)
            return
        }
        if (checkSelfPermission(Manifest.permission.NEARBY_WIFI_DEVICES) != PackageManager.PERMISSION_GRANTED) {
            openApp()
            return
        }
        try {
            TunnelService.start(this)
        } catch (e: ForegroundServiceStartNotAllowedException) {
            Log.w(TAG, "tile: starting the service was refused, opening the app: ${e.message}")
            openApp()
        }
    }

    private fun show(running: Boolean, spot: Spot.State) {
        val tile = qsTile ?: return
        tile.label = getString(R.string.app_name)
        tile.icon = Icon.createWithResource(this, R.drawable.ic_stat_wirebridge)
        if (running) {
            tile.state = Tile.STATE_ACTIVE
            tile.subtitle = when {
                spot.error != null -> "Wi-Fi error"
                spot.network == null -> "Starting"
                else -> "${Tunnel.connectedClients()} connected"
            }
        } else {
            tile.state = Tile.STATE_INACTIVE
            // Android shows "Off" in its place.
            tile.subtitle = null
        }
        tile.updateTile()
    }

    /** Opens the app, unlocking the phone first if it is locked, and closes the panel. */
    private fun openApp() {
        val intent = Intent(this, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        val open = Runnable {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
                startActivityAndCollapse(PendingIntent.getActivity(this, 0, intent, PendingIntent.FLAG_IMMUTABLE))
            } else {
                @Suppress("DEPRECATION")
                startActivityAndCollapse(intent)
            }
        }
        if (isLocked) unlockAndRun(open) else open.run()
    }

    private companion object {
        const val TAG = "wirebridge"
        const val REFRESH_MS = 5_000L
    }
}
