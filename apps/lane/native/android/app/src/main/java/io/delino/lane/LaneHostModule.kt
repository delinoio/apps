// SPDX-License-Identifier: Apache-2.0
package io.delino.lane

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.content.Intent
import android.net.Uri
import com.lynx.jsbridge.LynxMethod
import com.lynx.jsbridge.LynxModule
import com.lynx.react.bridge.Callback
import java.net.URI

class LaneHostModule(context: Context, param: Any) : LynxModule(context, param) {
    private val session = param as LaneSession
    @LynxMethod fun copyText(text: String, callback: Callback) = run(callback) {
        val clipboard = session.runtime.application.getSystemService(ClipboardManager::class.java)
        clipboard.setPrimaryClip(ClipData.newPlainText("Lane", text))
    }
    @LynxMethod fun openURL(url: String, callback: Callback) = run(callback) {
        require(isHttps(url))
        val activity = session.activity.get() ?: error("No active window")
        activity.startActivity(Intent(Intent.ACTION_VIEW, Uri.parse(url)).addCategory(Intent.CATEGORY_BROWSABLE))
    }
    private fun run(callback: Callback, action: () -> Unit) {
        session.runtime.main.post {
            if (!session.alive) return@post
            val result = try { action(); "{\"ok\":true}" }
                catch (_: IllegalArgumentException) { BridgeJson.error("INVALID_INPUT", "Only valid HTTPS URLs are supported") }
                catch (_: Exception) { BridgeJson.error("DEPENDENCY_UNAVAILABLE", "The requested device action is unavailable") }
            callback.invoke(result)
        }
    }
    companion object {
        internal fun isHttps(value: String): Boolean = try {
            val uri = URI(value)
            uri.scheme.equals("https", ignoreCase = true) && !uri.host.isNullOrEmpty() && uri.rawUserInfo == null &&
                uri.port in -1..65535 && !value.contains('\\')
        } catch (_: Exception) { false }
    }
}
