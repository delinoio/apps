// SPDX-License-Identifier: Apache-2.0
package io.delino.lane

import java.util.concurrent.CompletableFuture
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.atomic.AtomicBoolean

/** Each session owns one native acquisition for each space in this map. */
internal class SessionStoreHandles {
    val spaces = ConcurrentHashMap<String, String>()

    fun release(space: String, close: (String) -> String): String? =
        if (spaces.remove(space) != null) close(space) else null

    fun releaseAll(close: (String) -> Unit) {
        spaces.keys.toList().forEach { space ->
            if (spaces.remove(space) != null) close(space)
        }
    }
}

/** Balances one worker bootstrap open, including an open that finishes after its waiter exits. */
internal class WorkerStoreLease(
    private val execute: (() -> Unit) -> Unit,
    private val open: () -> String,
    private val release: (String) -> Unit,
) {
    private val started = AtomicBoolean(false)
    private val closing = AtomicBoolean(false)
    private val acquired = CompletableFuture<String>()
    private val released = CompletableFuture<Unit>()

    private fun start() {
        if (!started.compareAndSet(false, true)) return
        try {
            execute {
                try { acquired.complete(open()) }
                catch (failure: Throwable) { acquired.completeExceptionally(failure) }
            }
        } catch (failure: Throwable) { acquired.completeExceptionally(failure) }
    }

    fun close(): CompletableFuture<Unit> {
        if (closing.compareAndSet(false, true)) acquired.whenComplete { space, failure ->
            if (failure != null) released.complete(Unit)
            else try {
                execute {
                    try { release(space); released.complete(Unit) }
                    catch (error: Throwable) { released.completeExceptionally(error) }
                }
            } catch (error: Throwable) { released.completeExceptionally(error) }
        }
        return released
    }

    fun run(stopped: () -> Boolean, synchronize: () -> Boolean): Boolean {
        var success = false
        var interrupted = false
        try {
            start()
            acquired.get()
            if (!stopped()) success = synchronize()
        } catch (_: InterruptedException) {
            interrupted = true
        } catch (_: Exception) {
            // WorkManager retries failed opens and synchronization attempts.
        } finally {
            // Wait for a queued open and its release before reporting work complete.
            val cleanup = close()
            while (true) {
                try { cleanup.get(); break }
                catch (_: InterruptedException) { interrupted = true }
                catch (_: Exception) { success = false; break }
            }
            if (interrupted) {
                Thread.currentThread().interrupt()
                success = false
            }
        }
        return success
    }
}
