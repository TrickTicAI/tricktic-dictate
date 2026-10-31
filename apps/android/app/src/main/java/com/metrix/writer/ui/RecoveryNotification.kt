package com.metrix.writer.ui

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.net.Uri
import com.metrix.writer.HistoryActivity
import com.metrix.writer.PRODUCT_NAME
import com.metrix.writer.R

/**
 * The persistent "Saved, not inserted" / "Check the field" notification (D-4). It replaces the old
 * Toast, stays until the result is copied, inserted or handled, and shows no dictated text: its
 * content is the reason only, and the lock screen shows a public version without even that.
 *
 * Actions open [HistoryActivity] with an extra; MainActivity handles them:
 * - [EXTRA_ROUTE] = "History/<sessionId>" opens the entry;
 * - [EXTRA_COPY] = sessionId copies the saved result (through [RecoveryActions.copy]) and opens the entry.
 */
object RecoveryNotification {
    const val CHANNEL = "writer-recovery"
    const val EXTRA_ROUTE = "writer_route"
    const val EXTRA_COPY = "writer_copy_session"

    private fun id(sessionId: String) = ("recovery:$sessionId").hashCode()

    /** Posts or updates the notification for [sessionId]. [unconfirmed] selects "Check the field". Never throws. */
    fun post(context: Context, sessionId: String, unconfirmed: Boolean, reason: String) {
        try {
            val app = context.applicationContext
            val notifications = app.getSystemService(NotificationManager::class.java)
            notifications.createNotificationChannel(NotificationChannel(CHANNEL, "Saved results not inserted", NotificationManager.IMPORTANCE_DEFAULT).apply {
                description = "A dictation was saved but not typed into a field. Copy it or open it in $PRODUCT_NAME."
                lockscreenVisibility = Notification.VISIBILITY_PRIVATE
            })
            fun intent(action: String, extra: String, value: String, code: Int) = PendingIntent.getActivity(app, id(sessionId) + code,
                Intent(app, HistoryActivity::class.java).setAction(action).setData(Uri.parse("writer-recovery://$sessionId/$action"))
                    .putExtra(extra, value).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TOP),
                PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
            val open = intent("open", EXTRA_ROUTE, "History/$sessionId", 1)
            val copy = intent("copy", EXTRA_COPY, sessionId, 2)
            val title = if (unconfirmed) "Check the field" else "Saved, not inserted"
            val text = if (unconfirmed) "It may already be there. Copy it only if it is missing." else "$reason Nothing was typed."
            val public = Notification.Builder(app, CHANNEL).setSmallIcon(R.drawable.ic_writer_wave)
                .setContentTitle(PRODUCT_NAME).setContentText("A dictation is saved for you").build()
            notifications.notify(id(sessionId), Notification.Builder(app, CHANNEL).setSmallIcon(R.drawable.ic_writer_wave)
                .setContentTitle(title).setContentText(text).setStyle(Notification.BigTextStyle().bigText(text))
                .setContentIntent(open).setOngoing(false).setAutoCancel(false).setOnlyAlertOnce(true)
                .setVisibility(Notification.VISIBILITY_PRIVATE).setPublicVersion(public)
                .addAction(Notification.Action.Builder(null, "Copy", copy).build())
                .addAction(Notification.Action.Builder(null, "Open", open).build()).build())
        } catch (_: Exception) { /* The result is durable in History either way. */ }
    }

    fun cancel(context: Context, sessionId: String) {
        runCatching { context.applicationContext.getSystemService(NotificationManager::class.java).cancel(id(sessionId)) }
    }
}
