// SPDX-License-Identifier: Apache-2.0
package io.delino.lane

import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test

class BridgeContractTest {
    private val id = "019d0000-0000-7000-8000-000000000001"
    @Test fun routesRejectUntrustedTargetsAndPreserveValidIdentity() {
        for (host in listOf("task", "repository")) {
            assertNotNull(LaneLinks.parse("lane://$host/$id", "lane"))
            assertNull(LaneLinks.parse("lane-dev://$host/$id", "lane"))
            assertNull(LaneLinks.parse("lane://user@$host/$id", "lane"))
            assertNull(LaneLinks.parse("lane://$host/$id/extra", "lane"))
            assertNull(LaneLinks.parse("lane://$host/$id#fragment", "lane"))
            assertNull(LaneLinks.parse("lane://$host/%2e%2e", "lane"))
            assertNull(LaneLinks.parse("lane://$host/$id?spaceId=../other", "lane"))
        }
        assertEquals(LaneLinks.Kind.AUTH, LaneLinks.parse("lane://auth/callback?code=x&state=y", "lane")?.kind)
        assertNull(LaneLinks.parse("lane://auth/other?code=x", "lane"))
    }
    @Test fun externalUrlsAreHttpsOnlyAndCannotCarryEmbeddedCredentials() {
        assertTrue(LaneHostModule.isHttps("https://example.com/path?value=1#section"))
        assertTrue(LaneHostModule.isHttps("HTTPS://example.com/"))
        for (value in listOf("http://example.com", "javascript:alert(1)", "file:///private/data", "lane://open",
            "https://user:secret@example.com/", "https://", "https://example.com\\@other.test")) assertFalse(LaneHostModule.isHttps(value))
    }
    @Test fun duplicateOAuthParametersAreRejected() {
        assertThrows(IllegalArgumentException::class.java) { LaneLinks.query("lane://auth/callback?state=a&state=b") }
        assertEquals("a+b", LaneLinks.query("lane://auth/callback?code=a%2Bb")["code"])
    }
    @Test fun nativeBridgeAssignsOriginAndPreservesMarkdown() {
        val request = JSONObject().put("title", "Task 🧪").put("prompt", "# Exact\n\u0000🧪")
            .put("meta", JSONObject().put("requestId", id).put("origin", JSONObject()
                .put("kind", "ORIGIN_KIND_MCP").put("accountId", "attacker")))
        LaneMethods.appRequest("CreateTask", request, id, "Android")
        val meta = request.getJSONObject("meta")
        assertEquals(id, meta.getString("requestId"))
        assertEquals("ORIGIN_KIND_APP", meta.getJSONObject("origin").getString("kind"))
        assertFalse(meta.getJSONObject("origin").has("accountId"))
        assertEquals("# Exact\n\u0000🧪", request.getString("prompt"))
    }
    @Test fun nativeExtensionsCannotBeInvokedFromJavaScript() {
        for (method in listOf("approveAccount", "acquireSync", "applySync", "claimNotifications", "exec", "poll"))
            assertFalse(method in LaneMethods.allowed)
    }
    @Test fun errorEnvelopeUsesProtoJsonNamesAndDecimalSequence() {
        val error = JSONObject(BridgeJson.error("ONLINE_REQUIRED", "Connect Lane"))
        assertFalse(error.getBoolean("ok"))
        assertEquals("ERROR_REASON_ONLINE_REQUIRED", error.getJSONObject("error").getString("reason"))
        assertEquals("9007199254740993", JSONObject(BridgeJson.success(sequence = "9007199254740993")).getString("sequence"))
    }
}
