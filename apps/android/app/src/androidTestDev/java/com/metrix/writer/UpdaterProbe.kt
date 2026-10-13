package com.metrix.writer

import android.content.ComponentName
import android.content.Context

/** Dev keeps the self-updater: forged install callbacks must reach a non-exported receiver only. */
object UpdaterProbe {
    const val PRESENT = true
    fun prime(context: Context) {
        // Keep activity startup's automatic feed check out of the callback-state assertion.
        // This only marks the isolated fixture's local check time; no live feed request is needed.
        context.getSharedPreferences(AndroidUpdater.PREFS_NAME, 0).edit()
            .putLong(AndroidUpdater.PREF_LAST_CHECKED_MS, System.currentTimeMillis()).commit()
    }
    fun receiverExported(context: Context) =
        context.packageManager.getReceiverInfo(ComponentName(context, InstallReceiver::class.java), 0).exported
    fun state(context: Context): Any = AndroidUpdater.getInstance(context).state.value
}
