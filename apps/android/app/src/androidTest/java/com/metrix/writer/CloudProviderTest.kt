@file:Suppress("DEPRECATION")
package com.metrix.writer

import android.test.AndroidTestCase
import org.json.JSONObject
import java.io.ByteArrayInputStream
import java.io.ByteArrayOutputStream
import java.io.File
import java.net.HttpURLConnection
import java.net.URL
import java.util.UUID

/** Protocol fixtures exercise production adapters; these are not live-provider evidence. */
class CloudProviderTest : AndroidTestCase() {
    private lateinit var store: SessionStore
    override fun setUp() { super.setUp(); store = SessionStore(File(context.cacheDir, "multi-cloud-${UUID.randomUUID()}")) }
    override fun tearDown() { store.directory.deleteRecursively(); super.tearDown() }
    private class Response(url: URL, private val status: Int = 200, private val body: String = "{\"text\":\"fixture transcript\"}") : HttpURLConnection(url) {
        val sent = ByteArrayOutputStream()
        override fun connect() {}
        override fun disconnect() {}
        override fun usingProxy() = false
        override fun getResponseCode() = status
        override fun getInputStream() = ByteArrayInputStream(body.toByteArray())
        override fun getOutputStream() = sent
    }
    private fun audio(): JSONObject = store.create("mixed").also { store.audio(it).appendBytes(ByteArray(640) { index -> (index % 127).toByte() }); store.finishCapture(it, null) }
    private fun snapshot(provider: String) = JSONObject().put("provider", provider).put("credential_id", UUID.randomUUID()).put("transport_version", 1)
    fun testMultipartUsesFixedProviderOriginAndPreservesMixedLanguage() {
        for (provider in listOf("openai", "groq")) {
            val spec = CloudProviders.get(provider); val item = audio(); var connection: Response? = null
            val client = CompatibleCloudClient(provider, "fixture-only-secret") { url -> Response(url).also { connection = it } }
            val options = CompanionPreferences(context).options(provider, spec.speech.first().id, "mixed")
            assertEquals("fixture transcript", client.transcribeAudio(store.audio(item), options))
            val sent = connection!!
            assertEquals(spec.origin + (if (provider == "groq") "/openai" else "") + "/v1/audio/transcriptions", sent.url.toString())
            assertFalse(sent.instanceFollowRedirects)
            assertEquals("Bearer fixture-only-secret", sent.getRequestProperty("Authorization"))
            val body = sent.sent.toString("UTF-8")
            assertTrue(body.contains("filename=\"dictation.wav\"")); assertTrue(body.contains(spec.speech.first().id))
            assertFalse(body.contains("name=\"language\"")); assertFalse(body.contains("fixture-only-secret"))
        }
    }
    fun testAuthQuotaNetworkAndRedirectFailuresRetainRequestAndEarlierOriginal() {
        for (provider in listOf("openai", "groq", "grok")) for (status in listOf(401,403,404,413,429,500,302,0)) {
            val spec = CloudProviders.get(provider); val item = audio()
            item.getJSONArray("attempts").put(JSONObject("""{"id":"earlier","status":{"state":"succeeded","result":{"transcript":{"raw_text":"earlier original"}}}}"""))
            store.prepareDirect(item, CompanionPreferences(context).options(provider, spec.speech.first().id, "en"), snapshot(provider), "raw")
            val before = store.load(item.getString("id")); val hash = PhoneModels.hash(store.audio(item))
            val client = CompatibleCloudClient(provider, "fixture-secret") { url -> if (status == 0) throw java.net.SocketTimeoutException("private-provider-body"); Response(url, status, "private-provider-body") }
            DirectCloudEngine(context, store) { client }.execute(before, spec.origin)
            val saved = store.load(item.getString("id"))
            assertEquals(before.getJSONObject("pending").toString(), saved.getJSONObject("pending").toString())
            assertEquals(before.getJSONArray("attempts").toString(), saved.getJSONArray("attempts").toString())
            assertEquals(hash, PhoneModels.hash(store.audio(saved)))
            assertFalse(saved.toString().contains("fixture-secret")); assertFalse(saved.toString().contains("private-provider-body"))
        }
    }
    fun testProviderAndKeyRevisionsDoNotReroutePendingSpeechOrCleanup() {
        val gemini = DirectCloud(context, "gemini"); val openai = DirectCloud(context, "openai"); val groq = DirectCloud(context, "groq")
        assertFalse(gemini.configured()); assertFalse(openai.configured()); assertFalse(groq.configured())
        val prefs = CompanionPreferences(context); val cleanup = prefs.cleanup(); val delivery = prefs.delivery()
        val cloudPrefs = context.getSharedPreferences("direct-cloud", 0)
        try {
            gemini.saveKey("gemini-fixture-credential-123456789")
            val oldGemini = gemini.snapshot()
            // Existing Gemini encrypted files and default provider remain readable, without migration/re-entry.
            assertEquals("gemini-fixture-credential-123456789", DirectCloud(context).key(oldGemini))
            openai.saveKey("openai-fixture-credential-123456789"); val oldOpenai = openai.snapshot()
            prefs.save("openai/gpt-4.1-mini", "cleaned")
            val choice = gemini.captureChoice()
            assertEquals(oldOpenai.toString(), choice.getJSONObject("cloud").getJSONObject("cleanup_cloud").toString())
            val item = audio(); DirectCloud.prepareChoice(store, item, choice)
            val before = store.load(item.getString("id")).toString()
            openai.saveKey("openai-replacement-credential-123456")
            groq.saveKey("groq-fixture-credential-123456789"); groq.select()
            assertEquals(before, store.load(item.getString("id")).toString())
            assertEquals("openai-fixture-credential-123456789", openai.key(oldOpenai))
            groq.forget(); assertEquals("gemini-fixture-credential-123456789", gemini.key(oldGemini))
        } finally { gemini.forget(); openai.forget(); groq.forget(); cloudPrefs.edit().remove("provider").commit(); prefs.save(cleanup, delivery) }
    }
    fun testCrossProviderCleanupSendsOnlyTextAndRetainsOriginalOnFailure() {
        for (provider in listOf("openai", "groq")) {
            val item = audio(); val spec = CloudProviders.get(provider)
            val options = CompanionPreferences(context).options("gemini", DirectCloud.DEFAULT_MODEL, "mixed")
            item.getJSONArray("attempts").put(JSONObject().put("id", "job").put("request", JSONObject().put("options", options)).put("status", JSONObject("""{"state":"succeeded","result":{"transcript":{"raw_text":"spoken wording"}}}""")).put("cleaned_text", "earlier polish"))
            store.save(item)
            var request: Response? = null
            val client = CompatibleCloudClient(provider, "fixture-secret") { url -> Response(url, 429).also { request = it } }
            val processing = DirectCloudProcessing(context, store) { client }
            processing.prepare(item.getString("id"), "job", JSONObject().put("provider", provider).put("model", spec.cleanup.first().id), snapshot(provider))
            val before = PhoneModels.hash(store.audio(item)); processing.run(item.getString("id"))
            val saved = store.load(item.getString("id")); assertFalse(saved.has("pending_postprocess"))
            val job = saved.getJSONArray("attempts").getJSONObject(0)
            assertEquals("spoken wording", CompanionProcessing.raw(job)); assertEquals("earlier polish", job.getString("cleaned_text")); assertEquals(before, PhoneModels.hash(store.audio(item)))
            assertTrue(job.getJSONObject("polish_outcome").getBoolean("original_used"))
            assertEquals("spoken wording", CompanionProcessing.deliveryText(job))
            assertTrue(request!!.url.path.endsWith("/chat/completions"))
            val sent = JSONObject(request!!.sent.toString("UTF-8")); assertTrue(sent.has("messages")); assertFalse(sent.has("audio"))
        }
    }
    fun testSuccessfulCompatibleTranscriptionIsDurableWithoutRemoteFileDeletion() {
        val provider = "groq"; val item = audio(); val spec = CloudProviders.get(provider)
        store.prepareDirect(item, CompanionPreferences(context).options(provider, spec.speech.first().id, "he"), snapshot(provider), "raw")
        val client = CompatibleCloudClient(provider, "fixture-secret") { Response(it) }
        DirectCloudEngine(context, store) { client }.execute(store.load(item.getString("id")), spec.origin)
        val saved = store.load(item.getString("id")); assertFalse(saved.has("pending")); assertFalse(saved.has("cloud_upload"))
        val job = saved.getJSONArray("attempts").getJSONObject(0)
        assertEquals("fixture transcript", CompanionProcessing.raw(job)); assertEquals("direct_groq", job.getString("transport")); assertFalse(job.has("cloud_file_cleanup"))
    }
    fun testUnsupportedSpeechModelAndOversizeAudioFailBeforeNetwork() {
        val client = CompatibleCloudClient("openai", "fixture-secret") { fail("Unexpected request"); Response(it) }
        val item = audio()
        try { client.transcribeAudio(store.audio(item), CompanionPreferences(context).options("openai", "gpt-4.1-mini", "en")); fail("Text model accepted for speech") } catch (_: IllegalArgumentException) {}
        java.io.RandomAccessFile(store.audio(item), "rw").use { it.setLength(25_000_001) }
        try { client.transcribeAudio(store.audio(item), CompanionPreferences(context).options("openai", "gpt-4o-mini-transcribe", "en")); fail("Oversize uploaded") } catch (_: IllegalArgumentException) {}
        assertEquals(25_000_001L, store.audio(item).length())
    }
    fun testMissingOrNonTextProviderResultNeverBecomesInventedTranscript() {
        for (body in listOf("{}", "{\"text\":null}", "{\"text\":42}")) {
            val item = audio(); val spec = CloudProviders.get("groq")
            store.prepareDirect(item, CompanionPreferences(context).options("groq", spec.speech.first().id, "en"), snapshot("groq"), "raw")
            val client = CompatibleCloudClient("groq", "fixture-secret") { Response(it, 200, body) }
            DirectCloudEngine(context, store) { client }.execute(item, spec.origin)
            val saved = store.load(item.getString("id"))
            assertTrue(saved.has("pending")); assertEquals(0, saved.getJSONArray("attempts").length())
            assertTrue(store.audio(saved).length() > 44)
        }
    }

}
