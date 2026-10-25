// SPDX-License-Identifier: Apache-2.0
package io.delino.lane

import java.net.URI
import java.net.URLDecoder

internal object LaneLinks {
    private val id = Regex("[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}")
    fun validId(value: String) = id.matches(value)
    enum class Kind { OPEN, REPOSITORY, TASK, AUTH }
    data class Route(val kind: Kind, val url: String)
    fun parse(value: String, scheme: String): Route? = try {
        val uri = URI(value)
        if (uri.scheme != scheme || uri.rawUserInfo != null || uri.port != -1 || uri.fragment != null) null
        else when (uri.host) {
            "open" -> if (uri.path in listOf("", "/") && uri.query == null) Route(Kind.OPEN, value) else null
            "auth" -> if (uri.path == "/callback" && uri.query != null) Route(Kind.AUTH, value) else null
            "repository", "task" -> {
                val target = uri.path.removePrefix("/")
                val query = uri.rawQuery
                val validSpace = query == null || (query.startsWith("spaceId=") && validId(query.removePrefix("spaceId=")))
                if (validId(target) && validSpace) Route(if (uri.host == "task") Kind.TASK else Kind.REPOSITORY, value) else null
            }
            else -> null
        }
    } catch (_: Exception) { null }
    fun query(value: String): Map<String, String> {
        val query = URI(value).rawQuery ?: return emptyMap()
        val result = mutableMapOf<String, String>()
        query.split('&').forEach { pair ->
            val parts = pair.split('=', limit = 2)
            require(parts.size == 2)
            val key = URLDecoder.decode(parts[0], "UTF-8")
            require(key !in result)
            result[key] = URLDecoder.decode(parts[1], "UTF-8")
        }
        return result
    }
}
