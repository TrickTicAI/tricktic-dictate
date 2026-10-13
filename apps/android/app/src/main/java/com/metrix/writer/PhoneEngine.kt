package com.metrix.writer

import android.content.*
import android.os.*
import org.json.JSONObject
import java.io.InterruptedIOException
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

class PhoneEngine(context: Context, private val store: SessionStore) : TranscriptionEngine {
    private val context = context.applicationContext
    @Volatile private var cancelled = false
    @Volatile private var remote: Messenger? = null
    private var audioPreparation: JSONObject? = null
    private var inferenceTiming: JSONObject? = null
    fun cancel() { cancelled = true; try { remote?.send(Message.obtain(null, 2)) } catch (_: RemoteException) { } }
    override fun execute(session: JSONObject, server: String): String {
        audioPreparation = null; inferenceTiming = null
        val pending = session.getJSONObject("pending"); val attempt = pending.getString("idempotency_key")
        val id = session.getString("id"); val options = pending.getJSONObject("options")
        val models = PhoneModels(context)
        val job = JSONObject().put("id", attempt).put("request", JSONObject(pending.toString()))
            .put("execution_location", "phone").put("engine_snapshot", session.getJSONObject("pending_engine"))
            .put("delivery_choice", session.optString("pending_delivery", "raw"))
        var resultMessage: String
        try {
            val raw = models.exclusive {
                val snapshot = session.getJSONObject("pending_engine")
                val model = snapshot.getString("id"); val language = options.getString("input_language")
                check(snapshot.getString("runtime_revision") == models.runtime && snapshot.getString("sha256") == models.model(model).getString("sha256")) { "Pending phone runtime/model version is unavailable. Cancel this attempt and retry with current settings." }
                require(language in models.languages(model)) { "Phone model does not support this language" }
                models.verify(model) { cancelled }
                infer(id, attempt, model, language)
            }
            if (cancelled) throw InterruptedIOException("Phone inference cancelled")
            job.put("status", JSONObject().put("state", "succeeded").put("result", JSONObject()
                .put("transcript", JSONObject().put("raw_text", raw)).put("cleanup", JSONObject().put("state", "not_requested"))))
            try { job.put("dictionary_text", OfflineVocabulary.process(raw, options.getJSONObject("vocabulary"))) }
            catch (_: Exception) { job.put("dictionary_error", "Dictionary processing failed; original text is retained.") }
            job.put("hint_status", "Recognition hints are unsupported by this qualified phone binding; corrections and snippets apply after recognition.")
            if (options.getJSONObject("cleanup").getString("mode") != "off") job.put("deferred_cleanup", JSONObject(options.getJSONObject("cleanup").toString()))
            resultMessage = if (job.has("deferred_cleanup")) "Phone transcription saved."
            else "Phone transcription saved. Original and dictionary output available offline."
        } catch (error: Exception) {
            resultMessage = error.message ?: "Phone inference failed; saved audio retained"
            job.put("status", JSONObject().put("state", "failed").put("error", JSONObject().put("message", resultMessage).put("retryable", true)))
        }
        // Reload and compare identity: a cancelled/replaced attempt can never publish a stale result.
        val item = store.load(id)
        check(item.optJSONObject("pending")?.optString("idempotency_key") == attempt) { "Phone result expired; saved audio retained" }
        val timing = item.optJSONObject("pending_timing") ?: JSONObject()
        audioPreparation?.let { SilencePreparation.addMetrics(timing, it) }
        inferenceTiming?.let { t -> for (key in listOf("asr_started_elapsed_ms", "asr_finished_elapsed_ms", "warm")) if (t.has(key)) timing.put(key, t.get(key)) }
        val now = SystemClock.elapsedRealtime(); timing.put("job_id", attempt).put("result_elapsed_ms", now)
        if (timing.has("request_elapsed_ms")) timing.put("request_to_result_ms", now - timing.getLong("request_elapsed_ms"))
        if (timing.has("capture_stop_elapsed_ms")) timing.put("stop_to_result_ms", now - timing.getLong("capture_stop_elapsed_ms"))
        job.put("client_timing", timing); item.put("last_timing", timing)
        item.getJSONArray("attempts").put(job)
        for (key in listOf("pending", "pending_server", "pending_engine", "pending_delivery", "pending_timing")) item.remove(key)
        item.put("last_status", resultMessage); store.saveRaw(item, job)
        return resultMessage
    }
    private fun infer(id: String, attempt: String, model: String, language: String): String {
        val lease = checkNotNull(PhoneResultCache.lease(context, true)) { "Temporary results are being cleaned. Retry saved audio." }
        return lease.use { inferLeased(id, attempt, model, language) }
    }
    private fun inferLeased(id: String, attempt: String, model: String, language: String): String {
        val application = context.applicationContext
        val closed = java.util.concurrent.atomic.AtomicBoolean(false)
        val done = CountDownLatch(1); var response: Bundle? = null
        val main = Handler(Looper.getMainLooper())
        val receiver = Messenger(Handler(Looper.getMainLooper()) { response = it.data; done.countDown(); true })
        val connection = object : ServiceConnection {
            override fun onServiceConnected(name: ComponentName, binder: IBinder) {
                if (closed.get()) return
                remote = Messenger(binder)
                try {
                    remote!!.send(Message.obtain(null, 1).apply { replyTo = receiver; data = Bundle().apply { putString("session", id); putString("attempt", attempt); putString("model", model); putString("language", language) } })
                    if (cancelled) cancel()
                } catch (_: RemoteException) { done.countDown() }
            }
            override fun onServiceDisconnected(name: ComponentName) { done.countDown() }
            override fun onNullBinding(name: ComponentName) { done.countDown() }
            override fun onBindingDied(name: ComponentName) { done.countDown() }
        }
        val bound = CountDownLatch(1); var didBind = false
        main.post {
            try { didBind = application.bindService(Intent(application, PhoneInferenceService::class.java), connection, Context.BIND_AUTO_CREATE) }
            catch (_: RuntimeException) { /* Report unavailable worker through the retained attempt. */ }
            finally { bound.countDown(); if (!didBind) done.countDown() }
        }
        val output = PhoneResultCache.file(context, attempt)
        try {
            bound.await()
            while (!done.await(250, TimeUnit.MILLISECONDS)) {
                if (cancelled) { cancel(); if (remote == null) throw InterruptedIOException("Phone inference cancelled before worker connection") }
            }
            if (cancelled) throw InterruptedIOException("Phone inference cancelled; audio retained")
            val result = response ?: error("Phone worker interrupted or ran out of memory. Retry saved audio with a smaller model.")
            result.getString("audio_preparation")?.let { audioPreparation = JSONObject(it) }
            result.getString("inference_timing")?.let { inferenceTiming = JSONObject(it) }
            check(!result.containsKey("error")) { result.getString("error")!! }
            check(result.getString("file") == output.name && output.isFile) { "Phone worker result unavailable" }
            return output.readText(Charsets.UTF_8)
        } finally {
            closed.set(true); remote = null
            val keepWarm = !cancelled && response?.containsKey("error") == false && response?.getString("file") == output.name
            main.post {
                if (didBind) {
                    if (keepWarm) PhoneWorkerWarmth.retain(application, connection)
                    else application.unbindService(connection)
                }
            }
            output.delete()
        }
    }
}
