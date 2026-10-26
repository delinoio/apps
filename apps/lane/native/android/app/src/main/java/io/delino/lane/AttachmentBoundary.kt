// SPDX-License-Identifier: Apache-2.0
package io.delino.lane

import org.json.JSONObject

/** Validate server negotiation against the already-open local space, never UI ownership fields. */
internal object AttachmentBoundary {
    fun validate(status: JSONObject, identity: JSONObject, info: JSONObject, epoch: String) {
        if (info.optInt("formatVersion") != 2) throw SyncBoundary.Failure("UNSUPPORTED_FORMAT")
        if (!status.optBoolean("syncEnabled") || status.optBoolean("reconnectRequired"))
            throw SyncBoundary.Failure("ONLINE_REQUIRED")
        if (info.optString("epoch").isEmpty() || info.optString("epoch") != epoch)
            throw SyncBoundary.Failure("STALE_EPOCH")
        if (identity.optString("account_id").isEmpty() || identity.optString("tenant_id").isEmpty() ||
            identity.optString("device_id").isEmpty() ||
            identity.optString("account_id") != status.optString("accountId") ||
            identity.optString("device_id") != status.optString("deviceId") ||
            info.optString("accountId") != identity.optString("account_id") ||
            info.optString("tenantId") != identity.optString("tenant_id"))
            throw SyncBoundary.Failure("FORBIDDEN")
    }
}
