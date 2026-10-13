package com.metrix.writer

import android.view.MotionEvent
import android.view.Window

/** Block both full and partial tapjacking, including separate Compose dialog windows. */
internal fun Window.protectTouches(): () -> Unit {
    setHideOverlayWindows(true)
    val original = callback
    val guarded = object : Window.Callback by original {
        override fun dispatchTouchEvent(event: MotionEvent): Boolean {
            if (event.flags and (MotionEvent.FLAG_WINDOW_IS_OBSCURED or
                    MotionEvent.FLAG_WINDOW_IS_PARTIALLY_OBSCURED) != 0) return true
            return original.dispatchTouchEvent(event)
        }
    }
    callback = guarded
    return { if (callback === guarded) callback = original }
}
