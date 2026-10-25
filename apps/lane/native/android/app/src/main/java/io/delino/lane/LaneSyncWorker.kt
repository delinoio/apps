// SPDX-License-Identifier: Apache-2.0
package io.delino.lane

import android.content.Context
import androidx.work.Worker
import androidx.work.WorkerParameters

class LaneSyncWorker(context: Context, parameters: WorkerParameters) : Worker(context, parameters) {
    override fun doWork(): Result {
        val runtime = (applicationContext as LaneApplication).runtime
        val deadline = System.nanoTime() + java.util.concurrent.TimeUnit.MINUTES.toNanos(8)
        return try {
            // Includes cold process starts; no Activity or JavaScript is needed.
            runtime.submit { BridgeJson.response(runtime.workerOpen()); runtime.pollSafely() }.get()
            if (runtime.host.synchronize({ isStopped || System.nanoTime() >= deadline }, inputData.getString("spaceId"))) Result.success() else Result.retry()
        } catch (_: Exception) { Result.retry() }
    }
}
