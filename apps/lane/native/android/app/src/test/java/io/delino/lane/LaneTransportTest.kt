// SPDX-License-Identifier: Apache-2.0
package io.delino.lane

import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import java.io.ByteArrayInputStream
import java.io.ByteArrayOutputStream
import java.io.DataOutputStream
import java.net.HttpURLConnection
import java.net.URL

class LaneTransportTest {
    private class Connection(url: URL, private val status: Int = 200, val response: ByteArray = "{}".toByteArray()) : HttpURLConnection(url) {
        val sent = ByteArrayOutputStream()
        var closed = false
        override fun connect() {}
        override fun disconnect() { closed = true }
        override fun usingProxy() = false
        override fun getOutputStream() = sent
        override fun getInputStream() = ByteArrayInputStream(response)
        override fun getErrorStream() = ByteArrayInputStream(response)
        override fun getResponseCode() = status
    }
    @Test fun unaryUsesConnectJsonWithoutRedirectingBearerCredentials() {
        lateinit var connection: Connection
        val transport = LaneTransport("https://unit.test") { Connection(it).also { connection = it } }
        transport.rpc("lane.v1.SyncService", "PushChanges", "{\"prompt\":\"🧪\"}", "private")
        assertEquals("/lane.v1.SyncService/PushChanges", connection.url.path)
        assertEquals("application/json", connection.getRequestProperty("Content-Type"))
        assertEquals("1", connection.getRequestProperty("Connect-Protocol-Version"))
        assertEquals("Bearer private", connection.getRequestProperty("Authorization"))
        assertEquals("{\"prompt\":\"🧪\"}", connection.sent.toString("UTF-8"))
        assertFalse(connection.instanceFollowRedirects)
        assertTrue(connection.closed)
    }
    @Test fun proofExchangeAndDeletionStatusKeepBearerNative() {
        lateinit var connection: Connection
        val transport = LaneTransport("https://unit.test") { Connection(it).also { connection = it } }
        transport.json("/auth/reauth/proof", JSONObject().put("challenge_id", "challenge"), "private")
        assertEquals("POST", connection.requestMethod)
        assertEquals("challenge", JSONObject(connection.sent.toString("UTF-8")).getString("challenge_id"))
        transport.deletionStatus("old", "expired")
        assertEquals("GET", connection.requestMethod)
        assertEquals("old_epoch=old", connection.url.query)
        assertEquals("Bearer expired", connection.getRequestProperty("Authorization"))
        assertEquals(0, connection.sent.size())
        assertFalse(connection.instanceFollowRedirects)
    }
    @Test fun transportFailureCannotBecomeASuccessfulEmptyResponse() {
        lateinit var connection: Connection
        val transport = LaneTransport("https://unit.test") { Connection(it, 401).also { connection = it } }
        val failure = assertThrows(LaneTransport.Failure::class.java) {
            transport.rpc("lane.v1.SyncService", "PullChanges", "{}", "expired")
        }
        assertEquals(401, failure.status)
        assertTrue(connection.closed)
    }
    @Test fun watchDecodesConnectFramesAndClosesAtEndStream() {
        val response = ByteArrayOutputStream()
        DataOutputStream(response).use { out ->
            val cursor = "{\"cursor\":\"9007199254740993\"}".toByteArray()
            out.writeByte(0); out.writeInt(cursor.size); out.write(cursor)
            out.writeByte(2); out.writeInt(2); out.write("{}".toByteArray())
        }
        lateinit var connection: Connection
        val transport = LaneTransport("https://unit.test") { Connection(it, response = response.toByteArray()).also { connection = it } }
        var changes = 0
        transport.watch(JSONObject().put("epoch", "epoch"), "private", { true }, { changes++ })
        assertEquals(1, changes)
        assertEquals(0, connection.sent.toByteArray()[0].toInt())
        assertEquals("application/connect+json", connection.getRequestProperty("Content-Type"))
        assertTrue(connection.closed)
    }
    @Test fun invalidWatchFrameNeverAllocatesUnboundedPayload() {
        val bytes = byteArrayOf(0, 0x7f, -1, -1, -1)
        val transport = LaneTransport("https://unit.test") { Connection(it, response = bytes) }
        assertThrows(IllegalArgumentException::class.java) {
            transport.watch(JSONObject(), "private", { true }, {})
        }
    }
}
