package com.metrix.writer.ui

import android.content.Context

/**
 * Floating-control feedback (MW-ANDROID-02 U-2, decided 2026-10-27): vibrate on start and stop on,
 * sound off, timer on. Presentation only; capture and delivery never read these.
 */
class FeedbackPreferences(context: Context) {
    private val prefs = context.getSharedPreferences("floating-feedback", Context.MODE_PRIVATE)
    var haptic: Boolean
        get() = prefs.getBoolean("haptic", true)
        set(value) { prefs.edit().putBoolean("haptic", value).apply() }
    /** A short start/stop tone. It plays only when the ringer is in normal mode, so silent mode stays silent. */
    var sound: Boolean
        get() = prefs.getBoolean("sound", false)
        set(value) { prefs.edit().putBoolean("sound", value).apply() }
    var timer: Boolean
        get() = prefs.getBoolean("timer", true)
        set(value) { prefs.edit().putBoolean("timer", value).apply() }
}
