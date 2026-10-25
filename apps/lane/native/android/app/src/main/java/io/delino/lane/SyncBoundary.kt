// SPDX-License-Identifier: Apache-2.0
package io.delino.lane

import org.json.JSONObject

/** Transport/account binding checks. Deletion and merge rules remain in the C++ engine. */
internal object SyncBoundary {
    class Failure(val reason: String) : Exception("Lane synchronization requires attention")
    fun deletionStatus(status: JSONObject, identity: JSONObject, oldEpoch: String): JSONObject? {
        if (status.optString("account_id") != identity.getString("account_id") ||
            status.optString("device_id") != identity.getString("device_id") ||
            status.optString("old_epoch") != oldEpoch || !status.optBoolean("disconnected")) throw Failure("FORBIDDEN")
        val epoch = status.getString("new_epoch")
        if (!LaneLinks.validId(epoch) || epoch == oldEpoch) throw Failure("STALE_EPOCH")
        return when (status.getString("mode")) {
            "DATA_DELETION_MODE_DELETE_EVERYWHERE" -> JSONObject().put("nextCursor", "").put("hasMore", false)
                .put("changes", org.json.JSONArray().put(JSONObject().put("deletion", JSONObject()
                    .put("kind", "ENTITY_KIND_OR_REMOTE_KIND_ACCOUNT").put("id", identity.getString("account_id"))
                    .put("deletionVersionId", epoch))))
            "DATA_DELETION_MODE_CLEAR_SERVER_KEEP_LOCAL" -> null
            else -> throw Failure("STALE_EPOCH")
        }
    }
    fun validatePull(response: JSONObject, authenticatedAccount: String, changedEpoch: Boolean): Boolean {
        var accountDeletion = false
        val changes = response.optJSONArray("changes")
        if (changes != null) for (index in 0 until changes.length()) {
            val deletion = changes.getJSONObject(index).optJSONObject("deletion")
            val kind = deletion?.opt("kind")
            val isAccount = kind == "ENTITY_KIND_OR_REMOTE_KIND_ACCOUNT" || kind == 5
            if (isAccount) {
                if (authenticatedAccount.isEmpty() || deletion!!.optString("id") != authenticatedAccount)
                    throw Failure("FORBIDDEN")
                accountDeletion = true
            } else if (changedEpoch) throw Failure("STALE_EPOCH")
        }
        // A new server epoch, a failed pull, or an empty response never authorizes erasure.
        if (changedEpoch && !accountDeletion) throw Failure("STALE_EPOCH")
        return accountDeletion
    }
}
