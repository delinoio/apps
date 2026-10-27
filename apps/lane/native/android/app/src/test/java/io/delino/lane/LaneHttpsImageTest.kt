// SPDX-License-Identifier: Apache-2.0
package io.delino.lane

import java.io.ByteArrayInputStream
import java.io.IOException
import java.net.HttpURLConnection
import java.net.URL
import java.util.Base64
import org.junit.Assert.*
import org.junit.Test

class LaneHttpsImageTest {
    private val png = Base64.getDecoder().decode(
        "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+/RZkAAAAASUVORK5CYII="
    )

    private class Reply(url: URL, private val status: Int, private val bytes: ByteArray = byteArrayOf(),
        private val location: String? = null) : HttpURLConnection(url) {
        var disconnected = false
        override fun connect() {}
        override fun disconnect() { disconnected = true }
        override fun usingProxy() = false
        override fun getResponseCode() = status
        override fun getInputStream() = ByteArrayInputStream(bytes)
        override fun getHeaderField(name: String?) = if (name == "Location") location else null
    }

    @Test fun loadsPngThroughHttpsWithoutCaches() {
        val reply = Reply(URL("https://fixture.test/image.png"), 200, png)
        val result = LaneHttpsImage.load(reply.url.toString(), LaneHttpsImage.Cancellation()) { reply }
        assertArrayEquals(png, result)
        assertFalse(reply.useCaches)
        assertFalse(reply.instanceFollowRedirects)
        assertTrue(reply.disconnected)
    }

    @Test fun rejectsHttpAndRedirectDowngradeBeforeOpeningDestination() {
        val opened = mutableListOf<String>()
        val source = "https://fixture.test/image.png"
        val open: (URL) -> HttpURLConnection = {
            opened += it.toString()
            Reply(it, 302, location = "http://fixture.test/image.png")
        }
        assertThrows(IOException::class.java) { LaneHttpsImage.load("http://fixture.test/image.png", LaneHttpsImage.Cancellation(), open) }
        assertTrue(opened.isEmpty())
        assertThrows(IOException::class.java) { LaneHttpsImage.load(source, LaneHttpsImage.Cancellation(), open) }
        assertEquals(listOf(source), opened)
    }

    @Test fun followsOnlyHttpsRedirectsAndFailsOnBadResponses() {
        val opened = mutableListOf<String>()
        val source = "https://fixture.test/first"
        val target = "https://fixture.test/image.png"
        val result = LaneHttpsImage.load(source, LaneHttpsImage.Cancellation()) {
            opened += it.toString()
            if (it.toString() == source) Reply(it, 302, location = "/image.png") else Reply(it, 200, png)
        }
        assertArrayEquals(png, result)
        assertEquals(listOf(source, target), opened)
        for (status in listOf(404, 500)) assertThrows(IOException::class.java) {
            LaneHttpsImage.load(source, LaneHttpsImage.Cancellation()) { Reply(it, status) }
        }
        assertThrows(IOException::class.java) {
            LaneHttpsImage.load(source, LaneHttpsImage.Cancellation()) { Reply(it, 200, "not a png".toByteArray()) }
        }
        assertThrows(IOException::class.java) {
            LaneHttpsImage.load(source, LaneHttpsImage.Cancellation()) { throw IOException("TLS failure") }
        }
    }

    @Test fun canceledRequestCannotReturnBytes() {
        val cancellation = LaneHttpsImage.Cancellation()
        val reply = Reply(URL("https://fixture.test/image.png"), 200, png)
        assertThrows(IOException::class.java) {
            LaneHttpsImage.load(reply.url.toString(), cancellation) {
                cancellation.cancel()
                reply
            }
        }
        assertTrue(reply.disconnected)
    }
}
