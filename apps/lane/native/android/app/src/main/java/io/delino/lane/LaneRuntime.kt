// SPDX-License-Identifier: Apache-2.0
package io.delino.lane

import android.os.Build
import android.os.Handler
import android.os.Looper
import android.util.Log
import com.lynx.react.bridge.Callback
import com.lynx.react.bridge.JavaOnlyArray
import com.lynx.react.bridge.JavaOnlyMap
import com.lynx.tasm.LynxView
import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.lang.ref.WeakReference
import java.util.concurrent.Executors
import java.util.concurrent.ScheduledFuture
import java.util.concurrent.TimeUnit

internal class LaneSession(val runtime: LaneRuntime, activity: MainActivity) {
    val activity = WeakReference(activity)
    var view: LynxView? = null // UI thread only
    @Volatile var alive = true
    val spaces = java.util.concurrent.ConcurrentHashMap<String, String>() // worker only: space -> device
}

/** Process owner. Activity destruction releases subscriptions, not the store or workers. */
internal class LaneRuntime(val application: LaneApplication) {
    val main = Handler(Looper.getMainLooper())
    private val executor = Executors.newSingleThreadScheduledExecutor { runnable ->
        Thread(runnable, "LaneStore").apply { isDaemon = true }
    }
    private var native: NativeStore? = null
    private val sessions = mutableSetOf<LaneSession>()
    private val allSessions = mutableSetOf<LaneSession>()
    private val sequences = mutableMapOf<String, String>()
    private var polling: ScheduledFuture<*>? = null
    val notifications = LaneNotifications(application)
    val host = LaneHostServices(this)

    fun store(): NativeStore = native ?: NativeStore(
        File(application.filesDir, "io.delino.lane").absolutePath, host).also { native = it }
    fun <T> submit(action: () -> T) = executor.submit<T> { action() }
    fun execute(action: () -> Unit) { executor.execute(action) }
    fun engine(method: String, request: String) = store().invoke(method, request)
    fun workerOpen(): String = store().open("{\"formatVersion\":1,\"spaceId\":\"\"}")

    fun foreground(session: LaneSession) = execute {
        sessions.add(session)
        host.foreground()
        pollSafely()
        // Reconcile even if another lifecycle consumed poll() while no view existed.
        session.spaces.forEach { (space, _) -> sequences[space]?.let { emit(session, space, it) } }
        if (polling == null) polling = executor.scheduleWithFixedDelay(
            { pollSafely() }, 500, 500, TimeUnit.MILLISECONDS)
        host.scheduleNow()
    }
    fun background(session: LaneSession) = execute {
        sessions.remove(session)
        if (sessions.isEmpty()) { polling?.cancel(false); polling = null; host.background() }
    }
    fun destroy(session: LaneSession) {
        session.alive = false
        session.view = null
        background(session)
        execute {
            allSessions.remove(session)
            session.spaces.keys.forEach { space ->
                if (allSessions.none { it.spaces.containsKey(space) }) safe { store().release(space) }
            }
            session.spaces.clear()
        }
    }
    fun open(session: LaneSession, request: String, callback: Callback) = operation(session, callback) {
        val result = store().open(request)
        val envelope = JSONObject(result)
        if (envelope.optBoolean("ok")) {
            val response = envelope.getJSONObject("response")
            val space = response.getString("spaceId")
            allSessions.add(session)
            session.spaces[space] = response.getString("deviceId")
            sequences[space] = response.getString("sequence")
            safe { host.onOpen(space); "{}" }
            main.post { dispatchRoutes(session) }
        }
        result
    }
    fun close(session: LaneSession, space: String, callback: Callback) = operation(session, callback) {
        session.spaces.remove(space)
        if (allSessions.none { it !== session && it.spaces.containsKey(space) }) store().release(space)
        else BridgeJson.success(sequence = sequences[space] ?: "0")
    }
    fun invoke(session: LaneSession, method: String, raw: String, callback: Callback) {
        if (method !in LaneMethods.allowed) {
            deliver(session, callback, BridgeJson.error("INVALID_INPUT", "Unknown Lane method")); return
        }
        operation(session, callback) {
            val wrapper = JSONObject(raw)
            val space = wrapper.getString("spaceId")
            val device = session.spaces[space] ?: return@operation BridgeJson.error(
                "NOT_FOUND", "Open this data space before using it")
            val request = wrapper.getJSONObject("request")
            LaneMethods.appRequest(method, request, device, Build.MODEL)
            val result = engine(method, wrapper.toString())
            if (JSONObject(result).optBoolean("ok")) {
                when (method) {
                    "SetNotificationPreferences" -> {
                        safe { host.refreshPush(space); "{}" }
                    }
                    "DisconnectSync", "RemoveDeviceData" -> safe { host.onDisconnect(space); "{}" }
                }
                if (method in LaneMethods.mutations) safe { host.scheduleNow(); "{}" }
            }
            decorate(method, result)
        }
    }
    private fun decorate(method: String, raw: String): String {
        if (method !in setOf("GetLocalStatus", "GetNotificationPreferences", "SetNotificationPreferences")) return raw
        val envelope = JSONObject(raw)
        if (!envelope.optBoolean("ok")) return raw
        val response = envelope.getJSONObject("response")
        val preferences = if (method == "GetLocalStatus") response.optJSONObject("notificationPreferences") else response
        preferences?.put("permission", notifications.permission())
        return envelope.toString()
    }
    private fun operation(session: LaneSession, callback: Callback, action: () -> String) = execute {
        if (!session.alive) return@execute
        val result = safe(action)
        deliver(session, callback, result)
        pollSafely()
    }
    fun safe(action: () -> String): String = try { action() }
        catch (_: org.json.JSONException) { BridgeJson.error("INVALID_INPUT", "Invalid Lane request") }
        catch (_: LinkageError) { BridgeJson.error("DEPENDENCY_UNAVAILABLE", "Native Lane storage is unavailable") }
        catch (_: Exception) {
            native?.log("lane_host_operation_failed")
            BridgeJson.error("DEPENDENCY_UNAVAILABLE", "Lane operation failed; retry", true)
        }
    fun deliver(session: LaneSession, callback: Callback, result: String) {
        // Lynx CallbackImpl schedules through the JS bridge; serialize lifetime checks on UI.
        main.post { if (session.alive) callback.invoke(result) }
    }
    fun pollSafely() {
        safe {
            val changes = JSONArray(store().poll())
            for (i in 0 until changes.length()) {
                val change = changes.getJSONObject(i)
                val space = change.getString("spaceId")
                val sequence = change.getString("sequence")
                sequences[space] = sequence
                sessions.forEach { emit(it, space, sequence) }
                safe { host.onChange(space); "{}" }
            }
            "{}"
        }
    }
    fun requestPermission() {
        val session = sessions.firstOrNull { it.alive } ?: return
        main.post { if (session.alive) session.activity.get()?.requestLaneNotificationPermission() }
    }
    fun invalidatePermissions() = execute {
        sessions.forEach { session -> session.spaces.forEach { (space, _) ->
            safe { store().permission(space, notifications.permission()) }
            safe { host.refreshPush(space); "{}" }
            emit(session, space, sequences[space] ?: "0")
        } }
    }
    private val pendingLinks = java.util.ArrayDeque<LaneLinks.Route>() // UI thread only
    fun receiveLink(value: String) {
        val route = LaneLinks.parse(value, if (BuildConfig.DEBUG) "lane-dev" else "lane") ?: return
        if (route.kind == LaneLinks.Kind.AUTH) { host.authorizationCallback(value); return }
        pendingLinks.addLast(route)
        execute { sessions.firstOrNull { it.alive && it.spaces.isNotEmpty() }?.let { session ->
            main.post { dispatchRoutes(session) }
        } }
    }
    private fun dispatchRoutes(session: LaneSession) {
        if (!session.alive || session.spaces.isEmpty()) return
        val view = session.view ?: return
        while (pendingLinks.isNotEmpty()) {
            val route = pendingLinks.removeFirst()
            val argument = JavaOnlyMap().apply { putString("kind", route.kind.name.lowercase()) }
            if (route.kind != LaneLinks.Kind.OPEN) argument.putString("id", java.net.URI(route.url).path.removePrefix("/"))
            java.net.URI(route.url).rawQuery?.removePrefix("spaceId=")?.let { argument.putString("spaceId", it) }
            view.sendGlobalEvent("lane.openRoute", JavaOnlyArray.of(argument))
        }
    }
    fun publishHostError(reason: String, message: String) = execute {
        sessions.toList().forEach { session -> main.post {
            if (session.alive) session.view?.sendGlobalEvent("lane.hostError", JavaOnlyArray.of(
                JavaOnlyMap().apply { putString("reason", "ERROR_REASON_$reason"); putString("message", message) }))
        } }
    }
    private fun emit(session: LaneSession, space: String, sequence: String) {
        if (!session.spaces.containsKey(space)) return
        main.post {
            if (session.alive && session.spaces.containsKey(space)) session.view?.sendGlobalEvent("lane.storeChanged", JavaOnlyArray.of(
                JavaOnlyMap().apply { putString("spaceId", space); putString("sequence", sequence) }))
        }
    }
}
