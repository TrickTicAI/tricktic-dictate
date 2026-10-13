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

/**
 * Added text models, reasoning levels and the two providers added in MW-73.
 * Protocol fixtures exercise production adapters; they are not live-provider evidence.
 */
class CustomTextModelsTest : AndroidTestCase() {
    private lateinit var store: CustomTextModels
    override fun setUp() {
        super.setUp()
        context.getSharedPreferences("custom-text-models", android.content.Context.MODE_PRIVATE).edit().clear().commit()
        store = CustomTextModels(context)
    }
    override fun tearDown() {
        context.getSharedPreferences("custom-text-models", android.content.Context.MODE_PRIVATE).edit().clear().commit()
        super.tearDown()
    }

    private class Response(url: URL, private val status: Int = 200, private val body: String) : HttpURLConnection(url) {
        val sent = ByteArrayOutputStream()
        override fun connect() {}
        override fun disconnect() {}
        override fun usingProxy() = false
        override fun getResponseCode() = status
        override fun getInputStream() = ByteArrayInputStream(body.toByteArray())
        override fun getOutputStream() = sent
    }
    private val chat = "{\"choices\":[{\"finish_reason\":\"stop\",\"message\":{\"content\":\"Polished text.\"}}]}"

    fun testOpenRouterIdsRequirePublisherAndOthersRejectSlashes() {
        assertTrue(CustomTextModels.wellFormed("openrouter", "mistralai/mistral-medium-3-5"))
        assertFalse(CustomTextModels.wellFormed("openrouter", "mistral-medium"))
        assertFalse(CustomTextModels.wellFormed("openrouter", "mistralai/"))
        assertTrue(CustomTextModels.wellFormed("openai", "gpt-5.6-terra"))
        assertFalse(CustomTextModels.wellFormed("openai", "openai/gpt-5.6"))
        assertFalse(CustomTextModels.wellFormed("openai", "gpt 5"))
        assertFalse(CustomTextModels.wellFormed("openai", "../secrets"))
        assertFalse(CustomTextModels.wellFormed("openai", ""))
        assertFalse(CustomTextModels.wellFormed("openai", "a".repeat(129)))
        assertTrue(CustomTextModels.wellFormed("openai", "a".repeat(128)))
    }

    fun testValidationRejectsDuplicatesBuiltInsAndOverLimit() {
        assertNull(CustomTextModels.validate(listOf(CustomTextModel("openai", "gpt-5.6-terra"))))
        assertNotNull(CustomTextModels.validate(listOf(CustomTextModel("openai", "gpt-4.1-mini"))))
        assertNotNull(CustomTextModels.validate(listOf(CustomTextModel("openai", "one"), CustomTextModel("openai", "one"))))
        assertNull(CustomTextModels.validate(listOf(CustomTextModel("openai", "one"), CustomTextModel("gemini", "one"))))
        assertNotNull(CustomTextModels.validate((1..CustomTextModels.LIMIT + 1).map { CustomTextModel("openai", "model-$it") }))
        assertNull(CustomTextModels.validate((1..CustomTextModels.LIMIT).map { CustomTextModel("openai", "model-$it") }))
    }

    fun testAddedModelsSurviveAndUnreadableListReadsEmpty() {
        store.add("openrouter", "acme-labs/atlas-1.2")
        assertEquals(listOf("acme-labs/atlas-1.2"), CustomTextModels(context).forProvider("openrouter").map { it.model })
        assertTrue(CustomTextModels(context).forProvider("openai").isEmpty())
        assertTrue(CloudProviders.cleanupAllowed(context, "openrouter", "acme-labs/atlas-1.2"))
        store.remove("openrouter", "acme-labs/atlas-1.2")
        assertFalse(CloudProviders.cleanupAllowed(context, "openrouter", "acme-labs/atlas-1.2"))
        context.getSharedPreferences("custom-text-models", android.content.Context.MODE_PRIVATE)
            .edit().putString("models", "not json").commit()
        assertTrue(CustomTextModels(context).list().isEmpty())
    }

    fun testReasoningDefaultSendsNothingAndOffMapsToNone() {
        assertNull(CustomTextModels.effort("provider_default"))
        assertEquals("none", CustomTextModels.effort("off"))
        assertEquals("high", CustomTextModels.effort("high"))
    }

    fun testOpenRouterCleanupRoutesWithoutTemperatureOrTokenCap() {
        var connection: Response? = null
        val client = CompatibleCloudClient("openrouter", "fixture-only-secret", setOf("acme-labs/atlas-1.2")) { url ->
            Response(url, body = chat).also { connection = it }
        }
        assertEquals("Polished text.", client.cleanup("acme-labs/atlas-1.2", "hello there", "low", "Fixture polish instructions."))
        val sent = connection!!
        assertEquals("https://openrouter.ai/api/v1/chat/completions", sent.url.toString())
        val body = JSONObject(sent.sent.toString("UTF-8"))
        assertEquals("Fixture polish instructions.", body.getJSONArray("messages").getJSONObject(0).getString("content"))
        assertEquals("latency", body.getJSONObject("provider").getString("sort"))
        assertTrue(body.getJSONObject("provider").getBoolean("require_parameters"))
        assertEquals("low", body.getJSONObject("reasoning").getString("effort"))
        assertFalse(body.has("temperature"))
        assertFalse(body.has("max_tokens"))
    }

    fun testGrokCleanupSendsSharedEffortFieldAndDefaultSendsNone() {
        var connection: Response? = null
        var client = CompatibleCloudClient("grok", "fixture-only-secret") { url -> Response(url, body = chat).also { connection = it } }
        client.cleanup("grok-4.6", "hello there", "medium", "Fixture polish instructions.")
        var body = JSONObject(connection!!.sent.toString("UTF-8"))
        assertEquals("https://api.x.ai/v1/chat/completions", connection!!.url.toString())
        assertEquals("medium", body.getString("reasoning_effort"))
        assertEquals(0, body.getInt("temperature"))
        client = CompatibleCloudClient("grok", "fixture-only-secret") { url -> Response(url, body = chat).also { connection = it } }
        client.cleanup("grok-4.6", "hello there", "provider_default", "Fixture polish instructions.")
        body = JSONObject(connection!!.sent.toString("UTF-8"))
        assertFalse(body.has("reasoning_effort"))
    }

    fun testGeminiReasoningUsesThinkingFields() {
        val reply = "{\"candidates\":[{\"finishReason\":\"STOP\",\"content\":{\"parts\":[{\"text\":\"Polished text.\"}]}}]}"
        var connection: Response? = null
        var client = GeminiClient("fixture-only-secret") { url -> Response(url, body = reply).also { connection = it } }
        client.cleanup("gemini-3.8-flash", "hello there", "off", "Fixture polish instructions.")
        var sent = JSONObject(connection!!.sent.toString("UTF-8"))
        assertEquals("Fixture polish instructions.", sent.getJSONObject("systemInstruction").getJSONArray("parts").getJSONObject(0).getString("text"))
        var config = sent.getJSONObject("generationConfig")
        assertEquals(0, config.getJSONObject("thinkingConfig").getInt("thinkingBudget"))
        client = GeminiClient("fixture-only-secret") { url -> Response(url, body = reply).also { connection = it } }
        client.cleanup("gemini-3.8-flash", "hello there", "high", "Fixture polish instructions.")
        config = JSONObject(connection!!.sent.toString("UTF-8")).getJSONObject("generationConfig")
        assertEquals("HIGH", config.getJSONObject("thinkingConfig").getString("thinkingLevel"))
        client = GeminiClient("fixture-only-secret") { url -> Response(url, body = reply).also { connection = it } }
        client.cleanup("gemini-3.8-flash", "hello there", "provider_default", "Fixture polish instructions.")
        assertFalse(JSONObject(connection!!.sent.toString("UTF-8")).getJSONObject("generationConfig").has("thinkingConfig"))
    }

    fun testTextOnlyProviderIsNeverADictationEngine() {
        assertFalse(CloudProviders.get("openrouter").transcribes)
        assertTrue(CloudProviders.transcribers.none { it.id == "openrouter" })
        assertTrue(CloudProviders.get("openrouter").note.isNotEmpty())
        // Selecting it as the engine fails even if a key is saved: there is no speech model to run.
        try { DirectCloud(context, "openrouter").select(); fail("A text-only provider must not become the dictation engine.") }
        catch (_: IllegalStateException) {}
    }

    fun testGrokSpeechRefusesNonEnglishBeforeUpload() {
        val store = SessionStore(File(context.cacheDir, "grok-speech-${UUID.randomUUID()}"))
        try {
            val item = store.create("he").also { store.audio(it).appendBytes(ByteArray(640)); store.finishCapture(it, null) }
            val client = CompatibleCloudClient("grok", "fixture-only-secret") { url -> Response(url, body = "{\"text\":\"unused\"}") }
            val options = CompanionPreferences(context).options("grok", "grok-voice-transcribe-2.0", "he")
            try { client.transcribeAudio(store.audio(item), options); fail("Grok speech is published English only.") }
            catch (_: IllegalArgumentException) {}
        } finally { store.directory.deleteRecursively() }
    }
}
