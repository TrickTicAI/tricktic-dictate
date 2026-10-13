@file:Suppress("DEPRECATION")
package com.metrix.writer

import android.test.AndroidTestCase
import org.json.JSONArray
import org.json.JSONObject
import java.io.ByteArrayInputStream
import java.io.ByteArrayOutputStream
import java.io.File
import java.net.HttpURLConnection
import java.net.URL
import java.util.UUID

/** Synthetic audio + captured production requests. No provider/account success is claimed. */
class SpeechConformanceTest : AndroidTestCase() {
    private lateinit var store: SessionStore
    override fun setUp() { super.setUp(); store = SessionStore(File(context.cacheDir, "speech-contract-${UUID.randomUUID()}")) }
    override fun tearDown() { store.directory.deleteRecursively(); super.tearDown() }
    private class Response(url: URL, val body: String = "{\"text\":\"fixture\"}", val status: Int = 200) : HttpURLConnection(url) {
        val sent = ByteArrayOutputStream()
        override fun connect() {}
        override fun disconnect() {}
        override fun usingProxy() = false
        override fun getResponseCode() = status
        override fun getInputStream() = ByteArrayInputStream(body.toByteArray())
        override fun getOutputStream() = sent
    }
    private fun audio(): File {
        val item = store.create("en"); store.audio(item).appendBytes(ByteArray(640)); store.finishCapture(item, null)
        return store.audio(item)
    }
    private fun options(provider: String, language: String = "en") = CompanionPreferences(context)
        .options(provider, CloudProviders.get(provider).speech.first().id, language)
        .put("vocabulary", JSONObject().put("hints", JSONArray().put("Metrix").put("TrickTic")))
    private fun names(body: String) = Regex("Content-Disposition: form-data; name=\"([^\"]+)\"").findAll(body).map { it.groupValues[1] }.toList()

    fun testXaiEndpointBooleanFormatRepeatedKeytermsAndFileLast() {
        val wav = audio(); val before = wav.readBytes(); var request: Response? = null
        val client = CompatibleCloudClient("grok", "fixture-only") { Response(it, "{\"text\":\" spoken wording\\n\",\"duration\":1,\"words\":[]}").also { request = it } }
        assertEquals(" spoken wording\n", client.transcribeAudio(wav, options("grok")))
        val sent = request!!; val body = sent.sent.toString("UTF-8")
        assertEquals("https://api.x.ai/v1/stt", sent.url.toString())
        assertEquals(listOf("model", "language", "format", "keyterm", "keyterm", "file"), names(body))
        assertTrue(body.contains("name=\"model\"\r\n\r\ngrok-voice-transcribe-2.0"))
        assertTrue(body.contains("name=\"format\"\r\n\r\ntrue"))
        for (term in listOf("Metrix", "TrickTic")) assertTrue(body.contains("name=\"keyterm\"\r\n\r\n$term"))
        assertFalse(body.contains("fixture-only")); assertFalse(sent.instanceFollowRedirects)
        assertTrue(before.contentEquals(wav.readBytes()))
    }
    fun testXaiLimitsLanguagesTranslationAndStaleModelFailBeforeNetwork() {
        val wav = audio(); val client = CompatibleCloudClient("grok", "fixture-only") { fail("Unexpected upload"); Response(it) }
        val cases = listOf(
            options("grok", "he"), options("grok", "mixed"),
            options("grok").put("output_language", JSONObject().put("mode", "translate").put("language", "en")),
            options("grok").put("selection", JSONObject().put("provider", "grok").put("model", "grok-speech-to-text")),
            options("grok").put("vocabulary", JSONObject().put("hints", JSONArray().put("a".repeat(51)))),
            options("grok").put("vocabulary", JSONObject().put("hints", JSONArray(List(101) { "term" }))),
        )
        for (settings in cases) {
            try { client.transcribeAudio(wav, settings); fail("Unsupported settings accepted") } catch (_: IllegalArgumentException) {}
        }
        // Unicode limit counts characters, not UTF-16 code units.
        val valid = options("grok").put("vocabulary", JSONObject().put("hints", JSONArray().put("😀".repeat(50))))
        assertEquals("fixture", CompatibleCloudClient("grok", "fixture-only") { Response(it) }.transcribeAudio(wav, valid))
    }
    fun testCompatibleModelsUseOnlyTheirSupportedVocabularyAndLanguageFields() {
        val wav = audio()
        for (provider in listOf("openai", "groq")) for (model in CloudProviders.get(provider).speech) for (language in listOf("en", "he", "mixed")) {
            var request: Response? = null
            val client = CompatibleCloudClient(provider, "fixture-only") { Response(it).also { request = it } }
            val settings = options(provider, language).put("selection", JSONObject().put("provider", provider).put("model", model.id))
            assertEquals("fixture", client.transcribeAudio(wav, settings))
            val body = request!!.sent.toString("UTF-8")
            assertEquals(listOfNotNull("model", "response_format", if (language != "mixed") "language" else null, "prompt", "file"), names(body))
            assertTrue(body.contains("name=\"response_format\"\r\n\r\njson"))
            assertTrue(body.contains("name=\"prompt\"\r\n\r\nMetrix, TrickTic"))
            assertTrue(request!!.url.path.endsWith("/audio/transcriptions"))
            assertFalse(body.contains("keyterm")); assertFalse(body.contains("keywords[]"))
        }
    }
    fun testAllMultipartProvidersRejectMalformedTextAndNeverFallback() {
        val wav = audio()
        for (provider in listOf("openai", "groq", "grok")) {
            for (body in listOf("{}", "{\"text\":null}", "{\"text\":42}")) {
                var calls = 0
                val client = CompatibleCloudClient(provider, "fixture-only") { calls++; Response(it, body) }
                try { client.transcribeAudio(wav, options(provider)); fail("Malformed text accepted") } catch (_: IllegalStateException) {}
                assertEquals(1, calls)
            }
            assertEquals("", CompatibleCloudClient(provider, "fixture-only") { Response(it, "{\"text\":\"\"}") }.transcribeAudio(wav, options(provider)))
            for (status in listOf(401, 403, 404, 413, 429, 500, 302)) {
                var calls = 0
                val client = CompatibleCloudClient(provider, "fixture-only") { calls++; Response(it, "private error", status) }
                try { client.transcribeAudio(wav, options(provider)); fail("HTTP failure accepted") }
                catch (failure: CloudFailure) { assertEquals(status, failure.status); assertFalse(failure.message!!.contains("private error")) }
                assertEquals(1, calls); assertTrue(wav.exists())
            }
        }
    }
    fun testProviderSpecificFileCeilingsAreCheckedBeforeNetwork() {
        val wav = audio()
        for (provider in listOf("openai", "groq", "grok")) {
            java.io.RandomAccessFile(wav, "rw").use { it.setLength(if (provider == "grok") 500_000_001 else 25_000_001) }
            val client = CompatibleCloudClient(provider, "fixture-only") { fail("Oversize upload"); Response(it) }
            try { client.transcribeAudio(wav, options(provider)); fail("Oversize accepted") } catch (_: IllegalArgumentException) {}
        }
        java.io.RandomAccessFile(wav, "rw").use { it.setLength(2_000_000_001) }
        val gemini = GeminiClient("fixture-only") { fail("Oversize Gemini upload"); Response(it) }
        try { gemini.upload(wav); fail("Oversize accepted") } catch (_: IllegalArgumentException) {}
        assertEquals(2_000_000_001L, wav.length()) // Saved recording remains; fixture file is sparse.
    }
    fun testGeminiFileGenerationRequestPreservesEachLanguageAndVocabulary() {
        val file = JSONObject().put("name", "files/fixture").put("uri", DirectCloud.ORIGIN + "/v1beta/files/fixture")
        for (language in listOf("en", "he", "mixed")) {
            val requests = mutableListOf<Response>()
            val client = GeminiClient("fixture-only") { url ->
                Response(url, if (url.path.endsWith(":generateContent"))
                    """{"candidates":[{"finishReason":"STOP","content":{"parts":[{"text":"fixture"}]}}]}"""
                    else """{"state":"ACTIVE"}""").also { requests += it }
            }
            assertEquals("fixture", client.transcribe(file, options("gemini", language)))
            assertEquals(2, requests.size)
            val generated = requests.last()
            assertEquals("https://generativelanguage.googleapis.com/v1beta/models/gemini-3.8-flash:generateContent", generated.url.toString())
            assertEquals("fixture-only", generated.getRequestProperty("x-goog-api-key"))
            val body = JSONObject(generated.sent.toString("UTF-8"))
            val prompt = body.getJSONObject("systemInstruction").getJSONArray("parts").getJSONObject(0).getString("text")
            assertTrue(prompt.contains("do not translate")); assertTrue(prompt.contains("Metrix")); assertTrue(prompt.contains("TrickTic"))
            assertTrue(prompt.contains(when (language) { "he" -> "Hebrew."; "en" -> "English, possibly accented."; else -> "mixed Hebrew and English; preserve both." }))
            val part = body.getJSONArray("contents").getJSONObject(0).getJSONArray("parts").getJSONObject(1).getJSONObject("fileData")
            assertEquals("audio/wav", part.getString("mimeType")); assertEquals(file.getString("uri"), part.getString("fileUri"))
            assertFalse(body.has("language")); assertFalse(body.has("keyterm")); assertFalse(body.toString().contains("fixture-only"))
            val translated = options("gemini", language).put("output_language", JSONObject().put("mode", "translate").put("language", "en"))
            try { client.transcribe(file, translated); fail("Translation accepted") } catch (_: IllegalArgumentException) {}
            assertEquals(3, requests.size) // Existing upload readiness lookup, but no generation/fallback.
        }
    }
    fun testGeminiStrictTextParsingPreservesEmptyAndRejectsBlockedOrTruncatedResults() {
        fun result(parts: String, reason: String = "STOP") = JSONObject("""{"candidates":[{"finishReason":"$reason","content":{"parts":$parts}}]}""")
        assertEquals(" שלום English\n", GeminiClient.text(result("""[{"thought":true,"text":"private reasoning"},{"text":" שלום "},{"text":"English\n"}]""")))
        assertEquals("", GeminiClient.text(result("""[{"text":""}]""")))
        for (document in listOf(result("[]"), result("""[{"text":42}]"""), result("""[{"text":null}]"""), result("""[{"thought":true,"text":"private"}]"""), result("""[{"text":"partial"}]""", "MAX_TOKENS"), result("""[{"text":"blocked"}]""", "SAFETY"), result("""[{"text":"blocked"}]""").put("promptFeedback", JSONObject().put("blockReason", "SAFETY")))) {
            try { GeminiClient.text(document); fail("Malformed or incomplete candidate accepted") } catch (_: IllegalStateException) {}
        }
    }
}
