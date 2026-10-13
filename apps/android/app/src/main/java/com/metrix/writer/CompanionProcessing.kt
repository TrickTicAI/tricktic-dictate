package com.metrix.writer

import org.json.JSONArray
import org.json.JSONObject

/** All transformations use the authenticated server's existing vocabulary/cleanup pipeline. */
class CompanionProcessing(private val store: SessionStore, private val mobile: MobileConnection) {
    @Volatile private var cancelled = false
    @Volatile private var active: java.net.HttpURLConnection? = null
    fun cancel() { cancelled = true; active?.disconnect() }
    private fun request(origin: String, path: String, body: JSONObject? = null): JSONObject {
        if (cancelled) throw java.io.InterruptedIOException()
        val connection = mobile.open(origin, path, if (body == null) "GET" else "POST")
        active = connection
        if (cancelled) { connection.disconnect(); throw java.io.InterruptedIOException() }
        try {
            if (body != null) {
                connection.doOutput = true; connection.setRequestProperty("Content-Type", "application/json")
                connection.outputStream.use { it.write(JSONObject().put("api_version", "1").put("data", body).toString().toByteArray()) }
            }
            return MobileConnection.response(connection).getJSONObject("data")
        } finally { connection.disconnect(); active = null }
    }
    fun prepare(id: String, jobId: String, selection: JSONObject?) {
        val item = store.load(id)
        check(!item.has("pending")) { "Finish the pending transcription first." }
        if (item.has("pending_postprocess")) {
            check(item.getJSONObject("pending_postprocess").getString("job_id") == jobId) { "Resume the other attempt's pending text processing first." }
            return
        }
        val job = find(item, jobId)
        check(job.getJSONObject("status").getString("state") == "succeeded")
        check(job.has("server_origin")) { "Retry this older recording once to establish its original server." }
        item.put("pending_postprocess", JSONObject().put("job_id", jobId).put("server", job.getString("server_origin"))
            .put("selection", selection?.let { JSONObject(it.toString()) } ?: JSONObject.NULL))
        store.save(item)
    }
    fun afterTranscription(id: String) {
        val item = store.load(id)
        val attempts = item.getJSONArray("attempts")
        if (attempts.length() == 0 || item.has("pending")) return
        val job = attempts.getJSONObject(attempts.length() - 1)
        if (job.getJSONObject("status").getString("state") != "succeeded") return
        val options = job.getJSONObject("request").getJSONObject("options")
        val vocabulary = options.getJSONObject("vocabulary")
        val cleanup = options.getJSONObject("cleanup")
        if (listOf("hints", "corrections", "snippets").all { vocabulary.getJSONArray(it).length() == 0 } && cleanup.getString("mode") == "off") return
        prepare(id, job.getString("id"), cleanup.optJSONObject("selection"))
        run(id)
    }
    fun run(id: String) {
        var item = store.load(id)
        val pending = item.getJSONObject("pending_postprocess")
        val jobId = pending.getString("job_id"); val origin = pending.getString("server")
        val started = android.os.SystemClock.elapsedRealtime()
        val receipt = JSONObject().put("started_at_ms", System.currentTimeMillis()).put("selection", pending.opt("selection"))
        try {
            val processed = request(origin, "/api/v1/jobs/$jobId/processed").getString("text")
            item = store.load(id); find(item, jobId).put("dictionary_text", processed); store.save(item)
            val selection = pending.optJSONObject("selection")
            if (selection != null) {
                var result = request(origin, "/api/v1/jobs/$jobId/cleanup", selection)
                val deadline = android.os.SystemClock.elapsedRealtime() + 90000
                while (true) {
                    check(result.getString("id") == jobId)
                    val response = result.getJSONObject("status").getJSONObject("result")
                    check(response.getJSONObject("transcript").getString("raw_text") == raw(find(store.load(id), jobId)))
                    val cleanup = response.getJSONObject("cleanup")
                    if (cleanup.getString("state") != "pending") {
                        receipt.put("cleanup", cleanup)
                        if (cleanup.getString("state") == "succeeded") {
                            item = store.load(id); find(item, jobId).put("cleaned_text", cleanup.getString("cleaned_text")); store.save(item)
                        }
                        break
                    }
                    check(android.os.SystemClock.elapsedRealtime() < deadline) { "Cleanup still pending" }
                    Thread.sleep(250)
                    result = request(origin, "/api/v1/jobs/$jobId")
                }
            }
            receipt.put("state", "complete")
            item = store.load(id); item.remove("pending_postprocess")
        } catch (error: Exception) {
            receipt.put("state", "incomplete").put("message", MobileConnection.explain(error))
            item = store.load(id) // Pending server and selection remain immutable for explicit resumption.
        }
        val job = find(item, jobId)
        val history = job.optJSONArray("processing_attempts") ?: JSONArray()
        receipt.put("elapsed_ms", android.os.SystemClock.elapsedRealtime() - started)
        receipt.put("finished_at_ms", System.currentTimeMillis()) // Measured duration for Statistics; never inferred later.
        history.put(receipt); job.put("processing_attempts", history)
        store.save(item)
    }
    companion object {
        @JvmStatic fun find(item: JSONObject, jobId: String): JSONObject {
            val jobs = item.getJSONArray("attempts")
            return (0 until jobs.length()).map { jobs.getJSONObject(it) }.first { it.getString("id") == jobId }
        }
        @JvmStatic fun raw(job: JSONObject): String = job.getJSONObject("status").getJSONObject("result").getJSONObject("transcript").getString("raw_text")
        @JvmStatic fun deliveryText(job: JSONObject): String = when (job.optString("delivery_choice", "raw")) {
            "cleaned" -> if (job.optJSONObject("polish_outcome")?.optBoolean("original_used") == true)
                job.optString("dictionary_text", raw(job)) else job.optString("cleaned_text", job.optString("dictionary_text", raw(job)))
            "dictionary" -> job.optString("dictionary_text", raw(job))
            else -> raw(job)
        }
    }
}
