package dev.typester.wirebridge.ui

import dev.typester.wirebridge.Tunnel
import uniffi.wirebridge_core.ClientStats

fun bytes(n: ULong): String {
    val v = n.toDouble()
    return when {
        v >= 1e9 -> "%.2f GB".format(v / 1e9)
        v >= 1e6 -> "%.1f MB".format(v / 1e6)
        v >= 1e3 -> "%.0f kB".format(v / 1e3)
        else -> "$n B"
    }
}

fun rate(bytesPerSecond: Double): String {
    val bits = bytesPerSecond * 8
    return when {
        bits >= 1e6 -> "%.1f Mbit/s".format(bits / 1e6)
        bits >= 1e3 -> "%.0f kbit/s".format(bits / 1e3)
        else -> "0 kbit/s"
    }
}

fun ago(seconds: ULong): String {
    val s = seconds.toLong()
    return when {
        s < 60 -> "$s s ago"
        s < 3600 -> "${s / 60} min ago"
        s < 86_400 -> "${s / 3600} h ago"
        else -> "${s / 86_400} d ago"
    }
}

/** Whether a client has handshaken recently enough to count as connected. */
fun ClientStats?.isConnected(): Boolean = this?.secondsSinceHandshake?.let { it < Tunnel.CONNECTED_SECONDS } == true

/** A client's state in a few words: connected, when it was last heard from, or never. */
fun ClientStats?.describe(): String {
    val since = this?.secondsSinceHandshake
    return when {
        since == null -> "Not connected"
        since < Tunnel.CONNECTED_SECONDS -> "Connected"
        else -> "Last seen ${ago(since)}"
    }
}
