// SPDX-License-Identifier: Apache-2.0
package io.delino.anydash

import android.app.PendingIntent
import android.appwidget.AppWidgetManager
import android.appwidget.AppWidgetProvider
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.view.View
import android.widget.RemoteViews
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit

/**
 * Android home screen widget. It shows the selected widget's summary from the
 * snapshot the app wrote after its last refresh. When Android runs a periodic
 * update and the stored session is valid, it calls QueryWidgetData as
 * CLIENT_KIND_NATIVE_WIDGET first; otherwise it keeps the latest snapshot and
 * its time. ENDED snapshots remove displayed data and show "Sharing ended".
 */
class AnyDashWidgetProvider : AppWidgetProvider() {
    override fun onUpdate(context: Context, manager: AppWidgetManager, ids: IntArray) {
        val pending = goAsync()
        refresher.execute {
            try {
                val host = (context.applicationContext as AnyDashApplication).host
                for (id in ids) {
                    host.widgets.snapshot(host.widgets.selection(id))
                        ?.let { host.widgets.stored(it.widgetId) }
                        ?.let { refresh(host, it) }
                    render(context, manager, id)
                }
            } finally {
                pending.finish()
            }
        }
    }

    override fun onDeleted(context: Context, ids: IntArray) {
        val widgets = (context.applicationContext as AnyDashApplication).host.widgets
        for (id in ids) widgets.forget(id)
    }

    companion object {
        private val refresher = Executors.newSingleThreadExecutor { Thread(it, "AnyDashWidget").apply { isDaemon = true } }

        /** Redraws every placed widget from stored snapshots, without network calls. */
        fun updateAll(context: Context) {
            val manager = AppWidgetManager.getInstance(context)
            for (id in manager.getAppWidgetIds(ComponentName(context, AnyDashWidgetProvider::class.java)))
                render(context, manager, id)
        }

        private fun refresh(host: AnyDashHost, stored: StoredSnapshot) {
            val snapshot = stored.snapshot
            if (snapshot.sharingState == SharingState.ENDED || !host.auth.signedIn) return
            val worker = Executors.newSingleThreadExecutor()
            try {
                val future = worker.submit<Unit> {
                    val token = host.auth.accessToken(BuildConfig.ANYDASH_RESOURCE)
                    val result = AnyDashApi.call("anydash.v1.WidgetDataService", "QueryWidgetData",
                        SnapshotCodec.queryWidgetData(snapshot.dashboardId, snapshot.widgetId, ""),
                        mapOf("X-AnyDash-Client-Kind" to "CLIENT_KIND_NATIVE_WIDGET",
                            "X-AnyDash-Client-Version" to BuildConfig.VERSION_NAME), token)
                    if (result.status != 200) return@submit
                    val data = SnapshotCodec.widgetDataSummary(result.body)
                    // A response without a summary keeps the latest snapshot unless sharing ended.
                    if (data.summary == null && data.state != SharingState.ENDED) return@submit
                    val next = snapshot.copy(
                        sharingState = if (data.state == SharingState.UNSPECIFIED) snapshot.sharingState else data.state,
                        summary = data.summary,
                    )
                    // Sign-out or a newer app-written snapshot (such as ENDED) during the request wins.
                    if (!host.widgets.replaceIfCurrent(stored, SnapshotCodec.encodeSnapshot(next, data.summaryBytes, System.currentTimeMillis())))
                        AnyDashLog.info("anydash_widget_refresh_discarded")
                }
                // Broadcast receivers have about ten seconds; keep the latest snapshot on timeout.
                future.get(8, TimeUnit.SECONDS)
            } catch (_: Exception) {
                AnyDashLog.info("anydash_widget_refresh_skipped")
            } finally {
                worker.shutdownNow()
            }
        }

        fun render(context: Context, manager: AppWidgetManager, id: Int) {
            val host = (context.applicationContext as AnyDashApplication).host
            val selected = host.widgets.selection(id)
            val snapshot = host.widgets.snapshot(selected)
            val views = RemoteViews(context.packageName, R.layout.anydash_widget)
            val now = System.currentTimeMillis()
            val summary = snapshot?.summary
            when {
                snapshot == null -> {
                    views.setTextViewText(R.id.widget_title, "AnyDash")
                    views.setViewVisibility(R.id.widget_value, View.GONE)
                    views.setTextViewText(R.id.widget_supporting, WidgetText.EMPTY)
                    views.setViewVisibility(R.id.widget_updated, View.GONE)
                    views.setContentDescription(R.id.widget_root, WidgetText.EMPTY)
                }
                summary == null || snapshot.sharingState == SharingState.ENDED -> {
                    val title = host.widgets.title(snapshot.widgetId)
                    views.setTextViewText(R.id.widget_title, title)
                    views.setViewVisibility(R.id.widget_value, View.VISIBLE)
                    views.setTextViewText(R.id.widget_value, WidgetText.SHARING_ENDED)
                    views.setTextViewTextSize(R.id.widget_value, android.util.TypedValue.COMPLEX_UNIT_SP, 15f)
                    views.setTextColor(R.id.widget_value, context.getColor(R.color.anydash_muted))
                    views.setTextViewText(R.id.widget_supporting, WidgetText.SHARING_ENDED_DETAIL)
                    views.setViewVisibility(R.id.widget_updated, View.GONE)
                    views.setContentDescription(R.id.widget_root, WidgetText.accessibility(snapshot, title, now))
                }
                else -> {
                    val notProvided = summary.primaryValue.state == MetricState.NOT_PROVIDED
                    views.setTextViewText(R.id.widget_title, summary.title)
                    views.setViewVisibility(R.id.widget_value, View.VISIBLE)
                    views.setTextViewText(R.id.widget_value, WidgetText.value(summary.primaryValue, now))
                    views.setTextViewTextSize(R.id.widget_value, android.util.TypedValue.COMPLEX_UNIT_SP, if (notProvided) 18f else 26f)
                    // "Not provided" is muted grey; failures show the stale value at 55% opacity.
                    val color = context.getColor(if (notProvided) R.color.anydash_muted else R.color.anydash_fg)
                    views.setTextColor(R.id.widget_value, if (WidgetText.stale(summary.primaryValue)) (color and 0x00FFFFFF) or (0x8C shl 24) else color)
                    views.setTextViewText(R.id.widget_supporting, WidgetText.supporting(summary, now))
                    val updated = summary.dataUpdatedAt ?: snapshot.refreshedAt
                    views.setViewVisibility(R.id.widget_updated, if (updated == null) View.GONE else View.VISIBLE)
                    updated?.let { views.setTextViewText(R.id.widget_updated, WidgetText.updated(it, now)) }
                    views.setContentDescription(R.id.widget_root, WidgetText.accessibility(snapshot, summary.title, now))
                }
            }
            val target = snapshot?.deepLink?.let(Uri::parse)
            val intent = Intent(context, MainActivity::class.java).apply {
                if (target != null) { action = Intent.ACTION_VIEW; data = target } else action = Intent.ACTION_MAIN
                addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            }
            views.setOnClickPendingIntent(R.id.widget_root,
                PendingIntent.getActivity(context, id, intent, PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT))
            manager.updateAppWidget(id, views)
        }
    }
}
