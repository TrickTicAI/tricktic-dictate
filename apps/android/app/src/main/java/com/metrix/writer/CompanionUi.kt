package com.metrix.writer

import android.app.Activity
import android.view.View
import android.view.WindowInsets
import android.view.WindowInsetsController

object CompanionUi {
    @JvmStatic fun languageName(code: String): String = when (code) { "en" -> "English"; "he" -> "Hebrew"; "mixed" -> "Hebrew + English"; else -> code }
    @JvmStatic fun install(activity: Activity, view: View) {
        view.setOnApplyWindowInsetsListener { content, insets ->
            val safe = insets.getInsets(WindowInsets.Type.systemBars() or WindowInsets.Type.displayCutout() or WindowInsets.Type.ime())
            content.setPadding(safe.left, safe.top, safe.right, safe.bottom)
            insets
        }
        activity.setContentView(view)
        activity.window.insetsController?.setSystemBarsAppearance(
            WindowInsetsController.APPEARANCE_LIGHT_STATUS_BARS or WindowInsetsController.APPEARANCE_LIGHT_NAVIGATION_BARS,
            WindowInsetsController.APPEARANCE_LIGHT_STATUS_BARS or WindowInsetsController.APPEARANCE_LIGHT_NAVIGATION_BARS)
        view.requestApplyInsets()
    }
}
