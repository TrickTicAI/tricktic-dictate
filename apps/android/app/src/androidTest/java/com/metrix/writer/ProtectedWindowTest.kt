@file:Suppress("DEPRECATION")
package com.metrix.writer

import android.app.Dialog
import android.content.Intent
import android.os.SystemClock
import android.test.InstrumentationTestCase
import android.view.MotionEvent
import android.view.Window
import android.widget.Button

/** Injected flags verify application dispatch; separate-UID component tests live in AndroidBoundaryTest. */
class ProtectedWindowTest : InstrumentationTestCase() {
    fun testActivityAndDialogRejectFullAndPartialObscurationButAcceptOrdinaryTouches() {
        val activity = instrumentation.startActivitySync(Intent(instrumentation.targetContext, MainActivity::class.java)
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)) as MainActivity
        try {
            lateinit var button: Button
            var clicks = 0
            instrumentation.runOnMainSync {
                button = Button(activity).apply { text = "Synthetic protected action"; setOnClickListener { clicks++ } }
                activity.setContentView(button)
            }
            instrumentation.waitForIdleSync()
            fun check(window: Window, view: Button) {
                SystemClock.sleep(400)
                fun tap(flags: Int) {
                    instrumentation.runOnMainSync {
                        val location = IntArray(2); val origin = IntArray(2)
                        view.getLocationOnScreen(location); window.decorView.getLocationOnScreen(origin)
                        val x = (location[0] - origin[0] + view.width / 2).toFloat()
                        val y = (location[1] - origin[1] + view.height / 2).toFloat()
                        val now = SystemClock.uptimeMillis()
                        for (action in listOf(MotionEvent.ACTION_DOWN, MotionEvent.ACTION_UP)) {
                            val event = MotionEvent.obtain(now, now + if (action == MotionEvent.ACTION_UP) 30 else 0,
                                action, 1, arrayOf(MotionEvent.PointerProperties().apply { id = 0; toolType = MotionEvent.TOOL_TYPE_FINGER }),
                                arrayOf(MotionEvent.PointerCoords().apply { this.x = x; this.y = y; pressure = 1f; size = 1f }),
                                0, 0, 1f, 1f, 0, 0, android.view.InputDevice.SOURCE_TOUCHSCREEN, flags)
                            window.callback.dispatchTouchEvent(event); event.recycle()
                        }
                    }
                    instrumentation.waitForIdleSync()
                }
                val before = clicks
                tap(MotionEvent.FLAG_WINDOW_IS_OBSCURED)
                tap(MotionEvent.FLAG_WINDOW_IS_PARTIALLY_OBSCURED)
                assertEquals("Obscured action executed", before, clicks)
                tap(0)
                assertEquals("Ordinary touch was blocked", before + 1, clicks)
            }
            check(activity.window, button)
            lateinit var dialog: Dialog
            instrumentation.runOnMainSync {
                dialog = Dialog(activity)
                button = Button(activity).apply { text = "Synthetic delete"; setOnClickListener { clicks++ } }
                dialog.setContentView(button); dialog.show(); dialog.window!!.protectTouches()
            }
            instrumentation.waitForIdleSync()
            try { check(dialog.window!!, button) }
            finally { instrumentation.runOnMainSync { dialog.dismiss() } }
        } finally { instrumentation.runOnMainSync { activity.finish() } }
    }
}
