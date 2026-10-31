package com.metrix.writer

import android.os.Handler
import android.os.Looper
import android.widget.Toast
import java.io.File
import java.util.UUID

/** Debug-only delivery fixture. Enabled solely by the independent instrumentation APK. */
internal object EditorFixture {
    @JvmField var enabled = false
    /**
     * Physical-phone acceptance (MW-ANDROID-03): instrumentation restarts the app and unbinds its
     * accessibility service, so the shell (DUMP holders only) triggers this fixture in the bound service.
     * Only fixed fixture texts exist; results are read from the content-free service dump.
     */
    private const val PROBE = "com.metrix.writer.debug.PHYSICAL_DELIVERY_PROBE"
    private val texts = mapOf("short" to "Writer test שלום 🌍", "plain" to "Writer test שלום, mixed English.",
        "multiline" to "Writer line one\nשלום line two", "emoji" to "Writer emoji 🌍",
        "long" to "Long scratch sentence שלום with mixed English. ".repeat(110))
    private var receiver: android.content.BroadcastReceiver? = null
    fun register(service: WriterInputService) {
        unregister(service)
        receiver = object : android.content.BroadcastReceiver() {
            override fun onReceive(context: android.content.Context, intent: android.content.Intent) {
                start(service, texts[intent.getStringExtra("text")] ?: texts.getValue("short"))
            }
        }.also { service.registerReceiver(it, android.content.IntentFilter(PROBE), android.Manifest.permission.DUMP, null, android.content.Context.RECEIVER_EXPORTED) }
    }
    fun unregister(service: WriterInputService) { receiver?.let { try { service.unregisterReceiver(it) } catch (_: Exception) { } }; receiver = null }
    fun start(service: WriterInputService) = start(service, texts.getValue("short"))
    private fun start(service: WriterInputService, text: String) {
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
