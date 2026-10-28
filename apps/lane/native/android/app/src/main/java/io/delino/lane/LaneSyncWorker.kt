// SPDX-License-Identifier: Apache-2.0
package io.delino.lane

import android.content.Context
import androidx.work.Worker
import androidx.work.WorkerParameters

class LaneSyncWorker(context: Context, parameters: WorkerParameters) : Worker(context, parameters) {
    override fun doWork(): Result {
        val runtime = (applicationContext as LaneApplication).runtime
        val deadline = System.nanoTime() + java.util.concurrent.TimeUnit.MINUTES.toNanos(8)
        // Includes cold process starts; no Activity or JavaScript is needed.
        val stopped = { isStopped || System.nanoTime() >= deadline }
        return if (runtime.workerStoreLease().run(stopped) {
            runtime.host.synchronize(stopped, inputData.getString("spaceId"))
        }) Result.success() else Result.retry()
    }
}
