// SPDX-License-Identifier: Apache-2.0
package io.delino.lane

import android.Manifest
import android.content.Intent
import android.app.Activity
import android.os.Build
import android.content.pm.PackageManager
import android.os.Bundle
import android.util.Log
import android.view.ViewGroup
import com.lynx.tasm.LynxView
import com.lynx.tasm.TemplateData
import com.lynx.tasm.provider.AbsTemplateProvider

class MainActivity : Activity() {
    private lateinit var lynxView: LynxView
    private lateinit var session: LaneSession
    private val images = LaneImageFetcher()
    private var permissionInFlight = false

    override fun onCreate(state: Bundle?) {
        super.onCreate(state)
        session = LaneSession((application as LaneApplication).runtime, this)
        val provider = object : AbsTemplateProvider() {
            override fun loadTemplate(url: String, callback: Callback) {
                if (url != "main.lynx.bundle") {
                    callback.onFailed("Unsupported template")
                    return
                }
                try {
                    callback.onSuccess(assets.open(url).use { it.readBytes() })
                } catch (error: Exception) {
                    Log.e("Lane", "{\"event\":\"lane_template_failed\"}")
                    callback.onFailed("Cannot read embedded Lane template")
                }
            }
        }
        val builder = LynxView.builder().setTemplateProvider(provider).setImageFetcher(images)
        builder.registerModule("LaneStore", LaneStoreModule::class.java, session)
        builder.registerModule("LaneHost", LaneHostModule::class.java, session)
        lynxView = builder.build(this)
        session.view = lynxView
        setContentView(lynxView, ViewGroup.LayoutParams(
            ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT))
        intent?.dataString?.let { session.runtime.receiveLink(it) }
        lynxView.renderTemplateUrl("main.lynx.bundle", TemplateData.empty())
    }

    override fun onResume() {
        super.onResume()
        lynxView.onEnterForeground()
        session.runtime.foreground(session)
        session.runtime.invalidatePermissions()
    }
    override fun onPause() {
        session.runtime.background(session)
        lynxView.onEnterBackground()
        super.onPause()
    }
    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        intent.dataString?.let { session.runtime.receiveLink(it) }
    }
    internal fun requestLaneNotificationPermission() {
        if (isFinishing || permissionInFlight) return
        session.runtime.notifications.requested()
        if (Build.VERSION.SDK_INT >= 33 && checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED) {
            permissionInFlight = true
            requestPermissions(arrayOf(Manifest.permission.POST_NOTIFICATIONS), 7001)
        } else session.runtime.invalidatePermissions()
    }
    override fun onRequestPermissionsResult(requestCode: Int, permissions: Array<out String>, grantResults: IntArray) {
        super.onRequestPermissionsResult(requestCode, permissions, grantResults)
        if (requestCode == 7001) { permissionInFlight = false; session.runtime.invalidatePermissions() }
    }
    override fun onDestroy() {
        session.runtime.destroy(session)
        lynxView.destroy()
        images.close()
        super.onDestroy()
    }
}
