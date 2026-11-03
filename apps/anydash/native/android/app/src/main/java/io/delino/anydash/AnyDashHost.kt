// SPDX-License-Identifier: Apache-2.0
package io.delino.anydash

import android.app.Activity
import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.content.Intent
import android.content.res.Configuration
import android.net.Uri
import android.os.Handler
import android.os.Looper
import com.lynx.jsbridge.LynxMethod
import com.lynx.jsbridge.LynxModule
import com.lynx.react.bridge.Callback
import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.lang.ref.WeakReference
import java.net.URI
import java.util.concurrent.Executors

/**
 * The Android implementation of the AnyDashHost contract (src/host.ts): Logto
 * PKCE sign-in, the Keystore session store, API requests, the persisted widget
 * cache and App Widget snapshots. The app is a viewer only, so collector RPCs
 * and desktop actions are rejected.
 */
internal class AnyDashHost(private val context: Context) {
    val store = SessionStore(context)
    val auth = LogtoAuth(store)
    val widgets = WidgetStore(context)
    val main = Handler(Looper.getMainLooper())
    private val executor = Executors.newFixedThreadPool(4) { runnable ->
        Thread(runnable, "AnyDashHost").apply { isDaemon = true }
    }
    private val cacheDirectory = File(context.filesDir, "cache-v1")
    var activity = WeakReference<Activity>(null)
    private var pendingSignIn: ((String) -> Unit)? = null
    private var browserOpened = false
    private val sessionGate = SessionGate()
    /** Notifies the current bundle that the session changed. Main thread. */
    @Volatile var sessionChanged: (() -> Unit)? = null

    fun call(method: String, raw: String, reply: (String) -> Unit) {
        executor.execute {
            val payload = try { if (raw.isEmpty()) JSONObject() else JSONObject(raw) } catch (_: Exception) { null }
            val result = if (payload == null) failure("Invalid host request.") else try {
                dispatch(method, payload, reply) ?: return@execute
            } catch (error: AuthException) {
                failure(error.message ?: "Sign in to AnyDash.")
            } catch (error: Exception) {
                AnyDashLog.error("anydash_host_failed", "method" to method)
                failure("The host operation failed.")
            }
            reply(result)
        }
    }

    /** Returns the reply, or null when [reply] is called later. */
    private fun dispatch(method: String, payload: JSONObject, reply: (String) -> Unit): String? = when (method) {
        "info" -> success(JSONObject()
            .put("platform", "android")
            .put("collector", false)
            .put("platformLabel", if (tablet()) "Android tablet" else "Android")
            .put("hostVersion", BuildConfig.VERSION_NAME))
        "session" -> {
            sessionGate.read { reply(session()) }
            null
        }
        "signIn" -> { signIn(reply); null }
        "signOut" -> {
            auth.signOut()
            cacheDirectory.deleteRecursively()
            widgets.clear()
            AnyDashWidgetProvider.updateAll(context)
            success(JSONObject())
        }
        "rpc" -> rpc(payload)
        "copyText" -> {
            val text = payload.optString("text")
            main.post {
                context.getSystemService(ClipboardManager::class.java).setPrimaryClip(ClipData.newPlainText("AnyDash", text))
            }
            success(JSONObject())
        }
        "openURL" -> {
            val url = payload.optString("url")
            if (!isHttps(url)) failure("Only HTTPS links can be opened.") else {
                main.post {
                    val owner = activity.get()
                    val intent = Intent(Intent.ACTION_VIEW, Uri.parse(url)).addCategory(Intent.CATEGORY_BROWSABLE)
                    if (owner != null) owner.startActivity(intent)
                    else context.startActivity(intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
                }
                success(JSONObject())
            }
        }
        "cacheRead" -> {
            val file = cacheFile(payload.optString("key"))
            success(if (file.isFile) JSONObject().put("value", file.readText()) else JSONObject())
        }
        "cacheWrite" -> {
            val key = payload.optString("key")
            synchronized(cacheDirectory) {
                cacheDirectory.mkdirs()
                val index = cacheIndex().put(key, true)
                File(cacheDirectory, "index.json").writeText(index.toString())
                cacheFile(key).writeText(payload.optString("value"))
            }
            success(JSONObject())
        }
        "cacheRemove" -> {
            val prefix = payload.optString("prefix")
            synchronized(cacheDirectory) {
                val index = cacheIndex()
                for (key in index.keys().asSequence().toList()) if (key.startsWith(prefix)) {
                    cacheFile(key).delete()
                    index.remove(key)
                }
                if (cacheDirectory.isDirectory) File(cacheDirectory, "index.json").writeText(index.toString())
            }
            success(JSONObject())
        }
        "widgetSnapshots" -> {
            val list = payload.optJSONArray("snapshots") ?: JSONArray()
            widgets.write((0 until list.length()).map { list.optString(it) }, payload.optBoolean("replaceAll"))
            AnyDashWidgetProvider.updateAll(context)
            success(JSONObject())
        }
        "route" -> success(JSONObject())
        else -> failure("This action is available in the desktop app.")
    }

    private fun rpc(payload: JSONObject): String {
        val service = payload.optString("service")
        val method = payload.optString("method")
        if (payload.optString("target") != "api" || !AnyDashApi.validTarget(service, method))
            return failure("Invalid RPC target.")
        var body = unbase64(payload.optString("body"))
        val token = if (auth.signedIn) {
            try {
                if (AnyDashApi.needsDelcoreToken(service, method))
                    body = AnyDashApi.withDelcoreToken(body, auth.accessToken(BuildConfig.DELCORE_RESOURCE))
                auth.accessToken(BuildConfig.ANYDASH_RESOURCE)
            } catch (_: AuthException) {
                return success(JSONObject().put("status", 401).put("body", ""))
            }
        } else null
        val headers = payload.optJSONObject("headers")?.let { json ->
            json.keys().asSequence().associateWith { json.optString(it) }
        } ?: emptyMap()
        val result = try {
            AnyDashApi.call(service, method, body, headers, token)
        } catch (_: java.io.IOException) {
            return failure("The network request failed.")
        }
        return success(JSONObject().put("status", result.status).put("body", base64(result.body))
            .put("correlationId", result.correlationId))
    }

    private fun signIn(reply: (String) -> Unit) {
        val url = auth.startSignIn()
        main.post {
            pendingSignIn?.invoke(failure("Sign-in was cancelled."))
            pendingSignIn = reply
            browserOpened = true
            val owner = activity.get()
            val intent = Intent(Intent.ACTION_VIEW, Uri.parse(url)).addCategory(Intent.CATEGORY_BROWSABLE)
            if (owner != null) owner.startActivity(intent)
            else context.startActivity(intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
        }
    }

    private fun session(): String {
        val signedIn = auth.signedIn
        sessionGate.answered(signedIn)
        return success(if (signedIn) JSONObject().put("signedIn", true).put("userKey", auth.userKey ?: "")
            else JSONObject().put("signedIn", false))
    }

    /**
     * anydash://auth/callback from the browser. Main thread. After Android ends
     * the process during sign-in, the callback starts a new process without a
     * pending `signIn` reply; the session gate then holds the bundle's session
     * read until the exchange ends, or notifies a bundle that read "signed out".
     */
    fun completeSignIn(callback: Uri) {
        val reply = pendingSignIn
        pendingSignIn = null
        browserOpened = false
        sessionGate.begin()
        executor.execute {
            var signedIn = false
            val result = try {
                auth.completeSignIn(callback)
                signedIn = true
                success(JSONObject())
            } catch (error: AuthException) {
                failure(error.message ?: "Sign-in was cancelled.")
            } catch (_: Exception) {
                failure("Sign-in was cancelled.")
            }
            reply?.invoke(result)
            if (sessionGate.finish(signedIn, replied = reply != null)) {
                AnyDashLog.info("anydash_recovered_sign_in_notified")
                main.post { sessionChanged?.invoke() }
            }
        }
    }

    /** Returning to the app without a callback means the user left the sign-in page. Main thread. */
    fun resumed() {
        if (!browserOpened) return
        browserOpened = false
        val reply = pendingSignIn ?: return
        pendingSignIn = null
        auth.cancelSignIn()
        reply(failure("Sign-in was cancelled."))
    }

    private fun tablet() =
        (context.resources.configuration.screenLayout and Configuration.SCREENLAYOUT_SIZE_MASK) >= Configuration.SCREENLAYOUT_SIZE_LARGE

    private fun cacheFile(key: String) = File(cacheDirectory, "${sha256Hex(key)}.bin")
    private fun cacheIndex(): JSONObject = try {
        JSONObject(File(cacheDirectory, "index.json").readText())
    } catch (_: Exception) { JSONObject() }

    companion object {
        fun success(value: JSONObject): String = JSONObject().put("ok", true).put("value", value).toString()
        fun failure(message: String): String =
            JSONObject().put("ok", false).put("error", JSONObject().put("message", message)).toString()
        fun isHttps(value: String): Boolean = try {
            val uri = URI(value)
            uri.scheme.equals("https", ignoreCase = true) && !uri.host.isNullOrEmpty() && uri.rawUserInfo == null &&
                !value.contains('\\')
        } catch (_: Exception) { false }
    }
}

/** The `AnyDashHost` native module the shared client calls (src/host.ts). */
class AnyDashHostModule(context: Context, param: Any?) : LynxModule(context, param) {
    private val host = (context.applicationContext as AnyDashApplication).host
    @LynxMethod fun call(method: String, payload: String, callback: Callback) {
        host.call(method, payload) { reply -> host.main.post { callback.invoke(reply) } }
    }
}
