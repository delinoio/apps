// SPDX-License-Identifier: Apache-2.0
package io.delino.anydash

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.File
import java.util.TimeZone

/** Decoder checks against fixtures from the TypeScript bindings (scripts/widget-fixtures.ts). */
class SnapshotsTest {
    private val fixtures = File(System.getProperty("anydash.fixtures") ?: "../../widgets/fixtures")
    private val utc = TimeZone.getTimeZone("UTC")
    private val now = 1793504460000L // 2026-11-01T03:41:00Z

    @Test fun decodesSnapshotList() {
        val list = SnapshotCodec.snapshotList(File(fixtures, "snapshots.pb").readBytes())
        assertEquals(3, list.size)
        val codex = list[0]
        assertEquals("Codex · 5-hour", codex.summary?.title)
        assertEquals(MetricContent.Percent(62.0), codex.summary?.primaryValue?.content)
        assertEquals("62%", WidgetText.value(codex.summary!!.primaryValue, now, utc))
        assertEquals("left · Resets 05:20", WidgetText.supporting(codex.summary!!, now, utc))
        assertEquals("Updated 03:41", WidgetText.updated(codex.summary!!.dataUpdatedAt!!, now, utc))
        assertEquals(
            "anydash://spaces/0192f000-0000-7000-8000-0000000000a1/dashboards/0192f000-0000-7000-8000-0000000000c1?widget=0192f4aa-0000-7000-8000-000000000001",
            codex.deepLink,
        )
        val ended = list[1]
        assertEquals(SharingState.ENDED, ended.sharingState)
        assertNull(ended.summary)
        assertTrue(WidgetText.accessibility(ended, "Supabase · Requests", now, utc).contains("Sharing ended"))
        assertEquals("$41.80", WidgetText.value(list[2].summary!!.primaryValue, now, utc))
    }

    @Test fun decodesWidgetDataAndReencodesSnapshot() {
        val data = SnapshotCodec.widgetDataSummary(File(fixtures, "widget-data.pb").readBytes())
        assertEquals(SharingState.PRIVATE, data.state)
        assertEquals(MetricContent.Percent(61.0), data.summary?.primaryValue?.content)
        val previous = SnapshotCodec.snapshotList(File(fixtures, "snapshots.pb").readBytes())[0]
        val encoded = SnapshotCodec.encodeSnapshot(previous.copy(summary = data.summary), data.summaryBytes, now)
        val next = SnapshotCodec.snapshot(encoded)
        assertEquals(previous.widgetId, next.widgetId)
        assertEquals(previous.deepLink, next.deepLink)
        assertEquals(data.summary, next.summary)
        assertEquals(now, next.refreshedAt)
    }

    @Test fun encodesQueryWidgetDataRequest() {
        assertArrayEquals(byteArrayOf(10, 1, 100, 18, 1, 119), SnapshotCodec.queryWidgetData("d", "w", ""))
    }

    @Test fun endedSnapshotsDropData() {
        val ended = NativeWidgetSnapshot(widgetId = "w", sharingState = SharingState.ENDED)
        val decoded = SnapshotCodec.snapshot(SnapshotCodec.encodeSnapshot(ended, byteArrayOf(10, 1, 65), now))
        assertEquals(SharingState.ENDED, decoded.sharingState)
        assertNull(decoded.summary)
    }
}
