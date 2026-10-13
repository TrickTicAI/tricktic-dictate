package com.metrix.writer

import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.io.InterruptedIOException
import java.net.HttpURLConnection
import java.net.URL
import java.util.UUID

/** Fixed-origin OpenAI-shaped adapters. No redirect, key in URL, provider error body or transcript logging. */
class CompatibleCloudClient(private val provider: String, private val apiKey: String,
    private val added: Set<String> = emptySet(),
    private val connect: (URL) -> HttpURLConnection = { it.openConnection() as HttpURLConnection }) : CloudClient {
    private val spec = CloudProviders.get(provider).also { require(provider in listOf("openai", "groq", "grok", "openrouter")) }
    private val prefix = when (provider) {
        "groq" -> "/openai/v1"
        "openrouter" -> "/api/v1"
        else -> "/v1"
    }
    override val usesFiles = false
    @Volatile private var cancelled = false
    @Volatile private var active: HttpURLConnection? = null
    override fun cancel() { cancelled = true; active?.disconnect() }
    private fun checkCancelled() { if (cancelled) throw InterruptedIOException("Request interrupted; audio remains saved.") }
    private fun open(path: String, method: String): HttpURLConnection {
        checkCancelled()
        require(path.startsWith("$prefix/") && !path.contains("..") && !path.contains('?') && !path.contains('#'))
        return connect(URL(spec.origin + path)).apply {
            active = this; requestMethod = method; instanceFollowRedirects = false
            connectTimeout = 15000; readTimeout = 120000
            setRequestProperty("Authorization", "Bearer $apiKey"); checkCancelled()
        }
    }
    private fun response(connection: HttpURLConnection): JSONObject {
        checkCancelled()
        val code = connection.responseCode
        if (code !in 200..299) throw CloudFailure(when (code) {
            401, 403 -> "${spec.name} rejected this key or its access. Check the saved key; audio and earlier text remain."
            404 -> "This ${spec.name} model is unavailable to your account. Choose a model and explicitly retry saved audio."
            413 -> "The recording exceeds ${spec.name}'s upload limit. The full audio remains in History."
            429 -> "${spec.name} quota or rate limit reached. Retry later; audio and earlier text remain."
            in 300..399 -> "${spec.name} redirected the request. $PRODUCT_NAME did not forward your key."
            else -> "${spec.name} request failed (HTTP $code). Saved audio and earlier text remain."
        }, code, CloudFailure.retryAfter(connection.getHeaderField("Retry-After")))
        val bytes = connection.inputStream.use { it.readNBytes(8 * 1024 * 1024 + 1) }
        check(bytes.size <= 8 * 1024 * 1024) { "Cloud response exceeded the supported size." }
        val document = JSONObject(bytes.toString(Charsets.UTF_8))
        document.optJSONObject("error")?.let { error ->
            val status = error.optInt("code", 502)
            throw CloudFailure(if (status == 429) "${spec.name} quota or rate limit reached." else "${spec.name} could not complete this request.", status)
        }
        return document
    }
    private fun json(path: String, body: JSONObject? = null): JSONObject {
        val connection = open(path, if (body == null) "GET" else "POST")
        try {
            if (body != null) {
                val bytes = body.toString().toByteArray()
                connection.doOutput = true; connection.setRequestProperty("Content-Type", "application/json")
                connection.setFixedLengthStreamingMode(bytes.size); connection.outputStream.use { it.write(bytes) }
            }
            return response(connection)
        } finally { connection.disconnect(); active = null }
    }
    override fun checkModel(model: String) {
        require((spec.speech + spec.cleanup).any { it.id == model } || model in added) { "Choose a supported model." }
        // OpenRouter serves its catalogue to anyone, so the key itself is checked separately.
        if (provider == "openrouter") json("$prefix/key")
        // Listing works for model identifiers containing a slash and never claims inference success.
        val models = json("$prefix/models").getJSONArray("data")
        check((0 until models.length()).any { models.getJSONObject(it).optString("id") == model }) { "${spec.name} did not list this model for your key." }
    }
    override fun transcribeAudio(audio: File, options: JSONObject): String {
        val selection = options.getJSONObject("selection")
        require(selection.getString("provider") == provider && spec.speech.any { it.id == selection.getString("model") })
        require(options.getJSONObject("output_language").getString("mode") == "preserve")
        val language = options.getString("input_language"); require(language in listOf("en", "he", "mixed"))
        require(provider != "grok" || language == "en") { "Writer offers Grok speech for English. xAI does not publish Hebrew support; choose another engine for Hebrew or mixed speech." }
        // Provider-specific attachment ceilings; capture has no duration cutoff.
        val limit = if (provider == "grok") 500_000_000L else 25_000_000L
        require(audio.length() in 45..limit) { "${spec.name} accepts audio attachments up to ${limit / 1_000_000} MB. Export the full saved recording or choose another engine." }
        val length = audio.length()
        val boundary = "Writer${UUID.randomUUID().toString().replace("-", "")}"
        val hints = options.getJSONObject("vocabulary").getJSONArray("hints")
        val terms = (0 until hints.length()).map { hints.getString(it) }
        val fields = mutableListOf("model" to selection.getString("model"))
        if (provider == "grok") {
            require(terms.size <= 100 && terms.all { it.isNotBlank() && it.codePointCount(0, it.length) <= 50 && !it.contains('\r') && !it.contains('\n') }) { "xAI supports at most 100 single-line keyterms of 50 characters each." }
            fields += "language" to language
            fields += "format" to "true"
            terms.forEach { fields += "keyterm" to it }
        } else {
            fields += "response_format" to "json"
            if (language != "mixed") fields += "language" to language // Mixed detection; never /translations.
            if (terms.isNotEmpty()) fields += "prompt" to terms.joinToString(", ").take(800)
        }
        val header = buildString {
            fields.forEach { (key, value) -> append("--$boundary\r\nContent-Disposition: form-data; name=\"$key\"\r\n\r\n$value\r\n") }
            append("--$boundary\r\nContent-Disposition: form-data; name=\"file\"; filename=\"dictation.wav\"\r\nContent-Type: audio/wav\r\n\r\n")
        }.toByteArray()
        val tail = "\r\n--$boundary--\r\n".toByteArray()
        val connection = open(if (provider == "grok") "$prefix/stt" else "$prefix/audio/transcriptions", "POST")
        try {
            connection.doOutput = true; connection.setRequestProperty("Content-Type", "multipart/form-data; boundary=$boundary")
            connection.setFixedLengthStreamingMode(header.size + length + tail.size)
            connection.outputStream.use { out ->
                out.write(header)
                audio.inputStream().use { input ->
                    val buffer = ByteArray(65536)
                    var remaining = length
                    while (remaining > 0) { checkCancelled(); val count = input.read(buffer, 0, minOf(remaining, buffer.size.toLong()).toInt()); check(count > 0) { "Saved audio changed while uploading." }; out.write(buffer, 0, count); remaining -= count }
                    check(input.read() == -1) { "Saved audio changed while uploading." }
                }
                checkCancelled(); out.write(tail)
            }
            val text = response(connection).opt("text")
            check(text is String) { "The provider returned no transcript. Saved audio remains available." }
            return text
        } finally { connection.disconnect(); active = null }
    }
    override fun cleanup(model: String, text: String, reasoning: String, systemPrompt: String): String {
        require(spec.cleanup.any { it.id == model } || model in added) { "Choose a supported polishing model." }
        require(systemPrompt.isNotBlank() && systemPrompt.toByteArray(Charsets.UTF_8).size <= 32768) { "Polishing instructions are missing or too long." }
        val messages = JSONArray().put(JSONObject().put("role", "system").put("content", systemPrompt))
            .put(JSONObject().put("role", "user").put("content", text))
        val body = JSONObject().put("model", model).put("messages", messages)
        val effort = CustomTextModels.effort(reasoning)
        if (provider == "openrouter") {
            // Routing takes the lowest-latency host that honours every parameter sent. Temperature and
            // max_tokens are omitted deliberately: hosts that reject one would otherwise be excluded.
            body.put("provider", JSONObject().put("sort", "latency").put("require_parameters", true))
            if (effort != null) body.put("reasoning", JSONObject().put("effort", effort))
        } else {
            body.put("temperature", 0)
            if (effort != null) body.put("reasoning_effort", effort)
        }
        val result = json("$prefix/chat/completions", body)
        val choice = result.getJSONArray("choices").getJSONObject(0)
        check(choice.optString("finish_reason") == "stop") { "Polishing was incomplete. The original remains available." }
        val content = choice.getJSONObject("message").opt("content")
        check(content is String && content.isNotBlank()) { "Polishing returned no text. The original remains available." }
        return content
    }
}
