// SPDX-License-Identifier: Apache-2.0
package io.delino.lane

import android.os.Build
import android.util.Base64
import androidx.work.*
import org.json.JSONObject
import java.security.MessageDigest
import java.security.SecureRandom
import java.util.UUID
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit

internal class LaneHostServices(private val runtime: LaneRuntime) {
    private val credentials = LaneCredentials(runtime.application)
    private val transport = LaneTransport()
    private val network = Executors.newCachedThreadPool()
    private val claimant = uuid7()
    private val watching = java.util.concurrent.ConcurrentHashMap.newKeySet<String>()
    @Volatile private var visible = false
    private val scheme = if (BuildConfig.DEBUG) "lane-dev" else "lane"
    private val redirect = "$scheme://auth/callback"
    private val clientId = "io.delino.lane.android"

    fun call(operation: Int, space: String, method: String, request: String): String = when (operation) {
        0 -> remote(space, method, request)
        1 -> authorize(request)
        2 -> { runtime.requestPermission(); runtime.notifications.permission() }
        3 -> { onDisconnect(space); "{}" }
        else -> error("Unknown native hook")
    }
    fun onOpen(space: String) {
        runtime.store().permission(space, runtime.notifications.permission())
        onChange(space)
    }
    fun onChange(space: String) {
        drainNotifications(space)
        val status = BridgeJson.response(runtime.engine("GetLocalStatus", BridgeJson.request(space)))
        if (status.optBoolean("syncEnabled") && credentials.get(space) != null) {
            schedulePeriodic()
            scheduleNow()
            startWatch(space)
        }
    }
    private fun drainNotifications(space: String) {
        if (runtime.notifications.permission() != "NOTIFICATION_PERMISSION_GRANTED") return
        val envelope = JSONObject(runtime.store().claim(space, claimant))
        if (!envelope.optBoolean("ok")) return
        val items = envelope.getJSONObject("response").optJSONArray("items") ?: return
        for (i in 0 until items.length()) {
            val item = items.getJSONObject(i)
            if (runtime.notifications.deliver(space, item)) runtime.store().acknowledge(space, item.getString("proposalId"))
        }
    }
    fun foreground() {
        visible = true
        credentials.spaces().forEach { startWatch(it) }
    }
    fun background() { visible = false }
    fun scheduleNow() {
        if (credentials.spaces().isEmpty()) return
        WorkManager.getInstance(runtime.application).enqueueUniqueWork("lane-sync-now",
            ExistingWorkPolicy.KEEP, OneTimeWorkRequest.Builder(LaneSyncWorker::class.java)
                .setConstraints(Constraints.Builder().setRequiredNetworkType(NetworkType.CONNECTED).build()).build())
    }
    private fun schedulePeriodic() {
        WorkManager.getInstance(runtime.application).enqueueUniquePeriodicWork("lane-sync-periodic",
            ExistingPeriodicWorkPolicy.KEEP, PeriodicWorkRequest.Builder(LaneSyncWorker::class.java, 15, TimeUnit.MINUTES)
                .setConstraints(Constraints.Builder().setRequiredNetworkType(NetworkType.CONNECTED).build()).build())
    }
    fun onDisconnect(space: String) {
        val token = credentials.get(space)
        // Erase locally even when offline; no subsequent worker may read this credential.
        credentials.remove(space)
        if (credentials.spaces().isEmpty()) {
            WorkManager.getInstance(runtime.application).cancelUniqueWork("lane-sync-periodic")
            WorkManager.getInstance(runtime.application).cancelUniqueWork("lane-sync-now")
        }
        if (token != null) network.execute {
            try { transport.form("/oauth/revoke", mapOf("token" to token.getString("refresh_token"), "client_id" to clientId)) }
            catch (_: Exception) { /* Local disconnection remains effective while server revocation is unavailable. */ }
        }
    }
    private fun authorize(account: String): String {
        val verifier = randomToken()
        val state = randomToken()
        val challenge = Base64.encodeToString(MessageDigest.getInstance("SHA-256")
            .digest(verifier.toByteArray(Charsets.US_ASCII)), Base64.URL_SAFE or Base64.NO_WRAP or Base64.NO_PADDING)
        credentials.put("pending-authorization", JSONObject().put("verifier", verifier).put("state", state)
            .put("accountId", account).put("createdAt", System.currentTimeMillis()))
        val fields = linkedMapOf("response_type" to "code", "client_id" to clientId, "redirect_uri" to redirect,
            "state" to state, "code_challenge" to challenge, "code_challenge_method" to "S256", "device_name" to Build.MODEL)
        val url = "https://lane.api.delino.io/oauth/authorize?" + fields.entries.joinToString("&") {
            "${LaneTransport.encode(it.key)}=${LaneTransport.encode(it.value)}"
        }
        return JSONObject().put("authorizationUrl", url).put("challengeId", state).toString()
    }
    fun authorizationCallback(url: String) = network.execute {
        try {
            val route = LaneLinks.parse(url, scheme)
            require(route?.kind == LaneLinks.Kind.AUTH)
            val parameters = LaneLinks.query(url)
            val pending = credentials.get("pending-authorization") ?: error("No authorization")
            require(System.currentTimeMillis() - pending.getLong("createdAt") in 0..600000)
            require(MessageDigest.isEqual(parameters.getValue("state").toByteArray(Charsets.UTF_8),
                pending.getString("state").toByteArray(Charsets.UTF_8)))
            // Consume before exchange. A duplicate callback can never approve another space.
            credentials.remove("pending-authorization")
            val token = transport.form("/oauth/token", mapOf("grant_type" to "authorization_code",
                "code" to parameters.getValue("code"), "client_id" to clientId,
                "redirect_uri" to redirect, "code_verifier" to pending.getString("verifier")))
            require(token.getString("token_type").equals("Bearer", true))
            val requestedAccount = pending.getString("accountId")
            require(requestedAccount.isEmpty() || requestedAccount == token.getString("account_id"))
            val info = JSONObject(transport.rpc("lane.v1.SyncService", "GetSyncInfo", "{}", token.getString("access_token")))
            require(info.getInt("formatVersion") == 1)
            require(info.getString("accountId") == token.getString("account_id") &&
                info.getString("tenantId") == token.getString("tenant_id"))
            val identity = JSONObject().put("issuer", token.getString("issuer"))
                .put("accountId", info.getString("accountId")).put("tenantId", info.getString("tenantId"))
                .put("epoch", info.getString("epoch"))
            token.put("expiresAt", System.currentTimeMillis() + token.getLong("expires_in") * 1000)
            runtime.submit {
                // ConnectSync and the browser approval are the explicit initial merge approval.
                val result = BridgeJson.response(runtime.store().approve(identity.toString(), true))
                credentials.put(result.getString("spaceId"), token)
                runtime.pollSafely()
            }.get()
            schedulePeriodic(); scheduleNow()
        } catch (_: Exception) {
            runtime.publishHostError("UNAUTHENTICATED", "Connection approval failed; connect again")
        }
    }
    @Synchronized private fun accessToken(space: String): String {
        var token = credentials.get(space) ?: error("No credential")
        if (token.optLong("expiresAt") <= System.currentTimeMillis() + 60000) {
            val previous = token
            token = transport.form("/oauth/token", mapOf("grant_type" to "refresh_token",
                "refresh_token" to token.getString("refresh_token"), "client_id" to clientId))
            require(token.getString("account_id") == previous.getString("account_id") &&
                token.getString("tenant_id") == previous.getString("tenant_id") &&
                token.getString("issuer") == previous.getString("issuer"))
            require(credentials.get(space) != null) // Disconnection can race a network response.
            token.put("expiresAt", System.currentTimeMillis() + token.getLong("expires_in") * 1000)
            credentials.put(space, token)
        }
        return token.getString("access_token")
    }
    private fun remote(space: String, method: String, request: String): String {
        if (credentials.get(space) == null) return BridgeJson.error("ONLINE_REQUIRED", "Connect synchronization to use this operation")
        val service = when (method) {
            "ListDevices", "DisconnectDevice", "ListConnections", "RevokeConnection", "RegisterPush", "BeginDataDeletion", "DeleteAccountData" -> "AccountService"
            "ListReceivers", "CreateReceiver", "SaveReceiver", "RotateReceiverKey", "DeleteReceiver", "ListEvents", "GetEvent",
            "DeleteEvent", "ClaimEvent", "RenewEventLease", "ReleaseEventLease", "CompleteEvent", "ReopenEvent" -> "WebhookService"
            else -> "LaneService"
        }
        return try { transport.rpc("lane.v1.$service", method, request, accessToken(space)) }
        catch (failure: LaneTransport.Failure) {
            when (failure.status) {
                401 -> BridgeJson.error("UNAUTHENTICATED", "Reconnect Lane synchronization")
                403 -> BridgeJson.error("FORBIDDEN", "This device cannot access the account")
                else -> BridgeJson.error("DEPENDENCY_UNAVAILABLE", "Lane server request failed", true)
            }
        } catch (_: Exception) { BridgeJson.error("ONLINE_REQUIRED", "Lane server is unavailable", true) }
    }
    fun synchronize(stopped: () -> Boolean): Boolean {
        var success = true
        for (space in credentials.spaces()) {
            if (stopped()) return false
            val owner = uuid7()
            try {
                val acquired = runtime.submit { BridgeJson.response(runtime.store().acquire(space, owner, 60)).getBoolean("acquired") }.get()
                if (!acquired) continue
                try {
                    var more = true
                    while (more && !stopped()) {
                        val pending = runtime.submit {
                            check(BridgeJson.response(runtime.store().acquire(space, owner, 60)).getBoolean("acquired"))
                            BridgeJson.response(runtime.store().pending(space, owner))
                        }.get()
                        val token = accessToken(space)
                        val mutations = pending.getJSONArray("mutations")
                        if (mutations.length() > 0) {
                            val push = JSONObject().put("formatVersion", 1).put("epoch", pending.getString("epoch"))
                                .put("deviceId", pending.getString("deviceId")).put("mutations", mutations)
                            val response = JSONObject(transport.rpc("lane.v1.SyncService", "PushChanges", push.toString(), token))
                            runtime.submit { BridgeJson.response(runtime.store().apply(space, JSONObject()
                                .put("formatVersion", 1).put("epoch", pending.getString("epoch")).put("owner", owner)
                                .put("push", response).toString())) }.get()
                        }
                        if (stopped() || credentials.get(space) == null) break
                        runtime.submit { check(BridgeJson.response(runtime.store().acquire(space, owner, 60)).getBoolean("acquired")) }.get()
                        val pull = JSONObject().put("formatVersion", 1).put("epoch", pending.getString("epoch"))
                            .put("cursor", pending.optString("cursor")).put("pageSize", 100)
                        val response = JSONObject(transport.rpc("lane.v1.SyncService", "PullChanges", pull.toString(), token))
                        runtime.submit {
                            BridgeJson.response(runtime.store().apply(space, JSONObject().put("formatVersion", 1)
                                .put("epoch", pending.getString("epoch")).put("owner", owner).put("pull", response).toString()))
                            runtime.pollSafely()
                        }.get()
                        more = response.optBoolean("hasMore")
                    }
                } finally { runtime.submit { runtime.store().acquire(space, owner, 0) }.get() }
            } catch (_: Exception) { success = false }
        }
        return success
    }
    private fun startWatch(space: String) {
        if (!visible || !watching.add(space)) return
        network.execute {
            try {
                while (visible && credentials.get(space) != null) {
                    try {
                        val info = JSONObject(transport.rpc("lane.v1.SyncService", "GetSyncInfo", "{}", accessToken(space)))
                        transport.watch(JSONObject().put("epoch", info.getString("epoch")), accessToken(space),
                            { visible && credentials.get(space) != null }, { scheduleNow() })
                    } catch (_: Exception) { /* Reconnect from the durable pull cursor through the sync worker. */ }
                    if (visible) { scheduleNow(); Thread.sleep(5000) }
                }
            } finally { watching.remove(space) }
        }
    }
    fun refreshAllPush() { credentials.spaces().forEach { space -> runtime.safe { refreshPush(space); "{}" } } }
    fun refreshPush(space: String) { LanePush.register(runtime, space, ::remote) }
    companion object {
        private fun randomToken(): String = Base64.encodeToString(ByteArray(32).also { SecureRandom().nextBytes(it) },
            Base64.URL_SAFE or Base64.NO_PADDING or Base64.NO_WRAP)
        fun uuid7(): String {
            val random = SecureRandom()
            val time = System.currentTimeMillis() and 0xffffffffffffL
            val high = (time shl 16) or 0x7000L or (random.nextInt(4096).toLong())
            val low = (random.nextLong() and 0x3fffffffffffffffL) or Long.MIN_VALUE
            return UUID(high, low).toString()
        }
    }
}
