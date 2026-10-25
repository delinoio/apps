// SPDX-License-Identifier: Apache-2.0
package io.delino.lane

import android.app.Activity
import android.os.Bundle
import android.util.Log
import android.view.ViewGroup
import com.lynx.tasm.LynxView
import com.lynx.tasm.TemplateData
import com.lynx.tasm.provider.AbsTemplateProvider

class MainActivity : Activity() {
    private lateinit var lynxView: LynxView

    override fun onCreate(state: Bundle?) {
        super.onCreate(state)
        val provider = object : AbsTemplateProvider() {
            override fun loadTemplate(url: String, callback: Callback) {
                if (url != "main.lynx.bundle") {
                    callback.onFailed("Unsupported template")
                    return
                }
                try {
                    callback.onSuccess(assets.open(url).use { it.readBytes() })
                } catch (error: Exception) {
                    Log.e("Lane", "{\"event\":\"lane_template_failed\"}", error)
                    callback.onFailed("Cannot read embedded Lane template")
                }
            }
        }
        lynxView = LynxView.builder().setTemplateProvider(provider).build(this)
        setContentView(lynxView, ViewGroup.LayoutParams(
            ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT))
        lynxView.renderTemplateUrl("main.lynx.bundle", TemplateData.empty())
    }

    override fun onResume() {
        super.onResume()
        lynxView.onEnterForeground()
    }
    override fun onPause() {
        lynxView.onEnterBackground()
        super.onPause()
    }
    override fun onDestroy() {
        lynxView.destroy()
        super.onDestroy()
    }
}
