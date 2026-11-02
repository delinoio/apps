// SPDX-License-Identifier: Apache-2.0
package io.delino.anydash

/**
 * Orders the bundle's session reads around sign-in code exchanges. When
 * Android ends the process while the sign-in browser is open, the callback
 * starts a new process whose bundle has no pending `signIn` reply to resolve.
 * Session reads therefore wait for an in-flight exchange, and a bundle that
 * already read "signed out" is told to read the session again once a
 * recovered exchange succeeds.
 */
internal class SessionGate {
    private var exchanges = 0
    private val waiting = mutableListOf<() -> Unit>()
    private var answeredSignedOut = false

    /** Marks the start of a code exchange. */
    fun begin() = synchronized(this) { exchanges += 1 }

    /** Runs [respond] now or, while an exchange is in flight, after it ends. */
    fun read(respond: () -> Unit) {
        val now = synchronized(this) {
            if (exchanges > 0) {
                waiting += respond
                false
            } else true
        }
        if (now) respond()
    }

    /** Records the state that a session read returned to the bundle. */
    fun answered(signedIn: Boolean) = synchronized(this) { answeredSignedOut = !signedIn }

    /**
     * Ends a code exchange and answers the waiting reads. Returns true when the
     * bundle must be notified: the exchange signed the user in, no `signIn`
     * reply reported it, and the bundle last read "signed out".
     */
    fun finish(signedIn: Boolean, replied: Boolean): Boolean {
        val (ready, notify) = synchronized(this) {
            exchanges = maxOf(0, exchanges - 1)
            val ready = if (exchanges == 0) waiting.toList().also { waiting.clear() } else emptyList()
            val notify = signedIn && !replied && answeredSignedOut
            if (notify) answeredSignedOut = false
            ready to notify
        }
        ready.forEach { it() }
        return notify
    }
}
