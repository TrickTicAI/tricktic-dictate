package com.metrix.writer.securityoverlay

import android.app.Activity
import android.graphics.Color
import android.graphics.PixelFormat
import android.os.Bundle
import android.view.Gravity
import android.view.WindowManager
import android.widget.TextView

/** Separate app with only the explicit overlay permission. Never reads input or touches Writer data. */
class OverlayActivity : Activity() {
    companion object { private var overlay: TextView? = null }
    override fun onCreate(state: Bundle?) {
        super.onCreate(state)
        val manager = applicationContext.getSystemService(WindowManager::class.java)
        overlay?.let { manager.removeView(it) }; overlay = null
        if (!intent.getBooleanExtra("stop", false)) {
            check(android.provider.Settings.canDrawOverlays(this))
            overlay = TextView(applicationContext).apply {
                text = "Writer isolated overlay test"; setTextColor(Color.WHITE); setBackgroundColor(Color.MAGENTA)
            }.also { view ->
                manager.addView(view, WindowManager.LayoutParams(-1, 180,
                    WindowManager.LayoutParams.TYPE_APPLICATION_OVERLAY,
                    WindowManager.LayoutParams.FLAG_NOT_FOCUSABLE or WindowManager.LayoutParams.FLAG_NOT_TOUCHABLE,
                    PixelFormat.TRANSLUCENT).apply { gravity = Gravity.TOP; y = 250 })
            }
        }
        finish()
    }
}
