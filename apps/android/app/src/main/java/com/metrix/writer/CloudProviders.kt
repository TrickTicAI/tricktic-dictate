package com.metrix.writer

import android.content.Context
import org.json.JSONObject
import java.io.File

data class CloudModel(val id: String, val name: String)
data class CloudProvider(
    val id: String,
    val name: String,
    val origin: String,
    val speech: List<CloudModel>,
    val cleanup: List<CloudModel>,
    /** Stated capability limits, shown in Settings rather than discovered at request time. */
    val note: String = "",
    /** Where the owner creates a key, opened from the key page. */
    val keyUrl: String = "",
) {
    /** A provider with no speech model can hold a key and polish text, but is never a dictation engine. */
    val transcribes get() = speech.isNotEmpty()
    val text get() = cleanup.isNotEmpty()
}

/** Curated capabilities, checked against primary API docs; access is checked separately per account. */
object CloudProviders {
    val all = listOf(
        CloudProvider("gemini", "Gemini", DirectCloud.ORIGIN,
            listOf(CloudModel("gemini-3.8-flash", "Gemini 3.8 Flash")),
            listOf(CloudModel("gemini-3.8-flash", "Gemini 3.8 Flash")),
            keyUrl = "https://aistudio.google.com/apikey"),
        CloudProvider("openai", "OpenAI", "https://api.openai.com",
            listOf(CloudModel("gpt-4o-mini-transcribe", "GPT-4o mini Transcribe"), CloudModel("gpt-4o-transcribe", "GPT-4o Transcribe"), CloudModel("whisper-1", "Whisper")),
            listOf(CloudModel("gpt-4.1-mini", "GPT-4.1 mini"), CloudModel("gpt-5.6-luna", "GPT-5.6 Luna")),
            keyUrl = "https://platform.openai.com/api-keys"),
        CloudProvider("groq", "Groq", "https://api.groq.com",
            listOf(CloudModel("whisper-large-v3-turbo", "Whisper Large v3 Turbo"), CloudModel("whisper-large-v3", "Whisper Large v3")),
            listOf(CloudModel("openai/gpt-oss-20b", "GPT OSS 20B"), CloudModel("openai/gpt-oss-120b", "GPT OSS 120B")),
            keyUrl = "https://console.groq.com/keys"),
        CloudProvider("grok", "Grok", "https://api.x.ai",
            listOf(CloudModel("grok-voice-transcribe-2.0", "Grok Voice Transcribe 2.0")),
            listOf(CloudModel("grok-4.6", "Grok 4.6"), CloudModel("grok-4.3", "Grok 4.3")),
            "Writer offers Grok speech for English; xAI does not publish Hebrew support, so Hebrew and mixed speech use another engine.",
            "https://console.x.ai"),
        CloudProvider("openrouter", "OpenRouter", "https://openrouter.ai",
            emptyList(),
            listOf(CloudModel("anthropic/claude-haiku-4.5", "Claude Haiku 4.5"), CloudModel("openai/gpt-5.6-luna", "GPT-5.6 Luna"),
                CloudModel("x-ai/grok-4.3", "Grok 4.3"), CloudModel("mistralai/mistral-medium-3-5", "Mistral Medium 3.5"),
                CloudModel("inception/mercury-2.5", "Mercury 2.5")),
            "OpenRouter routes text models only. It cannot transcribe speech, so it is not offered as a dictation engine.",
            "https://openrouter.ai/settings/keys"),
    )
    fun get(id: String) = requireNotNull(all.find { it.id == id }) { "This cloud provider is not supported. Saved requests remain available." }

    /** Speech engines only: a text-only provider never appears in the dictation engine list. */
    val transcribers get() = all.filter { it.transcribes }

    /** Built-in polishing models plus the ones this phone's owner added by ID. */
    fun cleanupModels(context: Context, provider: String): List<CloudModel> =
        get(provider).cleanup + CustomTextModels(context).forProvider(provider).map { CloudModel(it.model, it.model) }

    fun cleanupAllowed(context: Context, provider: String, model: String) = cleanupModels(context, provider).any { it.id == model }

    /** What uses this provider's key right now, in the words the key page shows. */
    fun uses(context: Context, provider: String): List<String> {
        val cleanup = CompanionPreferences(context).cleanup()
        return listOfNotNull(
            if (!DictationChoice(context).isPhone() && DirectCloud.selectedProvider(context) == provider) "Speech" else null,
            if (cleanup != "off" && cleanup.substringBefore('/') == provider) "Polishing" else null,
        )
    }

    /** The model a key check asks about: the one in use, else this provider's first. */
    fun checkModel(context: Context, provider: String): String {
        val spec = get(provider); val cleanup = CompanionPreferences(context).cleanup()
        val usesSpeech = !DictationChoice(context).isPhone() && DirectCloud.selectedProvider(context) == provider
        return when {
            usesSpeech -> DirectCloud(context, provider).model()
            cleanup != "off" && cleanup.substringBefore('/') == provider -> cleanup.substringAfter('/')
            spec.transcribes -> DirectCloud(context, provider).model()
            else -> spec.cleanup.first().id
        }
    }

    fun modelName(provider: String, model: String) = (get(provider).speech + get(provider).cleanup).find { it.id == model }?.name ?: model
    fun client(context: Context, snapshot: JSONObject): CloudClient {
        val provider = snapshot.getString("provider")
        val key = DirectCloud(context, provider).key(snapshot)
        val added = CustomTextModels(context).forProvider(provider).map { it.model }.toSet()
        return if (provider == "gemini") GeminiClient(key, added = added) else CompatibleCloudClient(provider, key, added = added)
    }
}

interface CloudClient {
    val usesFiles: Boolean get() = true
    /** Request-specific routing; existing/mock file clients keep their previous behavior. */
    fun usesFilesFor(audio: File, options: JSONObject): Boolean = usesFiles
    fun cancel()
    fun checkModel(model: String)
    fun upload(audio: File): JSONObject = error("This provider uploads audio with the transcription request.")
    fun transcribe(file: JSONObject, options: JSONObject): String = error("This provider does not use persistent file uploads.")
    fun transcribeAudio(audio: File, options: JSONObject): String = error("This provider requires a file upload.")
    /**
     * `reasoning` is one of CustomTextModels.REASONING; the default sends no reasoning parameter at all.
     * `systemPrompt` is the full instruction text, built from the owner's polishing settings.
     */
    fun cleanup(model: String, text: String, reasoning: String = "provider_default", systemPrompt: String): String
    fun delete(file: JSONObject) {}
}
