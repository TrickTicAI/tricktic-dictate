@file:Suppress("DEPRECATION")
package com.metrix.writer

import android.test.AndroidTestCase
import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.io.IOException
import java.util.UUID
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

/** MW-SETTINGS-8: bounded live polish for phone-transcribed jobs, through a fake polisher. */
class PhonePolishTest : AndroidTestCase() {
    private lateinit var store: SessionStore
    private val latch = CountDownLatch(1)
    private var reliability = emptyMap<String, Any?>()

    override fun setUp() {
        super.setUp()
        store = SessionStore(File(context.cacheDir, "phone-polish-${UUID.randomUUID()}"))
        val prefs = context.getSharedPreferences("companion", 0)
        reliability = listOf("cleanup_fallback", "cleanup_fallback_after_ms", "cleanup_timeout_ms").associateWith { prefs.all[it] }
        prefs.edit().remove("cleanup_fallback").remove("cleanup_fallback_after_ms").remove("cleanup_timeout_ms").commit()
    }

    override fun tearDown() {
        latch.countDown()
        store.directory.deleteRecursively()
        val edit = context.getSharedPreferences("companion", 0).edit()
        reliability.forEach { (key, value) -> when (value) { is String -> edit.putString(key, value); is Long -> edit.putLong(key, value); else -> edit.remove(key) } }; edit.commit()
        super.tearDown()
    }

    /**
     * A credential snapshot without a real key: the fake polisher never
     * resolves it, so no Keystore entry is created. Returns a restore block.
     */
    private fun fakeCredential(): () -> Unit {
        val prefs = context.getSharedPreferences("direct-cloud", 0)
        val had = prefs.contains("credential")
        val before = prefs.getString("credential", null)
        check(prefs.edit().putString("credential", UUID.randomUUID().toString()).commit())
        return {
            val edit = prefs.edit()
            if (had) edit.putString("credential", before) else edit.remove("credential")
            edit.commit()
        }
    }

    private fun selection() = JSONObject().put("provider", "gemini").put("model", DirectCloud.DEFAULT_MODEL).put("reasoning", "provider_default")
    private fun vocabulary() = JSONObject().put("hints", JSONArray()).put("corrections", JSONArray()).put("snippets", JSONArray())

    /** A succeeded phone job as PhoneEngine leaves it, with a cleanup selection but no cloud snapshot. */
    private fun phoneItem(withSelection: Boolean): JSONObject {
        val item = store.create("en")
        store.audio(item).appendBytes(ByteArray(32000) { (it % 117).toByte() })
        store.finishCapture(item, null)
        val cleanup = if (withSelection) JSONObject().put("mode", "enabled").put("selection", selection())
        else JSONObject().put("mode", "off")
        val job = JSONObject().put("id", "phone-job-${UUID.randomUUID()}").put("execution_location", "phone")
            .put("delivery_choice", "cleaned")
            .put("request", JSONObject().put("options", JSONObject().put("vocabulary", vocabulary()).put("cleanup", cleanup)))
            .put("status", JSONObject().put("state", "succeeded")
                .put("result", JSONObject().put("transcript", JSONObject().put("raw_text", "hello world"))))
        if (withSelection) job.put("deferred_cleanup", JSONObject(cleanup.toString()))
        item.getJSONArray("attempts").put(job)
        store.save(item)
        return item
    }

    private fun fake(behavior: (String) -> String): CloudClient = object : CloudClient {
        override fun cancel() {}
        override fun checkModel(model: String) {}
        override fun cleanup(model: String, text: String, reasoning: String, systemPrompt: String): String {
            assertTrue("polisher received no instructions", systemPrompt.isNotBlank())
            return behavior(text)
        }
    }

    fun testLivePolishWritesCleanedTextAndConsumesTheDeferredMarker() {
        val restoreCredential = fakeCredential()
        val id = phoneItem(true).getString("id")
        val processing = DirectCloudProcessing(context, store) { fake { "Hello, world." } }
        try {
            val outcome = processing.polishPhoneLive(id)
            assertTrue(outcome is PhonePolish.Polished)
            assertTrue((outcome as PhonePolish.Polished).label.contains("Gemini"))
            val item = store.load(id)
            assertFalse(item.has("pending_postprocess"))
            val job = item.getJSONArray("attempts").getJSONObject(0)
            assertEquals("Hello, world.", job.getString("cleaned_text"))
            assertFalse(job.has("deferred_cleanup"))
            assertEquals("hello world", CompanionProcessing.raw(job))
            val receipts = job.getJSONArray("processing_attempts")
            assertEquals("complete", receipts.getJSONObject(receipts.length() - 1).getString("state"))
            assertEquals("Hello, world.", CompanionProcessing.deliveryText(job))
        } finally {
            restoreCredential()
        }
    }

    fun testCleanupOffSkipsWithoutTouchingTheJob() {
        val restoreCredential = fakeCredential()
        try {
            val id = phoneItem(false).getString("id")
            val processing = DirectCloudProcessing(context, store) { fake { fail("polisher ran with cleanup off"); "" } }
            assertEquals(PhonePolish.Skipped, processing.polishPhoneLive(id))
            val item = store.load(id)
            assertFalse(item.has("pending_postprocess"))
            assertFalse(item.getJSONArray("attempts").getJSONObject(0).has("processing_attempts"))
        } finally {
            restoreCredential()
        }
    }

    fun testProviderFailureSettlesOriginalAndKeepsAnExplicitRetry() {
        val restoreCredential = fakeCredential()
        try {
            val id = phoneItem(true).getString("id")
            val processing = DirectCloudProcessing(context, store) { fake { throw IOException("provider failure") } }
            val outcome = processing.polishPhoneLive(id)
            assertTrue(outcome is PhonePolish.FellBack)
            assertTrue((outcome as PhonePolish.FellBack).reason.contains("Cloud connection failed"))
            val item = store.load(id)
            assertFalse("settled failure must not look like ongoing processing", item.has("pending_postprocess"))
            val job = item.getJSONArray("attempts").getJSONObject(0)
            assertTrue(job.has("deferred_cleanup"))
            assertFalse(job.has("cleaned_text"))
            assertEquals("hello world", CompanionProcessing.deliveryText(job))
            val receipts = job.getJSONArray("processing_attempts")
            assertEquals("complete", receipts.getJSONObject(receipts.length() - 1).getString("state"))
            assertTrue(job.getJSONObject("polish_outcome").getBoolean("original_used"))
        } finally {
            restoreCredential()
        }
    }

    fun testTimeoutFallsBackAndCancelsTheRequest() {
        val restoreCredential = fakeCredential()
        try {
            val id = phoneItem(true).getString("id")
            val processing = DirectCloudProcessing(context, store) {
                fake { latch.await(30, TimeUnit.SECONDS); "too late" }
            }
            assertEquals(PhonePolish.TimedOut, processing.polishPhoneLive(id, 300))
            val item = store.load(id)
            assertFalse("timed-out polish is settled", item.has("pending_postprocess"))
            assertTrue(item.getJSONArray("attempts").getJSONObject(0).getJSONObject("polish_outcome").getBoolean("original_used"))
            assertFalse(item.getJSONArray("attempts").getJSONObject(0).has("cleaned_text"))
        } finally {
            restoreCredential()
        }
    }

    fun testMissingKeyFallsBackBeforeAnyRequest() {
        val prefs = context.getSharedPreferences("direct-cloud", 0)
        val had = prefs.contains("credential")
        val before = prefs.getString("credential", null)
        check(prefs.edit().remove("credential").commit())
        try {
            val id = phoneItem(true).getString("id")
            val processing = DirectCloudProcessing(context, store) { fake { fail("polisher ran without a key"); "" } }
            val outcome = processing.polishPhoneLive(id)
            assertTrue(outcome is PhonePolish.FellBack)
            assertTrue((outcome as PhonePolish.FellBack).reason.contains("Add your Gemini key"))
            val item = store.load(id)
            assertFalse(item.has("pending_postprocess"))
            assertTrue(item.getJSONArray("attempts").getJSONObject(0).has("deferred_cleanup"))
        } finally {
            val edit = prefs.edit()
            if (had) edit.putString("credential", before) else edit.remove("credential")
            edit.commit()
        }
    }

    fun testFailedJobThrows() {
        val item = phoneItem(true)
        item.getJSONArray("attempts").getJSONObject(0).getJSONObject("status").put("state", "failed")
        store.save(item)
        try {
            DirectCloudProcessing(context, store) { fake { "" } }.polishPhoneLive(item.getString("id"))
            fail("polished a failed job")
        } catch (_: IllegalStateException) { }
    }
}
