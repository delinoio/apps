// SPDX-License-Identifier: Apache-2.0
package io.delino.anydash

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class PkceTest {
    @Test fun challengeMatchesRfc7636Example() {
        assertEquals("E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM",
            Pkce.challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"))
    }

    @Test fun verifierAndStateAreUnpaddedBase64Url() {
        val verifier = Pkce.verifier()
        assertEquals(43, verifier.length)
        assertTrue(verifier.matches(Regex("[A-Za-z0-9_-]+")))
        assertTrue(Pkce.state().matches(Regex("[A-Za-z0-9_-]{22}")))
    }

    @Test fun authorizationUrlRequestsBothResources() {
        val url = Pkce.authorizationUrl("https://auth.example", "app", "anydash://auth/callback", "c", "s",
            listOf("https://anydash.api.delino.io", "https://delcore.api.delino.io"))
        assertTrue(url.startsWith("https://auth.example/oidc/auth?client_id=app&"))
        assertTrue(url.contains("redirect_uri=anydash%3A%2F%2Fauth%2Fcallback"))
        assertTrue(url.contains("code_challenge_method=S256"))
        assertTrue(url.contains("resource=https%3A%2F%2Fanydash.api.delino.io&resource=https%3A%2F%2Fdelcore.api.delino.io"))
    }

    @Test fun readsSubjectWithoutTrustingIt() {
        val payload = java.util.Base64.getUrlEncoder().withoutPadding().encodeToString("{\"sub\":\"user-1\"}".toByteArray())
        assertEquals("user-1", Pkce.subject("h.$payload.s"))
        assertNull(Pkce.subject("not-a-token"))
    }
}
