// SPDX-License-Identifier: Apache-2.0
package io.delino.anydash

import android.content.SharedPreferences
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.File
import java.lang.reflect.Proxy
import java.nio.file.Files

/** Widget refresh results must not overwrite a sign-out or a newer app-written snapshot. */
class WidgetStoreTest {
    private val directory = Files.createTempDirectory("widget-store").toFile().also { it.deleteOnExit() }
    private val store = WidgetStore(directory, preferences())
    private val widgetId = "0192f4aa-0000-7000-8000-000000000001"
    private val shown = SnapshotCodec.encodeSnapshot(NativeWidgetSnapshot(widgetId = widgetId, sharingState = SharingState.PRIVATE), null, 1L)
    private val refreshed = SnapshotCodec.encodeSnapshot(NativeWidgetSnapshot(widgetId = widgetId, sharingState = SharingState.PRIVATE), null, 2L)

    /** Simulates a snapshot the app wrote through the widgetSnapshots host method. */
    private fun appWrite(bytes: ByteArray) {
        directory.mkdirs()
        File(directory, "${sha256Hex(widgetId)}.pb").writeBytes(bytes)
    }

    @Test fun replacesTheSnapshotThatStartedTheRefresh() {
        appWrite(shown)
        val expected = store.stored(widgetId)!!
        assertTrue(store.replaceIfCurrent(expected, refreshed))
        assertArrayEquals(refreshed, store.stored(widgetId)!!.bytes)
    }

    @Test fun discardsRefreshResultsAfterSignOut() {
        appWrite(shown)
        val expected = store.stored(widgetId)!!
        store.clear()
        assertFalse(store.replaceIfCurrent(expected, refreshed))
        assertNull(store.stored(widgetId))
        assertTrue(store.all().isEmpty())
    }

    @Test fun discardsRefreshResultsAfterAFullReplacement() {
        appWrite(shown)
        val expected = store.stored(widgetId)!!
        store.write(emptyList(), replaceAll = true)
        // Even when the same bytes return, the replacement happened after the request started.
        appWrite(shown)
        assertFalse(store.replaceIfCurrent(expected, refreshed))
        assertArrayEquals(shown, store.stored(widgetId)!!.bytes)
    }

    @Test fun keepsAnEndedSnapshotWrittenDuringTheRefresh() {
        appWrite(shown)
        val expected = store.stored(widgetId)!!
        val ended = SnapshotCodec.encodeSnapshot(NativeWidgetSnapshot(widgetId = widgetId, sharingState = SharingState.ENDED), null, 3L)
        appWrite(ended)
        assertFalse(store.replaceIfCurrent(expected, refreshed))
        assertEquals(SharingState.ENDED, store.stored(widgetId)!!.snapshot.sharingState)
    }

    /** SharedPreferences whose reads return defaults and whose edits are accepted and dropped. */
    private fun preferences(): SharedPreferences {
        val loader = SharedPreferences::class.java.classLoader
        lateinit var editor: SharedPreferences.Editor
        editor = Proxy.newProxyInstance(loader, arrayOf(SharedPreferences.Editor::class.java)) { _, method, _ ->
            when (method.returnType) {
                SharedPreferences.Editor::class.java -> editor
                java.lang.Boolean.TYPE -> true
                else -> null
            }
        } as SharedPreferences.Editor
        return Proxy.newProxyInstance(loader, arrayOf(SharedPreferences::class.java)) { _, method, args ->
            when (method.name) {
                "edit" -> editor
                "contains" -> false
                else -> args?.getOrNull(1)
            }
        } as SharedPreferences
    }
}
