// SPDX-License-Identifier: Apache-2.0
package io.delino.anydash

import android.content.Context
import java.io.File
import java.io.IOException
import java.net.HttpURLConnection
import java.net.URL

/**
 * Native widget snapshots in app-private storage shared with the widget
 * provider: one binary NativeWidgetSnapshot per widget. ENDED snapshots replace
 * the previous file, so the widget removes displayed data at its next update.
 */
internal class WidgetStore(context: Context) {
    private val directory = File(context.filesDir, "widget-snapshots")
    private val preferences = context.getSharedPreferences("anydash-widgets", Context.MODE_PRIVATE)

    @Synchronized fun write(encoded: List<String>, replaceAll: Boolean) {
        if (replaceAll) directory.deleteRecursively()
        directory.mkdirs()
        for (item in encoded) {
            val bytes = try { unbase64(item) } catch (_: IllegalArgumentException) { continue }
            val snapshot = try { SnapshotCodec.snapshot(bytes) } catch (_: DecodeException) { continue }
            if (snapshot.widgetId.isEmpty()) continue
            snapshot.summary?.title?.let { rememberTitle(snapshot.widgetId, it) }
            val target = File(directory, "${sha256Hex(snapshot.widgetId)}.pb")
            val temporary = File(directory, "${target.name}.tmp")
            temporary.writeBytes(bytes)
            if (!temporary.renameTo(target)) temporary.delete()
        }
    }

    @Synchronized fun replace(snapshot: NativeWidgetSnapshot, encoded: ByteArray) {
        directory.mkdirs()
        File(directory, "${sha256Hex(snapshot.widgetId)}.pb").writeBytes(encoded)
    }

    @Synchronized fun all(): List<NativeWidgetSnapshot> =
        (directory.listFiles { file -> file.extension == "pb" } ?: emptyArray())
            .mapNotNull { file -> try { SnapshotCodec.snapshot(file.readBytes()) } catch (_: Exception) { null } }
            .sortedByDescending { it.refreshedAt ?: 0L }

    /** The selected widget, or the most recently refreshed one when none is selected. */
    fun snapshot(widgetId: String?): NativeWidgetSnapshot? =
        all().let { list -> if (widgetId == null) list.firstOrNull() else list.firstOrNull { it.widgetId == widgetId } }

    fun select(appWidgetId: Int, widgetId: String) =
        preferences.edit().putString("selection.$appWidgetId", widgetId).apply()
    fun selection(appWidgetId: Int): String? = preferences.getString("selection.$appWidgetId", null)
    fun forget(appWidgetId: Int) = preferences.edit().remove("selection.$appWidgetId").apply()

    /** Remembers widget titles so an ended widget can still name what ended. */
    fun title(widgetId: String?): String =
        widgetId?.let { preferences.getString("title.$it", null) } ?: "AnyDash"
    private fun rememberTitle(widgetId: String, title: String) =
        preferences.edit().putString("title.$widgetId", title).apply()

    @Synchronized fun clear() {
        directory.deleteRecursively()
        preferences.edit().clear().apply()
    }
}

internal data class RpcResult(val status: Int, val body: ByteArray, val correlationId: String)

/** Connect binary unary calls to the AnyDash API with a Logto access token. */
internal object AnyDashApi {
    private val name = Regex("^[A-Za-z][A-Za-z0-9_.]*$")
    private val forwarded = setOf("x-anydash-client-kind", "x-anydash-client-version")

    fun validTarget(service: String, method: String) =
        service.startsWith("anydash.v1.") && service != "anydash.v1.LocalCollectorService" &&
            name.matches(service) && name.matches(method)

    /** Requests whose field 1 is `string delcore_access_token`; the host injects that token. */
    fun needsDelcoreToken(service: String, method: String) =
        "$service/$method" in setOf("anydash.v1.AccountService/ConnectAccount", "anydash.v1.DashboardService/ListSpaces")

    /**
     * Appends `delcore_access_token` (field 1, length-delimited) to a binary Protobuf request, so
     * the Delcore token never crosses into the Lynx bundle. A later occurrence of a singular field
     * wins when the server parses the message, so a value set by the bundle is replaced.
     */
    fun withDelcoreToken(body: ByteArray, token: String): ByteArray {
        val value = token.toByteArray(Charsets.UTF_8)
        val out = java.io.ByteArrayOutputStream(body.size + value.size + 6)
        out.write(body)
        out.write(0x0a)
        var remaining = value.size
        while (remaining > 0x7f) {
            out.write((remaining and 0x7f) or 0x80)
            remaining = remaining ushr 7
        }
        out.write(remaining)
        out.write(value)
        return out.toByteArray()
    }

    fun call(service: String, method: String, body: ByteArray, headers: Map<String, String>, token: String?): RpcResult {
        require(validTarget(service, method))
        val connection = URL("${BuildConfig.ANYDASH_API_URL}/$service/$method").openConnection() as HttpURLConnection
        try {
            connection.requestMethod = "POST"
            connection.connectTimeout = 15_000
            connection.readTimeout = 30_000
            connection.doOutput = true
            connection.setRequestProperty("Content-Type", "application/proto")
            connection.setRequestProperty("Connect-Protocol-Version", "1")
            for ((key, value) in headers) if (key.lowercase() in forwarded) connection.setRequestProperty(key, value)
            if (token != null) connection.setRequestProperty("Authorization", "Bearer $token")
            connection.outputStream.use { it.write(body) }
            val status = connection.responseCode
            val stream = if (status in 200..299) connection.inputStream else connection.errorStream
            val bytes = stream?.use { it.readBytes() } ?: ByteArray(0)
            val correlation = connection.getHeaderField("X-AnyDash-Correlation-Id").orEmpty()
            if (status != 200) AnyDashLog.error("anydash_rpc_failed", "method" to method, "status" to status, "correlationId" to correlation)
            return RpcResult(status, bytes, correlation)
        } catch (error: IOException) {
            AnyDashLog.error("anydash_rpc_unreachable", "method" to method)
            throw error
        } finally {
            connection.disconnect()
        }
    }
}
