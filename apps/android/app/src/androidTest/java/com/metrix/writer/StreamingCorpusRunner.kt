package com.metrix.writer

import android.app.Instrumentation
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import org.json.JSONObject
import java.io.File
import java.io.RandomAccessFile
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

/**
 * MW-ANDROID-05 physical corpus run: one consented/public WAV through the production capture,
 * streaming, job, recognition and polishing path of the debug app, paced in real time at the
 * capture boundary instead of the microphone. Uses the debug app's own engine, key and polishing
 * settings without changing them; never inserts (no editor target) and deletes the session after.
 *
 * Arguments: wav (file name in the app's files dir), language (en|he|mixed), mode (streaming|whole),
 * label. Writes corpus-result.json (timings, receipts, text of the public clip) to the files dir.
 */
class StreamingCorpusRunner : Instrumentation() {
    private class PacedFile(private val file: File) : AudioCapture.Source {
        private val input = RandomAccessFile(file, "r").apply { seek(44) }
        private var started = 0L; private var sent = 0L
        val total = file.length() - 44
        val exhausted get() = sent >= total
        override fun start() { started = SystemClock.elapsedRealtime() }
        override fun read(bytes: ByteArray, length: Int): Int {
            val due = ((SystemClock.elapsedRealtime() - started) * 32L).coerceAtMost(total) - sent
            val n = minOf(length.toLong(), due).toInt() and 1.inv()
            if (n <= 0) return 0
            input.readFully(bytes, 0, n); sent += n; return n
        }
        override fun capacity() = 6400
        override fun silenced() = false
        override fun route() = 1
        override fun close() { input.close() }
    }

    /** Closes the real connection after [limit] bytes and reports it as the network would. */
    private class Dropping(private val live: LiveConnection, private val listener: GeminiLive.Listener, private val limit: Long) : LiveConnection {
        private var sent = 0L; @Volatile private var dropped = false
        override val turnOpen get() = live.turnOpen
        override fun audio(bytes: ByteArray, offset: Int, count: Int) {
            if (dropped) throw java.io.IOException("dropped")
            live.audio(bytes, offset, count); sent += count
            if (sent >= limit) { dropped = true; live.close(); Thread { listener.failed(java.io.IOException("dropped")) }.start() }
        }
        override fun end() = live.end()
        override fun close() = live.close()
    }

    override fun onCreate(arguments: Bundle?) {
        super.onCreate(arguments)
        Thread {
            val result = Bundle()
            var created: String? = null
            try {
                check(targetContext.packageName == "com.metrix.writer.debug") { "Debug candidate only" }
                val args = arguments ?: Bundle()
                val wav = File(targetContext.filesDir, args.getString("wav")!!).also { check(it.isFile && it.name.endsWith(".wav")) }
                val language = args.getString("language", "en").also { require(it in listOf("en", "he", "mixed")) }
                val streaming = args.getString("mode", "streaming") == "streaming"
                val choice = DictationChoice(targetContext)
                check(!choice.isPhone()) { "Select the Gemini cloud engine in the debug app first" }
                val capture = choice.capture(streaming = streaming)
                check(capture.getJSONObject("cloud").getString("provider") == "gemini")
                check(!streaming || capture.getJSONObject("cloud").optString("recognition") == "streaming") { "Streaming is off in the debug app" }
                // The clip's language for this request only; the app's settings are not changed.
                capture.put("input_language", language); capture.getJSONObject("options").put("input_language", language)
                // Optional polishing model (provider/model) and reasoning for this request only.
                args.getString("polish")?.let { choiceId ->
                    val selection = JSONObject().put("provider", choiceId.substringBefore('/')).put("model", choiceId.substringAfter('/'))
                        .put("reasoning", args.getString("polish_reasoning", "provider_default"))
                    val cleanupCloud = DirectCloud(targetContext, selection.getString("provider")).snapshot()
                    capture.getJSONObject("options").put("cleanup", JSONObject().put("mode", "enabled").put("selection", selection))
                    capture.getJSONObject("cloud").put("cleanup_cloud", cleanupCloud).put("polish_policy", CompanionPreferences(targetContext).polishPolicy(selection, cleanupCloud))
                    capture.put("delivery", "cleaned")
                }
                // Production capture holds a microphone foreground service; without it Android 15+ blocks a
                // background process's network. Showing the app gives this run the same network access.
                targetContext.startActivity(android.content.Intent(targetContext, MainActivity::class.java).addFlags(android.content.Intent.FLAG_ACTIVITY_NEW_TASK))
                waitForIdleSync(); Thread.sleep(1000)
                val manager = SessionManager.get(targetContext); val store = manager.store
                val main = Handler(Looper.getMainLooper())
                val item = store.create(language, { it.put("capture_origin", "bubble").put("capture_choice", capture)
                    .put("startup_timing", JSONObject().put("tap_elapsed_ms", SystemClock.elapsedRealtime())) })
                val id = item.getString("id"); created = id
                val source = PacedFile(wav)
                val recorder = AudioCapture(store) { source }
                // drop_at (0..1): the live connection is really closed at that point of the audio, to measure the fallback.
                val dropAt = args.getString("drop_at")?.toDouble()?.let { (it * (wav.length() - 44)).toLong() }
                val recognition = if (dropAt == null) StreamingRecognition.begin(targetContext, store, item) else streaming.takeIf { it }?.let {
                    val cloud = capture.getJSONObject("cloud"); val key = DirectCloud(targetContext, "gemini").key(cloud)
                    StreamingRecognition(id, store.audio(item), JSONObject(capture.getJSONObject("options").toString()), StreamingRecognition.Checkpoints.of(targetContext),
                        live = { listener -> Dropping(GeminiLive.connect(key, language, listener, cloud.optString("live_model", GeminiLive.MODEL)), listener, dropAt) },
                        ranges = { GeminiClient(key, readTimeoutMs = 20_000) }).also { StreamingRecognition.register(it); it.start() }
                }
                check(streaming == (recognition != null)) { "Streaming did not start" }
                val saved = CountDownLatch(1); var stoppedAt = 0L; var captureError: String? = null
                recorder.start(item, { _, error -> captureError = error; saved.countDown() }, {}, recognition)
                while (!source.exhausted) Thread.sleep(20)
                stoppedAt = SystemClock.elapsedRealtime(); recorder.stop()
                check(saved.await(30, TimeUnit.SECONDS) && captureError == null) { captureError ?: "Capture did not finish" }
                store.load(id).also { it.put("capture_timing", JSONObject().put("capture_stop_elapsed_ms", stoppedAt)); store.save(it) }
                main.post { manager.completeBubble(id, stoppedAt, live = true) }
                val deadline = SystemClock.elapsedRealtime() + 600_000
                var final: JSONObject
                while (true) {
                    final = store.load(id)
                    // The job itself must be over too: raw commits before polishing is queued.
                    val busy = final.has("pending") || final.has("pending_postprocess") || DirectCloudProcessing.isContinuing(id) || manager.isBusy()
                    if (!busy && final.getJSONArray("attempts").length() > 0) break
                    // Wait for the job to end first: finishing kills this process, and Android would restart a running job.
                    if (final.has("pending_cloud_error") && !manager.isBusy()) error(final.getString("pending_cloud_error"))
                    check(SystemClock.elapsedRealtime() < deadline) { "Processing did not finish" }
                    Thread.sleep(100)
                }
                val job = final.getJSONArray("attempts").getJSONObject(0)
                val raw = CompanionProcessing.raw(job)
                val receipts = job.optJSONArray("processing_attempts")
                val polish = receipts?.let { it.getJSONObject(it.length() - 1) }
                val out = JSONObject().put("label", args.getString("label", "")).put("mode", if (streaming) "streaming" else "whole")
                    .put("language", language).put("audio_ms", source.total / 32).put("transport", job.getString("transport"))
                    .put("timing", job.getJSONObject("client_timing")).put("recognition", job.opt("recognition") ?: JSONObject.NULL)
                    .put("raw_text", raw).put("polish", polish?.let { p -> JSONObject().put("elapsed_ms", p.opt("elapsed_ms")).put("outcome", p.opt("outcome"))
                        .put("blocks", p.opt("blocks")).put("budget_ms", p.opt("budget_ms")) } ?: JSONObject.NULL)
                    .put("polish_initial", job.opt("polish_outcome") ?: JSONObject.NULL).put("polished_text", job.optString("cleaned_text"))
                    .put("delivery_version", CompanionProcessing.deliveryVersion(job))
                File(targetContext.filesDir, "corpus-result.json").writeText(out.toString())
                result.putString("result", "PASS")
            } catch (error: Exception) {
                result.putString("result", "FAIL: " + (error.message ?: error.javaClass.simpleName).take(300))
            }
            // Corpus runs never stay in the owner's History, failed ones included.
            created?.let { id -> runCatching {
                val manager = SessionManager.get(targetContext); val until = SystemClock.elapsedRealtime() + 60_000
                while (manager.isBusy() && SystemClock.elapsedRealtime() < until) Thread.sleep(100)
                manager.store.deleteSaved(id)
            } }
            finish(if (result.getString("result") == "PASS") 0 else 1, result)
        }.start()
    }
}
