@file:Suppress("DEPRECATION")
package com.metrix.writer

import android.test.AndroidTestCase
import com.metrix.writer.ui.Dictation
import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.util.UUID
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicInteger

/** Offline transport fakes exercise the real coordinator, AtomicFile history and delivery selection. */
class PolishReliabilityTest : AndroidTestCase() {
    private lateinit var store: SessionStore
    private val primary = JSONObject().put("provider", "openai").put("model", "gpt-4.1-mini")
    private val backup = JSONObject().put("provider", "gemini").put("model", DirectCloud.DEFAULT_MODEL)
    private fun cloud(provider: String) = JSONObject().put("provider", provider).put("credential_id", UUID.randomUUID().toString()).put("transport_version", 1)
    private fun policy() = JSONObject().put("selection", primary).put("cloud", cloud("openai"))
        .put("fallback", backup).put("fallback_cloud", cloud("gemini")).put("fallback_after_ms", 250L).put("timeout_ms", 3000L).put("system_prompt", "Frozen instructions: preserve English and Hebrew.")
    override fun setUp() { super.setUp(); store = SessionStore(File(context.cacheDir, "polish-reliability-${UUID.randomUUID()}")) }
    override fun tearDown() { store.directory.deleteRecursively(); super.tearDown() }
    private fun item(raw: String = "שלום hello signature", location: String = "phone", exact: String = "PRIVATE EXACT"): JSONObject {
        val item = store.create("mixed"); store.audio(item).appendBytes(ByteArray(32000)); store.finishCapture(item, null)
        val vocabulary = JSONObject().put("hints", JSONArray()).put("corrections", JSONArray())
            .put("snippets", JSONArray().put(JSONObject().put("phrase", "signature").put("text", exact)))
        val job = JSONObject().put("id", UUID.randomUUID().toString()).put("execution_location", location).put("delivery_choice", "cleaned")
            .put("request", JSONObject().put("options", JSONObject().put("vocabulary", vocabulary).put("cleanup", JSONObject().put("mode", "enabled").put("selection", primary))))
            .put("status", JSONObject().put("state", "succeeded").put("result", JSONObject().put("transcript", JSONObject().put("raw_text", raw))))
        item.getJSONArray("attempts").put(job); store.save(item); return item
    }
    private fun job(id: String) = store.load(id).getJSONArray("attempts").getJSONObject(0)
    private fun fake(run: (String, String, String) -> String): CloudClient = fake(run, {})
    private fun fake(run: (String, String, String) -> String, onCancel: () -> Unit): CloudClient = object : CloudClient {
        override fun cancel() { onCancel() }
        override fun checkModel(model: String) {}
        override fun cleanup(model: String, text: String, reasoning: String, systemPrompt: String): String = run(model, text, systemPrompt)
    }
    private fun start(item: JSONObject, policy: JSONObject, factory: (JSONObject) -> CloudClient): DirectCloudProcessing {
        val runner = DirectCloudProcessing(context, store, factory)
        runner.prepare(item.getString("id"), item.getJSONArray("attempts").getJSONObject(0).getString("id"), primary, policy = policy)
        return runner
    }
    fun testRateLimitStartsBackupImmediatelyAndRecordsActualWinnerWithoutSendingSnippets() {
        val item = item(); val id = item.getString("id"); val calls = AtomicInteger(); val frozen = policy().put("fallback_after_ms", 5000L)
        val runner = start(item, frozen) { snapshot -> fake { _, text, prompt ->
            calls.incrementAndGet(); assertEquals(frozen.getString("system_prompt"), prompt); assertFalse(text.contains("PRIVATE EXACT"))
            if (snapshot.getString("provider") == "openai") throw CloudFailure("Quota", 429, 60000L)
            text.uppercase()
        } }
        val at = android.os.SystemClock.elapsedRealtime(); assertTrue(runner.run(id) is PhonePolish.Polished)
        assertTrue(android.os.SystemClock.elapsedRealtime() - at < 2000L)
        assertEquals(2, calls.get()); val outcome = job(id).getJSONObject("polish_outcome")
        assertTrue(outcome.getBoolean("fallback_used")); assertFalse(outcome.getBoolean("original_used"))
        assertEquals("gemini", outcome.getJSONObject("selected_model").getString("provider"))
        assertTrue(CompanionProcessing.deliveryText(job(id)).contains("PRIVATE EXACT"))
        assertEquals("rate_limited", outcome.getJSONArray("model_attempts").getJSONObject(0).getString("error"))
        assertEquals("שלום hello signature", CompanionProcessing.raw(job(id)))
    }
    fun testBlankAndLostLanguageResponsesCannotBeatAValidBackup() {
        for (invalid in listOf("", "Translated English", "שלום בלבד", "hello\u0000world")) {
            val item = item(); val id = item.getString("id")
            val runner = start(item, policy()) { snapshot -> fake { _, text, _ -> if (snapshot.getString("provider") == "openai") invalid else text } }
            assertTrue(runner.run(id) is PhonePolish.Polished)
            val outcome = job(id).getJSONObject("polish_outcome"); assertTrue(outcome.getBoolean("fallback_used"))
            assertEquals("invalid_provider_response", outcome.getJSONArray("model_attempts").getJSONObject(0).getString("error"))
        }
    }
    fun testMissingPrimaryCredentialStillUsesTheFrozenBackupWithoutOpeningPrimaryTransport() {
        val item = item(); val id = item.getString("id"); val frozen = policy()
        frozen.put("cloud", JSONObject().put("provider", "openai").put("unavailable", true))
        val calls = AtomicInteger()
        val runner = start(item, frozen) { snapshot ->
            assertEquals(frozen.getJSONObject("fallback_cloud").toString(), snapshot.toString())
            fake { _, text, _ -> calls.incrementAndGet(); text }
        }
        assertTrue(runner.run(id) is PhonePolish.Polished); assertEquals(1, calls.get())
        val outcome = job(id).getJSONObject("polish_outcome")
        assertTrue(outcome.getBoolean("fallback_used")); assertFalse(outcome.getBoolean("original_used"))
        assertEquals("provider_unavailable", outcome.getJSONArray("model_attempts").getJSONObject(0).getString("error"))
    }
    fun testFailedBackupLeavesPrimaryTimeToFinish() {
        val item = item(); val id = item.getString("id")
        val runner = start(item, policy()) { snapshot -> fake { _, text, _ ->
            if (snapshot.getString("provider") == "gemini") throw java.io.IOException("backup failed")
            Thread.sleep(400); text
        } }
        assertTrue(runner.run(id) is PhonePolish.Polished)
        assertFalse(job(id).getJSONObject("polish_outcome").getBoolean("fallback_used"))
    }
    fun testSpokenFormattingCanBecomePunctuationWithoutCountingAsLostLanguage() {
        for ((raw, polished) in listOf("שלום new paragraph עולם" to "שלום\nעולם", "hello פסיק world" to "Hello, world")) {
            for (enabled in listOf(true, false)) {
                val item = item(raw); val id = item.getString("id"); val calls = AtomicInteger()
                val runner = start(item, policy().put("spoken_punctuation", enabled)) { fake { _, _, _ -> calls.incrementAndGet(); polished } }
                val outcome = runner.run(id)
                if (enabled) {
                    assertTrue(outcome is PhonePolish.Polished); assertEquals(1, calls.get())
                    assertEquals(polished, CompanionProcessing.deliveryText(job(id)))
                } else {
                    assertTrue(outcome is PhonePolish.FellBack); assertEquals(2, calls.get())
                    assertEquals(raw, CompanionProcessing.deliveryText(job(id)))
                }
            }
        }
    }
    fun testBothFailuresSelectOriginalAndCurrentSettingsRetryPreservesDeliveryAndRaw() {
        val item = item(); val id = item.getString("id"); val originalJob = item.getJSONArray("attempts").getJSONObject(0)
        originalJob.put("cleaned_text", "Earlier valid wording"); store.save(item)
        val runner = start(item, policy()) { fake { _, _, _ -> throw java.io.IOException("failed") } }
        assertTrue(runner.run(id) is PhonePolish.FellBack)
        assertEquals("שלום hello PRIVATE EXACT", CompanionProcessing.deliveryText(job(id)))
        assertFalse(store.load(id).has("pending_postprocess")); assertEquals("Earlier valid wording", job(id).getString("cleaned_text"))
        val saved = store.load(id); jobFrom(saved).put("polish_delivery", JSONObject().put("original_used", true).put("pasted", true)); store.save(saved)
        val retry = start(store.load(id), policy().put("system_prompt", "New current instructions")) { fake { _, text, prompt -> assertEquals("New current instructions", prompt); text.uppercase() } }
        assertTrue(retry.run(id) is PhonePolish.Polished)
        val loaded = store.load(id); assertEquals(1, loaded.getJSONArray("attempts").length())
        assertEquals(2, job(id).getJSONArray("processing_attempts").length()); assertEquals("שלום hello signature", CompanionProcessing.raw(job(id)))
        val row = Dictation.from(loaded).latest!!; assertFalse(row.originalUsed); assertTrue(row.originalDelivered)
        assertEquals(CompanionProcessing.deliveryText(job(id)), row.selectedText)
        assertEquals(3, row.edits.size)
        assertEquals(1, row.edits.count { it.succeeded })
        assertEquals("openai", row.edits.single { it.succeeded }.provider)
    }
    private fun jobFrom(item: JSONObject) = item.getJSONArray("attempts").getJSONObject(0)
    fun testBothTimeoutsSettleOnceAndLateWorkersNeverWrite() {
        val item = item(); val id = item.getString("id"); val release = CountDownLatch(1); val entered = CountDownLatch(2); val cancelled = AtomicInteger()
        val runner = start(item, policy()) { fake({ _, text, _ ->
            entered.countDown()
            while (release.count > 0) try { release.await(2, TimeUnit.SECONDS) } catch (_: InterruptedException) {}
            text.uppercase()
        }, { cancelled.incrementAndGet() }) }
        try {
            assertEquals(PhonePolish.TimedOut, runner.run(id, 650))
            assertTrue(entered.await(1, TimeUnit.SECONDS)); assertEquals(2, cancelled.get())
            val before = store.load(id).toString(); val outcome = job(id).getJSONObject("polish_outcome")
            assertTrue(outcome.getBoolean("original_used")); assertEquals(2, outcome.getJSONArray("model_attempts").length())
            val attempts = outcome.getJSONArray("model_attempts")
            assertTrue(attempts.getJSONObject(1).getLong("started_after_ms") >= 250L)
            assertTrue(attempts.getJSONObject(1).getLong("elapsed_ms") < attempts.getJSONObject(0).getLong("elapsed_ms"))
            release.countDown(); Thread.sleep(200)
            assertEquals(before, store.load(id).toString()); assertFalse(job(id).has("cleaned_text"))
        } finally { release.countDown() }
    }
    fun testCooldownSkipsRepeatedPrimaryButStillTriesBackup() {
        val frozen = policy(); val primaryCalls = AtomicInteger(); val backupCalls = AtomicInteger()
        val factory: (JSONObject) -> CloudClient = { snapshot -> fake { _, text, _ ->
            if (snapshot.getString("provider") == "openai") { primaryCalls.incrementAndGet(); throw CloudFailure("Quota", 429, 60000L) }
            backupCalls.incrementAndGet(); text
        } }
        repeat(2) { val item = item(); assertTrue(start(item, frozen, factory).run(item.getString("id")) is PhonePolish.Polished) }
        assertEquals(1, primaryCalls.get()); assertEquals(2, backupCalls.get())
    }
    fun testCancellationKeepsOriginalAndNeverCommitsPolish() {
        val item = item(); val id = item.getString("id")
        val runner = start(item, policy()) { fake { _, _, _ -> fail("request after cancellation"); "" } }
        runner.cancel()
        try { runner.run(id); fail("cancelled polish committed") } catch (_: java.io.InterruptedIOException) {}
        assertTrue(store.load(id).has("pending_postprocess")); assertFalse(job(id).has("polish_outcome"))
        assertEquals("שלום hello PRIVATE EXACT", CompanionProcessing.deliveryText(job(id)))
    }
    fun testRetryAfterSupportsSecondsAndDates() {
        assertEquals(9000L, CloudFailure.retryAfter("9"))
        val date = java.time.ZonedDateTime.now(java.time.ZoneOffset.UTC).plusSeconds(10).format(java.time.format.DateTimeFormatter.RFC_1123_DATE_TIME)
        assertTrue(CloudFailure.retryAfter(date)!! in 8000L..10000L); assertNull(CloudFailure.retryAfter("invalid"))
    }
    fun testMultipleEditableSpansUseOneRequestAndKeepExactValuesOffTheWire() {
        val item = item("hello signature שלום signature goodbye")
        val id = item.getString("id"); val calls = AtomicInteger()
        val runner = start(item, policy()) { fake { _, text, prompt ->
            calls.incrementAndGet(); assertFalse(text.contains("PRIVATE EXACT")); assertFalse(prompt.contains("PRIVATE EXACT"))
            assertTrue(prompt.contains("same order"))
            val spans = JSONObject(text).getJSONArray("editable_spans")
            assertEquals(listOf("hello", "שלום", "goodbye"), (0 until spans.length()).map { spans.getString(it) })
            """{"spans":["Hello!","שלום!","Goodbye!"]}"""
        } }
        assertTrue(runner.run(id) is PhonePolish.Polished); assertEquals(1, calls.get())
        assertEquals("Hello! PRIVATE EXACT שלום! PRIVATE EXACT Goodbye!", CompanionProcessing.deliveryText(job(id)))
        assertEquals("hello signature שלום signature goodbye", CompanionProcessing.raw(job(id)))
    }
    fun testMalformedBatchCannotWinOrPartiallyReplaceProtectedText() {
        for (invalid in listOf("plain text", """{"spans":["Hello"]}""", """{"spans":["Hello",4]}""",
            """{"spans":["Hello","Goodbye"],"extra":true}""", """{"spans":["Hello","Goodbye"]} trailing""",
            """{"spans":["Hello",""]}""")) {
            val item = item("hello signature goodbye"); val id = item.getString("id")
            val runner = start(item, policy()) { snapshot -> fake { _, _, _ ->
                if (snapshot.getString("provider") == "openai") invalid else """{"spans":["Hello!","Goodbye!"]}"""
            } }
            assertTrue(runner.run(id) is PhonePolish.Polished)
            assertTrue(job(id).getJSONObject("polish_outcome").getBoolean("fallback_used"))
            assertEquals("Hello! PRIVATE EXACT Goodbye!", CompanionProcessing.deliveryText(job(id)))
        }
    }
    fun testSnippetOnlyNeverCallsPolishingAndDropsRecognizerPunctuation() {
        val item = item(" Signature."); val id = item.getString("id")
        val runner = start(item, policy()) { fake { _, _, _ -> fail("Exact-only text must not reach a model"); "" } }
        assertTrue(runner.run(id) is PhonePolish.Polished)
        assertEquals("PRIVATE EXACT", CompanionProcessing.deliveryText(job(id)))
    }
    fun testSharedMeaningFixturesThroughCoordinatorStorageAndFinalDelivery() {
        val assets = context.createPackageContext(context.packageName + ".test", 0).assets
        val vectors = JSONObject(assets.open("cleanup/meaning.json").bufferedReader().use { it.readText() })
        val cases = vectors.getJSONArray("cases")
        val exact = vectors.getJSONObject("exact_snippet").getString("text")
        for (index in 0 until cases.length()) {
            val case = cases.getJSONObject(index); val name = case.getString("id")
            val raw = case.getString("raw") + " " + vectors.getJSONObject("exact_snippet").getString("phrase")
            val item = item(raw, exact = exact); val id = item.getString("id")
            val audio = store.audio(item).readBytes(); val calls = AtomicInteger()
            val selectedPolicy = policy().put("spoken_punctuation", case.getBoolean("spoken"))
                .put("remove_fillers", case.getBoolean("fillers"))
            val runner = start(item, selectedPolicy) { fake { _, text, _ ->
                calls.incrementAndGet(); assertFalse(name, text.contains(exact)); case.getString("output")
            } }
            val outcome = runner.run(id)
            val accepted = case.getBoolean("accepted")
            assertEquals(name, accepted, outcome is PhonePolish.Polished)
            val savedJob = job(id); val expected = (if (accepted) case.getString("output") else case.getString("raw")) + " " + exact
            assertEquals(name, expected, CompanionProcessing.deliveryText(savedJob))
            assertEquals(name, raw, CompanionProcessing.raw(savedJob))
            assertEquals(name, !accepted, savedJob.getJSONObject("polish_outcome").getBoolean("original_used"))
            assertEquals(name, if (accepted) 1 else 2, calls.get())
            assertEquals(name, accepted, savedJob.has("cleaned_text"))
            assertTrue(name, audio.contentEquals(store.audio(store.load(id)).readBytes()))
            assertFalse(name, store.load(id).has("pending_postprocess"))
            val row = Dictation.from(store.load(id)).latest!!
            assertEquals(name, expected, row.selectedText)
            assertEquals(name, !accepted, row.originalUsed)
            if (!accepted) {
                assertEquals(name, "invalid_provider_response", savedJob.getJSONObject("polish_outcome").getString("reason"))
                val attempt = savedJob.getJSONArray("processing_attempts").getJSONObject(0)
                assertEquals(name, "failed", attempt.getJSONObject("cleanup").getString("state"))
                assertTrue(name, attempt.getString("message").contains("original"))
            }
            store.directory.resolve(id).deleteRecursively()
        }
    }

    fun testRejectedCriticalChangesCannotWinBeforeValidBackup() {
        val raw = "לא לשלם -5.25 at 09:30 signature"
        for (changed in listOf("לשלם -5.25 at 09:30", "לא לשלם 5.25 at 09:30", "לא לשלם -5,25 at 09:30", "לא לשלם -5.25 at 09.30")) {
            val item = item(raw); val id = item.getString("id"); val calls = AtomicInteger()
            val runner = start(item, policy().put("fallback_after_ms", 5000L)) { snapshot -> fake { _, text, _ ->
                calls.incrementAndGet(); if (snapshot.getString("provider") == "openai") changed else text
            } }
            assertTrue(runner.run(id) is PhonePolish.Polished)
            assertEquals(2, calls.get())
            val outcome = job(id).getJSONObject("polish_outcome")
            assertTrue(outcome.getBoolean("fallback_used")); assertFalse(outcome.getBoolean("original_used"))
            assertEquals("invalid_provider_response", outcome.getJSONArray("model_attempts").getJSONObject(0).getString("error"))
            assertEquals("לא לשלם -5.25 at 09:30 PRIVATE EXACT", CompanionProcessing.deliveryText(job(id)))
            assertEquals(raw, CompanionProcessing.raw(job(id)))
        }
    }

}
