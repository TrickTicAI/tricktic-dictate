@file:Suppress("DEPRECATION")
package com.metrix.writer

import android.test.AndroidTestCase
import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.util.UUID

/** Processing/view contract: durable projection, live ordering and content exclusion. */
class ProcessingStateTest : AndroidTestCase() {
    private val attempt = UUID.randomUUID().toString()
    override fun setUp() { super.setUp(); ProcessingEvents.reset() }
    override fun tearDown() { ProcessingEvents.reset(); super.tearDown() }
    private fun session(state: String = "complete") = JSONObject().put("id", UUID.randomUUID().toString()).put("capture_state", state)
        .put("byte_length", 32044).put("attempts", JSONArray())
    private fun request(cleanup: String = "off") = JSONObject().put("idempotency_key", attempt).put("options", JSONObject()
        .put("selection", JSONObject().put("provider", "phone").put("model", "base")).put("cleanup", JSONObject().put("mode", cleanup)))
    private fun pending(item: JSONObject, timing: JSONObject? = null) = item.put("pending", request()).also { if (timing != null) it.put("pending_timing", timing) }
    private fun succeeded(item: JSONObject) = item.also {
        it.getJSONArray("attempts").put(JSONObject().put("id", attempt).put("request", request("super")).put("status", JSONObject().put("state", "succeeded")
            .put("result", JSONObject().put("transcript", JSONObject().put("raw_text", "private words"))))
            .put("dictionary_text", "private dictionary").put("cleaned_text", "private polish")
            .put("client_timing", JSONObject().put("provider_attempt", 1).put("asr_started_elapsed_ms", 10).put("asr_finished_elapsed_ms", 70).put("warm", "warm").put("model_load_ms", 2)))
    }
    private fun failed(item: JSONObject, message: String) = item.also {
        it.getJSONArray("attempts").put(JSONObject().put("id", attempt).put("request", request()).put("status", JSONObject().put("state", "failed")
            .put("error", JSONObject().put("message", message).put("retryable", true))))
    }
    private fun ledger(state: String) = JSONObject().put("token", UUID.randomUUID().toString()).put("state", state).put("reason", "fixed reason")
    private fun view(item: JSONObject, ledger: JSONObject? = null) = ProcessingState.view(item, ledger, ProcessingEvents.get(item.getString("id")), 1_000)

    fun testEveryStageProjects() {
        assertEquals(ProcessingStage.RECORDING, view(session("capturing")).stage)
        assertEquals(ProcessingStage.CANCELLED, view(session("discarded")).stage)
        assertEquals(ProcessingStage.SAVED, view(pending(session())).stage)
        val queued = pending(session()); ProcessingEvents.emit(queued.getString("id"), attempt, ProcessingStage.PREPARING, 900)
        assertEquals(ProcessingStage.PREPARING, view(queued).stage); assertEquals(100L, view(queued).stageElapsedMs)
        ProcessingEvents.emit(queued.getString("id"), attempt, ProcessingStage.RECOGNIZING)
        assertEquals(ProcessingStage.RECOGNIZING, view(queued).stage)
        val raw = succeeded(session()); assertEquals(ProcessingStage.RAW_READY, view(raw).stage)
        ProcessingEvents.emit(raw.getString("id"), attempt, ProcessingStage.POLISHING)
        assertEquals(ProcessingStage.POLISHING, view(raw, ledger("awaiting_result")).stage)
        ProcessingEvents.emit(raw.getString("id"), attempt, ProcessingStage.READY_TO_DELIVER)
        assertEquals(ProcessingStage.READY_TO_DELIVER, view(raw, ledger("unconfirmed")).stage)
        ProcessingEvents.finish(raw.getString("id"))
        assertEquals(ProcessingStage.INSERTED, view(raw, ledger("delivered")).stage)
        assertEquals(DeliveryState.DELIVERED, view(raw, ledger("delivered")).delivery.state)
        val recovery = view(raw, ledger("recovery"))
        assertEquals(ProcessingStage.RECOVERY, recovery.stage); assertEquals(ProcessingAction.COPY_FROM_HISTORY, recovery.error!!.action)
        assertEquals("delivery_unconfirmed", view(raw, ledger("unconfirmed")).error!!.code)
        val failure = view(failed(session(), "Phone model exceeded available memory; choose a smaller model"))
        assertEquals(ProcessingStage.FAILED, failure.stage); assertEquals(ProcessingAction.CHOOSE_SMALLER_MODEL, failure.error!!.action)
        assertEquals(ProcessingStage.CANCELLED, view(failed(session(), "Phone inference cancelled; audio retained")).stage)
    }

    fun testInterruptedWorkIsRecoveryNeverRunning() {
        // Started before process death: no live owner, so the user is offered a retry of saved audio.
        val item = pending(session(), JSONObject().put("request_elapsed_ms", 5).put("asr_started_elapsed_ms", 6))
        val stranded = view(item)
        assertEquals(ProcessingStage.RECOVERY, stranded.stage); assertEquals("interrupted", stranded.error!!.code)
        assertTrue(stranded.versions.any { it.kind == VersionKind.AUDIO })
        // While it does run, cloud engines' persisted markers lift the live stage.
        ProcessingEvents.emit(item.getString("id"), attempt, ProcessingStage.PREPARING)
        assertEquals(ProcessingStage.RECOGNIZING, view(item).stage); assertNull(view(item).error)
        val text = succeeded(session()).put("pending_postprocess", JSONObject())
        assertEquals("polish_incomplete", view(text).error!!.code)
        assertEquals(ProcessingStage.RECOVERY, view(session()).stage) // Saved, never transcribed.
        assertEquals(ProcessingAction.TRANSCRIBE_SAVED_AUDIO, view(session()).error!!.action)
    }

    fun testEventOrderingIsForwardOnlyPerAttempt() {
        val id = UUID.randomUUID().toString()
        assertTrue(ProcessingEvents.emit(id, attempt, ProcessingStage.PREPARING))
        assertFalse(ProcessingEvents.emit(id, attempt, ProcessingStage.SAVED))
        assertFalse(ProcessingEvents.emit(id, attempt, ProcessingStage.PREPARING))
        assertFalse(ProcessingEvents.emit(id, attempt, ProcessingStage.INSERTED)) // Durable facts never come from the hint.
        assertTrue(ProcessingEvents.emit(id, attempt, ProcessingStage.POLISHING))
        val retry = UUID.randomUUID().toString()
        assertTrue(ProcessingEvents.emit(id, retry, ProcessingStage.SAVED)) // A new attempt starts its own order.
        ProcessingEvents.finish(id, attempt); assertEquals(retry, ProcessingEvents.get(id)!!.attemptId) // Stale finish ignored.
        val revision = ProcessingEvents.revision.value
        ProcessingEvents.finish(id); assertNull(ProcessingEvents.get(id)); assertTrue(ProcessingEvents.revision.value > revision)
    }

    fun testLiveHintNeverOverridesDurableOutcomeOrOtherAttempt() {
        val item = failed(session(), "Phone inference failed; saved audio retained")
        ProcessingEvents.emit(item.getString("id"), attempt, ProcessingStage.RECOGNIZING)
        assertEquals(ProcessingStage.FAILED, view(item).stage)
        val other = pending(session())
        ProcessingEvents.emit(other.getString("id"), UUID.randomUUID().toString(), ProcessingStage.RECOGNIZING)
        assertEquals(ProcessingStage.SAVED, view(other).stage)
        val raw = succeeded(session())
        ProcessingEvents.emit(raw.getString("id"), attempt, ProcessingStage.READY_TO_DELIVER)
        assertEquals(ProcessingStage.INSERTED, view(raw, ledger("delivered")).stage)
    }

    fun testVersionsTimingsAndContentExclusion() {
        val item = succeeded(session()); failed(item, "private provider message with key sk-private")
        succeeded(item)
        val view = view(item, ledger("delivered").put("original_used", true))
        assertEquals(listOf(VersionKind.AUDIO, VersionKind.RAW, VersionKind.DICTIONARY, VersionKind.POLISHED, VersionKind.RAW, VersionKind.DICTIONARY, VersionKind.POLISHED), view.versions.map { it.kind })
        assertEquals(60L, view.timingsMs["asr"]); assertEquals(2L, view.timingsMs["model_load"]); assertEquals("warm", view.warm)
        assertTrue(view.delivery.originalUsed)
        val json = view.toJson().toString()
        assertFalse(json.contains("private")); assertFalse(json.contains("fixed reason"))
        val failure = view(failed(session(), "private provider message")).toJson().toString()
        assertFalse(failure.contains("private")); assertTrue(failure.contains("recognition_failed"))
    }

    fun testOlderSidecarsAndExplicitCodes() {
        // A pre-contract sidecar: no timing, no request key, unknown provider fields.
        val old = JSONObject().put("id", UUID.randomUUID().toString()).put("capture_state", "complete").put("attempts", JSONArray()
            .put(JSONObject().put("id", "server-job").put("status", JSONObject().put("state", "succeeded"))))
        val view = view(old)
        assertEquals(ProcessingStage.RAW_READY, view.stage); assertNull(view.attemptId); assertTrue(view.timingsMs.isEmpty())
        assertEquals("unknown", view.warm); assertEquals(DeliveryState.NOT_REQUESTED, view.delivery.state)
        assertEquals("quota", ProcessingState.classify(JSONObject().put("code", "quota").put("message", "x"), "y").code)
    }

    fun testFileViewReadsSidecarAndLedger() {
        val root = File(context.cacheDir, "processing-${UUID.randomUUID()}")
        try {
            val store = SessionStore(File(root, "recordings")); val deliveries = DeliveryStore(File(root, "deliveries"))
            val item = store.create("en"); val token = UUID.randomUUID().toString()
            deliveries.create(item.getString("id"), token, false)
            item.put("capture_state", "complete").put("original_target", token); succeeded(item); store.save(item)
            ProcessingEvents.emit(item.getString("id"), attempt, ProcessingStage.POLISHING)
            val view = ProcessingState.view(root, item.getString("id"))
            assertEquals(ProcessingStage.POLISHING, view.stage); assertEquals(DeliveryState.AWAITING_RESULT, view.delivery.state)
        } finally { root.deleteRecursively() }
    }
}
