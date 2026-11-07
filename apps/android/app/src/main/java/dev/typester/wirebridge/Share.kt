package dev.typester.wirebridge

import android.content.Context
import android.content.Intent
import androidx.core.content.FileProvider
import java.io.File

/**
 * Hands a client's config to Android's share sheet, which offers Quick Share among the rest. The config holds the
 * client's private key, so it goes only where the owner sends it, and the file written for the share sheet is removed
 * the next time the app starts ([clear]).
 */
object Share {
    private const val DIR = "share"

    fun config(context: Context, client: Client, config: String) {
        val dir = File(context.cacheDir, DIR).apply { mkdirs() }
        val file = File(dir, ConfigServer.FILE_NAME)
        file.writeText(config)
        val uri = FileProvider.getUriForFile(context, "${context.packageName}.files", file)
        val send = Intent(Intent.ACTION_SEND)
            .setType("text/plain")
            .putExtra(Intent.EXTRA_STREAM, uri)
            .putExtra(Intent.EXTRA_TITLE, ConfigServer.FILE_NAME)
            .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
        context.startActivity(Intent.createChooser(send, "${client.name}'s WireGuard config"))
    }

    /** Removes what an earlier share left behind. */
    fun clear(context: Context) {
        File(context.cacheDir, DIR).deleteRecursively()
    }

    /**
     * The Wi-Fi as a QR code's text, which phones' and tablets' cameras join from: `WIFI:T:WPA;S:<name>;P:<pass>;;`,
     * with `\ ; , : "` escaped.
     */
    fun wifiQrText(network: String, passphrase: String): String = "WIFI:T:WPA;S:${escape(network)};P:${escape(passphrase)};;"

    private fun escape(text: String): String = text.replace(Regex("""([\\;,:"])"""), """\\$1""")
}
