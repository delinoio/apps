// SPDX-License-Identifier: Apache-2.0
package io.delino.lane

import org.json.JSONObject
import java.util.UUID

internal object BridgeJson {
    fun error(reason: String, message: String, retryable: Boolean = false): String =
        JSONObject().put("ok", false).put("error", JSONObject()
            .put("reason", "ERROR_REASON_$reason").put("message", message)
            .put("correlationId", UUID.randomUUID().toString()).put("retryable", retryable)).toString()
    fun success(response: JSONObject = JSONObject(), sequence: String = "0") =
        JSONObject().put("ok", true).put("response", response).put("sequence", sequence).toString()
    fun request(space: String, body: JSONObject = JSONObject()) =
        JSONObject().put("spaceId", space).put("request", body).toString()
    fun response(value: String): JSONObject {
        val envelope = JSONObject(value)
        check(envelope.optBoolean("ok")) { "Lane operation failed" }
        return envelope.getJSONObject("response")
    }
}

internal object LaneMethods {
    val mutations = setOf("CreateRepository", "SaveRepository", "DeleteRepository", "CreateTask",
        "SaveTask", "SetProgress", "SetProposal", "RestorePrompt", "DeleteTask", "ResolveConflict",
        "CreateReceiver", "SaveReceiver", "RotateReceiverKey", "DeleteReceiver", "DeleteEvent",
        "ClaimEvent", "RenewEventLease", "ReleaseEventLease", "CompleteEvent", "ReopenEvent")
    val allowed = mutations + setOf("GetRepository", "ListRepositories", "GetTask", "ListTasks",
        "ListHistory", "GetLocalStatus", "GetNotificationPreferences", "SetNotificationPreferences",
        "ConnectSync", "DisconnectSync", "RemoveDeviceData", "ListDevices", "DisconnectDevice",
        "ListConnections", "RevokeConnection", "RegisterPush", "BeginDataDeletion", "DeleteAccountData",
        "ListReceivers", "ListEvents", "GetEvent")

    fun appRequest(method: String, request: JSONObject, deviceId: String, deviceName: String) {
        if (method !in mutations) return
        val supplied = request.optJSONObject("meta") ?: JSONObject()
        request.put("meta", JSONObject().put("requestId", supplied.optString("requestId"))
            .put("origin", JSONObject().put("kind", "ORIGIN_KIND_APP")
                .put("deviceId", deviceId).put("deviceName", deviceName)))
        // Account attribution is filled from the pinned account by the shared engine.
        if (method == "CompleteEvent") {
            val tasks = request.optJSONArray("tasks") ?: return
            for (i in 0 until tasks.length()) appRequest("CreateTask", tasks.getJSONObject(i), deviceId, deviceName)
        }
    }
}
