// SPDX-License-Identifier: Apache-2.0
package io.delino.lane

import org.json.JSONArray
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test

class SyncBoundaryTest {
    private fun deletion(account: String) = JSONObject().put("changes", JSONArray().put(JSONObject()
        .put("deletion", JSONObject().put("kind", "ENTITY_KIND_OR_REMOTE_KIND_ACCOUNT").put("id", account))))
    @Test fun explicitAccountDeletionMustMatchPinnedAuthenticatedAccount() {
        assertTrue(SyncBoundary.validatePull(deletion("account-a"), "account-a", true))
        assertEquals("FORBIDDEN", assertThrows(SyncBoundary.Failure::class.java) {
            SyncBoundary.validatePull(deletion("account-b"), "account-a", false)
        }.reason)
    }
    @Test fun changedEpochWithoutAnExplicitDirectiveNeverErasesLocalContent() {
        // CLEAR_SERVER_KEEP_LOCAL and unknown epochs have no authorized deletion record.
        for (response in listOf(JSONObject(), JSONObject().put("changes", JSONArray()),
            JSONObject().put("error", JSONObject().put("reason", "ERROR_REASON_STALE_EPOCH")))) {
            assertEquals("STALE_EPOCH", assertThrows(SyncBoundary.Failure::class.java) {
                SyncBoundary.validatePull(response, "account-a", true)
            }.reason)
        }
    }
    @Test fun deletionStatusRequiresEveryPinnedFieldAndKnownMode() {
        val identity = JSONObject().put("account_id", "account").put("device_id", "device")
        fun status() = JSONObject().put("account_id", "account").put("device_id", "device")
            .put("old_epoch", "old").put("new_epoch", "019bf000-0000-7000-8000-000000000001")
            .put("mode", "DATA_DELETION_MODE_DELETE_EVERYWHERE").put("disconnected", true)
        assertTrue(SyncBoundary.validatePull(SyncBoundary.deletionStatus(status(), identity, "old")!!, "account", true))
        assertNull(SyncBoundary.deletionStatus(status().put("mode", "DATA_DELETION_MODE_CLEAR_SERVER_KEEP_LOCAL"), identity, "old"))
        for (field in listOf("account_id", "device_id", "old_epoch", "mode", "new_epoch"))
            assertThrows(SyncBoundary.Failure::class.java) { SyncBoundary.deletionStatus(status().put(field, "other"), identity, "old") }
        assertThrows(SyncBoundary.Failure::class.java) { SyncBoundary.deletionStatus(status().put("disconnected", false), identity, "old") }
    }
    @Test fun ordinaryPullIsNotAnAccountDeletion() {
        assertFalse(SyncBoundary.validatePull(JSONObject().put("changes", JSONArray()), "account-a", false))
    }
    @Test fun oldGenerationMayNotSmuggleEntityDataBesideDeletionDirective() {
        val response = deletion("account-a")
        response.getJSONArray("changes").put(JSONObject().put("entity", JSONObject()))
        assertEquals("STALE_EPOCH", assertThrows(SyncBoundary.Failure::class.java) {
            SyncBoundary.validatePull(response, "account-a", true)
        }.reason)
    }
}
