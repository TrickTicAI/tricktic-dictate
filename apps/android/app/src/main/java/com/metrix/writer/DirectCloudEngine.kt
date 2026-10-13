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
        private val fileDeletion = ThreadPoolExecutor(1, 1, 30, TimeUnit.SECONDS,
            ArrayBlockingQueue<Runnable>(16), { work -> Thread(work, "writer-file-deletion").apply { isDaemon = true } })
            .apply { allowCoreThreadTimeOut(true) }
    }
    @Volatile private var cancelled = false
    @Volatile private var client: CloudClient? = null
    fun cancel() { cancelled = true; client?.cancel() }
    override fun execute(session: JSONObject, server: String): String {
        val id = session.getString("id"); val pending = session.getJSONObject("pending")
        val attempt = pending.getString("idempotency_key"); val cloud = session.getJSONObject("pending_cloud")
        fun current(): JSONObject = store.load(id).also {
            check(it.getJSONObject("pending").getString("idempotency_key") == attempt) { "Cloud attempt changed; saved audio retained." }
        }
        try {
            check(server == CloudProviders.get(cloud.getString("provider")).origin)
            check(pending.getJSONObject("options").getJSONObject("selection").getString("provider") == cloud.getString("provider"))
            val api = factory(cloud).also { client = it; if (cancelled) it.cancel() }
            var item = current()
            val original = store.audio(item)
            check(original.isFile && original.length() == item.getLong("byte_length")) { "Saved audio changed." }
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
                val now = SystemClock.elapsedRealtime()
                val timing = item.optJSONObject("pending_timing") ?: JSONObject()
                timing.put("job_id", attempt).put("result_elapsed_ms", now)
                if (timing.has("request_elapsed_ms")) timing.put("request_to_result_ms", now - timing.getLong("request_elapsed_ms"))
                if (timing.has("capture_stop_elapsed_ms")) timing.put("stop_to_result_ms", now - timing.getLong("capture_stop_elapsed_ms"))
                val job = JSONObject().put("id", attempt).put("request", JSONObject(pending.toString()))
                    .put("execution_location", "cloud").put("transport", "direct_${cloud.getString("provider")}").put("cloud_snapshot", JSONObject(cloud.toString()))
                    .put("delivery_choice", item.optString("pending_delivery", "raw")).put("client_timing", timing)
                    .put("status", JSONObject().put("state", "succeeded").put("result", JSONObject()
                        .put("transcript", JSONObject().put("raw_text", raw)).put("cleanup", JSONObject().put("state", "not_requested"))))
                item.getJSONArray("attempts").put(job); item.put("last_timing", timing)
                for (field in listOf("pending", "pending_server", "pending_cloud", "pending_delivery", "pending_timing", "pending_cloud_error", "cloud_upload")) item.remove(field)
                if (uploaded != null) job.put("cloud_file_cleanup", "unconfirmed; best-effort asynchronous deletion; Gemini file expiry applies")
                item.put("last_status", "Cloud transcript saved on this phone.")
                store.saveRaw(item, job)
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
            item.put("pending_cloud_error", message).put("last_status", message); store.save(item)
            return message
        } finally { client = null }
    }
}
