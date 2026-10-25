// SPDX-License-Identifier: Apache-2.0
package io.delino.lane

import com.google.android.gms.common.ConnectionResult
import com.google.android.gms.common.GoogleApiAvailabilityLight
import com.google.firebase.FirebaseApp
import com.google.firebase.messaging.FirebaseMessaging
import com.google.firebase.messaging.FirebaseMessagingService
import com.google.firebase.messaging.RemoteMessage
import org.json.JSONObject

internal object LanePush {
    fun supported(runtime: LaneRuntime): Boolean =
        GoogleApiAvailabilityLight.getInstance().isGooglePlayServicesAvailable(runtime.application) == ConnectionResult.SUCCESS &&
            (FirebaseApp.getApps(runtime.application).isNotEmpty() || FirebaseApp.initializeApp(runtime.application) != null)

    fun register(runtime: LaneRuntime, space: String, remote: (String, String, String) -> String) {
        val status = BridgeJson.response(runtime.engine("GetLocalStatus", BridgeJson.request(space)))
        if (!status.optBoolean("syncEnabled")) return
        val preferences = status.getJSONObject("notificationPreferences")
        if (!supported(runtime)) {
            runtime.publishHostError("DEPENDENCY_UNAVAILABLE",
                "Remote notifications require Firebase configuration and Google Play services. Local notifications remain available.")
            return
        }
        val enabled = preferences.optBoolean("enabled") && runtime.notifications.permission() == "NOTIFICATION_PERMISSION_GRANTED"
        val messaging = FirebaseMessaging.getInstance()
        messaging.isAutoInitEnabled = enabled
        // Token fetch is only initiated by explicit notification enablement or token refresh.
        if (!enabled) {
            remote(space, "RegisterPush", JSONObject().put("deviceId", status.getString("deviceId"))
                .put("provider", "PUSH_PROVIDER_FCM").put("enabled", false)
                .put("content", preferences.optString("content", "NOTIFICATION_CONTENT_DETAIL")).toString())
            return
        }
        messaging.token.addOnSuccessListener { token -> runtime.execute {
            runtime.safe {
                val latest = BridgeJson.response(runtime.engine("GetLocalStatus", BridgeJson.request(space)))
                val current = latest.getJSONObject("notificationPreferences")
                if (latest.optBoolean("syncEnabled") && current.optBoolean("enabled")) remote(space, "RegisterPush",
                    JSONObject().put("deviceId", latest.getString("deviceId")).put("provider", "PUSH_PROVIDER_FCM")
                        .put("endpoint", token).put("enabled", true)
                        .put("content", current.optString("content", "NOTIFICATION_CONTENT_DETAIL")).toString()) else "{}"
            }
        } }.addOnFailureListener {
            runtime.publishHostError("DEPENDENCY_UNAVAILABLE", "Remote notification registration failed; local notifications remain available")
        }
    }
}

/** The server must send data-only messages. The shared store decides eligibility and deduplication. */
class LaneMessagingService : FirebaseMessagingService() {
    override fun onNewToken(token: String) {
        // Never put provider tokens in logs, intents, JavaScript or WorkManager Data.
        val runtime = (application as LaneApplication).runtime
        runtime.execute { runtime.host.refreshAllPush() }
    }
    override fun onMessageReceived(message: RemoteMessage) {
        // FCM payloads are wake-up hints, not trusted task state or OS notification content.
        (application as LaneApplication).runtime.host.scheduleNow()
    }
}
