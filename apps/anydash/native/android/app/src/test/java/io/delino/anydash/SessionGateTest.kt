// SPDX-License-Identifier: Apache-2.0
package io.delino.anydash

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class SessionGateTest {
    @Test fun sessionReadWaitsForARecoveredExchange() {
        val gate = SessionGate()
        var signedIn = false
        val answers = mutableListOf<Boolean>()
        gate.begin()
        gate.read { answers += signedIn.also(gate::answered) }
        assertEquals(emptyList<Boolean>(), answers)
        signedIn = true
        assertFalse(gate.finish(signedIn = true, replied = false))
        assertEquals(listOf(true), answers)
    }

    @Test fun notifiesABundleThatAlreadyReadSignedOut() {
        val gate = SessionGate()
        gate.read { gate.answered(false) }
        gate.begin()
        assertTrue(gate.finish(signedIn = true, replied = false))
        gate.begin()
        assertFalse(gate.finish(signedIn = true, replied = false))
    }

    @Test fun leavesInProcessSignInAndFailuresToTheirReply() {
        val gate = SessionGate()
        gate.read { gate.answered(false) }
        gate.begin()
        assertFalse(gate.finish(signedIn = true, replied = true))
        gate.begin()
        assertFalse(gate.finish(signedIn = false, replied = false))
    }
}
