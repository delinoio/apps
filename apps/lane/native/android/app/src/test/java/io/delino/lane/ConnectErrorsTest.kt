// SPDX-License-Identifier: Apache-2.0
package io.delino.lane

import org.json.JSONArray
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import java.util.Base64

class ConnectErrorsTest {
    private fun envelope(bytes: ByteArray, type: String = "lane.v1.ErrorDetail") = JSONObject()
        .put("details", JSONArray().put(JSONObject().put("type", type).put("value", Base64.getEncoder().withoutPadding().encodeToString(bytes)))).toString()
    @Test fun onlyTypedStaleEpochCanAuthorizeExplicitReconnect() {
        assertEquals("ERROR_REASON_STALE_EPOCH", ConnectErrors.reason(envelope(byteArrayOf(8, 17))))
        assertNull(ConnectErrors.reason("{\"code\":\"failed_precondition\",\"message\":\"ERROR_REASON_STALE_EPOCH\"}"))
        assertNull(ConnectErrors.reason(envelope(byteArrayOf(8, 17), "another.ErrorDetail")))
    }
    @Test fun malformedDetailsFailClosed() {
        for (bytes in listOf(byteArrayOf(8), byteArrayOf(18, 127), byteArrayOf(8, 99), byteArrayOf(18, -1, -1, -1, -1, 127)))
            assertNull(ConnectErrors.reason(envelope(bytes)))
    }
    @Test fun preservesConflictIdentifiersAndRetryFlagWithoutServerMessage() {
        val detail = ConnectErrors.detail(envelope(byteArrayOf(8, 3, 18, 1, 97, 26, 1, 98, 34, 1, 99, 40, 1)))!!
        assertEquals("ERROR_REASON_STALE_VERSION", detail.getString("reason"))
        assertEquals("a", detail.getString("correlationId"))
        assertEquals("b", detail.getString("entityId"))
        assertEquals("c", detail.getJSONArray("currentHeadIds").getString(0))
        assertTrue(detail.getBoolean("retryable"))
    }
}
