package com.metrix.writer.sync

import android.content.Context
import com.metrix.writer.CloudProviders
import com.metrix.writer.CompanionPreferences
import com.metrix.writer.DictationChoice
import com.metrix.writer.DirectCloud
import com.metrix.writer.PhoneModels
import com.metrix.writer.PolishPrompts
import com.metrix.writer.ui.AppearancePreferences
import org.json.JSONArray
import org.json.JSONObject

/** Result of applying one portable setting on this phone. [reason] is set when it was kept but not applied. */
data class SettingOutcome(val applied: Boolean, val reason: String? = null)

/**
 * Maps v1 portable settings and dictionary entries onto this phone's existing stores, and only
 * those. Provider keys, bubble/device settings, permissions, phone model files and queued
 * dictation work have no mapping. A value this phone cannot use (a model that isn't installed, a
 * provider without a saved key, a mode Android doesn't have) is reported as not applied; it never
 * switches to another provider, downloads a model or starts any provider work.
 */
class PortableAdapter(context: Context) {
    private val context = context.applicationContext

    /** Stores whose values the adapter reads or writes, for rollback snapshots and the import journal. */
    private val nativeKeys: Map<String, List<String>> = mapOf(
        "speech" to listOf("language"),
        "phone-engine" to listOf("enabled", "model", "language"),
        "direct-cloud" to listOf("provider", "language", "model") + CloudProviders.all.filter { it.id != "gemini" }.map { "${it.id}-model" },
        "companion" to listOf("cleanup", "delivery", "cleanup_reasoning", "dictionary", "polish_style", "polish_instructions", "polish_formatting"),
        "writer-appearance" to listOf("theme"),
    )

    /** The exact stored values the adapter touches; absent keys are recorded as absent. */
    fun snapshotNative(): JSONObject = JSONObject().apply {
        for ((file, keys) in nativeKeys) {
            val prefs = context.getSharedPreferences(file, Context.MODE_PRIVATE)
            put(file, JSONObject().apply {
                for (key in keys) when (val value = prefs.all[key]) {
                    null -> put(key, JSONObject.NULL)
                    is Boolean -> put(key, JSONObject().put("b", value))
                    is String -> put(key, JSONObject().put("s", value))
                    else -> error("unexpected preference type")
                }
            })
        }
    }

    fun restoreNative(snapshot: JSONObject) {
        for ((file, keys) in nativeKeys) {
            val values = snapshot.getJSONObject(file)
            val edit = context.getSharedPreferences(file, Context.MODE_PRIVATE).edit()
            for (key in keys) {
                val value = values.opt(key)
                when {
                    value !is JSONObject -> edit.remove(key)
                    value.has("b") -> edit.putBoolean(key, value.getBoolean("b"))
                    else -> edit.putString(key, value.getString("s"))
                }
            }
            check(edit.commit()) { "Could not save settings" }
        }
    }

    /** This phone's own value for a setting, or null when Android has no such setting or the value has no v1 form. */
    fun nativeSetting(key: String): Any? = when (key) {
        "input_language" -> DictationChoice(context).storedLanguage()
        "output_language" -> JSONObject().put("mode", "preserve") // Android always keeps the spoken language.
        "transcription_model" -> DictationChoice(context).let { choice ->
            if (choice.isPhone()) model("local", choice.model())
            else DirectCloud.selectedProvider(context).takeIf { it in Contract.PROVIDERS }?.let { model(it, DirectCloud(context, it).model()) }
        }
        "cleanup" -> CompanionPreferences(context).let { prefs ->
            val choice = prefs.cleanup()
            if (choice == "off") JSONObject().put("mode", "off")
            else choice.substringBefore('/').takeIf { it in Contract.PROVIDERS }?.let { provider ->
                model(provider, choice.substringAfter('/'))?.let { JSONObject().put("mode", "enabled").put("selection", it).put("reasoning", prefs.cleanupReasoning()) }
            }
        }
        "delivery" -> CompanionPreferences(context).delivery()
        "theme" -> AppearancePreferences(context).theme.takeIf { it in setOf("system", "light", "dark") }
        "polish_instructions" -> CompanionPreferences(context).polish().let {
            JSONObject().put("instructions", it.instructions).put("formatting", it.formatting.toJson())
        }
        else -> null // ui_language, dictation_mode: Android has no such setting.
    }?.takeIf { Contract.isSettingValue(key, it) }

    private fun model(provider: String, model: String) = JSONObject().put("provider", provider).put("model", model).takeIf { Contract.isModel(it) }

    /** Why [value] can't be used on this phone right now, or null when [apply] would use it. Reads only. */
    fun unsupported(key: String, value: Any): String? = when (key) {
        "ui_language" -> "This phone's interface is in English."
        "dictation_mode" -> "Android has no Super or Command mode."
        "output_language" -> if ((value as JSONObject).getString("mode") == "preserve") null else "Android keeps the spoken language and doesn't translate."
        "input_language" -> DictationChoice(context).let { choice ->
            if (choice.isPhone() && value !in PhoneModels(context).languages(choice.model())) "The selected phone model doesn't support this language." else null
        }
        "transcription_model" -> engineProblem(value as JSONObject)
        "cleanup" -> (value as JSONObject).optJSONObject("selection")?.let { selection ->
            val provider = selection.getString("provider")
            val name = CloudProviders.all.find { it.id == provider }?.name
            when {
                name == null -> "Polishing with this provider isn't available on Android."
                !CloudProviders.cleanupAllowed(context, provider, selection.getString("model")) -> "$name ${selection.getString("model")} isn't a polishing model on this phone. Add it under Wording & polishing."
                !DirectCloud(context, provider).configured() -> "Add your $name key in Settings › API keys to use it."
                else -> null
            }
        }
        else -> null
    }

    private fun engineProblem(value: JSONObject): String? {
        val provider = value.getString("provider")
        val id = value.getString("model")
        if (provider == "local") {
            val models = PhoneModels(context)
            if (models.models().none { it.getString("id") == id }) return "$id isn't a phone model on Android."
            if (!models.file(id).isFile) return "Download $id in Phone models to use it."
            if (DictationChoice(context).storedLanguage() !in models.languages(id)) return "$id doesn't support the current speech language."
            return null
        }
        val spec = CloudProviders.all.find { it.id == provider } ?: return "This provider isn't available on Android."
        if (!spec.transcribes) return "${spec.name} can't transcribe speech."
        if (spec.speech.none { it.id == id }) return "${spec.name} $id isn't offered for dictation on Android."
        if (!DirectCloud(context, provider).configured()) return "Add your ${spec.name} key in Settings › API keys to use it."
        return null
    }

    /**
     * Applies a validated setting if this phone can use it now; otherwise changes nothing and
     * returns the reason. Callers store unapplied values in the shadow store.
     */
    fun apply(key: String, value: Any): SettingOutcome {
        unsupported(key, value)?.let { return SettingOutcome(false, it) }
        when (key) {
            "output_language" -> Unit
            "input_language" -> DictationChoice(context).let { choice ->
                if (choice.isPhone()) choice.save(true, choice.model(), value as String) else choice.saveLanguage(value as String)
            }
            "transcription_model" -> (value as JSONObject).let { selection ->
                val provider = selection.getString("provider")
                if (provider == "local") DictationChoice(context).usePhone(selection.getString("model"))
                else {
                    DirectCloud(context, provider).saveSelection(selection.getString("model"), DictationChoice(context).language())
                    DictationChoice(context).useCloud(provider)
                }
            }
            "cleanup" -> (value as JSONObject).let { cleanup ->
                val prefs = CompanionPreferences(context)
                val selection = cleanup.optJSONObject("selection")
                if (selection == null) prefs.save("off", prefs.delivery())
                else prefs.save("${selection.getString("provider")}/${selection.getString("model")}", prefs.delivery(), cleanup.getString("reasoning"))
            }
            "delivery" -> CompanionPreferences(context).let { it.save(it.cleanup(), value as String) }
            "theme" -> AppearancePreferences(context).theme = value as String
            "polish_instructions" -> (PolishPrompts.fromJson(value) ?: return SettingOutcome(false, "This phone couldn't read these polishing instructions.")).let { polish ->
                CompanionPreferences(context).let { prefs -> prefs.savePolish(prefs.polishStyle(), polish.instructions, polish.formatting) }
            }
            else -> return SettingOutcome(false, "Not a setting on this phone.")
        }
        return SettingOutcome(true)
    }

    /** Dictionary entries as portable data keyed by ID, with their origin devices. */
    fun dictionary(): Map<String, Pair<String, JSONObject>> {
        val document = CompanionPreferences(context).document()
        val result = LinkedHashMap<String, Pair<String, JSONObject>>()
        for ((kind, array) in KINDS) {
            val rows = document.getJSONArray(array)
            for (i in 0 until rows.length()) {
                val row = rows.getJSONObject(i)
                val data = JSONObject().put("kind", kind).put("enabled", row.optBoolean("enabled", true))
                for (field in FIELDS.getValue(kind)) data.put(field, row.getString(field))
                result[row.getString("id")] = row.getString("origin") to data
            }
        }
        return result
    }

    /** Replaces the dictionary with [entries], in UUID order as the contract requires after import. */
    fun saveDictionary(entries: Map<String, Pair<String, JSONObject>>) {
        val document = JSONObject()
        for ((kind, array) in KINDS) document.put(array, JSONArray(entries.entries.filter { it.value.second.getString("kind") == kind }.sortedBy { it.key }.map { (id, entry) ->
            val (origin, data) = entry
            JSONObject().put("id", id).put("origin", origin).put("enabled", data.getBoolean("enabled")).apply { for (field in FIELDS.getValue(kind)) put(field, data.getString(field)) }
        }))
        CompanionPreferences(context).saveDocument(document)
    }

    companion object {
        val KINDS = listOf("hint" to "hints", "correction" to "corrections", "snippet" to "snippets")
        val FIELDS = mapOf("hint" to listOf("text"), "correction" to listOf("from", "to"), "snippet" to listOf("phrase", "text"))
    }
}
