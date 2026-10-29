// SPDX-License-Identifier: Apache-2.0
package io.delino.lane

import android.os.Build
import androidx.work.*
import org.json.JSONObject
import java.security.SecureRandom
import java.util.UUID
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit

internal class LaneHostServices(private val runtime: LaneRuntime) {
    private val credentials = LaneCredentials(runtime.application)
    private val transport = LaneTransport(BuildConfig.LANE_API_BASE_URL)
    private val network = Executors.newCachedThreadPool()
    private val claimant = uuid7()
    private val watching = java.util.concurrent.ConcurrentHashMap.newKeySet<String>()
    @Volatile private var visible = false
    private val scheme = if (BuildConfig.DEBUG) "lane-dev" else "lane"
    private val redirect = "$scheme://auth/callback"
    private var connectionIntent: JSONObject? = null // serial storage worker only
    private data class Reauthentication(val session: LaneSession, val space: String, val binding: JSONObject)
    private val reauth = java.util.concurrent.ConcurrentHashMap<String, Reauthentication>()
    private var uiSession: LaneSession? = null
    private var uiChallenge: String? = null
    fun beginUiCall(session: LaneSession, space: String, method: String, request: JSONObject) {
        uiSession = session
        uiChallenge = if (method == "DeleteAccountData") ReauthenticationBinding.stripChallenge(request) else null
        if (method == "BeginDataDeletion" || method == "ConnectSync") {
            cancelSession(session)
            reauth.entries.filter { it.value.space == space }.forEach { cancelReauthentication(it.value.session, it.key) }
        }
    }
    fun endUiCall() { uiSession = null; uiChallenge = null }
    fun isReauthenticationActive(session: LaneSession, challenge: String): Boolean =
        reauth[challenge]?.session === session
    fun cancelReauthentication(session: LaneSession, challenge: String) {
        if (reauth[challenge]?.session === session) {
            reauth.remove(challenge)
            credentials.remove("reauth-$challenge")
        }
    }
    fun cancelSession(session: LaneSession) {
        reauth.entries.filter { it.value.session === session }.forEach { cancelReauthentication(session, it.key) }
    }
    private fun reauthenticationCallback(challenge: String) {
        // All binding and proof changes run on the same worker as close, cancel and deletion.
        runtime.execute {
            try {
                val pending = reauth[challenge] ?: error("No pending challenge")
                val token = credentials.get(pending.space) ?: error("No credential")
                ReauthenticationBinding.validate(pending.binding, pending.session.generation, pending.space,
                    token, pending.binding.getString("mode"), challenge, System.currentTimeMillis())
                require(pending.session.alive && pending.session.spaces.containsKey(pending.space))
                require(credentials.get("reauth-$challenge") == null)
                val result = transport.json("/auth/reauth/proof", JSONObject().put("challenge_id", challenge), accessToken(pending.space))
                val proof = result.getString("reauth_proof")
                require(proof.isNotEmpty())
                // A destroyed view can invalidate the flow while network I/O is in progress.
                require(pending.session.alive)
                credentials.put("reauth-$challenge", JSONObject().put("proof", proof).put("createdAt", System.currentTimeMillis()))
                runtime.reauthenticated(pending.session, pending.space, challenge)
            } catch (_: Exception) {
                runtime.publishHostError("UNAUTHENTICATED", "Deletion verification expired or was canceled; start again")
            }
        }
    }
    private val clientId = BuildConfig.LANE_OAUTH_CLIENT_ID

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
        if (status.optBoolean("syncEnabled") && !status.optBoolean("reconnectRequired") && credentials.get(space) != null) {
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
    fun scheduleNow(space: String? = null) {
        if (credentials.spaces().isEmpty()) return
        WorkManager.getInstance(runtime.application).enqueueUniqueWork(if (space == null) "lane-sync-now" else "lane-sync-$space",
            ExistingWorkPolicy.KEEP, OneTimeWorkRequest.Builder(LaneSyncWorker::class.java)
                .setInputData(Data.Builder().apply { if (space != null) putString("spaceId", space) }.build())
                .setConstraints(Constraints.Builder().setRequiredNetworkType(NetworkType.CONNECTED).build()).build())
    }
    private fun schedulePeriodic() {
        WorkManager.getInstance(runtime.application).enqueueUniquePeriodicWork("lane-sync-periodic",
            ExistingPeriodicWorkPolicy.KEEP, PeriodicWorkRequest.Builder(LaneSyncWorker::class.java, 15, TimeUnit.MINUTES)
                .setConstraints(Constraints.Builder().setRequiredNetworkType(NetworkType.CONNECTED).build()).build())
    }
    fun onDisconnect(space: String) {
        reauth.entries.filter { it.value.space == space }.forEach { cancelReauthentication(it.value.session, it.key) }
        val token = credentials.take(space)
        // Erase locally even when offline; no subsequent worker may read this credential.
        WorkManager.getInstance(runtime.application).cancelUniqueWork("lane-sync-$space")
        if (credentials.spaces().isEmpty()) {
            WorkManager.getInstance(runtime.application).cancelUniqueWork("lane-sync-periodic")
            WorkManager.getInstance(runtime.application).cancelUniqueWork("lane-sync-now")
        }
        if (token != null) network.execute {
            try { transport.form("/oauth/revoke", mapOf("token" to token.getString("refresh_token"), "client_id" to clientId)) }
            catch (_: Exception) { /* Local disconnection remains effective while server revocation is unavailable. */ }
        }
    }
    /** Quiesces this app's work for a space before an explicit development reset. It never signs out. */
    fun beforeReset(space: String) {
        reauth.entries.filter { space.isEmpty() || it.value.space == space }.forEach { cancelReauthentication(it.value.session, it.key) }
        if (space.isNotEmpty()) WorkManager.getInstance(runtime.application).cancelUniqueWork("lane-sync-$space")
    }
    fun captureConnectionIntent(space: String) {
        val status = JSONObject(runtime.engine("GetLocalStatus", BridgeJson.request(space)))
        val body = BridgeJson.response(status.toString())
        connectionIntent = JSONObject().put("spaceId", space).put("deviceId", body.getString("deviceId"))
            .put("accountId", body.optString("accountId")).put("sequence", status.getString("sequence"))
            .put("reconnect", body.optBoolean("reconnectRequired"))
        if (body.optBoolean("reconnectRequired")) {
            val owner = uuid7()
            check(BridgeJson.response(runtime.store().acquire(space, owner, 60)).getBoolean("acquired"))
            try { connectionIntent!!.put("epoch", BridgeJson.response(runtime.store().pending(space, owner)).getString("epoch")) }
            finally { runtime.store().acquire(space, owner, 0) }
        }
    }
    fun clearConnectionIntent() { connectionIntent = null }
    private fun authorize(account: String): String {
        val binding = connectionIntent ?: error("No pinned connection request")
        if (binding.optBoolean("reconnect")) require(account.isNotEmpty() && account == binding.getString("accountId"))
        val verifier = OAuthProof.randomToken()
        val state = OAuthProof.randomToken()
        val challenge = OAuthProof.challenge(verifier)
        credentials.put("pending-authorization", JSONObject().put("verifier", verifier).put("state", state)
            .put("accountId", account).put("createdAt", System.currentTimeMillis()).put("binding", binding))
        val fields = linkedMapOf("response_type" to "code", "client_id" to clientId, "redirect_uri" to redirect,
            "state" to state, "code_challenge" to challenge, "code_challenge_method" to "S256", "device_name" to Build.MODEL, "device_id" to binding.getString("deviceId"))
        val url = BuildConfig.LANE_API_BASE_URL + "/oauth/authorize?" + fields.entries.joinToString("&") {
            "${LaneTransport.encode(it.key)}=${LaneTransport.encode(it.value)}"
        }
        return JSONObject().put("authorizationUrl", url).put("challengeId", state).toString()
    }
    fun authorizationCallback(url: String) = network.execute {
        try {
            val route = LaneLinks.parse(url, scheme)
            require(route?.kind == LaneLinks.Kind.AUTH)
            val parameters = LaneLinks.query(url)
            if (parameters.containsKey("reauth_challenge")) {
                require(parameters.keys == setOf("reauth_challenge"))
                reauthenticationCallback(parameters.getValue("reauth_challenge"))
                return@execute
            }
            // Compare and consume under one lock; two callbacks cannot reuse an approval.
            val pending = credentials.consumeAuthorization(parameters.getValue("state"), System.currentTimeMillis())
            val token = transport.form("/oauth/token", mapOf("grant_type" to "authorization_code",
                "code" to parameters.getValue("code"), "client_id" to clientId,
                "redirect_uri" to redirect, "code_verifier" to pending.getString("verifier")))
            require(token.getString("token_type").equals("Bearer", true))
            val binding = pending.getJSONObject("binding")
            require(token.getString("device_id") == binding.getString("deviceId"))
            val requestedAccount = pending.getString("accountId")
            require(requestedAccount.isEmpty() || requestedAccount == token.getString("account_id"))
            val info = JSONObject(transport.rpc("lane.v1.SyncService", "GetSyncInfo", "{}", token.getString("access_token")))
            require(info.getInt("formatVersion") == 3)
            require(info.getString("accountId") == token.getString("account_id") &&
                info.getString("tenantId") == token.getString("tenant_id"))
            val identity = JSONObject().put("issuer", token.getString("issuer"))
                .put("accountId", info.getString("accountId")).put("tenantId", info.getString("tenantId"))
                .put("epoch", info.getString("epoch")).put("deviceId", token.getString("device_id"))
            token.put("expiresAt", System.currentTimeMillis() + token.getLong("expires_in") * 1000)
            val approved = runtime.submit {
                if (binding.optBoolean("reconnect")) {
                    val current = JSONObject(runtime.engine("GetLocalStatus", BridgeJson.request(binding.getString("spaceId"))))
                    require(current.getJSONObject("response").optString("accountId") == binding.getString("accountId"))
                }
                // Only the initial connection approval permits merging the standalone dataset.
                val result = JSONObject(runtime.store().approve(identity.toString(), binding.optString("accountId").isEmpty() && !binding.optBoolean("reconnect")))
                val body = BridgeJson.response(result.toString())
                if (binding.optBoolean("reconnect")) require(body.getString("spaceId") == binding.getString("spaceId"))
                credentials.put(body.getString("spaceId"), token)
                result
            }.get()
            if (binding.optBoolean("reconnect")) completeReconnect(binding, token, info.getString("epoch"))
            runtime.selectApprovedSpace(BridgeJson.response(approved.toString()).getString("spaceId"))
            runtime.execute { runtime.pollSafely() }
            schedulePeriodic(); scheduleNow()
        } catch (_: Exception) {
            runtime.publishHostError("UNAUTHENTICATED", "Connection approval failed; connect again")
        }
    }
    private fun completeReconnect(binding: JSONObject, token: JSONObject, newEpoch: String) {
        val space = binding.getString("spaceId")
        val owner = uuid7()
        runtime.submit { check(BridgeJson.response(runtime.store().acquire(space, owner, 60)).getBoolean("acquired")) }.get()
        try {
            val oldPull = JSONObject().put("formatVersion", 3).put("epoch", binding.getString("epoch"))
                .put("cursor", "").put("pageSize", 100)
            try {
                val response = JSONObject(transport.rpc("lane.v1.SyncService", "PullChanges", oldPull.toString(), token.getString("access_token")))
                SyncBoundary.validatePull(response, binding.getString("accountId"), true)
                runtime.submit { BridgeJson.response(runtime.store().apply(space, JSONObject()
                    .put("formatVersion", 3).put("epoch", binding.getString("epoch")).put("owner", owner)
                    .put("pull", response).toString())) }.get()
                onDisconnect(space)
            } catch (failure: LaneTransport.Failure) {
                // Only the typed stale-generation result authorizes the explicitly confirmed reupload.
                if (ConnectErrors.reason(failure.body) != "ERROR_REASON_STALE_EPOCH") throw failure
                runtime.submit {
                    val current = JSONObject(runtime.engine("GetLocalStatus", BridgeJson.request(space)))
                    require(current.getJSONObject("response").optString("accountId") == binding.getString("accountId"))
                    // The engine compares its captured entity-change sequence in the confirmation transaction.
                    BridgeJson.response(runtime.store().confirmEpoch(space, newEpoch))
                }.get()
            }
        } finally { runtime.submit { runtime.store().acquire(space, owner, 0) }.get() }
    }
    @Synchronized private fun accessToken(space: String): String {
        var token = credentials.get(space) ?: error("No credential")
        if (token.optLong("expiresAt") <= System.currentTimeMillis() + 60000) {
            val previous = token
            token = transport.form("/oauth/token", mapOf("grant_type" to "refresh_token",
                "refresh_token" to token.getString("refresh_token"), "client_id" to clientId))
            require(token.getString("account_id") == previous.getString("account_id") &&
                token.getString("tenant_id") == previous.getString("tenant_id") &&
                token.getString("issuer") == previous.getString("issuer") &&
                token.getString("device_id") == previous.getString("device_id") &&
                token.getString("token_type").equals("Bearer", true))
            token.put("expiresAt", System.currentTimeMillis() + token.getLong("expires_in") * 1000)
            credentials.replaceCurrent(space, previous.getString("refresh_token"), token)
        }
        return token.getString("access_token")
    }
    // Runs on the serial storage worker. The shared engine authorizes the local
    // manifest before network access and validates PNG/hash/ownership when caching.
    fun attachment(session: LaneSession, space: String, request: JSONObject): String {
        val local = runtime.engine("GetAttachment", BridgeJson.request(space, request))
        val envelope = JSONObject(local)
        if (!envelope.optBoolean("ok") || envelope.getJSONObject("response").optString("state") !=
            "ATTACHMENT_STATE_WAITING_FOR_SYNC") return local
        val status = BridgeJson.response(runtime.engine("GetLocalStatus", BridgeJson.request(space)))
        if (!status.optBoolean("syncEnabled") || status.optBoolean("reconnectRequired")) return local
        val identity = credentials.get(space) ?: return local
        try {
            require(identity.getString("account_id") == status.getString("accountId") &&
                identity.getString("device_id") == status.getString("deviceId"))
            val owner = uuid7()
            if (!BridgeJson.response(runtime.store().acquire(space, owner, 60)).getBoolean("acquired"))
                return BridgeJson.error("STORAGE_BUSY", "Synchronization is using this data space; retry", true)
            val epoch = try { BridgeJson.response(runtime.store().pending(space, owner)).getString("epoch") }
                finally { runtime.store().acquire(space, owner, 0) }
            val token = accessToken(space)
            val info = JSONObject(transport.rpc("lane.v1.SyncService", "GetSyncInfo", "{}", token))
            AttachmentBoundary.validate(status, identity, info, epoch)
            val remote = JSONObject(transport.rpc("lane.v1.CaptureService", "GetAttachment", request.toString(), token))
            when (remote.optString("state")) {
                "ATTACHMENT_STATE_WAITING_FOR_SYNC", "ATTACHMENT_STATE_DELETED" ->
                    return BridgeJson.success(JSONObject().put("state", remote.getString("state")), envelope.optString("sequence", "0"))
                "ATTACHMENT_STATE_AVAILABLE" -> Unit
                else -> return BridgeJson.error("DEPENDENCY_UNAVAILABLE", "Invalid attachment response", true)
            }
            val png = if (remote.optString("png").isNotEmpty()) remote.getString("png") else
                java.util.Base64.getEncoder().encodeToString(transport.attachment(remote.getString("downloadUrl")))
            val current = credentials.get(space)
            require(session.alive && session.spaces.containsKey(space) && current != null)
            for (key in listOf("account_id", "tenant_id", "device_id", "issuer"))
                require(current.getString(key) == identity.getString(key))
            require(current.optString("session_id") == identity.optString("session_id"))
            val after = BridgeJson.response(runtime.engine("GetLocalStatus", BridgeJson.request(space)))
            require(after.optBoolean("syncEnabled") && !after.optBoolean("reconnectRequired") &&
                after.getString("accountId") == identity.getString("account_id"))
            return runtime.store().cacheAttachment(space, JSONObject().put("taskId", request.getString("taskId"))
                .put("attachmentId", request.getString("attachmentId")).put("png", png).toString())
        } catch (_: LaneTransport.AttachmentFailure) {
            return BridgeJson.error("DEPENDENCY_UNAVAILABLE", "Image access expired or is unavailable; retry to renew access", true)
        } catch (failure: SyncBoundary.Failure) {
            return BridgeJson.error(failure.reason, "Attachment synchronization requires attention")
        } catch (failure: LaneTransport.Failure) {
            val detail = ConnectErrors.detail(failure.body)
            return if (detail != null) JSONObject().put("ok", false).put("error", detail
                .put("message", "Attachment access failed")).toString()
            else BridgeJson.error(if (failure.status == 401) "UNAUTHENTICATED" else if (failure.status == 403) "FORBIDDEN"
                else "DEPENDENCY_UNAVAILABLE", "Attachment download failed; retry", failure.status >= 500)
        } catch (_: IllegalArgumentException) {
            return BridgeJson.error("FORBIDDEN", "Attachment account binding changed")
        } catch (_: Exception) {
            return BridgeJson.error("ONLINE_REQUIRED", "Attachment download failed; retry", true)
        }
    }
    private fun remote(space: String, method: String, request: String): String {
        if (credentials.get(space) == null) return BridgeJson.error("ONLINE_REQUIRED", "Connect synchronization to use this operation")
        val service = when (method) {
            "GetAttachment" -> "CaptureService"
            "ListDevices", "DisconnectDevice", "ListConnections", "RevokeConnection", "RegisterPush", "BeginDataDeletion", "DeleteAccountData" -> "AccountService"
            "ListReceivers", "CreateReceiver", "SaveReceiver", "RotateReceiverKey", "DeleteReceiver", "ListEvents", "GetEvent",
            "DeleteEvent", "ClaimEvent", "RenewEventLease", "ReleaseEventLease", "CompleteEvent", "ReopenEvent" -> "WebhookService"
            else -> "LaneService"
        }
        return try {
            val body = JSONObject(request)
            val session = uiSession
            val identity = credentials.get(space) ?: error("No credential")
            if (method == "DeleteAccountData") {
                val challenge = uiChallenge ?: error("No challenge")
                val pending = reauth[challenge] ?: error("No pending challenge")
                require(session === pending.session && session.alive && session.spaces.containsKey(space))
                require(!body.has("reauthProof") && !body.has("reauth_proof") && !body.has("challengeId"))
                ReauthenticationBinding.validate(pending.binding, session.generation, space, identity,
                    body.getString("mode"), challenge, System.currentTimeMillis())
                require(body.getString("confirmedAccountId") == identity.getString("account_id"))
                val secret = credentials.get("reauth-$challenge") ?: error("Verification required")
                require(System.currentTimeMillis() - secret.getLong("createdAt") in 0..300000)
                body.put("reauthProof", secret.getString("proof"))
            }
            val result = transport.rpc("lane.v1.$service", method, body.toString(), accessToken(space))
            if (method == "BeginDataDeletion") {
                require(session != null && session.alive && session.spaces.containsKey(space))
                require(body.getString("confirmedAccountId") == identity.getString("account_id"))
                val challenge = JSONObject(result).getString("challengeId")
                require(challenge.isNotEmpty())
                reauth[challenge] = Reauthentication(session, space, ReauthenticationBinding.create(
                    session.generation, space, identity, body.getString("mode"), challenge, System.currentTimeMillis()))
            }
            if (method == "DeleteAccountData") cancelReauthentication(session!!, uiChallenge!!)
            result
        }
        catch (failure: LaneTransport.Failure) {
            val detail = ConnectErrors.detail(failure.body)
            if (detail != null) JSONObject().put("ok", false).put("error", detail
                .put("message", "Lane server rejected this operation")).toString()
            else when (failure.status) {
                401 -> BridgeJson.error("UNAUTHENTICATED", "Reconnect Lane synchronization")
                403 -> BridgeJson.error("FORBIDDEN", "This device cannot access the account")
                else -> BridgeJson.error("DEPENDENCY_UNAVAILABLE", "Lane server request failed", true)
            }
        } catch (_: IllegalArgumentException) { BridgeJson.error("FORBIDDEN", "Deletion verification does not match this request") }
        catch (_: IllegalStateException) { BridgeJson.error("UNAUTHENTICATED", "Verify deletion again") }
        catch (_: Exception) { BridgeJson.error("ONLINE_REQUIRED", "Lane server is unavailable", true) }
    }
    fun synchronize(stopped: () -> Boolean, selectedSpace: String? = null): Boolean {
        var success = true
        for (space in credentials.spaces().filter { selectedSpace == null || it == selectedSpace }) {
            if (stopped()) return false
            val handled = runtime.backgroundStoreLease(space).run(stopped) {
                synchronizeSpace(stopped, space)
            }
            if (!handled) success = false
        }
        return success
    }
    private fun synchronizeSpace(stopped: () -> Boolean, space: String): Boolean {
        val owner = uuid7()
        try {
            val status = runtime.submit { BridgeJson.response(runtime.engine("GetLocalStatus", BridgeJson.request(space))) }.get()
            if (status.optBoolean("reconnectRequired")) return true
            val acquired = runtime.submit { BridgeJson.response(runtime.store().acquire(space, owner, 60)).getBoolean("acquired") }.get()
            if (!acquired) return true
            try {
                val info = JSONObject(transport.rpc("lane.v1.SyncService", "GetSyncInfo", "{}", accessToken(space)))
                val identity = credentials.get(space) ?: throw SyncBoundary.Failure("UNAUTHENTICATED")
                if (info.optString("accountId") != identity.getString("account_id") ||
                    info.optString("tenantId") != identity.getString("tenant_id")) throw SyncBoundary.Failure("FORBIDDEN")
                if (info.optInt("formatVersion") != 3) throw SyncBoundary.Failure("UNSUPPORTED_FORMAT")
                var more = true
                while (more && !stopped()) {
                    val pending = runtime.submit {
                        check(BridgeJson.response(runtime.store().acquire(space, owner, 60)).getBoolean("acquired"))
                        BridgeJson.response(runtime.store().pending(space, owner))
                    }.get()
                    val token = accessToken(space)
                    if (info.getString("epoch") != pending.getString("epoch")) {
                        // A fresh grant may read an old generation's deletion directive. Never upload first.
                        val response = JSONObject(transport.rpc("lane.v1.SyncService", "PullChanges", JSONObject()
                            .put("formatVersion", 3).put("epoch", pending.getString("epoch"))
                            .put("cursor", pending.optString("cursor")).put("pageSize", 100).toString(), token))
                        SyncBoundary.validatePull(response, identity.getString("account_id"), true)
                        runtime.submit { BridgeJson.response(runtime.store().apply(space, JSONObject()
                            .put("formatVersion", 3).put("epoch", pending.getString("epoch")).put("owner", owner)
                            .put("pull", response).toString())); runtime.pollSafely() }.get()
                        onDisconnect(space)
                        break
                    }
                    val mutations = pending.getJSONArray("mutations")
                    for (index in 0 until mutations.length()) {
                        if (stopped() || credentials.get(space) == null) break
                        runtime.submit { check(BridgeJson.response(runtime.store().acquire(space, owner, 60)).getBoolean("acquired")) }.get()
                        val mutation = mutations.getJSONObject(index)
                        val push = JSONObject().put("formatVersion", 3).put("epoch", pending.getString("epoch"))
                            .put("deviceId", pending.getString("deviceId")).put("mutations", org.json.JSONArray().put(mutation))
                        val response = try {
                            JSONObject(transport.rpc("lane.v1.SyncService", "PushChanges", push.toString(), token))
                        } catch (failure: LaneTransport.Failure) {
                            if (failure.status != 413) throw failure
                            JSONObject().put("results", org.json.JSONArray().put(JSONObject()
                                .put("requestId", mutation.getJSONObject("meta").getString("requestId"))
                                .put("error", JSONObject().put("reason", "ERROR_REASON_ITEM_TOO_LARGE").put("retryable", false))))
                        }
                        if (credentials.get(space) == null) break
                        runtime.submit { BridgeJson.response(runtime.store().apply(space, JSONObject()
                            .put("formatVersion", 3).put("epoch", pending.getString("epoch")).put("owner", owner)
                            .put("push", response).toString())) }.get()
                    }
                    if (stopped() || credentials.get(space) == null) break
                    runtime.submit { check(BridgeJson.response(runtime.store().acquire(space, owner, 60)).getBoolean("acquired")) }.get()
                    val pull = JSONObject().put("formatVersion", 3).put("epoch", pending.getString("epoch"))
                        .put("cursor", pending.optString("cursor")).put("pageSize", 100)
                    val response = JSONObject(transport.rpc("lane.v1.SyncService", "PullChanges", pull.toString(), token))
                    val accountDeleted = SyncBoundary.validatePull(response, identity.getString("account_id"), false)
                    runtime.submit {
                        BridgeJson.response(runtime.store().apply(space, JSONObject().put("formatVersion", 3)
                            .put("epoch", pending.getString("epoch")).put("owner", owner).put("pull", response).toString()))
                        runtime.pollSafely()
                    }.get()
                    if (accountDeleted) { onDisconnect(space); break }
                    more = response.optBoolean("hasMore")
                }
            } finally { runtime.submit { runtime.store().acquire(space, owner, 0) }.get() }
        } catch (failure: Exception) {
            if (failure is LaneTransport.Failure && failure.status == 401 && recoverDeletion(space)) return true
            val reason = when {
                failure is LaneTransport.Failure && ConnectErrors.reason(failure.body) != null -> ConnectErrors.reason(failure.body)!!.removePrefix("ERROR_REASON_")
                failure is LaneTransport.Failure && failure.status == 401 -> "UNAUTHENTICATED"
                failure is LaneTransport.Failure && failure.status == 403 -> "FORBIDDEN"
                failure is SyncBoundary.Failure -> failure.reason
                else -> "DEPENDENCY_UNAVAILABLE"
            }
            runtime.submit { runtime.safe { runtime.store().log("lane_sync_failed"); runtime.store().syncFailure(space, "ERROR_REASON_$reason") } }.get()
            return false
        }
    }
    private fun recoverDeletion(space: String): Boolean {
        val identity = credentials.get(space) ?: return false
        val owner = uuid7()
        var acquired = false
        return try {
            acquired = runtime.submit { BridgeJson.response(runtime.store().acquire(space, owner, 60)).getBoolean("acquired") }.get()
            if (!acquired) return false
            val pending = runtime.submit { BridgeJson.response(runtime.store().pending(space, owner)) }.get()
            val epoch = pending.getString("epoch")
            // This endpoint alone accepts an expired/revoked token bound to a recorded deletion.
            val status = transport.deletionStatus(epoch, identity.getString("access_token"))
            val pull = SyncBoundary.deletionStatus(status, identity, epoch)
            runtime.submit {
                require(credentials.get(space)?.optString("account_id") == identity.getString("account_id"))
                if (pull != null) BridgeJson.response(runtime.store().apply(space, JSONObject().put("formatVersion", 3)
                    .put("epoch", epoch).put("owner", owner).put("pull", pull).toString()))
                else BridgeJson.response(runtime.store().syncFailure(space, "ERROR_REASON_STALE_EPOCH"))
                runtime.pollSafely()
            }.get()
            if (pull != null) onDisconnect(space)
            true
        } catch (_: Exception) { false }
        finally { if (acquired) runtime.submit { runtime.store().acquire(space, owner, 0) }.get() }
    }
    private fun startWatch(space: String) {
        if (!visible || !watching.add(space)) return
        network.execute {
            try {
                while (visible && credentials.get(space) != null) {
                    try {
                        val status = runtime.submit { BridgeJson.response(runtime.engine("GetLocalStatus", BridgeJson.request(space))) }.get()
                        if (status.optBoolean("reconnectRequired")) break
                        val info = JSONObject(transport.rpc("lane.v1.SyncService", "GetSyncInfo", "{}", accessToken(space)))
                        transport.watch(JSONObject().put("epoch", info.getString("epoch")), accessToken(space),
                            { visible && credentials.get(space) != null }, { scheduleNow() })
                    } catch (_: Exception) { /* Reconnect from the durable pull cursor through the sync worker. */ }
                    if (visible) { scheduleNow(); Thread.sleep(5000) }
                }
            } finally { watching.remove(space) }
        }
    }
    fun pushHint(accountId: String, proposalId: String) {
        if (accountId.isEmpty() || !LaneLinks.validId(proposalId)) return
        val spaces = BridgeJson.response(runtime.store().accountSpaces(accountId)).getJSONArray("items")
        for (index in 0 until spaces.length()) {
            val space = spaces.getString(index)
            val token = credentials.get(space) ?: continue
            runtime.withStoreHandle(space) {
                val status = BridgeJson.response(runtime.engine("GetLocalStatus", BridgeJson.request(space)))
                if (token.getString("account_id") == accountId && token.getString("device_id") == status.getString("deviceId") &&
                    status.optBoolean("syncEnabled")) scheduleNow(space)
            }
        }
    }
    fun refreshAllPush() { credentials.spaces().forEach { space -> runtime.safe { refreshPush(space); "{}" } } }
    fun refreshPush(space: String) { LanePush.register(runtime, space, ::remote) }
    companion object {
        fun uuid7(): String {
            val random = SecureRandom()
            val time = System.currentTimeMillis() and 0xffffffffffffL
            val high = (time shl 16) or 0x7000L or (random.nextInt(4096).toLong())
            val low = (random.nextLong() and 0x3fffffffffffffffL) or Long.MIN_VALUE
            return UUID(high, low).toString()
        }
    }
}
