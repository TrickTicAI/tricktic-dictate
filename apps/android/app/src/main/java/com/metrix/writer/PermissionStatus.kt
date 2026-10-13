package com.metrix.writer

import android.Manifest
import android.app.NotificationManager
import android.content.ComponentName
import android.content.Context
import android.content.pm.PackageManager
import android.provider.Settings

data class PermissionStatus(val microphone: Boolean, val overlay: Boolean, val notifications: Boolean, val enabled: Boolean, val bound: Boolean) {
    companion object {
        fun read(context: Context): PermissionStatus {
            val target = ComponentName(context, WriterInputService::class.java)
            val enabled = Settings.Secure.getString(context.contentResolver, Settings.Secure.ENABLED_ACCESSIBILITY_SERVICES).orEmpty()
                .split(':').any { ComponentName.unflattenFromString(it) == target }
            return PermissionStatus(context.checkSelfPermission(Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED,
                Settings.canDrawOverlays(context), context.getSystemService(NotificationManager::class.java).areNotificationsEnabled(), enabled, WriterInputService.instance != null)
        }
    }
}

/**
 * The in-app Accessibility decision required by Play's AccessibilityService policy. Only an
 * explicit Accept or Decline is recorded; dismissing the disclosure records nothing. Until it is
 * accepted, WriterInputService stays connected but reads nothing and inserts nothing.
 */
class AccessibilityConsent(context: Context) {
    private val prefs = context.getSharedPreferences("writer-consent", Context.MODE_PRIVATE)
    val choice: String get() = prefs.getString("accessibility", "unset")!!
    val accepted get() = choice == "accepted"
    fun record(accepted: Boolean) {
        prefs.edit().putString("accessibility", if (accepted) "accepted" else "declined").putLong("accessibility_at_ms", System.currentTimeMillis()).apply()
    }
}
