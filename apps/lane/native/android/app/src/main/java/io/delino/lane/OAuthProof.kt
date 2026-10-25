// SPDX-License-Identifier: Apache-2.0
package io.delino.lane

import java.security.MessageDigest
import java.security.SecureRandom
import java.util.Base64

internal object OAuthProof {
    fun randomToken(): String = Base64.getUrlEncoder().withoutPadding().encodeToString(
        ByteArray(32).also { SecureRandom().nextBytes(it) })
    fun challenge(verifier: String): String = Base64.getUrlEncoder().withoutPadding().encodeToString(
        MessageDigest.getInstance("SHA-256").digest(verifier.toByteArray(Charsets.US_ASCII)))
}
