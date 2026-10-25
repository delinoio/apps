// SPDX-License-Identifier: Apache-2.0
package io.delino.lane

import org.json.JSONObject

/** Transport/account binding checks. Deletion and merge rules remain in the C++ engine. */
internal object SyncBoundary {
    class Failure(val reason: String) : Exception("Lane synchronization requires attention")
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
