package dev.typester.wirebridge

import java.io.BufferedReader
import java.io.InputStreamReader
import java.net.InetAddress
import java.net.InetSocketAddress
import java.net.ServerSocket
import java.net.Socket
import java.net.SocketException
import kotlin.concurrent.thread

/**
 * Serves the clients' WireGuard configs over HTTP on the phone's address in the Wi-Fi Direct group, and nowhere else:
 * bound to that address, it cannot be reached from home Wi-Fi or cellular. One request at a time, on one thread.
 *
 * `GET /c/<token>.conf` gets that client's config, made at the moment from the clients as they are, as a download
 * named `wirebridge.conf`, which the WireGuard app names the tunnel after. Every device on the group can reach this
 * server, so a client's URL carries a random token of its own ([Clients]) rather than anything guessable. Anything else
 * gets 404.
 */
class ConfigServer private constructor(
    private val socket: ServerSocket,
    /** A client's name and config by its token, or null when no client has it. */
    private val configFor: (String) -> Pair<String, String>?,
    private val log: (String) -> Unit,
) : AutoCloseable {
    init {
        thread(name = "wirebridge-config", isDaemon = true) { serve() }
    }

    private fun serve() {
        while (!socket.isClosed) {
            val client = try {
                socket.accept()
            } catch (e: SocketException) {
                return
            }
            try {
                client.use { answer(it) }
            } catch (e: Exception) {
                log("config: ${client.inetAddress.hostAddress}: ${e.message ?: e}")
            }
        }
    }

    private fun answer(client: Socket) {
        client.soTimeout = READ_TIMEOUT_MS
        val reader = BufferedReader(InputStreamReader(client.getInputStream(), Charsets.US_ASCII))
        val request = reader.readLine() ?: return
        // The headers are read and ignored, up to the blank line that ends them.
        while (true) {
            val line = reader.readLine() ?: break
            if (line.isEmpty()) break
        }
        val parts = request.split(' ')
        val from = client.inetAddress.hostAddress
        val out = client.getOutputStream()
        val token = parts.getOrNull(1)?.let { PATH.matchEntire(it)?.groupValues?.get(1) }
        val found = token?.takeIf { parts[0] == "GET" }?.let(configFor)
        if (found != null) {
            val (name, config) = found
            val body = config.toByteArray(Charsets.UTF_8)
            out.write(
                head(
                    "200 OK",
                    body.size,
                    "Content-Disposition: attachment; filename=\"$FILE_NAME\"",
                    "Cache-Control: no-store",
                ),
            )
            out.write(body)
            log("config: $name's sent to $from")
        } else {
            val body = "Not found\n".toByteArray(Charsets.US_ASCII)
            out.write(head("404 Not Found", body.size))
            out.write(body)
            log("config: 404 for a request from $from")
        }
        out.flush()
    }

    private fun head(status: String, length: Int, vararg extra: String): ByteArray {
        val lines = listOf(
            "HTTP/1.0 $status",
            "Content-Type: text/plain; charset=utf-8",
            "Content-Length: $length",
            "Connection: close",
        ) + extra
        return (lines.joinToString("\r\n") + "\r\n\r\n").toByteArray(Charsets.US_ASCII)
    }

    override fun close() {
        runCatching { socket.close() }
    }

    companion object {
        const val PORT = 8080

        /** What every client's config is called, wherever it goes: the WireGuard app names the tunnel after it. */
        const val FILE_NAME = "wirebridge.conf"

        private val PATH = Regex("/c/([a-z0-9]+)\\.conf")
        private const val BACKLOG = 4
        private const val READ_TIMEOUT_MS = 5_000

        /** Where [token]'s config is, with the phone at [address] on the group. */
        fun url(address: String, token: String): String = "http://$address:$PORT/c/$token.conf"

        /** Starts serving on [address]. Throws if the port cannot be bound there. */
        fun start(
            address: InetAddress,
            configFor: (String) -> Pair<String, String>?,
            log: (String) -> Unit,
        ): ConfigServer {
            val socket = ServerSocket()
            try {
                socket.reuseAddress = true
                socket.bind(InetSocketAddress(address, PORT), BACKLOG)
            } catch (e: Exception) {
                socket.close()
                throw e
            }
            return ConfigServer(socket, configFor, log)
        }
    }
}
