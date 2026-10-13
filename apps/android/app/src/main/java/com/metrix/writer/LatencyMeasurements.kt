package com.metrix.writer

import android.util.AtomicFile
import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.security.MessageDigest
import java.util.UUID

/** Bounded projection of existing client_timing/processing receipts. Never export a session. */
object LatencyMeasurements {
    const val RETAINED_ATTEMPTS = 256
    private const val MAX_BYTES = 2 * 1024 * 1024
    private const val MAX_MS = 7L * 24 * 60 * 60 * 1000
    private val fields = setOf("recording_id", "attempt_id", "retry_of", "provider_attempt", "platform", "provider", "model_sha256", "mode", "warm", "measurement", "outcome", "stages_ms", "stage_outcomes")
    private val stages = setOf("capture_readiness", "preparation", "upload", "queue", "asr", "cleanup", "raw_availability", "insertion", "stop_to_insertion", "cloud_request")
    private fun uuid(value: String) = UUID.fromString(value).toString().also { require(it == value) }
    private fun hash(value: String) = MessageDigest.getInstance("SHA-256").digest(value.toByteArray()).joinToString("") { "%02x".format(it) }
    private fun duration(t: JSONObject?, start: String, end: String): Long? {
        if (t == null || !t.has(start) || !t.has(end)) return null
        val from = t.optLong(start, -1); val to = t.optLong(end, -1)
        return (to - from).takeIf { from >= 0 && to >= from && it <= MAX_MS }
    }
    private fun numeric(t: JSONObject?, key: String) = t?.takeIf { it.has(key) }?.optLong(key, -1)?.takeIf { it in 0..MAX_MS }
    fun project(item: JSONObject, job: JSONObject): JSONObject {
        val request = job.getJSONObject("request"); val options = request.getJSONObject("options")
        val selection = options.getJSONObject("selection")
        val provider = selection.getString("provider").also { require(it in setOf("local", "openai", "gemini", "grok", "groq", "openrouter")) }
        val timing = job.optJSONObject("client_timing")
        val measured = JSONObject()
        fun put(stage: String, value: Long?) { if (value != null) measured.put(stage, value) }
        // Readiness endpoints are from a single capture boot. Persist the delta,
        // never combine boot-relative clocks with server/wall-clock timestamps.
        put("capture_readiness", duration(item.optJSONObject("startup_timing"), "tap_elapsed_ms", "first_audio_read_elapsed_ms"))
        put("queue", duration(timing, "queued_elapsed_ms", "request_elapsed_ms"))
        put("preparation", numeric(timing, "silence_prepare_ms"))
        put("upload", duration(timing, "upload_started_elapsed_ms", "upload_finished_elapsed_ms"))
        put("asr", duration(timing, "asr_started_elapsed_ms", "asr_finished_elapsed_ms"))
        put("cloud_request", duration(timing, "cloud_request_started_elapsed_ms", "cloud_request_finished_elapsed_ms"))
        if (job.optJSONObject("status")?.optString("state") == "succeeded") put("raw_availability", numeric(timing, "stop_to_raw_ms"))
        val stageOutcomes = JSONObject()
        val processing = job.optJSONArray("processing_attempts")
        if (processing != null && processing.length() > 0) {
            val last = processing.getJSONObject(processing.length()-1)
            put("cleanup", numeric(last, "elapsed_ms"))
            stageOutcomes.put("cleanup", if (last.optJSONObject("cleanup")?.optString("state") == "succeeded") "succeeded" else "failed")
        }
        val delivered = job.optJSONObject("polish_delivery")?.optJSONObject("timing")
        put("insertion", duration(delivered, "insertion_started_elapsed_ms", "delivery_elapsed_ms"))
        put("stop_to_insertion", numeric(delivered, "stop_to_delivery_ms"))
        val status = job.optJSONObject("status")?.optString("state", "pending") ?: "pending"
        return JSONObject().put("recording_id", uuid(item.getString("id")))
            .put("attempt_id", uuid(request.getString("idempotency_key")))
            .put("retry_of", request.optString("retry_of_job_id").takeIf { it.isNotEmpty() && it != "null" }?.let(::uuid) ?: JSONObject.NULL)
            .put("provider_attempt", (timing?.optLong("provider_attempt", 1) ?: 1).coerceIn(1, 1000000))
            .put("platform", "android").put("provider", provider).put("model_sha256", hash(selection.getString("model")))
            .put("mode", if (options.getJSONObject("cleanup").optString("mode") == "off") "regular" else "super")
            .put("warm", timing?.optString("warm")?.takeIf { it in setOf("cold", "warm") } ?: "unknown")
            .put("measurement", "unclassified")
            .put("outcome", status.takeIf { it in setOf("succeeded", "failed") } ?: "pending").put("stages_ms", measured).put("stage_outcomes", stageOutcomes)
    }
    fun valid(row: JSONObject): Boolean = runCatching {
        require(row.keys().asSequence().toSet() == fields)
        uuid(row.getString("recording_id")); uuid(row.getString("attempt_id"))
        if (!row.isNull("retry_of")) uuid(row.getString("retry_of"))
        require(row.getString("platform") == "android")
        require(row.getString("provider") in setOf("local", "openai", "gemini", "grok", "groq", "openrouter"))
        require(row.getString("model_sha256").matches(Regex("[a-f0-9]{64}")))
        require(row.getString("mode") in setOf("regular", "super", "command"))
        require(row.getString("warm") in setOf("cold", "warm", "unknown"))
        require(row.getString("measurement") in setOf("synthetic", "physical_device", "unclassified"))
        require(row.getString("outcome") in setOf("pending", "succeeded", "failed"))
        require(row.getLong("provider_attempt") in 1..1000000)
        val outcomes = row.getJSONObject("stage_outcomes")
        outcomes.keys().forEach { require(it in stages && outcomes.getString(it) in setOf("succeeded", "failed", "pending")) }
        val measured = row.getJSONObject("stages_ms")
        measured.keys().forEach { require(it in stages && measured.get(it) is Number && measured.getLong(it) in 0..MAX_MS) }
    }.isSuccess
    @Synchronized fun read(directory: File): JSONObject = runCatching {
        val file = File(directory.parentFile, "latency.json")
        require(file.length() <= MAX_BYTES)
        val export = JSONObject(String(AtomicFile(file).readFully(), Charsets.UTF_8))
        require(export.keys().asSequence().toSet() == setOf("records"))
        val rows = export.getJSONArray("records"); require(rows.length() <= RETAINED_ATTEMPTS)
        for (i in 0 until rows.length()) require(valid(rows.getJSONObject(i)))
        export
    }.getOrElse { JSONObject().put("records", JSONArray()) }
    /** Timing write failure never invalidates successfully saved audio/text. */
    @Synchronized fun capture(directory: File, item: JSONObject) { runCatching {
        if (item.optString("capture_state") == "capturing") return
        val jobs = item.optJSONArray("attempts") ?: JSONArray()
        val incoming = mutableListOf<JSONObject>()
        // Every save reprojects only the bounded tail; manual polishing updates
        // a retained attempt without retaining an unbounded second history.
        for (i in maxOf(0, jobs.length()-RETAINED_ATTEMPTS) until jobs.length()) {
            runCatching { project(item, jobs.getJSONObject(i)) }.getOrNull()?.let { incoming += it }
        }
        item.optJSONObject("pending")?.let { pending ->
            if (item.optJSONObject("pending_timing")?.has("request_elapsed_ms") == true) runCatching {
                project(item, JSONObject().put("request", pending).put("client_timing", item.opt("pending_timing"))
                    .put("status", JSONObject().put("state", if (item.has("pending_cloud_error") || item.optBoolean("pending_latency_failed")) "failed" else "pending")))
            }.getOrNull()?.let { incoming += it }
        }
        if (incoming.isEmpty()) return
        val before = read(directory); val rows = before.getJSONArray("records")
        val retained = (0 until rows.length()).map { rows.getJSONObject(it) }.toMutableList()
        for (row in incoming) {
            retained.removeAll { it.getString("attempt_id") == row.getString("attempt_id") && it.getLong("provider_attempt") == row.getLong("provider_attempt") }
            retained += row
        }
        val export = JSONObject().put("records", JSONArray(retained.takeLast(RETAINED_ATTEMPTS)))
        if (export.toString() == before.toString()) return
        val bytes = export.toString().toByteArray(); require(bytes.size <= MAX_BYTES)
        val atomic = AtomicFile(File(directory.parentFile, "latency.json")); val out = atomic.startWrite()
        try { out.write(bytes); out.fd.sync(); atomic.finishWrite(out) } catch (e: Exception) { atomic.failWrite(out); throw e }
    } }
}
