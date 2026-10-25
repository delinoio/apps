// SPDX-License-Identifier: Apache-2.0
package io.delino.lane

import org.junit.Assert.*
import org.junit.Test

class OAuthProofTest {
    @Test fun s256MatchesRfc7636AppendixB() {
        assertEquals("E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM",
            OAuthProof.challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"))
    }
    @Test fun verifierAndStateHaveIndependentUnpaddedEntropy() {
        val values = (1..100).map { OAuthProof.randomToken() }
        assertEquals(100, values.toSet().size)
        assertTrue(values.all { Regex("[A-Za-z0-9_-]{43}").matches(it) })
    }
}
