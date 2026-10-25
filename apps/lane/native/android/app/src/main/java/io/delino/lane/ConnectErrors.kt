// SPDX-License-Identifier: Apache-2.0
package io.delino.lane

import org.json.JSONArray
import org.json.JSONObject
import java.util.Base64

/** Decodes the frozen lane.v1.ErrorDetail wire fields without exposing server message text. */
internal object ConnectErrors {
    private val reasons = listOf("UNSPECIFIED", "INVALID_INPUT", "NOT_FOUND", "STALE_VERSION", "CONFLICT_READ_ONLY",
        "STALE_RESOLUTION", "REQUEST_ID_REUSED", "DELETED", "UNSUPPORTED_FORMAT", "STORAGE_BUSY", "STORAGE_FULL",
        "ONLINE_REQUIRED", "UNAUTHENTICATED", "FORBIDDEN", "DEPENDENCY_UNAVAILABLE", "ITEM_TOO_LARGE",
        "RATE_LIMITED", "STALE_EPOCH", "LEASE_INVALID").map { "ERROR_REASON_$it" }
    fun reason(body: String): String? = detail(body)?.optString("reason")
    fun detail(body: String): JSONObject? = try {
        val details = JSONObject(body).optJSONArray("details")
        var found: JSONObject? = null
        if (details != null) for (i in 0 until details.length()) {
            val item = details.getJSONObject(i)
            if (item.optString("type") != "lane.v1.ErrorDetail") continue
            val bytes = Base64.getDecoder().decode(item.getString("value"))
            require(bytes.size <= 65536)
            val reader = Reader(bytes)
            val result = JSONObject().put("retryable", false)
            val heads = JSONArray()
            while (reader.remaining()) {
                val tag = reader.number().toInt()
                when (tag) {
                    8 -> result.put("reason", reasons.getOrNull(reader.number().toInt()) ?: error("Unknown reason"))
                    18 -> result.put("correlationId", reader.string())
                    26 -> result.put("entityId", reader.string())
                    34 -> heads.put(reader.string())
                    40 -> result.put("retryable", reader.number() != 0L)
                    else -> reader.skip(tag and 7)
                }
            }
            if (heads.length() > 0) result.put("currentHeadIds", heads)
            if (result.has("reason") && result.getString("reason") != reasons[0]) found = result
        }
        found
    } catch (_: Exception) { null }
    private class Reader(private val bytes: ByteArray) {
        private var index = 0
        fun remaining() = index < bytes.size
        fun number(): Long {
            var value = 0L
            for (shift in 0..63 step 7) {
                require(index < bytes.size)
                val part = bytes[index++].toInt() and 255
                require(shift != 63 || part <= 1)
                value = value or ((part and 127).toLong() shl shift)
                if (part and 128 == 0) return value
            }
            error("Invalid varint")
        }
        private fun length(): Int {
            val value = number()
            require(value in 0..(bytes.size - index).toLong())
            return value.toInt()
        }
        fun string(): String {
            val size = length()
            val value = bytes.copyOfRange(index, index + size).toString(Charsets.UTF_8)
            index += size
            return value
        }
        fun skip(wire: Int) {
            when (wire) {
                0 -> number()
                1 -> { require(bytes.size - index >= 8); index += 8 }
                2 -> { val size = length(); index += size }
                5 -> { require(bytes.size - index >= 4); index += 4 }
                else -> error("Unsupported wire type")
            }
        }
    }
}
