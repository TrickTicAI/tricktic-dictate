package com.metrix.writer

import android.os.Handler
import android.os.Looper
import android.widget.Toast
import java.io.File
import java.util.UUID

/** Debug-only delivery fixture. Enabled solely by the independent instrumentation APK. */
internal object EditorFixture {
    @JvmField var enabled = false
    fun start(service: WriterInputService) {
        val text = "Writer test שלום 🌍"
        val session = UUID.randomUUID().toString()
        val token = try { service.bind(session, true) } catch (_: Exception) { null }
        if (token == null) {
            Toast.makeText(service, "Editor cannot be verified. Finish typing and try again.", Toast.LENGTH_LONG).show()
            return
        }
        val deliveries = DeliveryStore(File(service.filesDir, "deliveries"))
        try { deliveries.load(token).also { it.put("queued_fixture", text); deliveries.save(it) } }
        catch (_: Exception) { return }
        Toast.makeText(service, "Delivery-only fixture in 3 seconds. No recording.", Toast.LENGTH_SHORT).show()
        val delivery = AndroidTextDelivery(service.applicationContext)
        Handler(Looper.getMainLooper()).postDelayed({
            try { delivery.deliver(session, text, token) }
            catch (_: Exception) { Toast.makeText(service, "Could not save the test result. Check $PRODUCT_NAME storage.", Toast.LENGTH_LONG).show() }
        }, 3000)
    }
}
