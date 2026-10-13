package com.metrix.writer

import android.content.Context
import android.os.Looper
import java.io.File

/** Main-thread delivery entrypoint for session owners, including when Accessibility is disconnected. */
class AndroidTextDelivery(context: Context) : TextDelivery {
    private val store = DeliveryStore(File(context.applicationContext.filesDir, "deliveries"))
    override fun deliver(sessionId: String, text: String, originalTarget: String): DeliveryResult {
        check(Looper.myLooper() == Looper.getMainLooper())
        WriterInputService.instance?.let { return it.deliver(sessionId, text, originalTarget) }
        val item = try { store.load(originalTarget) } catch (_: Exception) { return DeliveryResult.UNSUPPORTED }
        if (item.getString("session_id") != sessionId) return DeliveryResult.UNSUPPORTED
        if (!item.has("text")) {
            item.put("text", text).put("state", "recovery").put("reason", "Editor service unavailable; automatic delivery expired")
            store.save(item)
        }
        return if (item.optString("state") == "delivered") DeliveryResult.DELIVERED else DeliveryResult.PENDING_ORIGINAL_TARGET
    }
}
