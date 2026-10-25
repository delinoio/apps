// SPDX-License-Identifier: Apache-2.0
package io.delino.lane

import org.json.JSONObject
import org.junit.Assert.assertThrows
import org.junit.Test

class ReauthenticationBindingTest {
    private fun identity() = JSONObject().put("account_id", "account").put("device_id", "device").put("session_id", "session")
    private fun binding() = ReauthenticationBinding.create("generation", "space", identity(), "mode", "challenge", 1000)
    @Test fun localChallengeIsStrippedAndBothProofSpellingsAreRejected() {
        val request = JSONObject().put("challengeId", "challenge").put("mode", "mode")
        org.junit.Assert.assertEquals("challenge", ReauthenticationBinding.stripChallenge(request))
        org.junit.Assert.assertFalse(request.has("challengeId"))
        for (key in listOf("reauthProof", "reauth_proof")) assertThrows(IllegalArgumentException::class.java) {
            ReauthenticationBinding.stripChallenge(JSONObject().put("challengeId", "challenge").put(key, "untrusted"))
        }
    }
    @Test fun matchingBinding() {
        ReauthenticationBinding.validate(binding(), "generation", "space", identity(), "mode", "challenge", 2000)
    }
    @Test fun rejectsChangedAccountDeviceOrSession() {
        for (field in listOf("account_id", "device_id", "session_id")) assertThrows(IllegalArgumentException::class.java) {
            ReauthenticationBinding.validate(binding(), "generation", "space", identity().put(field, "other"), "mode", "challenge", 2000)
        }
    }
    @Test fun rejectsLateWrongModeChallengeSpaceAndGeneration() {
        for (field in listOf("generation", "space", "mode", "challenge")) assertThrows(IllegalArgumentException::class.java) {
            ReauthenticationBinding.validate(binding().put(field, "other"), "generation", "space", identity(), "mode", "challenge", 2000)
        }
        for (now in listOf(999L, 601001L)) assertThrows(IllegalArgumentException::class.java) {
            ReauthenticationBinding.validate(binding(), "generation", "space", identity(), "mode", "challenge", now)
        }
    }
}
