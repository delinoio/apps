// SPDX-License-Identifier: Apache-2.0
package io.delino.lane

import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test

class AttachmentBoundaryTest {
    private fun status() = JSONObject().put("syncEnabled", true).put("accountId", "account-a").put("deviceId", "device-a")
    private fun identity() = JSONObject().put("account_id", "account-a").put("tenant_id", "tenant-a").put("device_id", "device-a")
    private fun info() = JSONObject().put("formatVersion", 2).put("accountId", "account-a").put("tenantId", "tenant-a").put("epoch", "epoch-a")
    @Test fun requiresFormatTwoAndPinnedAccountDeviceTenantEpoch() {
        AttachmentBoundary.validate(status(), identity(), info(), "epoch-a")
        for (format in listOf(0, 1, 99)) assertEquals("UNSUPPORTED_FORMAT",
            assertThrows(SyncBoundary.Failure::class.java) {
                AttachmentBoundary.validate(status(), identity(), info().put("formatVersion", format), "epoch-a")
            }.reason)
        for (key in listOf("accountId", "tenantId")) assertEquals("FORBIDDEN",
            assertThrows(SyncBoundary.Failure::class.java) {
                AttachmentBoundary.validate(status(), identity(), info().put(key, "other"), "epoch-a")
            }.reason)
        assertEquals("FORBIDDEN", assertThrows(SyncBoundary.Failure::class.java) {
            AttachmentBoundary.validate(status().put("deviceId", "other"), identity(), info(), "epoch-a")
        }.reason)
        assertEquals("STALE_EPOCH", assertThrows(SyncBoundary.Failure::class.java) {
            AttachmentBoundary.validate(status(), identity(), info(), "old")
        }.reason)
    }
    @Test fun disconnectedOrReconnectingSpaceCannotFetchImages() {
        for (state in listOf(status().put("syncEnabled", false), status().put("reconnectRequired", true)))
            assertEquals("ONLINE_REQUIRED", assertThrows(SyncBoundary.Failure::class.java) {
                AttachmentBoundary.validate(state, identity(), info(), "epoch-a")
            }.reason)
    }
}
