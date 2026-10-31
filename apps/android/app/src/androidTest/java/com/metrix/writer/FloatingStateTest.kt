@file:Suppress("DEPRECATION")
package com.metrix.writer

import android.os.SystemClock
import android.test.AndroidTestCase
import com.metrix.writer.ui.*
import org.json.JSONArray
import org.json.JSONObject
import java.util.UUID

/**
 * Floating-control states derived from synthetic sessions and ledgers (MW-ANDROID-02 state mapping).
 * Covers the recovery permutations without an overlay; nothing here can insert text.
 */
class FloatingStateTest : AndroidTestCase() {
    private val attempt = UUID.randomUUID().toString()
    override fun setUp() { super.setUp(); ProcessingEvents.reset() }
    override fun tearDown() { ProcessingEvents.reset(); super.tearDown() }

    private fun session() = JSONObject().put("id", UUID.randomUUID().toString()).put("capture_state", "complete")
        .put("byte_length", 32044).put("attempts", JSONArray()).put("original_target", UUID.randomUUID().toString())
    private fun request(cleanup: String = "off") = JSONObject().put("idempotency_key", attempt).put("options", JSONObject()
        .put("selection", JSONObject().put("provider", "phone").put("model", "base")).put("cleanup", JSONObject().put("mode", cleanup)))
    private fun job(item: JSONObject) = item.getJSONArray("attempts").getJSONObject(0)
    private fun succeeded(item: JSONObject, choice: String = "cleaned", polished: Boolean = true) = item.also {
        it.getJSONArray("attempts").put(JSONObject().put("id", attempt).put("request", request("super")).put("delivery_choice", choice)
            .put("status", JSONObject().put("state", "succeeded").put("result", JSONObject().put("transcript", JSONObject().put("raw_text", "synthetic words"))))
            .put("dictionary_text", "synthetic words").also { job -> if (polished) job.put("cleaned_text", "Synthetic words.") })
    }
    private fun failed(item: JSONObject, message: String) = item.also {
        it.getJSONArray("attempts").put(JSONObject().put("id", attempt).put("request", request()).put("status", JSONObject().put("state", "failed")
            .put("error", JSONObject().put("message", message).put("retryable", true))))
    }
    private fun ledger(state: String, code: String = "", text: String? = "Synthetic words.\nSecond line") = JSONObject().put("token", UUID.randomUUID().toString())
        .put("session_id", UUID.randomUUID().toString()).put("state", state).put("reason_code", code).put("reason", "fixed").also { if (text != null) it.put("text", text) }
    private fun input(item: JSONObject, ledger: JSONObject? = null, now: Long = SystemClock.elapsedRealtime(), insertedSince: Long? = null, collapsed: Boolean = false) =
        FloatingInput(captureActive = false, capture = "idle", captureElapsedMs = 0, manager = "idle", managerBusy = false,
            tracked = Dictation.from(item, ledger = ledger), savedText = ledger?.optString("text")?.takeIf { ledger.has("text") }, phone = true,
            polishLimitMs = 3000, now = now, insertedSince = insertedSince, collapsed = collapsed)
    private fun state(item: JSONObject, ledger: JSONObject? = null, now: Long = SystemClock.elapsedRealtime(), insertedSince: Long? = null, collapsed: Boolean = false) =
        FloatingView.from(input(item, ledger, now, insertedSince, collapsed))
    private val idle = FloatingInput(captureActive = false, capture = "idle", captureElapsedMs = 0, manager = "idle", managerBusy = false, tracked = null)

    fun testRecordingShowsATimerAndCancelAsksFirst() {
        val recording = FloatingView.from(idle.copy(captureActive = true, capture = "recording", captureElapsedMs = 65_000))
        assertEquals(FloatingState.Recording(65_000), recording); assertEquals("1:05", formatClock(65_000))
        assertEquals("No duration cutoff", "1:00:01", formatClock(3_601_000))
        assertEquals(FloatingState.Starting, FloatingView.from(idle.copy(captureActive = true, capture = "starting")))
        assertEquals(FloatingState.Saving, FloatingView.from(idle.copy(captureActive = true, capture = "saving")))
        // ✕ never discards by itself: it opens the confirm, whose default is to continue.
        assertEquals(FloatingState.CancelConfirm(65_000), FloatingView.from(idle.copy(captureActive = true, capture = "recording", captureElapsedMs = 65_000, confirmingCancel = true)))
        assertEquals(FloatingState.Blocked("Allow microphone"), FloatingView.from(idle.copy(blocked = "Allow microphone")))
    }

    fun testTranscribingAndPolishingAreDistinctWithoutInventedProgress() {
        val queued = session().put("pending", request())
        ProcessingEvents.emit(queued.getString("id"), attempt, ProcessingStage.PREPARING)
        ProcessingEvents.emit(queued.getString("id"), attempt, ProcessingStage.RECOGNIZING)
        val working = state(queued) as FloatingState.Working
        assertEquals(ProcessingStage.RECOGNIZING, working.stage); assertNull("No engine reports progress today", working.progress)
        assertTrue(working.phone); assertNotNull(working.elapsedMs)
        val raw = succeeded(session())
        ProcessingEvents.emit(raw.getString("id"), attempt, ProcessingStage.POLISHING)
        val polishing = state(raw, ledger("awaiting_result", text = null)) as FloatingState.Polishing
        assertEquals(3000L, polishing.limitMs)
        assertTrue("The original still waits to be typed: Insert original now is offered", polishing.insertable)
        assertFalse("Nothing waits to be typed", (state(raw) as FloatingState.Polishing).insertable)
        // Keep audio, insert nothing: stopped on purpose, so neither a card nor a badge.
        val kept = session().put("capture_completion", JSONObject().put("kept", true))
        assertEquals("kept", Dictation.from(kept).view!!.error!!.code)
        assertNull(Dictation.from(kept).attention()); assertEquals(FloatingState.Idle, state(kept))
        assertEquals(FloatingState.Working(null, null, false, null), FloatingView.from(idle.copy(manager = "processing")))
    }

    fun testVerifiedInsertShowsBrieflyThenIdle() {
        val item = succeeded(session()); val delivered = ledger("delivered")
        val now = SystemClock.elapsedRealtime()
        assertEquals(FloatingState.Inserted(TextKind.POLISHED, false), state(item, delivered, now, insertedSince = now - 100))
        assertEquals(FloatingState.Idle, state(item, delivered, now, insertedSince = now - FloatingView.INSERTED_MS - 1))
        assertEquals("Not seen arriving: no pill", FloatingState.Idle, state(item, delivered, now, insertedSince = null))
        // Polishing produced nothing usable: the original was typed, and the pill says so.
        val original = succeeded(session(), polished = false)
        job(original).put("polish_outcome", JSONObject().put("original_used", true).put("reason", "provider_timeout"))
        val pill = state(original, ledger("delivered"), now, insertedSince = now) as FloatingState.Inserted
        assertTrue(pill.originalUsed); assertEquals(TextKind.ORIGINAL, pill.kind)
        assertEquals("Original inserted, verified", pill.announcement)
    }

    fun testSavedNotInsertedPersistsUntilHandled() {
        val item = succeeded(session()); val saved = ledger("recovery", "no_focused_editor")
        val recovery = state(item, saved) as FloatingState.NotInserted
        assertEquals(deliveryReason("no_focused_editor"), recovery.reason); assertEquals("Synthetic words.", recovery.preview)
        assertEquals("Persistent, never a 1.8 s chip", recovery, state(item, saved, SystemClock.elapsedRealtime() + 3_600_000))
        assertTrue(state(item, saved, collapsed = true) is FloatingState.Attention)
        assertEquals("Copied clears it", FloatingState.Idle, state(item, JSONObject(saved.toString()).put("recovered_at_ms", 1)))
        assertEquals("Handled clears it", FloatingState.Idle, state(item, JSONObject(saved.toString()).put("handled_at_ms", 1)))
    }

    fun testUnconfirmedHasNoPrimaryInsert() {
        val item = succeeded(session())
        val check = state(item, ledger("unconfirmed", "readback_mismatch"))
        assertTrue("Check the field, not Insert here", check is FloatingState.Unconfirmed)
        assertEquals("Check the field", check.announcement)
        assertEquals(FloatingState.Idle, state(item, ledger("unconfirmed", "readback_mismatch").put("handled_at_ms", 1)))
    }

    fun testFailureCancelledAndInterruptedNeverInsert() {
        val failure = state(failed(session(), "Cloud request failed: network unreachable")) as FloatingState.Failed
        assertTrue(failure.retryable); assertTrue(failure.reason.contains("network"))
        assertEquals("Stopped with audio kept returns to idle", FloatingState.Idle, state(failed(session(), "Phone inference cancelled; audio retained")))
        // Process death during work: History offers Transcribe again; the control shows nothing to insert.
        val interrupted = session().put("pending", request()).put("pending_timing", JSONObject().put("request_elapsed_ms", 5))
        assertEquals(FloatingState.Idle, state(interrupted))
        assertEquals(FloatingState.Idle, state(session())) // Saved audio, never transcribed.
    }

    fun testLatePolishIsNeverInserted() {
        // The original was typed; a later explicit polish succeeded. It is shown in History, never typed.
        val item = succeeded(session())
        job(item).put("polish_delivery", JSONObject().put("original_used", true).put("pasted", true))
            .put("polish_outcome", JSONObject().put("original_used", false))
        val d = Dictation.from(item, ledger = ledger("delivered"))
        assertTrue(d.latest!!.latePolish)
        val now = SystemClock.elapsedRealtime()
        val shown = state(item, ledger("delivered"), now, insertedSince = null)
        assertEquals(FloatingState.Idle, shown)
        assertFalse(shown is FloatingState.NotInserted || shown is FloatingState.Unconfirmed)
    }
}
