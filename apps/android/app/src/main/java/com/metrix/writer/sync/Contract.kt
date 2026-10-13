package com.metrix.writer.sync

import org.json.JSONArray
import org.json.JSONObject

/**
 * Portable data and sync contract v1 (docs/testing/account-sync/README.md), checked in Kotlin
 * against the same closed allowlist as packages/contracts/account-sync.schema.mjs. Every
 * function returns false or throws [ContractException] instead of stripping unknown input.
 */
object Contract {
    const val SCHEMA_VERSION = 1
    const val FORMAT = "metrix-writer-portable"
    const val MAX_FILE_BYTES = 512 * 1024
    const val MAX_DICTIONARY_BYTES = 256 * 1024
    const val MAX_ENTRIES_PER_KIND = 200
    const val MAX_TEXT_BYTES = 8192

    val SETTING_KEYS = listOf("ui_language", "input_language", "output_language", "transcription_model",
        "dictation_mode", "cleanup", "delivery", "theme", "polish_instructions")
    val SETTING_IDS: Map<String, String> = SETTING_KEYS.withIndex().associate { (index, key) -> key to "00000000-0000-4000-8000-%012d".format(index + 1) }
    val SETTING_KEY_BY_ID: Map<String, String> = SETTING_IDS.entries.associate { (key, id) -> id to key }
    val PROVIDERS = setOf("openai", "gemini", "grok", "openrouter", "local")
    val REASONING = setOf("provider_default", "off", "minimal", "low", "medium", "high")
    val DICTIONARY_KINDS = listOf("hint", "correction", "snippet")
    private val UUID = Regex("^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$")
    private val MODEL = Regex("^[A-Za-z0-9][A-Za-z0-9._:/~-]*$")
    // JavaScript trim() whitespace, as the rules and the reference oracle use it.
    private val BLANK = Regex("^[\\t\\n\\u000B\\f\\r \\u00A0\\u1680\\u2000-\\u200A\\u2028\\u2029\\u202F\\u205F\\u3000\\uFEFF]*$")

    fun isUuid(value: Any?) = value is String && UUID.matches(value)
    fun isUint(value: Any?) = (value is Long || value is Int) && (value as Number).toLong() in 0..StrictJson.MAX_SAFE
    fun isPositive(value: Any?) = isUint(value) && (value as Number).toLong() >= 1
    private fun JSONObject.exactly(vararg keys: String) = length() == keys.size && keys.all { has(it) }
    private fun codePoints(text: String) = text.codePointCount(0, text.length)

    fun isText(value: Any?, maxCodePoints: Int, minCodePoints: Int = 0) =
        value is String && StrictJson.wellFormed(value) && value.indexOf('\u0000') < 0 && codePoints(value) in minCodePoints..maxCodePoints

    fun isDictionaryText(value: Any?) = isText(value, 8192, 1) && !BLANK.matches(value as String) &&
        value.toByteArray(Charsets.UTF_8).size <= MAX_TEXT_BYTES

    fun isModel(value: Any?) = value is JSONObject && value.exactly("provider", "model") &&
        value.opt("provider") in PROVIDERS && (value.opt("model") as? String)?.let { model ->
            model.length in 1..256 && MODEL.matches(model) && !model.contains("..") && model.split('/').none { it.isEmpty() }
        } == true

    fun isOutputLanguage(value: Any?) = value is JSONObject && (
        (value.exactly("mode") && value.opt("mode") == "preserve") ||
            (value.exactly("mode", "language") && value.opt("mode") == "translate" && value.opt("language") in setOf("en", "he")))

    fun isCleanup(value: Any?) = value is JSONObject && (
        (value.exactly("mode") && value.opt("mode") == "off") ||
            (value.exactly("mode", "selection", "reasoning") && value.opt("mode") == "enabled" && isModel(value.opt("selection")) &&
                value.opt("reasoning") in REASONING))

    val REWRITE = setOf("light", "polished", "concise")
    val LAYOUT = setOf("automatic", "single_line")
    const val MAX_POLISH_BYTES = 16384

    /**
     * Super polishing instructions plus the desktop Modes writing preferences
     * (MW-SETTINGS-8). The byte limit and non-blank rule match the desktop
     * profile validation; the blank check uses this file's trim definition and
     * any corner the desktop rejects stays preserved, never applied.
     */
    fun isPolishFormatting(value: Any?) = value is JSONObject &&
        value.exactly("rewrite", "layout", "lists", "remove_fillers", "self_corrections", "spoken_punctuation") &&
        value.opt("rewrite") in REWRITE && value.opt("layout") in LAYOUT &&
        value.opt("lists") is Boolean && value.opt("remove_fillers") is Boolean &&
        value.opt("self_corrections") is Boolean && value.opt("spoken_punctuation") is Boolean

    fun isPolishInstructions(value: Any?) = value is JSONObject && value.exactly("instructions", "formatting") &&
        (value.opt("instructions") as? String)?.let { text ->
            StrictJson.wellFormed(text) && text.indexOf('\u0000') < 0 && !BLANK.matches(text) &&
                text.toByteArray(Charsets.UTF_8).size in 1..MAX_POLISH_BYTES
        } == true && isPolishFormatting(value.opt("formatting"))

    fun isSettingValue(key: String, value: Any?): Boolean = when (key) {
        "ui_language" -> value in setOf("en", "he")
        "input_language" -> value in setOf("en", "he", "mixed")
        "output_language" -> isOutputLanguage(value)
        "transcription_model" -> isModel(value)
        "dictation_mode" -> value in setOf("regular", "super", "command")
        "cleanup" -> isCleanup(value)
        "delivery" -> value in setOf("raw", "dictionary", "cleaned")
        "theme" -> value in setOf("system", "light", "dark")
        "polish_instructions" -> isPolishInstructions(value)
        else -> false
    }

    /** PortableSetting for a fixed ID. */
    fun isSetting(id: String, data: Any?) = data is JSONObject && data.exactly("kind", "key", "value") && data.opt("kind") == "setting" &&
        (data.opt("key") as? String)?.let { key -> SETTING_IDS[key] == id && isSettingValue(key, data.opt("value")) } == true

    /** PortableDictionaryEntry; its ID must not be a reserved setting ID. */
    fun isDictionaryEntry(id: String, data: Any?): Boolean {
        if (data !is JSONObject || id in SETTING_KEY_BY_ID || data.opt("enabled") !is Boolean) return false
        return when (data.opt("kind") as? String) {
            "hint" -> data.exactly("kind", "enabled", "text") && isDictionaryText(data.opt("text"))
            "correction" -> data.exactly("kind", "enabled", "from", "to") && isDictionaryText(data.opt("from")) && isDictionaryText(data.opt("to"))
            "snippet" -> data.exactly("kind", "enabled", "phrase", "text") && isDictionaryText(data.opt("phrase")) && isDictionaryText(data.opt("text"))
            else -> false
        }
    }

    fun isTranscript(id: String, data: Any?): Boolean {
        if (data !is JSONObject || !isUuid(id) || id in SETTING_KEY_BY_ID || !isText(data.opt("text"), 131072)) return false
        return when (data.opt("kind")) {
            "original" -> data.exactly("kind", "attempt_id", "recorded_at_ms", "input_language", "output_language", "selection", "text") &&
                isUuid(data.opt("attempt_id")) && isUint(data.opt("recorded_at_ms")) && data.opt("input_language") in setOf("en", "he", "mixed") &&
                isOutputLanguage(data.opt("output_language")) && isModel(data.opt("selection"))
            "corrected", "cleaned", "edited" -> data.exactly("kind", "original_record_id", "parent_record_id", "created_at_ms", "text") &&
                isUuid(data.opt("original_record_id")) && isUuid(data.opt("parent_record_id")) && isUint(data.opt("created_at_ms"))
            else -> false
        }
    }

    fun isData(category: String, id: String, data: Any?) = when (category) {
        "settings" -> isSetting(id, data)
        "dictionary" -> isDictionaryEntry(id, data)
        "transcripts" -> isTranscript(id, data)
        else -> false
    }

    fun isContent(category: String, id: String, content: Any?) = content is JSONObject && (
        (content.exactly("state") && content.opt("state") == "deleted") ||
            (content.exactly("state", "data") && content.opt("state") == "live" && isData(category, id, content.opt("data"))))

    /** RemoteRecord for settings or dictionary. A newer schema version is [ContractException] "unsupported_version". */
    fun checkRecord(record: Any?) {
        if (record !is JSONObject) throw ContractException("invalid")
        if (record.opt("schema_version") != null && !StrictJson.equal(record.opt("schema_version"), 1L)) throw ContractException("unsupported_version")
        val id = record.opt("id") as? String
        val category = record.opt("category") as? String
        if (!record.exactly("schema_version", "id", "category", "origin_device_id", "revision", "content") || id == null || !isUuid(id) ||
            category !in setOf("settings", "dictionary", "transcripts") || !isUuid(record.opt("origin_device_id")) || !isPositive(record.opt("revision")) ||
            !isContent(category!!, id, record.opt("content"))) throw ContractException("invalid")
    }

    private fun checkExportRecord(record: Any?, category: String) {
        if (record !is JSONObject) throw ContractException("invalid")
        if (!StrictJson.equal(record.opt("schema_version"), 1L)) {
            throw ContractException(if (record.opt("schema_version") is Number) "unsupported_version" else "invalid")
        }
        val id = record.opt("id") as? String
        if (!record.exactly("schema_version", "id", "origin_device_id", "category", "data") || id == null || !isUuid(id) ||
            record.opt("category") != category || !isUuid(record.opt("origin_device_id")) || !isData(category, id, record.opt("data"))) {
            throw ContractException("invalid")
        }
    }

    /**
     * Parses a whole manual export. Size, version, closed fields, duplicate IDs, per-kind
     * capacity and the dictionary byte budget are all checked before anything is applied.
     */
    fun parseBackup(bytes: ByteArray): JSONObject {
        if (bytes.size > MAX_FILE_BYTES) throw ContractException("too_large")
        val backup = StrictJson.parseObject(bytes)
        if (backup.opt("format") != FORMAT) throw ContractException("invalid")
        if (!StrictJson.equal(backup.opt("format_version"), 1L)) {
            throw ContractException(if (backup.opt("format_version") is Number) "unsupported_version" else "invalid")
        }
        if (!backup.exactly("format", "format_version", "export_id", "created_at_ms", "settings", "dictionary") ||
            !isUuid(backup.opt("export_id")) || !isUint(backup.opt("created_at_ms"))) throw ContractException("invalid")
        val settings = backup.opt("settings") as? JSONArray ?: throw ContractException("invalid")
        val dictionary = backup.opt("dictionary") as? JSONArray ?: throw ContractException("invalid")
        if (settings.length() > SETTING_KEYS.size || dictionary.length() > 600) throw ContractException("too_large")
        for (i in 0 until settings.length()) checkExportRecord(settings.opt(i), "settings")
        for (i in 0 until dictionary.length()) checkExportRecord(dictionary.opt(i), "dictionary")
        val ids = (0 until settings.length()).map { settings.getJSONObject(it).getString("id") } +
            (0 until dictionary.length()).map { dictionary.getJSONObject(it).getString("id") }
        if (ids.toSet().size != ids.size) throw ContractException("duplicate_id")
        if (dictionary.toString().toByteArray(Charsets.UTF_8).size > MAX_DICTIONARY_BYTES) throw ContractException("too_large")
        val kinds = (0 until dictionary.length()).map { dictionary.getJSONObject(it).getJSONObject("data").getString("kind") }
        if (DICTIONARY_KINDS.any { kind -> kinds.count { it == kind } > MAX_ENTRIES_PER_KIND }) throw ContractException("too_large")
        return backup
    }
}
