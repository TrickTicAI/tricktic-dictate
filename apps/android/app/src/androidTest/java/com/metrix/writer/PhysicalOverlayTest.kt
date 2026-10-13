@file:Suppress("DEPRECATION")
package com.metrix.writer

import android.app.Dialog
import android.app.UiAutomation
import android.content.Intent
import android.graphics.Color
import android.os.SystemClock
import android.test.InstrumentationTestCase
import android.widget.Button

/** Real differently signed TYPE_APPLICATION_OVERLAY; only the isolated phone package is allowed. */
class PhysicalOverlayTest : InstrumentationTestCase() {
    fun testExternalOverlayVisibleOverEditorButHiddenOverWriterAndDialog() {
        val context = instrumentation.targetContext
        check(context.packageName == "com.metrix.writer.securityphone")
        val ui = instrumentation.getUiAutomation(UiAutomation.FLAG_DONT_SUPPRESS_ACCESSIBILITY_SERVICES)
        fun start(pkg: String, name: String) = context.startActivity(Intent().setClassName(pkg, name).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
        fun hasMagenta(): Boolean {
            SystemClock.sleep(1800)
            val picture = ui.takeScreenshot() ?: error("Screenshot unavailable")
            try {
                var count = 0
                for (y in 0 until picture.height step 4) for (x in 0 until picture.width step 4)
                    if (picture.getPixel(x, y).let { Color.red(it) > 180 && Color.blue(it) > 180 && Color.green(it) < 130 }) count++
                return count > 1000
            } finally { picture.recycle() }
        }
        start("com.metrix.writer.editor", "com.metrix.writer.editor.EditorActivity")
        SystemClock.sleep(1500)
        start("com.metrix.writer.securityoverlay", "com.metrix.writer.securityoverlay.OverlayActivity")
        try {
            assertTrue("Independent overlay positive control absent", hasMagenta())
            val activity = instrumentation.startActivitySync(Intent(context, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
            try {
                assertFalse("External overlay remained over Writer", hasMagenta())
                lateinit var dialog: Dialog
                instrumentation.runOnMainSync {
                    dialog = Dialog(activity)
                    dialog.setContentView(Button(activity).apply { text = "Synthetic destructive confirmation" })
                    dialog.show(); dialog.window!!.protectTouches()
                }
                try { assertFalse("External overlay remained over dialog", hasMagenta()) }
                finally { instrumentation.runOnMainSync { dialog.dismiss() } }
            } finally { instrumentation.runOnMainSync { activity.finish() } }
        } finally {
            context.startActivity(Intent().setClassName("com.metrix.writer.securityoverlay", "com.metrix.writer.securityoverlay.OverlayActivity")
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK).putExtra("stop", true))
        }
    }
}
