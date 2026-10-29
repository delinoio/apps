// SPDX-License-Identifier: Apache-2.0
package io.delino.lane

import org.json.JSONObject
import java.io.DataInputStream
import java.net.HttpURLConnection
import java.net.URL
import java.net.URLEncoder

internal class LaneTransport(private val base: String = "https://lane.api.delino.io",
    private val open: (URL) -> HttpURLConnection = { it.openConnection() as HttpURLConnection }) {
    class AttachmentFailure : Exception("Signed attachment access failed; renew access and retry")
    class Failure(val status: Int, val body: String) : Exception("Lane network request failed")
    fun form(path: String, fields: Map<String, String>): JSONObject = JSONObject(exchange(path,
        fields.entries.joinToString("&") { "${encode(it.key)}=${encode(it.value)}" },
        "application/x-www-form-urlencoded", null))
    fun deletionStatus(epoch: String, token: String): JSONObject {
        val connection = connection("/auth/deletion-status?old_epoch=${encode(epoch)}", "application/json", token)
        connection.requestMethod = "GET"
        connection.doOutput = false
        try {
            val status = connection.responseCode
            val response = (if (status in 200..299) connection.inputStream else connection.errorStream)
                ?.bufferedReader(Charsets.UTF_8)?.use { it.readText() } ?: "{}"
            if (status !in 200..299) throw Failure(status, response)
            return JSONObject(response)
        } finally { connection.disconnect() }
    }
    fun json(path: String, request: JSONObject, token: String): JSONObject =
        JSONObject(exchange(path, request.toString(), "application/json", token))
    fun rpc(service: String, method: String, request: String, token: String): String =
        exchange("/$service/$method", request, "application/json", token)
    // Signed object access never carries API credentials, cookies or redirect authority.
    fun attachment(url: String): ByteArray {
        val target = URL(url)
        require(target.protocol == "https" && target.host.isNotEmpty() && target.userInfo == null && target.ref == null)
        val connection = open(target).apply {
            instanceFollowRedirects = false
            useCaches = false
            connectTimeout = 15000
            readTimeout = 30000
            requestMethod = "GET"
        }
        try {
            val status = connection.responseCode
            if (status !in 200..299) throw AttachmentFailure()
            return connection.inputStream.use { it.readBytes() }
        } finally { connection.disconnect() }
    }
    private fun connection(path: String, type: String, token: String?): HttpURLConnection {
        require(base.startsWith("https://") && path.startsWith('/'))
        return open(URL(base + path)).apply {
            instanceFollowRedirects = false
            connectTimeout = 15000
            readTimeout = 30000
            requestMethod = "POST"
            doOutput = true
            setRequestProperty("Content-Type", type)
            setRequestProperty("Connect-Protocol-Version", "1")
            if (token != null) setRequestProperty("Authorization", "Bearer $token")
        }
    }
    private fun exchange(path: String, body: String, type: String, token: String?): String {
        val connection = connection(path, type, token)
        try {
            connection.outputStream.use { it.write(body.toByteArray(Charsets.UTF_8)) }
            val status = connection.responseCode
            val response = (if (status in 200..299) connection.inputStream else connection.errorStream)
                ?.bufferedReader(Charsets.UTF_8)?.use { it.readText() } ?: "{}"
            if (status !in 200..299) throw Failure(status, response)
            return response
        } finally { connection.disconnect() }
    }
    fun watch(request: JSONObject, token: String, active: () -> Boolean, changed: () -> Unit) {
        val connection = connection("/lane.v1.SyncService/WatchChanges", "application/connect+json", token)
        try {
            val bytes = request.toString().toByteArray(Charsets.UTF_8)
            java.io.DataOutputStream(connection.outputStream).use { output ->
                output.writeByte(0); output.writeInt(bytes.size); output.write(bytes)
            }
            if (connection.responseCode != 200) throw Failure(connection.responseCode, "{}")
            DataInputStream(connection.inputStream).use { input ->
                while (active()) {
                    val flags = input.readUnsignedByte()
                    val size = input.readInt()
                    // Watch frames contain only cursors/errors, never task content.
                    require(size in 0..65536 && flags in listOf(0, 2))
                    val payload = ByteArray(size)
                    input.readFully(payload)
                    if (flags == 2) return
                    JSONObject(payload.toString(Charsets.UTF_8)).getString("cursor")
                    changed()
                }
            }
        } finally { connection.disconnect() }
    }
    companion object { fun encode(value: String): String = URLEncoder.encode(value, "UTF-8") }
}
