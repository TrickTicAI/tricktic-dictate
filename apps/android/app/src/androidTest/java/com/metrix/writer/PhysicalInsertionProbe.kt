@file:Suppress("DEPRECATION")
package com.metrix.writer

import android.os.Bundle
import android.os.SystemClock
import android.test.InstrumentationTestCase
import android.test.InstrumentationTestRunner
import org.json.JSONObject
import java.io.File
import java.util.UUID

/**
 * Opt-in physical acceptance for a consented scratch editor that a person or adb already focused.
 * Run only with `-e scratch true`; any other run skips. Delivers the debug fixture through the real
 * service once, redelivers to prove no duplicate, and reports only content-free results. Editor text
 * is compared in memory and never logged or stored.
 */
class PhysicalInsertionProbe : InstrumentationTestCase() {
    private val text get() = (instrumentation as? InstrumentationTestRunner)?.arguments?.getString("text") ?: "Writer test שלום 🌍"
    private fun <T> main(action: () -> T): T {
        var value: T? = null; var error: Throwable? = null
        instrumentation.runOnMainSync { try { value = action() } catch (e: Throwable) { error = e } }
        error?.let { throw it }
        @Suppress("UNCHECKED_CAST") return value as T
    }
    private fun service(): WriterInputService {
        repeat(100) { WriterInputService.instance?.let { return it }; SystemClock.sleep(150) }
        fail("Writer service not connected"); throw IllegalStateException()
    }
    private data class Read(val text: String, val start: Int, val end: Int, val offset: Int)
    private fun read(): Read? = main {
        service().inputMethod?.currentInputConnection?.getSurroundingText(4096, 4096, 0)?.let {
            Read(it.text.toString(), it.selectionStart, it.selectionEnd, it.offset)
        }
    }

    fun testDeliverToFocusedScratchEditor() {
        if ((instrumentation as? InstrumentationTestRunner)?.arguments?.getString("scratch") != "true") return
        val app = instrumentation.targetContext.packageName
        check(app.endsWith(".debug")) { "Physical probe requires a debug app" }
        AccessibilityConsent(instrumentation.targetContext).record(true)
        service()
        repeat(40) { if (read() != null) return@repeat; SystemClock.sleep(250) }
        val before = read()
        val session = UUID.randomUUID().toString()
        val token = main { service().bind(session, true) }
        assertNotNull("Service must open a ledger", token)
        main { AndroidTextDelivery(instrumentation.targetContext).deliver(session, text, token!!) }
        SystemClock.sleep(2500)
        val store = DeliveryStore(File(instrumentation.targetContext.filesDir, "deliveries"))
        val ledger = store.load(token!!)
        val after = read()
        // Independent readback in this process: the fixture at the original caret, nothing else changed.
        val independent = if (before == null || after == null) "unreadable" else {
            val lo = minOf(before.start, before.end); val hi = maxOf(before.start, before.end)
            val exact = before.offset == 0 && after.offset == 0 &&
                after.text == before.text.substring(0, lo) + text + before.text.substring(hi) && after.start == lo + text.length && after.end == after.start
            if (exact) "exact" else if (after.text == before.text) "unchanged" else "changed_differently"
        }
        main { AndroidTextDelivery(instrumentation.targetContext).deliver(session, text, token) }
        SystemClock.sleep(1500)
        val afterDuplicate = read()
        val result = JSONObject()
            .put("outcome", DeliveryOutcome.of(ledger).code).put("state", ledger.optString("state"))
            .put("reason_code", ledger.optString("reason_code")).put("representation", ledger.optString("representation"))
            .put("selection_corroborated", ledger.opt("selection_corroborated")).put("diagnostic", ledger.optString("diagnostic"))
            .put("independent_readback", independent)
            .put("duplicate_changed_editor", after != null && afterDuplicate != null && afterDuplicate.text != after.text)
            .put("before_length", before?.text?.length).put("before_offset", before?.offset)
            .put("selection_was_range", before != null && before.start != before.end)
        instrumentation.sendStatus(0, Bundle().apply { putString("probe", result.toString()) })
        assertFalse("Redelivery must never insert again", result.getBoolean("duplicate_changed_editor"))
    }
}
