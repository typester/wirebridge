package dev.typester.wirebridge

import android.Manifest
import android.content.pm.PackageManager
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.material3.Surface
import androidx.compose.runtime.Composable
import androidx.navigation.compose.NavHost
import androidx.navigation.compose.composable
import androidx.navigation.compose.rememberNavController
import dev.typester.wirebridge.ui.ClientScreen
import dev.typester.wirebridge.ui.ClientsScreen
import dev.typester.wirebridge.ui.HomeScreen
import dev.typester.wirebridge.ui.WifiScreen
import dev.typester.wirebridge.ui.WirebridgeTheme

class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        // Without notifications the foreground service still runs, but out of sight. Without "Nearby devices" the app
        // cannot make the Wi-Fi Direct group the clients join.
        val wanted = listOf(
            Manifest.permission.POST_NOTIFICATIONS,
            Manifest.permission.NEARBY_WIFI_DEVICES,
        ).filter { checkSelfPermission(it) != PackageManager.PERMISSION_GRANTED }
        if (wanted.isNotEmpty()) requestPermissions(wanted.toTypedArray(), 1)
        Share.clear(this)
        Clients.load(this)
        setContent {
            WirebridgeTheme {
                Surface { App() }
            }
        }
    }
}

/** Home, the clients, one client, and the Wi-Fi's settings. */
@Composable
private fun App() {
    val nav = rememberNavController()
    NavHost(navController = nav, startDestination = "home") {
        composable("home") {
            HomeScreen(
                onWifi = { nav.navigate("wifi") },
                onClients = { nav.navigate("clients") },
                onClient = { nav.navigate("client/$it") },
            )
        }
        composable("clients") {
            ClientsScreen(onBack = { nav.popBackStack() }, onClient = { nav.navigate("client/$it") })
        }
        composable("client/{id}") { entry ->
            ClientScreen(id = entry.arguments?.getString("id").orEmpty(), onBack = { nav.popBackStack() })
        }
        composable("wifi") {
            WifiScreen(onBack = { nav.popBackStack() })
        }
    }
}
