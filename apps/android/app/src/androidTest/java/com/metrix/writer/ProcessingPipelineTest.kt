@file:Suppress("DEPRECATION")
package com.metrix.writer

import android.test.AndroidTestCase
import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.io.RandomAccessFile
import java.util.Collections
import java.util.UUID
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicInteger

/**
 * MW-ANDROID-05: engine commit/resume, charge-aware automatic reruns, progress and the polishing
 * policy (parallel blocks, separated failures, late completion that never inserts). Fakes only.
 */
class ProcessingPipelineTest : AndroidTestCase() {
    private lateinit var store: SessionStore
    private var restore: () -> Unit = {}
    override fun setUp() {
        super.setUp()
        store = SessionStore(File(context.cacheDir, "pipeline-${UUID.randomUUID()}"))
        ProcessingEvents.reset()
        // A credential reference without a key: fake clients never resolve it (see PhonePolishTest).
        val prefs = context.getSharedPreferences("direct-cloud", 0); val companion = context.getSharedPreferences("companion", 0)
        val had = prefs.getString("credential", null); val saved = companion.all.filterKeys { it.startsWith("cleanup_") }
        check(prefs.edit().putString("credential", UUID.randomUUID().toString()).commit())
        companion.edit().remove("cleanup_fallback").remove("cleanup_fallback_after_ms").remove("cleanup_timeout_ms").commit()
        restore = {
            prefs.edit().apply { if (had != null) putString("credential", had) else remove("credential") }.commit()
            val edit = companion.edit(); saved.forEach { (k, v) -> when (v) { is String -> edit.putString(k, v); is Long -> edit.putLong(k, v) } }; edit.commit()
        }
    }
    override fun tearDown() { restore(); store.directory.deleteRecursively(); ProcessingEvents.reset(); super.tearDown() }

    private val second = 32000
    private fun pcm(seconds: Int): ByteArray {
        val random = java.util.Random(3); val bytes = ByteArray(seconds * second)
        for (i in 0 until bytes.size / 2) { val v = random.nextInt(6000) - 3000; bytes[2 * i] = (v and 255).toByte(); bytes[2 * i + 1] = (v shr 8).toByte() }
        return bytes
    }
    private fun cloud(streaming: Boolean) = JSONObject().put("provider", "gemini").put("credential_id", UUID.randomUUID().toString()).put("transport_version", 1)
        .apply { if (streaming) put("recognition", "streaming").put("live_model", GeminiLive.MODEL) }
    private fun options() = CompanionPreferences(context).options("gemini", DirectCloud.DEFAULT_MODEL, "en").put("cleanup", JSONObject().put("mode", "off"))
    private fun recorded(seconds: Int, streaming: Boolean): JSONObject {
        val item = store.create("en"); store.audio(item).appendBytes(pcm(seconds)); store.finishCapture(item, null)
        store.prepareDirect(item, options(), cloud(streaming), "raw")
        return store.load(item.getString("id"))
    }

    /** Range client and cloud client in one, as GeminiClient is. */
    private open inner class Fake(val range: (Long, Long) -> String = { s, e -> "r$s-$e" }, val whole: () -> String = { "whole" }) : CloudClient, RangeClient {
        override val usesFiles = false
        override fun usesFilesFor(audio: File, options: JSONObject) = false
        override fun transcribeRange(audio: File, start: Long, end: Long, options: JSONObject) = range(start, end)
        override fun transcribeAudio(audio: File, options: JSONObject) = whole()
        override fun cancel() {}
        override fun checkModel(model: String) {}
        override fun cleanup(model: String, text: String, reasoning: String, systemPrompt: String) = text
    }

    fun testEngineCommitsTheLiveOutcomeWithReceiptAndDropsTheCheckpoint() {
        val item = recorded(6, streaming = true); val id = item.getString("id"); val data = store.audio(item).readBytes().copyOfRange(44, 44 + 6 * second)
        val recognition = StreamingRecognition(id, store.audio(item), item.getJSONObject("pending").getJSONObject("options"), StreamingRecognition.Checkpoints.of(context),
            live = { listener -> object : LiveConnection {
                override val turnOpen = true
                override fun audio(bytes: ByteArray, offset: Int, count: Int) {}
                override fun end() { Thread { listener.text("live text", true); listener.complete() }.start() }
                override fun close() {}
            } }, ranges = { fail("no batch request on the live path"); Fake() }).start()
        StreamingRecognition.register(recognition)
        var at = 0; while (at < data.size) { recognition.frames(data.copyOfRange(at, at + 3200), 3200, at.toLong()); at += 3200 }
        recognition.finished(data.size.toLong(), true)
        val engine = DirectCloudEngine(context, store) { Fake() }
        assertEquals("Cloud transcript saved on this phone.", engine.execute(item, DirectCloud.ORIGIN))
        val saved = store.load(id); val job = saved.getJSONArray("attempts").getJSONObject(0)
        assertFalse(saved.has("pending"))
        assertEquals("live text", CompanionProcessing.raw(job))
        assertEquals("direct_gemini_streaming", job.getString("transport"))
        assertEquals("live", job.getJSONObject("recognition").getString("method"))
        assertTrue(job.getJSONObject("client_timing").has("asr_finished_elapsed_ms"))
        assertNull(StreamingRecognition.Checkpoints.of(context).load(id)); assertNull(StreamingRecognition.get(id))
        assertEquals(ProcessingStage.RAW_READY, ProcessingState.view(saved, null, null).stage)
    }

    fun testEngineResumesFromTheCheckpointAndRequestsOnlyTheGap() {
        val item = recorded(50, streaming = true); val id = item.getString("id")
        val options = item.getJSONObject("pending").getJSONObject("options")
        val half = 25L * second
        StreamingRecognition.Checkpoints.of(context).update(id, options) { it.getJSONArray("pieces").put(JSONObject().put("start", 0).put("end", half).put("source", "live").put("text", "first half")) }
        val calls = Collections.synchronizedList(mutableListOf<Pair<Long, Long>>())
        val engine = DirectCloudEngine(context, store) { Fake({ s, e -> calls += s to e; "second half" }) }
        engine.execute(item, DirectCloud.ORIGIN)
        val job = store.load(id).getJSONArray("attempts").getJSONObject(0)
        assertEquals("first half second half", CompanionProcessing.raw(job))
        assertEquals(listOf(half to 50L * second), calls.toList())
        assertEquals("resumed", job.getJSONObject("recognition").getString("method"))
        assertNull(StreamingRecognition.Checkpoints.of(context).load(id))
    }

    fun testUnsentFailureQueuesABoundedAutomaticRerunWithoutAnError() {
        val item = recorded(5, streaming = true); val id = item.getString("id")
        StreamingRecognition.Checkpoints.of(context).update(id, item.getJSONObject("pending").getJSONObject("options")) { }
        repeat(DirectCloudEngine.MAX_AUTOMATIC_RERUNS) { attempt ->
            val engine = DirectCloudEngine(context, store) { Fake({ _, _ -> throw java.net.UnknownHostException("offline") }) }
            engine.execute(store.load(id), DirectCloud.ORIGIN)
            val saved = store.load(id)
            assertTrue(engine.waitingForNetwork); assertFalse(saved.has("pending_cloud_error"))
            assertEquals("network", saved.getString("pending_automatic_rerun"))
            assertEquals(attempt + 1, saved.getJSONObject("pending_timing").getInt("automatic_reruns"))
            val view = ProcessingState.view(saved, null, null)
            assertEquals(ProcessingStage.SAVED, view.stage); assertEquals("waiting_for_network", view.error!!.code)
            assertEquals(ProcessingAction.RETRY_WHEN_ONLINE, view.error!!.action)
        }
        val engine = DirectCloudEngine(context, store) { Fake({ _, _ -> throw java.net.UnknownHostException("offline") }) }
        engine.execute(store.load(id), DirectCloud.ORIGIN)
        val saved = store.load(id)
        assertFalse("reruns are bounded", engine.waitingForNetwork)
        assertEquals("network", saved.getString("pending_cloud_error_code"))
        assertEquals(ProcessingStage.RECOVERY, ProcessingState.view(saved, null, null).stage)
        StreamingRecognition.Checkpoints.of(context).delete(id)
    }

    fun testAFailedInMemoryRunIsUsedOnceAndTheRerunResumesFromItsCheckpoint() {
        val item = recorded(50, streaming = true); val id = item.getString("id")
        val data = store.audio(item).readBytes().copyOfRange(44, 44 + 50 * second)
        val offline = object : RangeClient {
            override fun transcribeRange(audio: File, start: Long, end: Long, options: JSONObject): String = throw java.net.UnknownHostException("offline")
            override fun cancel() {}
        }
        // Offline at Stop: no live connection, every range fails.
        val recognition = StreamingRecognition(id, store.audio(item), item.getJSONObject("pending").getJSONObject("options"),
            StreamingRecognition.Checkpoints.of(context), live = null, ranges = { offline }).start()
        StreamingRecognition.register(recognition)
        var at = 0; while (at < data.size) { recognition.frames(data.copyOfRange(at, at + 3200), 3200, at.toLong()); at += 3200 }
        recognition.finished(data.size.toLong(), true)
        val first = DirectCloudEngine(context, store) { Fake() }
        first.execute(item, DirectCloud.ORIGIN)
        assertTrue(first.waitingForNetwork); assertNull("a failed in-memory run is not awaited again", StreamingRecognition.get(id))
        assertEquals("live_unavailable", StreamingRecognition.Checkpoints.of(context).load(id)!!.getString("live_failure"))
        DirectCloudEngine(context, store) { Fake({ s, e -> "r$s-$e" }) }.execute(store.load(id), DirectCloud.ORIGIN)
        val job = store.load(id).getJSONArray("attempts").getJSONObject(0)
        val bounds = ChunkPlanner.plan(store.audio(item), 0, data.size.toLong())
        assertEquals(bounds.zipWithNext().joinToString(" ") { (s, e) -> "r$s-$e" }, CompanionProcessing.raw(job))
        assertEquals("resumed", job.getJSONObject("recognition").getString("method"))
    }

    fun testWholeFileTimeoutIsUncertainAndNeverRerunsAutomatically() {
        val item = recorded(3, streaming = false); val id = item.getString("id")
        val engine = DirectCloudEngine(context, store) { Fake(whole = { throw java.net.SocketTimeoutException("read") }) }
        engine.execute(item, DirectCloud.ORIGIN)
        val saved = store.load(id)
        assertFalse(engine.waitingForNetwork); assertFalse(saved.has("pending_automatic_rerun"))
        assertEquals("timeout", saved.getString("pending_cloud_error_code"))
        assertTrue("the charge warning is kept", saved.getString("pending_cloud_error").contains("another provider charge"))
        val view = ProcessingState.view(saved, null, null)
        assertEquals("timeout", view.error!!.code); assertEquals(ProcessingAction.RETRY_SAVED_AUDIO, view.error!!.action)
        // Before anything was sent, the same request may run again by itself.
        val offline = DirectCloudEngine(context, store) { Fake(whole = { throw java.net.ConnectException("no route") }) }
        offline.execute(store.load(id), DirectCloud.ORIGIN)
        assertTrue(offline.waitingForNetwork)
    }

    fun testExplicitRetriesNeverInheritStreaming() {
        val direct = DirectCloud(context, "gemini")
        val before = context.getSharedPreferences("direct-cloud", 0).all["streaming"]
        try {
            direct.saveStreaming(true)
            assertEquals("streaming", direct.captureChoice(streaming = true).getJSONObject("cloud").getString("recognition"))
            assertFalse(direct.captureChoice().getJSONObject("cloud").has("recognition"))
            direct.saveStreaming(false)
            assertFalse(direct.captureChoice(streaming = true).getJSONObject("cloud").has("recognition"))
            assertFalse(DirectCloud(context, "openai").streaming())
        } finally { context.getSharedPreferences("direct-cloud", 0).edit().apply { if (before is Boolean) putBoolean("streaming", before) else remove("streaming") }.commit() }
    }

    fun testExpeditedRefusalFallsBackToARegularJob() {
        val builder = android.app.job.JobInfo.Builder(56, android.content.ComponentName(context, TranscriptionJobService::class.java))
            .setRequiredNetworkType(android.app.job.JobInfo.NETWORK_TYPE_ANY)
        val seen = mutableListOf<Boolean>()
        assertEquals(false, SessionManager.scheduleWithFallback(builder) { seen += it.isExpedited; if (it.isExpedited) android.app.job.JobScheduler.RESULT_FAILURE else android.app.job.JobScheduler.RESULT_SUCCESS })
        assertEquals(listOf(true, false), seen)
        assertEquals("a quota exception is a refusal too", false, SessionManager.scheduleWithFallback(builder) { if (it.isExpedited) throw IllegalStateException("quota") else android.app.job.JobScheduler.RESULT_SUCCESS })
        assertEquals(true, SessionManager.scheduleWithFallback(builder) { android.app.job.JobScheduler.RESULT_SUCCESS })
        assertNull(SessionManager.scheduleWithFallback(builder) { android.app.job.JobScheduler.RESULT_FAILURE })
    }

    fun testOnlyCheckpointedStreamingWorkRerunsAfterAnAndroidStop() {
        fun pending(vararg fields: Pair<String, Any>) = JSONObject().put("pending", JSONObject()).apply { fields.forEach { (k, v) -> put(k, v) } }
        assertFalse("a stopped phone run records a failed attempt instead", SessionManager.safeRerun(pending("pending_engine" to JSONObject())))
        assertTrue("streaming keeps finished ranges", SessionManager.safeRerun(pending("pending_cloud" to cloud(true))))
        assertFalse("a sent whole-file request may be charged again", SessionManager.safeRerun(pending("pending_cloud" to cloud(false))))
        assertFalse("reruns are bounded", SessionManager.safeRerun(pending("pending_cloud" to cloud(true),
            "pending_timing" to JSONObject().put("automatic_reruns", DirectCloudEngine.MAX_AUTOMATIC_RERUNS))))
    }

    fun testAndroidRestartNeverResendsAWholeFileRequestThatMayHaveReachedTheProvider() {
        fun item(cloud: JSONObject, timing: JSONObject, vararg fields: Pair<String, Any>) = JSONObject().put("pending", JSONObject())
            .put("pending_cloud", cloud).put("pending_timing", timing).apply { fields.forEach { (k, v) -> put(k, v) } }
        val sent = JSONObject().put("cloud_request_started_elapsed_ms", 1L)
        // Found on the S25: process death during a whole-file request, then Android restarted the job and sent it again.
        assertTrue(SessionManager.uncertainCharge(item(cloud(false), sent)))
        assertTrue("an uploaded file's request too", SessionManager.uncertainCharge(item(cloud(false), JSONObject().put("asr_started_elapsed_ms", 1L))))
        assertTrue("a streaming session that fell back to the whole file too", SessionManager.uncertainCharge(item(cloud(true), sent)))
        assertFalse("streaming resumes from its checkpoint", SessionManager.uncertainCharge(item(cloud(true), JSONObject().put("asr_started_elapsed_ms", 1L))))
        assertFalse("the answer arrived", SessionManager.uncertainCharge(item(cloud(false), JSONObject(sent.toString()).put("cloud_request_finished_elapsed_ms", 2L))))
        assertFalse("nothing was sent", SessionManager.uncertainCharge(item(cloud(false), JSONObject().put("queued_elapsed_ms", 1L))))
        assertFalse("an unsent failure queued its own rerun", SessionManager.uncertainCharge(item(cloud(false), sent, "pending_automatic_rerun" to "network")))
        assertFalse("phone runs cost nothing", SessionManager.uncertainCharge(JSONObject().put("pending", JSONObject()).put("pending_engine", JSONObject()).put("pending_timing", sent)))
        val view = ProcessingState.view(recorded(1, streaming = false).put("pending_cloud_error", "x").put("pending_cloud_error_code", "interrupted"), null, null)
        assertEquals(ProcessingAction.RETRY_SAVED_AUDIO, view.error!!.action)
    }

    fun testADeclinedRequestKeepsItsReason() {
        val blocked = JSONObject().put("promptFeedback", JSONObject().put("blockReason", "OTHER"))
        try { GeminiClient.text(blocked); fail("declined") } catch (_: ProviderRefusal) { }
        val item = recorded(3, streaming = false); val id = item.getString("id")
        val engine = DirectCloudEngine(context, store) { Fake(whole = { GeminiClient.text(blocked) }) }
        engine.execute(item, DirectCloud.ORIGIN)
        val saved = store.load(id)
        assertFalse(engine.waitingForNetwork)
        assertEquals("provider_declined", saved.getString("pending_cloud_error_code"))
        assertTrue(saved.getString("pending_cloud_error").startsWith("Gemini declined this request"))
        assertEquals(ProcessingAction.RETRY_SAVED_AUDIO, ProcessingState.view(saved, null, null).error!!.action)
    }

    fun testProgressIsMeasuredForTheSameAttemptOnly() {
        val item = recorded(3, streaming = true); val id = item.getString("id"); val attempt = item.getJSONObject("pending").getString("idempotency_key")
        store.load(id).also { it.put("pending_timing", JSONObject().put("request_elapsed_ms", 1)); store.save(it) }
        assertTrue(ProcessingEvents.emit(id, attempt, ProcessingStage.RECOGNIZING))
        ProcessingEvents.progress(id, attempt, 2, 8)
        ProcessingEvents.progress(id, "another-attempt", 7, 8)
        val view = ProcessingState.view(store.load(id), null, ProcessingEvents.get(id))
        assertEquals(ProcessingStage.RECOGNIZING, view.stage); assertEquals(0.25f, view.progress)
        assertEquals(0.25, view.toJson().getDouble("progress"), 0.0001)
    }

    // --- Polishing -----------------------------------------------------------------------------

    private fun sentence(i: Int) = "On day $i we did not move the 3 boxes to the north room because the door was locked. "
    private fun polishItem(raw: String, corrections: JSONArray = JSONArray()): String {
        val item = store.create("en"); store.audio(item).appendBytes(ByteArray(3200)); store.finishCapture(item, null)
        val selection = JSONObject().put("provider", "gemini").put("model", DirectCloud.DEFAULT_MODEL).put("reasoning", "provider_default")
        val job = JSONObject().put("id", UUID.randomUUID().toString()).put("execution_location", "phone").put("delivery_choice", "cleaned")
            .put("request", JSONObject().put("idempotency_key", UUID.randomUUID().toString()).put("options", JSONObject()
                .put("vocabulary", JSONObject().put("hints", JSONArray()).put("corrections", corrections).put("snippets", JSONArray()))
                .put("cleanup", JSONObject().put("mode", "enabled").put("selection", selection))))
            .put("status", JSONObject().put("state", "succeeded").put("result", JSONObject().put("transcript", JSONObject().put("raw_text", raw))))
        item.getJSONArray("attempts").put(job); store.save(item)
        return item.getString("id")
    }
    private fun polisher(delayMs: Long = 0, change: (String) -> String = { it }, active: AtomicInteger = AtomicInteger(), peak: AtomicInteger = AtomicInteger()) = object : CloudClient {
        @Volatile var cancelled = false
        override fun cancel() { cancelled = true }
        override fun checkModel(model: String) {}
        override fun cleanup(model: String, text: String, reasoning: String, systemPrompt: String): String {
            peak.accumulateAndGet(active.incrementAndGet(), ::maxOf)
            try { var waited = 0L; while (waited < delayMs) { if (cancelled) throw java.io.InterruptedIOException(); Thread.sleep(20); waited += 20 }; return change(text) }
            finally { active.decrementAndGet() }
        }
    }

    fun testBlocksReproduceTheDictionaryTextExactly() {
        val raw = (1..40).joinToString("") { sentence(it) } + "And then Writer  wrote “quoted.”  End"
        val vocabulary = JSONObject().put("hints", JSONArray()).put("snippets", JSONArray())
            .put("corrections", JSONArray().put(JSONObject().put("from", "Writer").put("to", "TrickTic")))
        val segments = OfflineVocabulary.segments(raw, vocabulary)
        val blocks = DirectCloudProcessing.blocks(segments)
        assertTrue(blocks.size >= 4)
        assertEquals(segments.joinToString("") { it.text }, blocks.flatten().joinToString("") { it.text })
        assertTrue(blocks.flatten().any { it.exact && it.text == "TrickTic" })
        assertTrue("one editable span per run between exact values", blocks.all { block -> block.zipWithNext().none { (a, b) -> !a.exact && !b.exact } })
        val run = (1..700).joinToString(" ") { "w$it" }
        assertTrue("an unpunctuated run is still bounded", DirectCloudProcessing.sentences(run, 150).all { DuplicationGuard.words(it).size <= 150 })
        assertEquals(run, DirectCloudProcessing.sentences(run, 150).joinToString(""))
    }

    fun testLongEntryPolishesBlocksInParallelAndValidatesEach() {
        val raw = (1..40).joinToString("") { sentence(it) }.trim()
        val id = polishItem(raw); val active = AtomicInteger(); val peak = AtomicInteger()
        val outcome = DirectCloudProcessing(context, store) { polisher(200, { it }, active, peak) }.polishPhoneLive(id, 5000)
        assertTrue("$outcome ${store.load(id).getJSONArray("attempts").getJSONObject(0).optJSONObject("polish_outcome")}", outcome is PhonePolish.Polished)
        val job = store.load(id).getJSONArray("attempts").getJSONObject(0)
        assertEquals(raw, job.getString("cleaned_text"))
        val receipt = job.getJSONArray("processing_attempts").getJSONObject(0)
        assertTrue(receipt.getInt("blocks") >= 4); assertTrue("blocks ran concurrently", peak.get() > 1)
        assertEquals(PolishView("succeeded", null, false), ProcessingState.view(store.load(id), null, null).polish)
    }

    fun testOneRejectedBlockRejectsTheWholePolishAsValidation() {
        val raw = (1..40).joinToString("") { sentence(it) }.trim()
        val id = polishItem(raw)
        // One block silently drops a negation: meaning validation must reject the whole candidate.
        val outcome = DirectCloudProcessing(context, store) { polisher(change = { if ("day 33 " in it) it.replace("day 33 we did not", "day 33 we did") else it }) }.polishPhoneLive(id, 5000)
        assertTrue(outcome is PhonePolish.FellBack)
        val job = store.load(id).getJSONArray("attempts").getJSONObject(0)
        assertFalse("no partly polished text is offered", job.has("cleaned_text"))
        assertEquals("validation_rejected", job.getJSONObject("polish_outcome").getString("failure"))
        assertEquals(raw, CompanionProcessing.deliveryText(job))
        assertEquals(PolishView("failed", "validation_rejected", false, "anchors:negation:dropped"), ProcessingState.view(store.load(id), null, null).polish)
    }

    fun testTimeoutAndUnavailableAreSeparateFailures() {
        val id = polishItem(sentence(1).trim())
        DirectCloudProcessing(context, store) { polisher(3000) }.polishPhoneLive(id, 1000)
        assertEquals("timeout", store.load(id).getJSONArray("attempts").getJSONObject(0).getJSONObject("polish_outcome").getString("failure"))
        val other = polishItem(sentence(2).trim())
        DirectCloudProcessing(context, store) { throw ConnectionFailure("selection", "Add your key.") }.polishPhoneLive(other, 1000)
        assertEquals("unavailable", store.load(other).getJSONArray("attempts").getJSONObject(0).getJSONObject("polish_outcome").getString("failure"))
    }

    fun testLateCompletionSavesAPolishedVersionWithoutChangingWhatIsInserted() {
        val raw = sentence(1).trim(); val id = polishItem(raw)
        val processing = DirectCloudProcessing(context, store) { polisher(1200, { it.replace("On day", "On Day") }) }
        val started = android.os.SystemClock.elapsedRealtime()
        assertEquals(PhonePolish.TimedOut, processing.polishPhoneLive(id, 1000, continueLate = true))
        assertTrue("the insertion budget is honoured", android.os.SystemClock.elapsedRealtime() - started < 1200)
        var item = store.load(id); var job = item.getJSONArray("attempts").getJSONObject(0)
        assertTrue(job.getJSONObject("polish_outcome").getBoolean("original_used"))
        assertTrue(job.getJSONObject("polish_outcome").getBoolean("continuing"))
        assertEquals("the original is delivered now", raw, CompanionProcessing.deliveryText(job))
        assertTrue(DirectCloudProcessing.isContinuing(id))
        assertEquals("running", ProcessingState.view(item, null, null).polish.state)
        assertEquals(ProcessingStage.RAW_READY, ProcessingState.view(item, null, null).stage)
        try { DirectCloudProcessing(context, store) { polisher() }.run(id); fail() } catch (_: IllegalStateException) { }
        // Delivery verification writes to the same session meanwhile.
        store.update(id) { it.getJSONArray("attempts").getJSONObject(0).put("polish_delivery", JSONObject().put("pasted", true).put("original_used", true)) }
        val deadline = android.os.SystemClock.elapsedRealtime() + 10_000
        while (DirectCloudProcessing.isContinuing(id) && android.os.SystemClock.elapsedRealtime() < deadline) Thread.sleep(50)
        item = store.load(id); job = item.getJSONArray("attempts").getJSONObject(0)
        assertFalse(item.has("pending_postprocess"))
        assertEquals("On Day 1 we did not move the 3 boxes to the north room because the door was locked.", job.getString("cleaned_text"))
        assertTrue(job.getJSONObject("late_polish_outcome").getBoolean("late"))
        assertTrue("the delivery receipt survived the late commit", job.getJSONObject("polish_delivery").getBoolean("pasted"))
        assertEquals("inserted text never changes after the fact", raw, CompanionProcessing.deliveryText(job))
        assertEquals(PolishView("succeeded", null, true), ProcessingState.view(item, null, null).polish)
    }

    fun testSuperGuardMatchesTheAndroidCorpusExpectations() {
        val assets = context.createPackageContext(context.packageName + ".test", 0).assets
        val cases = JSONObject(assets.open("cleanup/desktop-polish.json").bufferedReader().use { it.readText() }).getJSONArray("cases")
        assertTrue(cases.length() >= 40)
        for (i in 0 until cases.length()) {
            val case = cases.getJSONObject(i)
            // Desktop Formatting defaults: lists on.
            val polish = CleanupValidation.Polish(case.getBoolean("spoken"), case.getBoolean("fillers"), case.getBoolean("self_corrections"), true)
            // Desktop Super may opt into looser checks; Android retains its own guard.
            assertEquals(case.getString("id"), case.optBoolean("android_accepted", case.getBoolean("accepted")), CleanupValidation.preservesPolish(case.getString("raw"), case.getString("output"), polish))
        }
        assertFalse(CleanupValidation.preservesPolish("pay 5 actually 10 dollars", "Pay 10 dollars.", CleanupValidation.Polish(true, true, false, true)))
    }

    fun testQueuedPolishWithNothingToTypeIgnoresTheInsertionWait() {
        // S25 (MW-ANDROID-06): History's Polish again types nothing, so a polish slower than the
        // 3 s insertion wait must still produce the polished version instead of a timeout.
        val raw = (1..4).joinToString("") { sentence(it) }.trim()
        val id = polishItem(raw)
        val job = store.load(id).getJSONArray("attempts").getJSONObject(0)
        val processing = DirectCloudProcessing(context, store) { polisher(delayMs = 4500) }
        processing.prepare(id, job.getString("id"), job.getJSONObject("request").getJSONObject("options").getJSONObject("cleanup").getJSONObject("selection"))
        processing.run(id, inserting = false)
        assertTrue(store.load(id).getJSONArray("attempts").getJSONObject(0).has("cleaned_text"))
        assertEquals(PolishView("succeeded", null, false), ProcessingState.view(store.load(id), null, null).polish)
    }

    fun testSuperGuardAcceptsCurrencySpellingsButNotChangedValues() {
        // S25 (MW-ANDROID-06): Gemini writes "$150" and a polish writes "150 dollars", or the reverse.
        val polish = CleanupValidation.Polish(true, true, false, true)
        fun rejection(raw: String, output: String) = CleanupValidation.polishRejection(raw, output, polish)
        assertNull(rejection("and to think we can save all that misery by the payment of $150", "And to think we can save all that misery by paying 150 dollars."))
        assertNull(rejection("we paid 150 dollars and then 20 percent more", "We paid $150, and then 20% more."))
        assertNull(rejection("it was one dollar", "It was $1."))
        assertEquals("anchors:digit:dropped", rejection("we paid $150", "We paid 160 dollars."))
        assertEquals("anchors:currency:dropped", rejection("we paid $150", "We paid 150 euros."))
        assertEquals("anchors:currency:added", rejection("we paid 150 euros", "We paid $150 euros."))
    }

    fun testSuperGrammarRewritePassesWhileMeaningChangesStillFail() {
        val raw = "so we was going to the store and we buyed 3 apples but we did not get the bread"
        val fixed = "So we were going to the store, and we bought 3 apples, but we did not get the bread."
        val good = polishItem(raw)
        assertTrue(DirectCloudProcessing(context, store) { polisher(change = { fixed }) }.polishPhoneLive(good, 5000) is PhonePolish.Polished)
        assertEquals(fixed, store.load(good).getJSONArray("attempts").getJSONObject(0).getString("cleaned_text"))
        for (changed in listOf(fixed.replace("3 apples", "4 apples"), fixed.replace("did not get", "got"), "Here is a summary of your shopping trip.")) {
            val id = polishItem(raw)
            assertTrue(changed, DirectCloudProcessing(context, store) { polisher(change = { changed }) }.polishPhoneLive(id, 5000) is PhonePolish.FellBack)
            assertEquals("validation_rejected", store.load(id).getJSONArray("attempts").getJSONObject(0).getJSONObject("polish_outcome").getString("failure"))
        }
        // Light keeps the strict guard.
        val light = context.getSharedPreferences("companion", 0); val style = light.getString("polish_style", null)
        try {
            light.edit().putString("polish_style", "light").commit()
            val id = polishItem(raw)
            assertTrue(DirectCloudProcessing(context, store) { polisher(change = { fixed }) }.polishPhoneLive(id, 5000) is PhonePolish.FellBack)
        } finally { light.edit().apply { if (style != null) putString("polish_style", style) else remove("polish_style") }.commit() }
    }

    fun testInsertOriginalNowEndsOnlyTheInsertionWait() {
        val raw = sentence(1).trim(); val id = polishItem(raw)
        assertFalse("nothing to skip yet", DirectCloudProcessing.insertOriginalNow(id))
        val processing = DirectCloudProcessing(context, store) { polisher(1500, { it.replace("On day", "On Day") }) }
        Thread { Thread.sleep(300); assertTrue(DirectCloudProcessing.insertOriginalNow(id)) }.start()
        val started = android.os.SystemClock.elapsedRealtime()
        assertEquals(PhonePolish.TimedOut, processing.polishPhoneLive(id, 10_000, continueLate = true))
        assertTrue("the wait ended early", android.os.SystemClock.elapsedRealtime() - started < 1500)
        val outcome = store.load(id).getJSONArray("attempts").getJSONObject(0).getJSONObject("polish_outcome")
        assertEquals("user_skipped", outcome.getString("reason")); assertEquals("skipped", outcome.getString("failure"))
        assertFalse("the late phase cannot be cut short", DirectCloudProcessing.insertOriginalNow(id))
        val deadline = android.os.SystemClock.elapsedRealtime() + 10_000
        while (DirectCloudProcessing.isContinuing(id) && android.os.SystemClock.elapsedRealtime() < deadline) Thread.sleep(50)
        assertTrue("polishing still finished and was saved", store.load(id).getJSONArray("attempts").getJSONObject(0).has("cleaned_text"))
    }

    fun testKeptAudioHasItsOwnState() {
        val item = store.create("en"); store.audio(item).appendBytes(pcm(2)); store.finishCapture(item, null)
        assertEquals("not_transcribed", ProcessingState.view(store.load(item.getString("id")), null, null).error!!.code)
        store.update(item.getString("id")) { it.getJSONObject("capture_completion").put("kept", true) }
        val view = ProcessingState.view(store.load(item.getString("id")), null, null)
        assertEquals("kept", view.error!!.code); assertEquals(ProcessingAction.TRANSCRIBE_SAVED_AUDIO, view.error!!.action)
    }

    fun testNativePlanMustTileTheRecording() {
        val item = store.create("en"); store.audio(item).appendBytes(ByteArray(64000)); store.finishCapture(item, null)
        val native = PhoneNative(); val handle = native.create()
        try {
            for (bounds in listOf(longArrayOf(0, 100), longArrayOf(0, 20000, 10000, 32000), longArrayOf(5, 32000))) {
                try { native.transcribe(handle, "/missing", store.audio(item).path, "en", bounds); fail() }
                catch (error: java.io.IOException) { assertTrue(error.message!!.contains("plan")) }
            }
        } finally { native.destroy(handle) }
    }
}
