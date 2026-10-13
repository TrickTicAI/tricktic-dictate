package com.metrix.writer

import android.content.Context
import com.metrix.writer.sync.Contract
import com.metrix.writer.sync.PortableChanges
import com.metrix.writer.sync.PortableStore
import org.json.JSONArray
import org.json.JSONObject
import java.util.UUID

/** Phone defaults are copied into requests. The selected engine owns transcription; dictionary processing is phone-local for standalone modes. */
class CompanionPreferences(private val context: Context) {
    private val prefs = context.getSharedPreferences("companion", Context.MODE_PRIVATE)
    /**
     * The dictionary. Each entry carries a stable random `id` and its `origin` device for
     * settings/dictionary sync (MW-OUATH-7). Entries saved before IDs existed get one here, once,
     * and it is stored before anything can queue them; it is never derived from position or text.
     */
    fun document(): JSONObject = synchronized(PortableStore.LOCK) {
        val document = JSONObject(prefs.getString("dictionary", "{\"hints\":[],\"corrections\":[],\"snippets\":[]}")!!)
        if (identify(document)) check(prefs.edit().putString("dictionary", document.toString()).commit())
        document
    }
    /** Adds an ID and origin to entries without one, in place. Returns whether anything changed. */
    private fun identify(document: JSONObject): Boolean {
        var changed = false
        val seen = mutableSetOf<String>()
        for (kind in listOf("hints", "corrections", "snippets")) {
            val entries = document.getJSONArray(kind)
            for (i in 0 until entries.length()) {
                val entry = entries.getJSONObject(i)
                if (!entry.has("id")) { entry.put("id", UUID.randomUUID().toString()); changed = true }
                if (!entry.has("origin")) { entry.put("origin", PortableStore.deviceId(context)); changed = true }
                require(Contract.isUuid(entry.opt("id")) && Contract.isUuid(entry.opt("origin")) && seen.add(entry.getString("id"))) { "Dictionary entry IDs must be unique." }
            }
        }
        return changed
    }
    fun saveDocument(document: JSONObject) = synchronized(PortableStore.LOCK) {
        identify(document)
        for (kind in listOf("hints", "corrections", "snippets")) {
            val entries = document.getJSONArray(kind)
            require(entries.length() <= 200) { "Use at most 200 entries per category." }
            for (i in 0 until entries.length()) {
                val entry = entries.getJSONObject(i)
                for (key in when(kind) { "hints" -> listOf("text"); "corrections" -> listOf("from", "to"); else -> listOf("phrase", "text") }) {
                    val value = entry.getString(key)
                    require(value.isNotBlank() && Contract.isDictionaryText(value)) { "Enter nonblank text up to 8192 bytes." }
                }
            }
        }
        require(document.toString().toByteArray().size <= 262144) { "Dictionary is too large." }
        check(prefs.edit().putString("dictionary", document.toString()).commit())
        PortableChanges.changed(context)
    }
    fun vocabulary(): JSONObject {
        val result = JSONObject(); val document = document()
        for (kind in listOf("hints", "corrections", "snippets")) {
            val values = JSONArray(); val entries = document.getJSONArray(kind)
            for (i in 0 until entries.length()) {
                val entry = entries.getJSONObject(i)
                if (!entry.optBoolean("enabled", true)) continue
                if (kind == "hints") values.put(entry.getString("text"))
                else values.put(JSONObject(entry.toString()).apply { remove("enabled"); remove("id"); remove("origin") })
            }
            result.put(kind, values)
        }
        return result
    }
    fun cleanup(): String = prefs.getString("cleanup", "off")!!
    /** The last polishing model chosen, kept while polishing is off so turning it back on restores it. Phone-local. */
    fun lastCleanup(): String? = prefs.getString("cleanup_last", null)
    /** Super polishes like the computer; Light keeps the previous punctuation-only behaviour. Phone-local. */
    fun polishStyle(): String = prefs.getString("polish_style", "super")!!.takeIf { it == "super" || it == "light" } ?: "super"
    /** The effective polishing instructions: stored overrides, else the computer's Super default. */
    fun polish(): PolishInstructions {
        val instructions = prefs.getString("polish_instructions", null)?.takeIf { PolishPrompts.validate(it) == null }
        val formatting = prefs.getString("polish_formatting", null)?.let {
            runCatching { PolishFormatting.fromJson(JSONObject(it)) }.getOrNull()
        }
        return PolishInstructions(instructions ?: PolishPrompts.SUPER, formatting ?: PolishFormatting())
    }
    /** The system prompt the next polishing request sends. */
    fun polishSystemPrompt(): String = polish().systemPrompt(polishStyle())
    fun isPolishDefault(): Boolean = polishStyle() == "super" && polish() == PolishPrompts.defaults()
    fun savePolish(style: String, instructions: String, formatting: PolishFormatting) {
        require(style == "super" || style == "light") { "Choose a polishing style." }
        PolishPrompts.validate(instructions)?.let { throw IllegalArgumentException(it) }
        check(prefs.edit().putString("polish_style", style).putString("polish_instructions", instructions)
            .putString("polish_formatting", formatting.toJson().toString()).commit())
        PortableChanges.changed(context)
    }
    /** Back to the computer's Super default. Kept edits sync away only through the normal account flow. */
    fun resetPolish() {
        check(prefs.edit().putString("polish_style", "super").remove("polish_instructions").remove("polish_formatting").commit())
        PortableChanges.changed(context)
    }
    fun delivery(): String = prefs.getString("delivery", "raw")!!
    /** Reasoning the selected polishing model is asked for; the default sends no reasoning parameter. */
    fun cleanupReasoning(): String = prefs.getString("cleanup_reasoning", "provider_default")!!
    fun fallbackCleanup(): String = prefs.getString("cleanup_fallback", "off")!!
    fun fallbackAfterMs(): Long = prefs.getLong("cleanup_fallback_after_ms", 1000L)
    fun polishTimeoutMs(): Long = maxOf(prefs.getLong("cleanup_timeout_ms", 3000L),
        if (fallbackCleanup() != "off") fallbackAfterMs() + 2000L else 0L)
    /** Phone-local reliability controls; existing portable cleanup selections remain compatible. */
    fun saveReliability(fallback: String, afterMs: Long = fallbackAfterMs(), timeoutMs: Long = polishTimeoutMs()) {
        require(fallback == "off" || fallback.substringBefore('/') in CloudProviders.all.map { it.id } && fallback.substringAfter('/').isNotBlank())
        require(fallback == "off" || fallback != cleanup()) { "Choose a different fallback model." }
        require(afterMs in 250L..15000L && timeoutMs in 1000L..30000L)
        check(prefs.edit().putString("cleanup_fallback", fallback).putLong("cleanup_fallback_after_ms", afterMs)
            .putLong("cleanup_timeout_ms", timeoutMs).commit())
    }
    /** Freeze both models, protected credential references and instructions when capture starts. */
    fun polishPolicy(selection: JSONObject?, primaryCloud: JSONObject? = null): JSONObject {
        fun cloud(provider: String) = runCatching { DirectCloud(context, provider).snapshot() }
            .getOrElse { JSONObject().put("provider", provider).put("unavailable", true) }
        val backup = fallbackCleanup().takeIf { it != "off" && it != selection?.let { s -> "${s.getString("provider")}/${s.getString("model")}" } }
            ?.let { JSONObject().put("provider", it.substringBefore('/')).put("model", it.substringAfter('/')).put("reasoning", "provider_default") }
        return JSONObject().put("selection", selection?.let { JSONObject(it.toString()) } ?: JSONObject.NULL)
            .put("cloud", primaryCloud ?: selection?.let { cloud(it.getString("provider")) } ?: JSONObject())
            .put("fallback", backup ?: JSONObject.NULL).put("fallback_cloud", backup?.let { cloud(it.getString("provider")) } ?: JSONObject.NULL)
            .put("fallback_after_ms", fallbackAfterMs()).put("timeout_ms", polishTimeoutMs()).put("system_prompt", polishSystemPrompt())
            .put("spoken_punctuation", polishStyle() == "super" && polish().formatting.spokenPunctuation)
            .put("remove_fillers", polishStyle() == "super" && polish().formatting.removeFillers)
    }
    fun save(cleanup: String, delivery: String, reasoning: String = cleanupReasoning()) {
        require(delivery in listOf("raw", "dictionary", "cleaned"))
        require(reasoning in CustomTextModels.REASONING) { "Choose a supported reasoning level." }
        require(cleanup == "off" || cleanup.substringBefore('/') in CloudProviders.all.map { it.id } && cleanup.substringAfter('/').isNotBlank())
        val edit = prefs.edit().putString("cleanup", cleanup).putString("delivery", delivery).putString("cleanup_reasoning", reasoning)
        if (cleanup != "off") edit.putString("cleanup_last", cleanup)
        check(edit.commit())
        PortableChanges.changed(context)
    }
    fun options(provider: String, model: String, language: String): JSONObject {
        val cleanup = cleanup()
        return JSONObject().put("selection", JSONObject().put("provider", provider).put("model", model))
            .put("input_language", language).put("output_language", JSONObject().put("mode", "preserve"))
            .put("vocabulary", vocabulary()).put("cleanup", if (cleanup == "off") JSONObject().put("mode", "off")
                else JSONObject().put("mode", "enabled").put("selection", JSONObject().put("provider", cleanup.substringBefore('/'))
                    .put("model", cleanup.substringAfter('/')).put("reasoning", cleanupReasoning())))
    }
}
