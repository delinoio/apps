// SPDX-License-Identifier: Apache-2.0
package io.delino.lane

import org.json.JSONObject

/** Native transport binding, never a bearer proof or a domain deletion decision. */
internal object ReauthenticationBinding {
    fun stripChallenge(request: JSONObject): String {
        require(!request.has("reauthProof") && !request.has("reauth_proof"))
        val challenge = request.getString("challengeId")
        require(challenge.isNotEmpty())
        request.remove("challengeId")
        return challenge
    }
    fun create(generation: String, space: String, identity: JSONObject, mode: String, challenge: String, now: Long) =
        JSONObject().put("generation", generation).put("space", space).put("account", identity.getString("account_id"))
            .put("device", identity.getString("device_id")).put("session", identity.getString("session_id"))
            .put("mode", mode).put("challenge", challenge).put("createdAt", now)
    fun validate(binding: JSONObject, generation: String, space: String, identity: JSONObject,
        mode: String, challenge: String, now: Long) {
        require(binding.getString("generation") == generation && binding.getString("space") == space)
        require(binding.getString("account") == identity.getString("account_id"))
        require(binding.getString("device") == identity.getString("device_id"))
        require(binding.getString("session") == identity.getString("session_id"))
        require(binding.getString("mode") == mode && binding.getString("challenge") == challenge)
        require(now - binding.getLong("createdAt") in 0..600000)
    }
}
