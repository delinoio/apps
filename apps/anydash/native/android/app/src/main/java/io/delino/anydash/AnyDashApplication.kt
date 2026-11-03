// SPDX-License-Identifier: Apache-2.0
package io.delino.anydash

import android.app.Application
import com.lynx.tasm.LynxEnv
import com.lynx.tasm.behavior.Behavior
import com.lynx.tasm.behavior.LynxContext
import com.lynx.xelement.input.LynxUIInput
import com.lynx.xelement.input.LynxUIInputShadowNode

class AnyDashApplication : Application() {
    internal val host by lazy { AnyDashHost(this) }

    override fun onCreate() {
        super.onCreate()
        LynxEnv.inst().init(this, null, null, null)
        // Core Lynx does not register the XElement input; the code, token and
        // time zone fields need it before any template renders.
        LynxEnv.inst().addBehaviors(listOf(
            object : Behavior("input", false, false) {
                override fun createUI(context: LynxContext) = LynxUIInput(context)
                override fun createUIFiber(context: LynxContext) = createUI(context)
                override fun createShadowNode() = LynxUIInputShadowNode()
            },
        ))
        AnyDashLog.info("anydash_started")
    }
}
