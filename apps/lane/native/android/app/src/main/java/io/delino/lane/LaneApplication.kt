// SPDX-License-Identifier: Apache-2.0
package io.delino.lane

import android.app.Application
import android.util.Log
import com.lynx.tasm.LynxEnv
import com.lynx.tasm.behavior.Behavior
import com.lynx.tasm.behavior.LynxContext
import com.lynx.xelement.input.LynxUIInput
import com.lynx.xelement.input.LynxUIInputShadowNode
import com.lynx.xelement.input.LynxUITextArea
import com.lynx.xelement.input.LynxUITextAreaShadowNode

class LaneApplication : Application() {
    internal val runtime by lazy { LaneRuntime(this) }
    override fun onCreate() {
        super.onCreate()
        LynxEnv.inst().init(this, null, null, null)
        // Core Lynx does not register XElement editors. Register their native views and
        // measurement nodes before any template renders; editors require UI-thread creation.
        LynxEnv.inst().addBehaviors(listOf(
            object : Behavior("input", false, false) {
                override fun createUI(context: LynxContext) = LynxUIInput(context)
                override fun createUIFiber(context: LynxContext) = createUI(context)
                override fun createShadowNode() = LynxUIInputShadowNode()
            },
            object : Behavior("textarea", false, false) {
                override fun createUI(context: LynxContext) = LynxUITextArea(context)
                override fun createUIFiber(context: LynxContext) = createUI(context)
                override fun createShadowNode() = LynxUITextAreaShadowNode()
            }
        ))
        Log.i("Lane", "{\"event\":\"lane_started\",\"platform\":\"android\"}")
    }
}
