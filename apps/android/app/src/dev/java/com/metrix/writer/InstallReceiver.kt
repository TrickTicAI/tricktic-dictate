package com.metrix.writer

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.pm.PackageInstaller
import android.os.Build
import android.util.Log

class InstallReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        val updater = AndroidUpdater.getInstance(context)
        if (!updater.acceptsInstallStatus(intent)) return
        val status = intent.getIntExtra(PackageInstaller.EXTRA_STATUS, PackageInstaller.STATUS_FAILURE)
        val message = intent.getStringExtra(PackageInstaller.EXTRA_STATUS_MESSAGE)
        val sessionId = intent.getIntExtra(PackageInstaller.EXTRA_SESSION_ID, -1)
        Log.i("InstallReceiver", "Install status received: $status ($message), sessionId: $sessionId")

        when (status) {
            PackageInstaller.STATUS_PENDING_USER_ACTION -> {
                val confirmIntent = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                    intent.getParcelableExtra(Intent.EXTRA_INTENT, Intent::class.java)
                } else {
                    @Suppress("DEPRECATION")
                    intent.getParcelableExtra(Intent.EXTRA_INTENT)
                }
                if (confirmIntent != null) {
                    confirmIntent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
                    try {
                        context.startActivity(confirmIntent)
                    } catch (e: Exception) {
                        Log.e("InstallReceiver", "Failed to start install confirmation intent: ${e.message}", e)
                    }
                }
                updater.onPendingUserAction(sessionId, confirmIntent)
            }
            PackageInstaller.STATUS_SUCCESS -> {
                Log.i("InstallReceiver", "Installation successful for session $sessionId")
                updater.onInstallSuccess(sessionId)
            }
            PackageInstaller.STATUS_FAILURE_ABORTED -> {
                Log.i("InstallReceiver", "Installation aborted by user for session $sessionId")
                updater.onInstallAborted(sessionId, message)
            }
            else -> {
                Log.e("InstallReceiver", "Installation failed with status $status: $message")
                updater.onInstallFailed(sessionId, status, message)
            }
        }
    }

    companion object {
        const val EXTRA_SESSION_ID = "com.metrix.writer.extra.SESSION_ID"
        const val EXTRA_VERSION_NAME = "com.metrix.writer.extra.VERSION_NAME"
        const val EXTRA_VERSION_CODE = "com.metrix.writer.extra.VERSION_CODE"
    }
}
