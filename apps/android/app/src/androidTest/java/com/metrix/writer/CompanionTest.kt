@file:Suppress("DEPRECATION")
package com.metrix.writer

import android.test.AndroidTestCase
import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.util.UUID

class CompanionTest : AndroidTestCase() {
    private lateinit var store: SessionStore
    override fun setUp() { super.setUp(); store = SessionStore(File(context.cacheDir, "companion-test-${UUID.randomUUID()}")) }
    override fun tearDown() { store.directory.deleteRecursively(); super.tearDown() }
    fun testSettingsAndUnicodeDictionarySurviveReopen() {
        val preferences = CompanionPreferences(context); val oldDoc = preferences.document(); val oldCleanup = preferences.cleanup(); val oldDelivery = preferences.delivery()
        try {
            val doc = JSONObject().put("hints", JSONArray().put(JSONObject().put("text", "Metrix עברית 🌍").put("enabled", true)))
                .put("corrections", JSONArray().put(JSONObject().put("from", "מטריקס").put("to", "Metrix").put("enabled", true)))
                .put("snippets", JSONArray().put(JSONObject().put("phrase", "חתימה").put("text", "שלום\nAlex 🌍").put("enabled", false)))
            preferences.saveDocument(doc); preferences.save("gemini/gemini-3.8-flash", "cleaned")
            val reopened = CompanionPreferences(context)
            assertEquals(doc.toString(), reopened.document().toString()); assertEquals("cleaned", reopened.delivery())
            assertEquals(0, reopened.vocabulary().getJSONArray("snippets").length())
            assertEquals("Metrix עברית 🌍", reopened.vocabulary().getJSONArray("hints").getString(0))
            assertEquals("enabled", reopened.options("gemini", "model", "mixed").getJSONObject("cleanup").getString("mode"))
        } finally { preferences.saveDocument(oldDoc); preferences.save(oldCleanup, oldDelivery) }
    }
    fun testRequestDeepSnapshotAndPendingDefaultsRemainImmutable() {
        val item = store.create("he").put("capture_state", "complete"); store.save(item)
        val options = CompanionPreferences(context).options("gemini", "first", "mixed")
        options.getJSONObject("vocabulary").getJSONArray("hints").put("עברית 🌍")
        store.prepareOptions(item, "https://example.invalid", options, "dictionary")
        val before = store.load(item.getString("id")).getJSONObject("pending").toString()
        options.getJSONObject("vocabulary").getJSONArray("hints").put("changed")
        assertEquals(before, item.getJSONObject("pending").toString())
        store.prepareOptions(item, "https://example.invalid", CompanionPreferences(context).options("openai", "changed", "en"), "raw")
        val restored = SessionStore(store.directory).restore().single()
        assertEquals(before, restored.getJSONObject("pending").toString()); assertEquals("dictionary", restored.getString("pending_delivery"))
    }
    fun testAudioShareSnapshotEqualsEveryByteAndRejectsWritesAndTraversal() {
        val item = store.create("mixed").put("capture_state", "complete"); store.save(item)
        val data = ByteArray(100000) { (it % 251).toByte() }; store.audio(item).appendBytes(data)
        val original = store.audio(item).readBytes()
        val uri = AudioExportProvider.snapshot(context, store, item.getString("id"))
        val provider = context.contentResolver
        assertTrue(provider.openInputStream(uri)!!.use { it.readBytes() }.contentEquals(original))
        store.audio(item).appendText("changed source")
        assertTrue(provider.openInputStream(uri)!!.use { it.readBytes() }.contentEquals(original))
        try { provider.openOutputStream(uri); fail("Writable share") } catch (_: Exception) { }
        try { provider.openInputStream(uri.buildUpon().appendPath("../recordings/private.json").build()); fail("Traversal accepted") } catch (_: Exception) { }
        val destination = File(store.directory, "export.wav")
        provider.openInputStream(uri)!!.use { input -> destination.outputStream().use { input.copyTo(it) } }
        assertTrue(original.contentEquals(destination.readBytes()))
    }
    fun testDeleteRedactsOriginalsAndRejectsPendingRequests() {
        val item = store.create("en").put("capture_state", "complete").put("private_text", "remove me"); store.save(item)
        store.prepare(item, "https://example.invalid", "gemini", "model", "en")
        try { store.deleteSaved(item.getString("id")); fail("Pending deletion accepted") } catch (_: IllegalStateException) { }
        item.remove("pending"); store.save(item); store.deleteSaved(item.getString("id"))
        assertTrue(SessionStore(store.directory).restore().isEmpty()); assertFalse(store.audio(item).exists())
        assertFalse(store.load(item.getString("id")).has("private_text"))
    }
    fun testPendingCleanupCannotBeRedirectedToAnotherAttemptOrNewModel() {
        val item = store.create("he").put("capture_state", "complete")
        val jobId = UUID.randomUUID().toString()
        val job = JSONObject().put("id", jobId).put("server_origin", "https://original.invalid")
            .put("status", JSONObject().put("state", "succeeded"))
        item.getJSONArray("attempts").put(job); store.save(item)
        val processing = CompanionProcessing(store, MobileConnection(context))
        val selection = JSONObject().put("provider", "gemini").put("model", "gemini-3.8-flash")
        processing.prepare(item.getString("id"), jobId, selection)
        val before = store.load(item.getString("id")).getJSONObject("pending_postprocess").toString()
        selection.put("model", "changed")
        processing.prepare(item.getString("id"), jobId, selection)
        assertEquals(before, store.load(item.getString("id")).getJSONObject("pending_postprocess").toString())
        try { processing.prepare(item.getString("id"), UUID.randomUUID().toString(), selection); fail("Switched pending job") }
        catch (_: IllegalStateException) { }
        assertEquals(before, SessionStore(store.directory).restore().single().getJSONObject("pending_postprocess").toString())
    }
    fun testDeletionResumesAfterCrashAndRemovesInsertionLedger() {
        val item = store.create("he").put("capture_state", "complete")
        val token = UUID.randomUUID().toString()
        val directory = File(store.directory.parentFile, "deliveries")
        val ledger = DeliveryStore(directory)
        ledger.create(item.getString("id"), token, true)
        ledger.save(ledger.load(token).put("text", "private fixture text"))
        // Crash after recording deletion intent, before deleting either file.
        store.save(JSONObject().put("id", item.getString("id")).put("capture_state", "discarded").put("delete_delivery", token))
        assertTrue(SessionStore(store.directory).restore().isEmpty())
        assertFalse(store.audio(item).exists()); assertFalse(File(directory, "$token.json").exists())
    }
    fun testDeliveryChoicePreservesRawAndFallsBackWithoutInventingCleanedText() {
        val job = JSONObject().put("status", JSONObject().put("state", "succeeded").put("result", JSONObject().put("transcript", JSONObject().put("raw_text", "raw עברית"))))
        job.put("delivery_choice", "cleaned")
        assertEquals("raw עברית", CompanionProcessing.deliveryText(job))
        job.put("dictionary_text", "exact 🌍")
        assertEquals("exact 🌍", CompanionProcessing.deliveryText(job))
        job.put("cleaned_text", "Exact 🌍.")
        assertEquals("Exact 🌍.", CompanionProcessing.deliveryText(job)); assertEquals("raw עברית", CompanionProcessing.raw(job))
    }
}
