// SPDX-License-Identifier: Apache-2.0
package io.delino.lane

import java.util.concurrent.CountDownLatch
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit
import org.junit.Assert.*
import org.junit.Test

class StoreHandlesTest {
    private class Store {
        var active = "A"
        private val counts = mutableMapOf<String, Int>()
        val releases = mutableListOf<String>()

        @Synchronized fun open(space: String = active): String {
            counts[space] = (counts[space] ?: 0) + 1
            return space
        }
        @Synchronized fun close(space: String): String {
            val count = counts[space] ?: 0
            check(count > 0) { "A different owner was released" }
            counts[space] = count - 1
            releases.add(space)
            return "closed"
        }
        @Synchronized fun count(space: String) = counts[space] ?: 0
        @Synchronized fun canReset(space: String) = count(space) == 0
    }

    private fun session(store: Store, space: String): SessionStoreHandles =
        SessionStoreHandles().also { it.spaces[store.open(space)] = "device" }

    @Test fun twoSessionsReleaseTheirOwnHandlesInEveryCleanupOrder() {
        for (reverse in listOf(false, true)) for (destroyFirst in listOf(false, true)) {
            val store = Store()
            val first = session(store, "A")
            val second = session(store, "A")
            val owners = if (reverse) listOf(second, first) else listOf(first, second)
            assertEquals(2, store.count("A"))
            if (destroyFirst) owners[0].releaseAll { store.close(it) }
            else assertEquals("closed", owners[0].release("A", store::close))
            assertEquals(1, store.count("A"))
            assertFalse(store.canReset("A"))
            assertTrue(owners[1].spaces.containsKey("A"))
            assertEquals("closed", owners[1].release("A", store::close))
            assertTrue(store.canReset("A"))
            assertEquals(listOf("A", "A"), store.releases)
        }
    }

    @Test fun repeatedCleanupNeverConsumesAnotherSessionOrSpace() {
        val store = Store()
        val first = session(store, "A")
        val second = session(store, "A")
        val other = session(store, "B")
        assertEquals("closed", first.release("A", store::close))
        assertNull(first.release("A", store::close))
        first.releaseAll { store.close(it) }
        assertEquals(1, store.count("A"))
        assertEquals(1, store.count("B"))
        assertFalse(store.canReset("A"))
        assertFalse(store.canReset("B"))
        second.releaseAll { store.close(it) }
        assertTrue(store.canReset("A"))
        assertEquals(1, store.count("B"))
        other.releaseAll { store.close(it) }
        assertTrue(store.canReset("B"))
    }

    private fun lease(store: Store, execute: (() -> Unit) -> Unit, open: () -> String = { store.open() }) =
        WorkerStoreLease(execute, open) { store.close(it) }

    @Test fun completedRetryAndFailedWorkersBalanceOnlySuccessfulOpens() {
        val executor = Executors.newSingleThreadExecutor()
        try {
            val store = Store()
            val ui = session(store, "A")
            val execute: (() -> Unit) -> Unit = { executor.execute(it) }
            repeat(3) { assertTrue(lease(store, execute).run({ false }) { true }) }
            assertFalse(lease(store, execute).run({ false }) { false })
            assertFalse(lease(store, execute).run({ false }) { throw IllegalStateException("sync failed") })
            assertFalse(lease(store, execute) { throw IllegalStateException("open failed") }
                .run({ false }) { fail("Sync must not start after a failed open"); true })
            assertEquals(1, store.count("A"))
            assertEquals(5, store.releases.size)
            assertFalse(store.canReset("A"))
            ui.releaseAll { store.close(it) }
            assertTrue(store.canReset("A"))
        } finally { executor.shutdownNow() }
    }

    @Test fun stoppedWorkerSkipsSynchronizationAndReleasesItsHandle() {
        val executor = Executors.newSingleThreadExecutor()
        try {
            val store = Store()
            val result = lease(store, { executor.execute(it) }).run({ true }) {
                fail("Stopped work must not synchronize")
                true
            }
            assertFalse(result)
            assertTrue(store.canReset("A"))
        } finally { executor.shutdownNow() }
    }

    @Test fun interruptedWaiterReleasesOpenThatWasQueuedBeforeInterruption() {
        val executor = Executors.newSingleThreadExecutor()
        val gate = CountDownLatch(1)
        val queued = CountDownLatch(1)
        try {
            executor.execute { gate.await() }
            val store = Store()
            val worker = lease(store, { action -> queued.countDown(); executor.execute(action) })
            var result: Boolean? = null
            val waiter = Thread { result = worker.run({ false }) { true } }
            waiter.start()
            assertTrue(queued.await(5, TimeUnit.SECONDS))
            waiter.interrupt()
            gate.countDown()
            waiter.join(5000)
            assertFalse(waiter.isAlive)
            assertEquals(false, result)
            worker.close().get(5, TimeUnit.SECONDS)
            assertEquals(listOf("A"), store.releases)
            assertTrue(store.canReset("A"))
        } finally { gate.countDown(); executor.shutdownNow() }
    }

    @Test fun workerReleasesItsOriginalSpaceWhileOtherOwnersRemainLive() {
        val executor = Executors.newSingleThreadExecutor()
        val syncing = CountDownLatch(1)
        val finish = CountDownLatch(1)
        try {
            val store = Store()
            val uiA = session(store, "A")
            val worker = lease(store, { executor.execute(it) })
            var result: Boolean? = null
            val waiter = Thread { result = worker.run({ false }) {
                syncing.countDown()
                finish.await()
                true
            } }
            waiter.start()
            assertTrue(syncing.await(5, TimeUnit.SECONDS))
            uiA.releaseAll { store.close(it) }
            assertEquals(1, store.count("A"))
            assertFalse(store.canReset("A"))
            val resumedA = session(store, "A")
            store.active = "B"
            val uiB = session(store, "B")
            assertEquals(2, store.count("A"))
            assertEquals(1, store.count("B"))
            assertFalse(store.canReset("A"))
            finish.countDown()
            waiter.join(5000)
            assertEquals(true, result)
            assertEquals(1, store.count("A"))
            assertEquals(1, store.count("B"))
            resumedA.releaseAll { store.close(it) }
            assertTrue(store.canReset("A"))
            assertFalse(store.canReset("B"))
            uiB.releaseAll { store.close(it) }
            assertTrue(store.canReset("B"))
        } finally { finish.countDown(); executor.shutdownNow() }
    }
}
