// SPDX-License-Identifier: Apache-2.0
package io.delino.lane

import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.json.JSONArray
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File

/** Real JNI/SQLite tests. Requires an Android device; JVM unit tests do not substitute for this gate. */
@RunWith(AndroidJUnit4::class)
class NativeStoreIntegrationTest {
    private val app get() = InstrumentationRegistry.getInstrumentation().targetContext.applicationContext as LaneApplication
    @Test fun unicodePersistsAndAnotherConnectionObservesOnlyCommittedChanges() {
        val directory = File(app.cacheDir, "lane-jni-${LaneHostServices.uuid7()}")
        val task = LaneHostServices.uuid7()
        try {
            NativeStore(directory.path, app.runtime.host).use { first ->
                val opened = BridgeJson.response(first.open("{\"formatVersion\":1,\"spaceId\":\"\"}"))
                val space = opened.getString("spaceId")
                first.poll()
                NativeStore(directory.path, app.runtime.host).use { second ->
                    BridgeJson.response(second.open(JSONObject().put("formatVersion", 1).put("spaceId", space).toString()))
                    val request = JSONObject().put("id", task).put("title", "Unicode 🧪")
                        .put("prompt", "# Exact\n😀\u0000end")
                        .put("repository", JSONObject().put("url", "https://example.com/lane.git"))
                        .put("meta", JSONObject().put("requestId", LaneHostServices.uuid7()))
                    BridgeJson.response(second.invoke("CreateTask", BridgeJson.request(space, request)))
                    val changes = JSONArray(first.poll())
                    assertTrue(changes.length() > 0)
                    assertEquals(space, changes.getJSONObject(0).getString("spaceId"))
                    val failed = JSONObject().put("id", LaneHostServices.uuid7()).put("title", "   ")
                        .put("repository", JSONObject().put("url", "https://example.com/other"))
                        .put("meta", JSONObject().put("requestId", LaneHostServices.uuid7()))
                    assertFalse(JSONObject(second.invoke("CreateTask", BridgeJson.request(space, failed))).getBoolean("ok"))
                    assertEquals(0, JSONArray(first.poll()).length())
                }
                BridgeJson.response(first.release(space))
            }
            NativeStore(directory.path, app.runtime.host).use { reopened ->
                val space = BridgeJson.response(reopened.open("{\"formatVersion\":1,\"spaceId\":\"\"}")).getString("spaceId")
                val result = BridgeJson.response(reopened.invoke("GetTask", BridgeJson.request(space, JSONObject().put("id", task))))
                assertEquals("# Exact\n😀\u0000end", result.getJSONObject("result").getJSONObject("version").getJSONObject("task").getString("prompt"))
                assertFalse(JSONObject(reopened.open("{\"formatVersion\":99,\"spaceId\":\"\"}")).getBoolean("ok"))
            }
        } finally { directory.deleteRecursively() }
    }
}
