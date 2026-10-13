package com.metrix.writer

import android.content.Context
import org.json.JSONArray
import org.json.JSONObject

/**
 * Text models the user adds by ID for a cloud provider.
 *
 * Writer cannot know what an arbitrary model accepts, so the provider stays the
 * judge: a model or reasoning level it rejects fails that polishing request and
 * the saved transcript is kept, exactly as for the built-in models.
 */
data class CustomTextModel(val provider: String, val model: String)

class CustomTextModels(context: Context) {
    private val prefs = context.getSharedPreferences("custom-text-models", Context.MODE_PRIVATE)

    fun list(): List<CustomTextModel> {
        val raw = prefs.getString("models", null) ?: return emptyList()
        val parsed = runCatching {
            val array = JSONArray(raw)
            (0 until array.length()).map {
                val entry = array.getJSONObject(it)
                CustomTextModel(entry.getString("provider"), entry.getString("model"))
            }
        }.getOrElse { return emptyList() }
        // An unreadable or now-invalid list reads as empty rather than blocking polishing.
        return if (validate(parsed) == null) parsed else emptyList()
    }

    fun forProvider(provider: String) = list().filter { it.provider == provider }

    fun add(provider: String, model: String) {
        val entry = CustomTextModel(provider, model.trim())
        val updated = list() + entry
        validate(updated)?.let { throw IllegalArgumentException(it) }
        save(updated)
    }

    fun remove(provider: String, model: String) = save(list().filterNot { it.provider == provider && it.model == model })

    private fun save(models: List<CustomTextModel>) {
        val array = JSONArray()
        for (entry in models) array.put(JSONObject().put("provider", entry.provider).put("model", entry.model))
        check(prefs.edit().putString("models", array.toString()).commit())
    }

    companion object {
        const val LIMIT = 50
        private const val MODEL_BYTES = 128

        /** Reasoning levels every supported provider spells the same way, plus the default of sending nothing. */
        val REASONING = listOf("provider_default", "off", "minimal", "low", "medium", "high")

        fun reasoningLabel(level: String) = when (level) {
            "provider_default" -> "Provider default"
            "off" -> "Off"
            else -> level.replaceFirstChar { it.uppercase() }
        }

        /** The effort string OpenAI, xAI and OpenRouter share; Gemini is mapped separately. */
        fun effort(level: String): String? = when (level) {
            "provider_default" -> null
            "off" -> "none"
            in REASONING -> level
            else -> null
        }

        /**
         * The ID goes into request paths and bodies, so it allows only the characters
         * provider model IDs use. Only OpenRouter IDs are publisher-qualified.
         */
        fun wellFormed(provider: String, model: String): Boolean {
            val routed = provider == "openrouter"
            if (model.isEmpty() || model.toByteArray(Charsets.UTF_8).size > MODEL_BYTES) return false
            val first = model.first()
            if (!(first.isAsciiAlphanumeric() || routed && first == '~')) return false
            if (!model.all { it.isAsciiAlphanumeric() || it in "._:-" || routed && it in "/~" }) return false
            if (model.contains("..")) return false
            return !routed || model.contains('/') && model.split('/').none { it.isEmpty() }
        }

        fun malformedMessage(provider: String) = if (provider == "openrouter")
            "Enter the OpenRouter model ID as publisher/model, for example mistralai/mistral-medium-3-5."
        else "Enter the model ID exactly as the provider's API names it, using letters, digits, '.', '_', ':' and '-'."

        /** Returns the first problem, or null when the whole list is usable. */
        fun validate(models: List<CustomTextModel>): String? {
            if (models.size > LIMIT) return "At most $LIMIT custom text models are supported."
            models.forEachIndexed { index, entry ->
                val provider = CloudProviders.all.find { it.id == entry.provider }
                    ?: return "That provider is not supported on this phone."
                if (!provider.text) return "${provider.name} does not run text models."
                if (!wellFormed(entry.provider, entry.model)) return malformedMessage(entry.provider)
                if (provider.cleanup.any { it.id == entry.model }) return "That model is already in $PRODUCT_NAME's list."
                if (models.take(index).any { it.provider == entry.provider && it.model == entry.model })
                    return "That custom model is already added."
            }
            return null
        }

        private fun Char.isAsciiAlphanumeric() = code < 128 && isLetterOrDigit()
    }
}
