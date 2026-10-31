package com.metrix.writer.ui

import android.content.Context
import android.os.Looper
import com.metrix.writer.DeliveryResult
import com.metrix.writer.DeliveryStore
import com.metrix.writer.PRODUCT_NAME
import com.metrix.writer.SessionStore
import com.metrix.writer.WriterInputService
import org.json.JSONObject
import java.io.File

/**
 * The explicit things a person can do with a saved result that was not inserted (D-4, D-5). Shared by
 * the floating control, Home, History, the entry and the recovery notification. Each writes only
 * additive, content-free marks on the delivery ledger; none of them inserts on its own.
 */
object RecoveryActions {
    /** The outcome of [insertHere]. [message] is the sentence to show; null when there is nothing to say yet. */
    enum class InsertHere(val message: String?) {
        /** The edit was issued and is being read back; the ledger records the verified outcome. */
        VERIFYING(null),
        ALREADY_INSERTED("Already inserted. Nothing was typed again."),
        /** The earlier insertion is unconfirmed: only "Insert anyway" may type it, because it may duplicate. */
        NEEDS_CONFIRMATION("Check the field first. Use Insert anyway only if the text is not there."),
        NO_FIELD("Tap the text field you want, then Insert here. Nothing was typed."),
        UNAVAILABLE("Text insertion is off or paused. Copy the text instead, or turn insertion on in $PRODUCT_NAME → Settings → Permissions."),
        NOTHING_SAVED("No saved text for this dictation. Open it in History."),
    }

    private fun deliveries(context: Context) = DeliveryStore(File(context.applicationContext.filesDir, "deliveries"))
    /** The session's delivery ledger, or null when it never had a target (a dictation recorded in the app). */
    private fun ledger(context: Context, sessionId: String): JSONObject? = runCatching {
        val token = SessionStore(File(context.applicationContext.filesDir, "recordings")).load(sessionId).optString("original_target")
        if (token.isEmpty()) null else deliveries(context).load(token)
    }.getOrNull()

    /**
     * Copies the saved result for [sessionId] to the clipboard and records `recovered_at_ms`, which
     * clears it from Needs attention. Returns the copied text, or null when nothing was saved.
     */
    fun copy(context: Context, sessionId: String): String? {
        val ledger = ledger(context, sessionId) ?: return null
        val text = runCatching { deliveries(context).recover(ledger.getString("token")) }.getOrNull() ?: return null
        copyToClipboard(context, "$PRODUCT_NAME saved result", text)
        RecoveryNotification.cancel(context, sessionId)
        return text
    }

    /** "Looks right" / "Mark handled": records `handled_at_ms` without copying or typing. Returns false when there is no ledger. */
    fun markHandled(context: Context, sessionId: String): Boolean {
        val ledger = ledger(context, sessionId) ?: return false
        return runCatching { deliveries(context).save(ledger.put("handled_at_ms", System.currentTimeMillis())) }.isSuccess
            .also { if (it) RecoveryNotification.cancel(context, sessionId) }
    }

    /**
     * Types the saved result into the field focused now, verified by read-back like an automatic
     * insertion. [anyway] is required when the earlier insertion is unconfirmed. Main thread only.
     */
    fun insertHere(context: Context, sessionId: String, anyway: Boolean): InsertHere {
        check(Looper.myLooper() == Looper.getMainLooper())
        val ledger = ledger(context, sessionId) ?: return InsertHere.NOTHING_SAVED
        if (!ledger.has("text")) return InsertHere.NOTHING_SAVED
        if (ledger.optString("state") == "delivered") return InsertHere.ALREADY_INSERTED
        if (ledger.optString("state") == "unconfirmed" && !anyway) return InsertHere.NEEDS_CONFIRMATION
        val service = WriterInputService.instance ?: return InsertHere.UNAVAILABLE
        return when (service.insertSaved(sessionId, anyway)) {
            DeliveryResult.DELIVERED -> InsertHere.ALREADY_INSERTED
            DeliveryResult.PENDING_ORIGINAL_TARGET -> InsertHere.VERIFYING.also { RecoveryNotification.cancel(context, sessionId) }
            DeliveryResult.UNSUPPORTED -> if (service.lastManualCheck == "insertion_paused") InsertHere.UNAVAILABLE else InsertHere.NO_FIELD
        }
    }
}
