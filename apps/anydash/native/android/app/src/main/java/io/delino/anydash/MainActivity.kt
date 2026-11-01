// SPDX-License-Identifier: Apache-2.0
package io.delino.anydash

import android.app.Activity
import android.content.Intent
import android.content.res.Configuration
import android.net.Uri
import android.os.Bundle
import android.view.ViewGroup
import com.lynx.react.bridge.JavaOnlyArray
import com.lynx.react.bridge.JavaOnlyMap
import com.lynx.tasm.LynxView
import com.lynx.tasm.TemplateData
import com.lynx.tasm.provider.AbsTemplateProvider
import java.lang.ref.WeakReference

class MainActivity : Activity() {
    private lateinit var lynxView: LynxView
    private val host get() = (application as AnyDashApplication).host

    override fun onCreate(state: Bundle?) {
        super.onCreate(state)
        host.activity = WeakReference(this)
        val provider = object : AbsTemplateProvider() {
            override fun loadTemplate(url: String, callback: Callback) {
                if (url != BUNDLE) {
                    callback.onFailed("Unsupported template")
                    return
                }
                try {
                    callback.onSuccess(assets.open(url).use { it.readBytes() })
                } catch (_: Exception) {
                    AnyDashLog.error("anydash_template_failed")
                    callback.onFailed("Cannot read the embedded AnyDash template")
                }
            }
        }
        val builder = LynxView.builder().setTemplateProvider(provider)
        builder.registerModule("AnyDashHost", AnyDashHostModule::class.java, null)
        lynxView = builder.build(this)
        setContentView(lynxView, ViewGroup.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT))
        val dark = (resources.configuration.uiMode and Configuration.UI_MODE_NIGHT_MASK) == Configuration.UI_MODE_NIGHT_YES
        // Widgets launch with anydash://spaces/<space>/dashboards/<dashboard>?widget=<widget>.
        val initial = intent?.data?.takeIf { it.scheme == "anydash" && it.host != "auth" }?.toString() ?: "/"
        intent?.data?.takeIf { it.scheme == "anydash" && it.host == "auth" }?.let(host::completeSignIn)
        lynxView.updateGlobalProps(TemplateData.fromMap(mapOf("initialPath" to initial, "theme" to if (dark) "dark" else "light")))
        lynxView.renderTemplateUrl(BUNDLE, TemplateData.empty())
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        val uri: Uri = intent.data ?: return
        if (uri.scheme != "anydash") return
        if (uri.host == "auth") host.completeSignIn(uri)
        // The shared client validates every route; unknown links show "not found".
        else send("anydash.route", JavaOnlyMap().apply { putString("path", uri.toString()) })
    }

    override fun onResume() {
        super.onResume()
        host.activity = WeakReference(this)
        lynxView.onEnterForeground()
        host.resumed()
        send("anydash.visibility", JavaOnlyMap().apply { putBoolean("visible", true) })
    }

    override fun onPause() {
        send("anydash.visibility", JavaOnlyMap().apply { putBoolean("visible", false) })
        lynxView.onEnterBackground()
        super.onPause()
    }

    override fun onDestroy() {
        lynxView.destroy()
        super.onDestroy()
    }

    private fun send(name: String, value: JavaOnlyMap) = lynxView.sendGlobalEvent(name, JavaOnlyArray.of(value))

    private companion object { const val BUNDLE = "main.lynx.bundle" }
}
