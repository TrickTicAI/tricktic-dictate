package com.metrix.writer

import android.content.Context
import android.os.SystemClock
import org.json.JSONObject
import java.util.concurrent.ArrayBlockingQueue
import java.util.concurrent.ThreadPoolExecutor
import java.util.concurrent.TimeUnit

/** Saved audio -> direct Google HTTPS -> durable original. No Writer server or pairing required. */
class DirectCloudEngine(private val context: Context, private val store: SessionStore,
    private val factory: (JSONObject) -> CloudClient = { CloudProviders.client(context, it) }) : TranscriptionEngine {
    companion object {
        // Deletion is best effort and must never hold up insertion or the next recording.
        // Bound the queue if the provider is slow; unconfirmed files retain provider expiry.
        const val MAX_AUTOMATIC_RERUNS = 5
        private val fileDeletion = ThreadPoolExecutor(1, 1, 30, TimeUnit.SECONDS,
            ArrayBlockingQueue<Runnable>(16), { work -> Thread(work, "writer-file-deletion").apply { isDaemon = true } })
            .apply { allowCoreThreadTimeOut(true) }
    }
    @Volatile private var cancelled = false
    @Volatile private var client: CloudClient? = null
    fun cancel() { cancelled = true; client?.cancel() }
    /** Set when the attempt stopped before any provider request could be sent and may safely run again. */
    var waitingForNetwork = false; private set

    /**
     * Streaming capture: wait for the live tail (or its chunked fallback) of this process's recognizer,
     * or resume from the durable checkpoint after process death. Null means nothing to stream from.
     */
    private fun streamed(item: JSONObject, pending: JSONObject, cloud: JSONObject, progress: (Int, Int) -> Unit): StreamingRecognition.Outcome? {
        val id = item.getString("id"); val options = pending.getJSONObject("options")
        val audio = store.audio(item)
        val deadline = SystemClock.elapsedRealtime() + 60_000 + item.optLong("duration_ms") / 2
        val ranges = { (factory(cloud) as? RangeClient ?: error("This provider cannot transcribe saved-audio ranges.")).also { if (it is CloudClient) { client = it; if (cancelled) it.cancel() } } }
        StreamingRecognition.get(id)?.let { live ->
            // Success or failure, this in-memory run is used once; a rerun resumes from its checkpoint,
            // which keeps every range it finished meanwhile.
            try { return live.await(deadline, progress) { cancelled } } finally { StreamingRecognition.remove(id) }
        }
        return StreamingRecognition.resume(id, audio, options, StreamingRecognition.Checkpoints.of(context), ranges, deadline, progress = progress) { cancelled }
    }
    override fun execute(session: JSONObject, server: String): String {
        val id = session.getString("id"); val pending = session.getJSONObject("pending")
        val attempt = pending.getString("idempotency_key"); val cloud = session.getJSONObject("pending_cloud")
        fun current(): JSONObject = store.load(id).also {
            check(it.getJSONObject("pending").getString("idempotency_key") == attempt) { "Cloud attempt changed; saved audio retained." }
        }
        try {
            check(server == CloudProviders.get(cloud.getString("provider")).origin)
            check(pending.getJSONObject("options").getJSONObject("selection").getString("provider") == cloud.getString("provider"))
            var item = current()
            val original = store.audio(item)
            check(original.isFile && original.length() == item.getLong("byte_length")) { "Saved audio changed." }
            if (cloud.optString("recognition") == "streaming") {
                fun mark(key: String) { item = current(); item.put("pending_timing", (item.optJSONObject("pending_timing") ?: JSONObject()).put(key, SystemClock.elapsedRealtime())); store.save(item) }
                mark("asr_started_elapsed_ms")
                ProcessingEvents.emit(id, attempt, ProcessingStage.RECOGNIZING)
                val outcome = streamed(item, pending, cloud) { done, total -> ProcessingEvents.progress(id, attempt, done, total) }
                if (outcome != null) {
                    mark("asr_finished_elapsed_ms")
                    item = current()
                    val message = "Cloud transcript saved on this phone."
                    commit(item, attempt, pending, cloud, outcome.text, "direct_gemini_streaming", message) { it.put("recognition", outcome.receipt) }
                    runCatching { StreamingRecognition.Checkpoints.of(context).delete(id) }
                    return message
                }
            }
            val api = factory(cloud).also { client = it; if (cancelled) it.cancel() }
            SilencePreparation.prepare(context, original) { cancelled }.use { prepared ->
                val audio = prepared.file
                val preparationTiming = item.optJSONObject("pending_timing") ?: JSONObject()
                SilencePreparation.addMetrics(preparationTiming, prepared.metrics)
                item.put("pending_timing", preparationTiming); store.save(item)
                val sourceHash = if (api.usesFiles) PhoneModels.hash(audio) else null
                val uploadBinding = "${cloud.getString("provider")}/${cloud.getString("credential_id")}/$server/$sourceHash/${item.optString("transcript_owner")}/$id"
                val recovery = UploadCheckpointStore(context,id,uploadBinding) { confirmed,total,resumed ->
                    val update = current()
                    update.put("last_status", "${if (resumed) "Resuming" else "Uploading"} saved audio · $confirmed / $total bytes confirmed")
                    store.save(update)
                }
                if (api is GeminiClient) api.recovery = recovery
                val cachedUpload = item.optJSONObject("cloud_upload")?.takeIf { it.optString("upload_binding") == uploadBinding }
                if (!api.usesFilesFor(audio,pending.getJSONObject("options"))) {
                    item.put("last_status","Sending full saved audio; this request does not support byte resume. A repeated request may incur another provider charge."); store.save(item)
                }
                fun mark(key: String) {
                    item = current()
                    val t = item.optJSONObject("pending_timing") ?: JSONObject()
                    t.put(key, SystemClock.elapsedRealtime()); item.put("pending_timing", t); store.save(item)
                }
                val separateUpload = api.usesFilesFor(audio, pending.getJSONObject("options"))
                if (separateUpload && cachedUpload == null) mark("upload_started_elapsed_ms")
                val uploaded = cachedUpload ?: if (!api.usesFilesFor(audio, pending.getJSONObject("options"))) null else api.upload(audio).put("preparation_sha256", sourceHash).put("upload_binding",uploadBinding).also {
                    item = current(); item.put("cloud_upload", it); store.save(item)
                }
                if (separateUpload && cachedUpload == null) mark("upload_finished_elapsed_ms")
                mark(if (uploaded == null) "cloud_request_started_elapsed_ms" else "asr_started_elapsed_ms")
                val raw = if (uploaded == null) api.transcribeAudio(audio, pending.getJSONObject("options")) else api.transcribe(uploaded, pending.getJSONObject("options"))
                mark(if (uploaded == null) "cloud_request_finished_elapsed_ms" else "asr_finished_elapsed_ms")
                item = current()
                commit(item, attempt, pending, cloud, raw, "direct_${cloud.getString("provider")}", "Cloud transcript saved on this phone.") { job ->
                    if (uploaded != null) job.put("cloud_file_cleanup", "unconfirmed; best-effort asynchronous deletion; Gemini file expiry applies")
                }
                runCatching { recovery.save(null) }
                // Save first, then return the result without another provider round trip.
                // The worker owns only this request's client/file. It never writes session
                // metadata: a late completion cannot overwrite cleanup, retry or deletion.
                if (uploaded != null) try { fileDeletion.execute { try { api.delete(uploaded) } catch (_: Exception) { } } }
                catch (_: java.util.concurrent.RejectedExecutionException) { /* Provider expiry remains the fallback. */ }
                return "Cloud transcript saved on this phone."
            }
        } catch (failure: Throwable) {
            if (failure !is Exception && failure !is OutOfMemoryError) throw failure
            val item = current()
            if (failure is CloudFailure && failure.status == 404) item.remove("cloud_upload")
            val message = if (failure is OutOfMemoryError) "Cloud processing exceeded available memory. Retry saved audio; earlier text remains."
                else GeminiClient.explain(failure as Exception)
            val timing = item.optJSONObject("pending_timing") ?: JSONObject()
            // Automatic reruns only when nothing billable can repeat: no provider request was sent, or
            // streaming checkpoints keep every finished range so only small unfinished ranges repeat.
            val streaming = cloud.optString("recognition") == "streaming"
            if (!cancelled && (unsent(failure) || streaming && failure is java.io.IOException && failure !is CloudFailure && failure !is java.io.InterruptedIOException) &&
                timing.optInt("automatic_reruns") < MAX_AUTOMATIC_RERUNS) {
                waitingForNetwork = true
                timing.put("automatic_reruns", timing.optInt("automatic_reruns") + 1)
                item.remove("pending_cloud_error"); item.remove("pending_cloud_error_code")
                item.put("pending_timing", timing).put("pending_automatic_rerun", "network")
                    .put("last_status", "Waiting for a connection. Audio is saved; transcription continues automatically.")
                store.save(item)
                return item.getString("last_status")
            }
            item.put("pending_cloud_error", message).put("last_status", message)
            item.put("pending_cloud_error_code", errorCode(failure)); store.save(item)
            return message
        } finally { client = null }
    }
    private fun commit(item: JSONObject, attempt: String, pending: JSONObject, cloud: JSONObject, raw: String, transport: String, message: String, extra: (JSONObject) -> Unit) {
        val now = SystemClock.elapsedRealtime()
        val timing = item.optJSONObject("pending_timing") ?: JSONObject()
        timing.put("job_id", attempt).put("result_elapsed_ms", now)
        if (timing.has("request_elapsed_ms")) timing.put("request_to_result_ms", now - timing.getLong("request_elapsed_ms"))
        if (timing.has("capture_stop_elapsed_ms")) timing.put("stop_to_result_ms", now - timing.getLong("capture_stop_elapsed_ms"))
        val job = JSONObject().put("id", attempt).put("request", JSONObject(pending.toString()))
            .put("execution_location", "cloud").put("transport", transport).put("cloud_snapshot", JSONObject(cloud.toString()))
            .put("delivery_choice", item.optString("pending_delivery", "raw")).put("client_timing", timing)
            .put("status", JSONObject().put("state", "succeeded").put("result", JSONObject()
                .put("transcript", JSONObject().put("raw_text", raw)).put("cleanup", JSONObject().put("state", "not_requested"))))
        extra(job)
        item.getJSONArray("attempts").put(job); item.put("last_timing", timing)
        for (field in listOf("pending", "pending_server", "pending_cloud", "pending_delivery", "pending_timing", "pending_cloud_error",
            "pending_cloud_error_code", "pending_automatic_rerun", "cloud_upload")) item.remove(field)
        item.put("last_status", message)
        store.saveRaw(item, job)
    }
    /** Failures raised before a request reached the provider: retrying cannot repeat a charge. */
    private fun unsent(error: Throwable): Boolean = error is java.net.UnknownHostException || error is java.net.ConnectException ||
        error is java.net.NoRouteToHostException || error.cause?.let(::unsent) == true
    private fun errorCode(error: Throwable): String = when {
        error is CloudFailure && error.status in listOf(401, 403) -> "cloud_credential"
        error is CloudFailure && error.status == 429 -> "rate_limited"
        error is CloudFailure && error.status == 413 -> "too_large"
        error is ChunkedTranscriber.ChunkFailure && error.code == "timeout" || error is java.net.SocketTimeoutException -> "timeout"
        error is ChunkedTranscriber.ChunkFailure && error.code == "incomplete" -> "incomplete_result"
        error is java.io.InterruptedIOException -> "cancelled"
        error is java.io.IOException && error !is CloudFailure -> "network"
        error is OutOfMemoryError -> "model_memory"
        error is ProviderRefusal -> "provider_declined"
        else -> "recognition_failed"
    }
}
