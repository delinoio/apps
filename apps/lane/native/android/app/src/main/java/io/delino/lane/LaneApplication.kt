// SPDX-License-Identifier: Apache-2.0
package io.delino.lane

import android.app.Application
import android.util.Log
import com.lynx.tasm.LynxEnv

class LaneApplication : Application() {
    override fun onCreate() {
        super.onCreate()
        LynxEnv.inst().init(this, null, null, null)
        Log.i("Lane", "{\"event\":\"lane_started\",\"platform\":\"android\"}")
    }
}
