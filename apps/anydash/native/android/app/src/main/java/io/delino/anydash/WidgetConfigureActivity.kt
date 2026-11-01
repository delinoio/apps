// SPDX-License-Identifier: Apache-2.0
package io.delino.anydash

import android.app.Activity
import android.appwidget.AppWidgetManager
import android.content.Intent
import android.os.Bundle
import android.widget.ArrayAdapter
import android.widget.ListView
import android.widget.TextView

/** Lets the user choose which in-app widget a home screen widget shows. Editing stays in the app. */
class WidgetConfigureActivity : Activity() {
    override fun onCreate(state: Bundle?) {
        super.onCreate(state)
        val id = intent?.extras?.getInt(AppWidgetManager.EXTRA_APPWIDGET_ID, AppWidgetManager.INVALID_APPWIDGET_ID)
            ?: AppWidgetManager.INVALID_APPWIDGET_ID
        setResult(RESULT_CANCELED, Intent().putExtra(AppWidgetManager.EXTRA_APPWIDGET_ID, id))
        if (id == AppWidgetManager.INVALID_APPWIDGET_ID) { finish(); return }
        val widgets = (application as AnyDashApplication).host.widgets
        val choices = widgets.all()
        if (choices.isEmpty()) {
            setContentView(TextView(this).apply {
                text = WidgetText.EMPTY
                setPadding(48, 48, 48, 48)
            })
            return
        }
        val list = ListView(this)
        list.adapter = ArrayAdapter(this, R.layout.widget_choice,
            choices.map { it.summary?.title ?: widgets.title(it.widgetId) })
        list.setOnItemClickListener { _, _, position, _ ->
            widgets.select(id, choices[position].widgetId)
            AnyDashWidgetProvider.render(this, AppWidgetManager.getInstance(this), id)
            setResult(RESULT_OK, Intent().putExtra(AppWidgetManager.EXTRA_APPWIDGET_ID, id))
            finish()
        }
        setContentView(list)
    }
}
