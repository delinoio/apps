// SPDX-License-Identifier: Apache-2.0
package io.delino.anydash

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class DelcoreTokenTest {
    @Test fun onlyConnectAccountAndListSpacesCarryTheDelcoreToken() {
        assertTrue(AnyDashApi.needsDelcoreToken("anydash.v1.AccountService", "ConnectAccount"))
        assertTrue(AnyDashApi.needsDelcoreToken("anydash.v1.DashboardService", "ListSpaces"))
        assertFalse(AnyDashApi.needsDelcoreToken("anydash.v1.AccountService", "GetAccount"))
    }

    @Test fun appendsFieldOneWithAVarintLength() {
        assertArrayEquals(byteArrayOf(0x0a, 2, 'a'.code.toByte(), 'b'.code.toByte()),
            AnyDashApi.withDelcoreToken(ByteArray(0), "ab"))
        val long = AnyDashApi.withDelcoreToken(byteArrayOf(0x0a, 0), "j".repeat(300))
        assertArrayEquals(byteArrayOf(0x0a, 0, 0x0a, 0xac.toByte(), 0x02), long.copyOfRange(0, 5))
        assertTrue(long.size == 305)
    }
}
