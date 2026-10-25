// SPDX-License-Identifier: Apache-2.0
package io.delino.lane

import android.Manifest
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import org.json.JSONObject

internal class LaneNotifications(private val context: Context) {
    private val manager = context.getSystemService(NotificationManager::class.java)
    private val settings = context.getSharedPreferences("lane-os-permissions", Context.MODE_PRIVATE)
    fun permission(): String {
        val allowed = (Build.VERSION.SDK_INT < 33 || context.checkSelfPermission(
            Manifest.permission.POST_NOTIFICATIONS) == PackageManager.PERMISSION_GRANTED) && manager.areNotificationsEnabled()
        val channel = manager.getNotificationChannel(CHANNEL)
        return when {
            allowed && channel?.importance != NotificationManager.IMPORTANCE_NONE -> "NOTIFICATION_PERMISSION_GRANTED"
            !settings.getBoolean("requested", false) -> "NOTIFICATION_PERMISSION_NOT_REQUESTED"
            else -> "NOTIFICATION_PERMISSION_DENIED"
        }
    }
    fun requested() { settings.edit().putBoolean("requested", true).apply() }
    fun deliver(space: String, item: JSONObject): Boolean {
        if (permission() != "NOTIFICATION_PERMISSION_GRANTED") return false
        manager.createNotificationChannel(NotificationChannel(CHANNEL, "Task proposals", NotificationManager.IMPORTANCE_DEFAULT))
        val proposal = item.getString("proposalId")
        if (!LaneLinks.validId(proposal) || !LaneLinks.validId(space)) return false
        val scheme = if (BuildConfig.DEBUG) "lane-dev" else "lane"
        val intent = Intent(context, MainActivity::class.java).apply {
            action = Intent.ACTION_VIEW
            data = Uri.parse("$scheme://task/$proposal?spaceId=$space")
            flags = Intent.FLAG_ACTIVITY_CLEAR_TOP or Intent.FLAG_ACTIVITY_SINGLE_TOP
        }
        // Explicit immutable intent; data URI distinguishes proposals even after a restart.
        val pending = PendingIntent.getActivity(context, 0, intent,
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE)
        val generic = item.optString("content") == "NOTIFICATION_CONTENT_GENERIC"
        val notification = Notification.Builder(context, CHANNEL)
            .setSmallIcon(android.R.drawable.ic_dialog_info)
            .setContentTitle(if (generic) "New task proposal" else item.getString("repositoryName"))
            .setContentText(if (generic) "Open Lane to review the proposal" else item.getString("title"))
            .setVisibility(Notification.VISIBILITY_PRIVATE)
            .setContentIntent(pending).setAutoCancel(true).setOnlyAlertOnce(true).build()
        return try {
            manager.notify("${item.getString("deviceId")}:$proposal", 0, notification)
            true
        } catch (_: SecurityException) { false }
    }
    companion object { private const val CHANNEL = "lane-proposals" }
}
