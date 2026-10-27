// SPDX-License-Identifier: Apache-2.0
package io.delino.lane

import java.io.ByteArrayOutputStream
import java.io.IOException
import java.net.HttpURLConnection
import java.net.URL
import java.util.concurrent.atomic.AtomicBoolean

/** Fetch HTTPS image bytes without following a redirect to another scheme or using a URL cache. */
internal object LaneHttpsImage {
    private const val maxBytes = 8 * 1024 * 1024
    private const val maxRedirects = 5
    private val pngSignature = byteArrayOf(0x89.toByte(), 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a)

    internal class Cancellation {
        val canceled = AtomicBoolean(false)
        @Volatile private var connection: HttpURLConnection? = null

        fun attach(value: HttpURLConnection) {
            connection = value
            if (canceled.get()) value.disconnect()
        }

        fun detach(value: HttpURLConnection) {
            if (connection === value) connection = null
        }

        fun cancel() {
            canceled.set(true)
            connection?.disconnect()
        }
    }

    internal fun allowed(url: URL): Boolean =
        url.protocol.equals("https", ignoreCase = true) && url.host.isNotEmpty() &&
            runCatching { url.toURI().userInfo == null }.getOrDefault(false)

    internal fun isPng(bytes: ByteArray): Boolean =
        bytes.size >= pngSignature.size && pngSignature.indices.all { bytes[it] == pngSignature[it] }

    internal fun load(
        source: String,
        cancellation: Cancellation,
        open: (URL) -> HttpURLConnection = { it.openConnection() as HttpURLConnection },
    ): ByteArray {
        var url = URL(source)
        repeat(maxRedirects + 1) { hop ->
            if (!allowed(url) || cancellation.canceled.get()) throw IOException("Image request denied")
            val connection = open(url)
            cancellation.attach(connection)
            try {
                connection.instanceFollowRedirects = false
                connection.useCaches = false
                connection.connectTimeout = 10_000
                connection.readTimeout = 15_000
                when (connection.responseCode) {
                    HttpURLConnection.HTTP_OK -> {
                        val output = ByteArrayOutputStream()
                        connection.inputStream.use { input ->
                            val chunk = ByteArray(8192)
                            while (true) {
                                if (cancellation.canceled.get()) throw IOException("Image request canceled")
                                val count = input.read(chunk)
                                if (count < 0) break
                                if (output.size() + count > maxBytes) throw IOException("Image exceeds limit")
                                output.write(chunk, 0, count)
                            }
                        }
                        val bytes = output.toByteArray()
                        if (!isPng(bytes)) throw IOException("Image is not PNG")
                        return bytes
                    }
                    HttpURLConnection.HTTP_MOVED_PERM, HttpURLConnection.HTTP_MOVED_TEMP,
                    HttpURLConnection.HTTP_SEE_OTHER, 307, 308 -> {
                        if (hop == maxRedirects) throw IOException("Too many image redirects")
                        val location = connection.getHeaderField("Location") ?: throw IOException("Image redirect missing location")
                        val target = URL(url, location)
                        if (!allowed(target)) throw IOException("Image redirect denied")
                        url = target
                    }
                    else -> throw IOException("Image HTTP failure")
                }
            } finally {
                cancellation.detach(connection)
                connection.disconnect()
            }
        }
        throw IOException("Too many image redirects")
    }
}
