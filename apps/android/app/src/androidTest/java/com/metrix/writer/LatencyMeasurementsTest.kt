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
