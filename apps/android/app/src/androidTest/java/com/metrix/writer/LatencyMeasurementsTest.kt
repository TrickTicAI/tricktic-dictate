@file:Suppress("DEPRECATION")
package com.metrix.writer

import android.test.AndroidTestCase
import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.util.UUID

class LatencyMeasurementsTest : AndroidTestCase() {
    private fun session() = JSONObject().put("id", UUID.randomUUID().toString()).put("capture_state", "complete")
        .put("startup_timing", JSONObject().put("tap_elapsed_ms", 10).put("first_audio_read_elapsed_ms", 25))
    private fun job(attempt: String = UUID.randomUUID().toString()) = JSONObject().put("request", JSONObject()
        .put("idempotency_key", attempt).put("options", JSONObject().put("selection", JSONObject().put("provider", "local").put("model", "arbitrary private custom model"))
            .put("cleanup", JSONObject().put("mode", "off"))))
        .put("status", JSONObject().put("state", "succeeded").put("raw_text", "private short text"))
        .put("client_timing", JSONObject().put("provider_attempt", 1).put("queued_elapsed_ms", 30).put("request_elapsed_ms", 45)
            .put("asr_started_elapsed_ms", 50).put("asr_finished_elapsed_ms", 80).put("stop_to_raw_ms", 100)
            .put("credential", "private key").put("context", JSONObject().put("app", "private application")))
    fun testMissingStagesAndStrictContentExclusion() {
        val row = LatencyMeasurements.project(session(), job())
        assertTrue(LatencyMeasurements.valid(row))
        assertFalse(row.toString().contains("private"))
        val stages = row.getJSONObject("stages_ms")
        assertEquals(15L, stages.getLong("capture_readiness")); assertEquals(30L, stages.getLong("asr"))
        assertFalse(stages.has("upload")); assertFalse(stages.has("insertion"))
        row.put("raw_text", "hi"); assertFalse(LatencyMeasurements.valid(row))
    }
    fun testFailureHasNoRawAvailabilityAndRebootMissingEndpoints() {
        val job = job(); job.getJSONObject("status").put("state", "failed")
        job.getJSONObject("client_timing").remove("queued_elapsed_ms")
        val stages = LatencyMeasurements.project(session(), job).getJSONObject("stages_ms")
        assertFalse(stages.has("raw_availability")); assertFalse(stages.has("queue"))
    }
    private fun phoneJob(state: String, warm: String?) = JSONObject().put("request", JSONObject()
        .put("idempotency_key", UUID.randomUUID().toString()).put("options", JSONObject().put("selection", JSONObject().put("provider", "phone").put("model", "base"))
            .put("cleanup", JSONObject().put("mode", "super"))))
        .put("execution_location", "phone").put("status", JSONObject().put("state", state))
        .put("client_timing", JSONObject().put("provider_attempt", 1).put("asr_started_elapsed_ms", 100).put("model_load_ms", 40)
            .also { t -> warm?.let { t.put("warm", it) }; if (state == "succeeded") t.put("asr_finished_elapsed_ms", 900).put("stop_to_raw_ms", 1000) })
    fun testProductionPhoneProviderWarmthAndFailedAttempts() {
        val cold = LatencyMeasurements.project(session(), phoneJob("succeeded", "cold"))
        assertTrue(LatencyMeasurements.valid(cold))
        assertEquals("phone", cold.getString("provider")); assertEquals("cold", cold.getString("warm"))
        assertEquals(40L, cold.getJSONObject("stages_ms").getLong("model_load")); assertEquals(800L, cold.getJSONObject("stages_ms").getLong("asr"))
        assertEquals("succeeded", cold.getJSONObject("stage_outcomes").getString("asr"))
        assertEquals("warm", LatencyMeasurements.project(session(), phoneJob("succeeded", "warm")).getString("warm"))
        // Older rows without measured warmth stay unknown rather than claiming cold.
        assertEquals("unknown", LatencyMeasurements.project(session(), phoneJob("succeeded", null)).getString("warm"))
        val failed = LatencyMeasurements.project(session(), phoneJob("failed", "cold"))
        assertTrue(LatencyMeasurements.valid(failed)); assertEquals("failed", failed.getString("outcome"))
        assertEquals("failed", failed.getJSONObject("stage_outcomes").getString("asr"))
        assertFalse(failed.getJSONObject("stages_ms").has("asr")); assertFalse(failed.getJSONObject("stages_ms").has("raw_availability"))
        assertEquals(40L, failed.getJSONObject("stages_ms").getLong("model_load"))
    }
    fun testPendingPhoneRequestIsExportedAndReplacedByItsResult() {
        val root = File(context.cacheDir, "latency-${UUID.randomUUID()}"); val directory = File(root, "recordings").apply { mkdirs() }
        try {
            val item = session().put("attempts", JSONArray()); val job = phoneJob("succeeded", "warm")
            item.put("pending", job.getJSONObject("request")).put("pending_timing", JSONObject().put("provider_attempt", 1).put("request_elapsed_ms", 60))
            LatencyMeasurements.capture(directory, item)
            val pending = LatencyMeasurements.read(directory).getJSONArray("records")
            assertEquals(1, pending.length()); assertEquals("pending", pending.getJSONObject(0).getString("outcome"))
            assertEquals("phone", pending.getJSONObject(0).getString("provider"))
            item.remove("pending"); item.remove("pending_timing"); item.getJSONArray("attempts").put(job)
            LatencyMeasurements.capture(directory, item)
            val done = LatencyMeasurements.read(directory).getJSONArray("records")
            assertEquals(1, done.length()); assertEquals("succeeded", done.getJSONObject(0).getString("outcome"))
        } finally { root.deleteRecursively() }
    }
    fun testCrossBootClockValuesAreNeverSubtracted() {
        val job = job(); val t = job.getJSONObject("client_timing")
        // A post-reboot clock is smaller than a pre-reboot one; the delta is dropped, never negative or huge.
        t.put("queued_elapsed_ms", 9_000_000).put("request_elapsed_ms", 1_000).put("asr_started_elapsed_ms", 50).put("asr_finished_elapsed_ms", 8L * 24 * 60 * 60 * 1000)
        val stages = LatencyMeasurements.project(session(), job).getJSONObject("stages_ms")
        assertFalse(stages.has("queue")); assertFalse(stages.has("asr"))
    }
    fun testTimingFailureNeverInvalidatesSavedSession() {
        val root = File(context.cacheDir, "latency-${UUID.randomUUID()}"); val directory = File(root, "recordings").apply { mkdirs() }
        try {
            // A non-empty directory cannot be replaced: the export write fails, the session save must not.
            File(root, "latency.json").mkdirs(); File(root, "latency.json/blocker").writeText("x")
            val store = SessionStore(directory); val item = store.create("en")
            item.put("capture_state", "complete").getJSONArray("attempts").put(phoneJob("succeeded", "cold"))
            store.save(item)
            assertEquals("complete", store.load(item.getString("id")).getString("capture_state"))
            assertEquals(0, LatencyMeasurements.read(directory).getJSONArray("records").length())
        } finally { root.deleteRecursively() }
    }
    fun testBoundedRetentionAndRetriedAttemptIdentity() {
        val root = File(context.cacheDir, "latency-${UUID.randomUUID()}"); val directory = File(root, "recordings").apply { mkdirs() }
        try {
            val item = session(); val jobs = JSONArray(); val attempt = UUID.randomUUID().toString()
            jobs.put(job(attempt)); item.put("attempts", jobs); LatencyMeasurements.capture(directory, item)
            jobs.getJSONObject(0).getJSONObject("client_timing").put("provider_attempt", 2)
            LatencyMeasurements.capture(directory, item)
            assertEquals(2, LatencyMeasurements.read(directory).getJSONArray("records").length())
            repeat(270) { jobs.put(job()) }; LatencyMeasurements.capture(directory, item)
            assertEquals(256, LatencyMeasurements.read(directory).getJSONArray("records").length())
            assertTrue(File(root, "latency.json").length() < 2 * 1024 * 1024)
            File(root, "latency.json").writeText("{\"records\":[{\"raw_text\":\"private\"}]}")
            assertEquals(0, LatencyMeasurements.read(directory).getJSONArray("records").length())
        } finally { root.deleteRecursively() }
    }
}
