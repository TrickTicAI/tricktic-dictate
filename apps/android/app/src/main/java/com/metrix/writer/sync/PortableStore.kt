package com.metrix.writer.sync

import android.content.Context
import android.util.AtomicFile
import com.metrix.writer.CloudProviders
import com.metrix.writer.CompanionPreferences
import com.metrix.writer.CompanionUi
import com.metrix.writer.PolishPrompts
import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.io.FileNotFoundException
import java.util.UUID
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.Executors

/** One portable record as this phone holds it. */
data class LocalRecord(val id: String, val category: String, val origin: String, val data: JSONObject)

/** One row of an import or restore preview. [action] is apply, unchanged, preserve, conflict or remove. */
data class PreviewItem(val id: String, val category: String, val action: String, val title: String, val detail: String, val reason: String? = null)

/** A persisted preview: the ImportPreview wire document plus what the user is shown. */
data class ImportPlan(val preview: JSONObject, val items: List<PreviewItem>) {
    val importId: String get() = preview.getString("import_id")
    val conflicts get() = items.filter { it.action == "conflict" }
}

/**
 * The phone's portable settings and dictionary (MW-OUATH-1 export/import, MW-OUATH-7). Existing
 * stores stay authoritative; this adds stable IDs, a local revision, a shadow store for recognized
 * values Android can't use, a crash-safe import journal and one rollback snapshot. Everything is in
 * app-private storage, which backup and device transfer already exclude.
 */
class PortableStore(context: Context) {
    private val context = context.applicationContext
    private val adapter = PortableAdapter(this.context)
    private val directory = File(this.context.filesDir, "portable-sync").apply { mkdirs() }
    private val stateFile = AtomicFile(File(directory, "state.json"))
    private val journalFile = AtomicFile(File(directory, "journal.json"))

    val deviceId: String get() = deviceId(context)

    private fun load(): JSONObject = try { JSONObject(String(stateFile.readFully(), Charsets.UTF_8)) } catch (_: FileNotFoundException) {
        JSONObject().put("local_revision", 0L).put("shadow", JSONObject()).put("origins", JSONObject()).put("observed", JSONObject())
            .put("receipts", JSONObject()).put("last_journal", "")
    }
    private fun save(state: JSONObject) = write(stateFile, state)

    /** Finishes or undoes an interrupted import or restore. Call before reading portable state. */
    fun recover() = synchronized(LOCK) {
        val journal = try { JSONObject(String(journalFile.readFully(), Charsets.UTF_8)) } catch (_: FileNotFoundException) { return@synchronized }
        if (load().optString("last_journal") != journal.getString("id")) {
            adapter.restoreNative(journal.getJSONObject("native"))
            save(journal.getJSONObject("state"))
        }
        journalFile.delete()
    }

    /**
     * Runs [mutate] against the stores and the state as one recoverable transaction. The state file
     * is the commit point: after a crash, [recover] restores the stored values the journal saved.
     */
    private fun <T> transaction(state: JSONObject, mutate: (JSONObject) -> T): T {
        val id = UUID.randomUUID().toString()
        write(journalFile, JSONObject().put("id", id).put("native", adapter.snapshotNative()).put("state", StrictJson.copy(state)))
        val result = try { mutate(state) } catch (error: Exception) {
            recover(); throw error
        }
        beforeCommit?.invoke()
        state.put("last_journal", id)
        save(state)
        journalFile.delete()
        return result
    }

    // ---- Projection and change tracking ----------------------------------

    /** The phone's current portable records: native values, or a preserved value Android can't use. */
    fun projection(): Map<String, LocalRecord> = synchronized(LOCK) { recover(); projection(load()) }

    private fun projection(state: JSONObject): Map<String, LocalRecord> {
        val result = LinkedHashMap<String, LocalRecord>()
        val shadow = state.getJSONObject("shadow")
        val origins = state.getJSONObject("origins")
        for (key in Contract.SETTING_KEYS) {
            val id = Contract.SETTING_IDS.getValue(key)
            val native = adapter.nativeSetting(key)
            val preserved = shadow.optJSONObject(key)?.takeIf { it.getString("masked") == StrictJson.canonical(native) }
            val value = preserved?.get("value") ?: native ?: continue
            result[id] = LocalRecord(id, "settings", preserved?.getString("origin") ?: origins.optString(id, deviceId),
                JSONObject().put("kind", "setting").put("key", key).put("value", value))
        }
        for ((id, entry) in adapter.dictionary()) result[id] = LocalRecord(id, "dictionary", entry.first, entry.second)
        return result
    }

    /** Settings kept but not applied, with the reason, by key. A local change to that setting replaces them. */
    fun preserved(): Map<String, String> = synchronized(LOCK) {
        recover()
        val state = load(); dropReplacedShadows(state)
        val shadow = state.getJSONObject("shadow")
        shadow.keys().asSequence().associateWith { shadow.getJSONObject(it).getString("reason") }
    }

    private fun dropReplacedShadows(state: JSONObject): Boolean {
        val shadow = state.getJSONObject("shadow")
        val stale = shadow.keys().asSequence().toList().filter { key -> shadow.getJSONObject(key).getString("masked") != StrictJson.canonical(adapter.nativeSetting(key)) }
        stale.forEach(shadow::remove)
        return stale.isNotEmpty()
    }

    /** Advances the local revision when the portable projection changed since it was last seen. */
    fun observe(): Long = synchronized(LOCK) {
        recover()
        val state = load()
        val dropped = dropReplacedShadows(state)
        val current = JSONObject().apply { projection(state).forEach { (id, record) -> put(id, fingerprint(record)) } }
        if (StrictJson.equal(current, state.getJSONObject("observed"))) { if (dropped) save(state); return state.getLong("local_revision") }
        state.put("observed", current).put("local_revision", state.getLong("local_revision") + 1)
        save(state)
        state.getLong("local_revision")
    }

    private fun fingerprint(record: LocalRecord) = StrictJson.canonical(JSONObject().put("o", record.origin).put("d", record.data))

    // ---- Manual export ----------------------------------------------------

    /** A PortableBackup of live settings and dictionary data only: no account, keys, device setup or history. */
    fun exportBackup(now: Long = System.currentTimeMillis()): JSONObject = synchronized(LOCK) {
        recover()
        val records = projection(load()).values
        fun rows(category: String) = JSONArray(records.filter { it.category == category }.map { record ->
            JSONObject().put("schema_version", 1L).put("id", record.id).put("origin_device_id", record.origin).put("category", category).put("data", record.data)
        })
        val backup = JSONObject().put("format", Contract.FORMAT).put("format_version", 1L).put("export_id", UUID.randomUUID().toString())
            .put("created_at_ms", now).put("settings", rows("settings")).put("dictionary", rows("dictionary"))
        // The same checks an import runs, so an export this phone writes is always importable.
        try { Contract.parseBackup(backup.toString().toByteArray(Charsets.UTF_8)) } catch (error: ContractException) {
            throw IllegalStateException(if (error.reason == "too_large") "Your dictionary is larger than one export file allows (256 KB). Nothing was removed." else "This phone's settings could not be exported.")
        }
        backup
    }

    // ---- Import, restore and rollback ------------------------------------

    /** Validates a file and persists its preview. Throws [ContractException] (reason invalid, unsupported_version, …) with nothing changed. */
    fun previewFile(bytes: ByteArray, categories: Set<String>, mode: String, disposition: String = "restore_once"): ImportPlan {
        val backup = Contract.parseBackup(bytes)
        val records = listOf("settings", "dictionary").flatMap { category ->
            val rows = backup.getJSONArray(category)
            (0 until rows.length()).map { rows.getJSONObject(it) }.map { LocalRecord(it.getString("id"), category, it.getString("origin_device_id"), it.getJSONObject("data")) }
        }
        return preview(records, backup.getString("export_id"), categories, mode, disposition, "file")
    }

    /**
     * Persists a preview of [incoming] against this phone. New dictionary IDs are added, equal ones
     * deduplicate, and differing content is a conflict the user resolves. A setting this phone can't
     * use is preserved without being applied. In replace_selected mode, dictionary entries missing
     * from [incoming] are shown for removal; settings are never removed.
     */
    fun preview(incoming: List<LocalRecord>, exportId: String, categories: Set<String>, mode: String, disposition: String, source: String): ImportPlan = synchronized(LOCK) {
        require(categories.isNotEmpty() && categories.all { it in setOf("settings", "dictionary") })
        require(mode in setOf("merge", "replace_selected") && disposition in setOf("restore_once", "restore_and_sync"))
        val revision = observe()
        val state = load()
        val local = projection(state)
        val items = mutableListOf<PreviewItem>()
        val selected = incoming.filter { it.category in categories }
        for (record in selected) {
            val current = local[record.id]
            if (record.category == "settings") {
                val key = record.data.getString("key")
                val value = record.data.get("value")
                val title = settingTitle(key)
                val change = if (current == null) describe(key, value) else if (StrictJson.equal(current.data.get("value"), value)) describe(key, value)
                    else "${describe(key, current.data.get("value"))} → ${describe(key, value)}"
                val reason = adapter.unsupported(key, value)
                items += when {
                    current != null && StrictJson.equal(current.data.get("value"), value) -> PreviewItem(record.id, "settings", "unchanged", title, change)
                    reason != null -> PreviewItem(record.id, "settings", "preserve", title, describe(key, value), reason)
                    else -> PreviewItem(record.id, "settings", "apply", title, change)
                }
            } else {
                val title = entryTitle(record.data)
                items += when {
                    current == null -> PreviewItem(record.id, "dictionary", "apply", title, "Add ${kindName(record.data)}")
                    StrictJson.equal(current.data, record.data) -> PreviewItem(record.id, "dictionary", "unchanged", title, "Already on this phone")
                    else -> PreviewItem(record.id, "dictionary", "conflict", title, "This phone: ${entryTitle(current.data)} · Incoming: $title")
                }
            }
        }
        if (mode == "replace_selected" && "dictionary" in categories) {
            val kept = selected.map { it.id }.toSet()
            local.values.filter { it.category == "dictionary" && it.id !in kept }.forEach { items += PreviewItem(it.id, "dictionary", "remove", entryTitle(it.data), "Remove from this phone") }
        }
        val after = local.values.filter { it.category == "dictionary" && items.none { item -> item.id == it.id && item.action == "remove" } }.map { it.data } +
            selected.filter { it.category == "dictionary" && it.id !in local }.map { it.data }
        for (kind in Contract.DICTIONARY_KINDS) check(after.count { it.getString("kind") == kind } <= Contract.MAX_ENTRIES_PER_KIND) {
            "Importing this would leave more than ${Contract.MAX_ENTRIES_PER_KIND} ${kindName(JSONObject().put("kind", kind))}s. Nothing was changed."
        }
        fun ids(vararg actions: String) = JSONArray(items.filter { it.action in actions && it.id in selected.map(LocalRecord::id) }.map { it.id })
        val preview = JSONObject().put("schema_version", 1L).put("import_id", UUID.randomUUID().toString()).put("export_id", exportId).put("mode", mode)
            .put("categories", JSONArray(listOf("settings", "dictionary").filter { it in categories })).put("disposition", disposition)
            .put("expected_local_revision", revision).put("apply_ids", ids("apply", "unchanged")).put("preserve_ids", ids("preserve")).put("conflict_ids", ids("conflict"))
        state.put("pending", JSONObject().put("preview", preview).put("source", source)
            .put("records", JSONArray(selected.map { JSONObject().put("id", it.id).put("category", it.category).put("origin", it.origin).put("data", it.data) }))
            .put("removals", JSONArray(items.filter { it.action == "remove" }.map { it.id })))
        save(state)
        ImportPlan(preview, items)
    }

    /**
     * Applies the persisted preview [importId] atomically. [keepLocal] must answer every conflict:
     * true keeps this phone's entry, false takes the incoming one. Returns an ImportReceipt.
     * A repeated import ID returns its first receipt and never applies twice.
     */
    fun apply(importId: String, keepLocal: Map<String, Boolean>): JSONObject = synchronized(LOCK) {
        recover()
        val state = load()
        state.getJSONObject("receipts").optJSONObject(importId)?.let { return it }
        val pending = state.optJSONObject("pending")?.takeIf { it.getJSONObject("preview").getString("import_id") == importId }
            ?: return notApplied(importId, "stale_preview")
        val preview = pending.getJSONObject("preview")
        if (observe() != preview.getLong("expected_local_revision")) return notApplied(importId, "stale_preview")
        val conflicts = preview.getJSONArray("conflict_ids").let { array -> (0 until array.length()).map { array.getString(it) } }
        if (conflicts.any { it !in keepLocal }) return notApplied(importId, "conflict")
        val fresh = load()
        val records = pending.getJSONArray("records").let { array -> (0 until array.length()).map { array.getJSONObject(it) } }
        val removals = pending.getJSONArray("removals").let { array -> (0 until array.length()).map { array.getString(it) }.toSet() }
        val snapshot = snapshot(fresh)
        return try {
            transaction(fresh) { next ->
                val take = records.filter { it.getString("id") !in conflicts || keepLocal[it.getString("id")] == false }
                applyRecords(next, take.map { LocalRecord(it.getString("id"), it.getString("category"), it.getString("origin"), it.getJSONObject("data")) }, removals)
                finish(next, snapshot.put("import_id", importId))
                val receipt = JSONObject().put("schema_version", 1L).put("import_id", importId).put("state", "applied")
                    .put("local_revision", next.getLong("local_revision")).put("rollback_snapshot_id", snapshot.getString("id"))
                next.getJSONObject("receipts").put(importId, receipt)
                next.remove("pending")
                receipt
            }
        } catch (error: Exception) { notApplied(importId, "storage_failed") }
    }

    /**
     * Restores the stored snapshot if nothing portable changed since [expectedLocalRevision].
     * Returns an applied receipt with a new undo snapshot, or not_applied (stale_preview).
     */
    fun rollback(importId: String, snapshotId: String, expectedLocalRevision: Long): JSONObject = synchronized(LOCK) {
        recover()
        val state = load()
        state.optJSONObject("rollbacks")?.optJSONObject(snapshotId)?.let { return it }
        val snapshot = state.optJSONObject("snapshot")?.takeIf { it.getString("id") == snapshotId } ?: return notApplied(importId, "stale_preview")
        if (observe() != expectedLocalRevision) return notApplied(importId, "stale_preview")
        val fresh = load()
        val undo = snapshot(fresh)
        return try {
            transaction(fresh) { next ->
                adapter.restoreNative(snapshot.getJSONObject("native"))
                next.put("shadow", snapshot.getJSONObject("shadow")).put("origins", snapshot.getJSONObject("origins"))
                finish(next, undo.put("import_id", importId))
                val receipt = JSONObject().put("schema_version", 1L).put("import_id", importId).put("state", "applied")
                    .put("local_revision", next.getLong("local_revision")).put("rollback_snapshot_id", undo.getString("id"))
                next.put("rollbacks", JSONObject().put(snapshotId, receipt))
                receipt
            }
        } catch (_: Exception) { notApplied(importId, "storage_failed") }
    }

    /** The kept rollback snapshot: its ID, the import it undoes and the local revision it expects. */
    fun undoAvailable(): Triple<String, String, Long>? = synchronized(LOCK) {
        recover()
        load().optJSONObject("snapshot")?.let { Triple(it.getString("id"), it.getString("import_id"), it.getLong("local_revision")) }
    }

    fun discardUndo() = synchronized(LOCK) { recover(); val state = load(); state.remove("snapshot"); save(state) }

    /**
     * Applies records received from the account (MW-OUATH-7 sync) with the same capability checks as
     * an import: deleted dictionary entries are removed, unusable settings preserved. No rollback
     * snapshot is kept; the change is still atomic across the stores.
     */
    fun applyRemote(records: List<LocalRecord>, deletions: Set<String>) = synchronized(LOCK) {
        recover()
        observe()
        val state = load()
        transaction(state) { next ->
            applyRecords(next, records, deletions)
            val current = JSONObject().apply { projection(next).forEach { (id, record) -> put(id, fingerprint(record)) } }
            if (!StrictJson.equal(current, next.getJSONObject("observed"))) next.put("observed", current).put("local_revision", next.getLong("local_revision") + 1)
        }
    }

    /**
     * Gives dictionary entry [id] a new ID from this phone, in place. Used when the account deleted
     * the old ID (tombstones are final) and the user chose to keep the entry. Returns the new ID.
     */
    fun reidentify(id: String): String? = synchronized(LOCK) {
        recover()
        val prefs = CompanionPreferences(context)
        val document = prefs.document()
        val fresh = UUID.randomUUID().toString()
        val entry = PortableAdapter.KINDS.map { document.getJSONArray(it.second) }.flatMap { array -> (0 until array.length()).map { array.getJSONObject(it) } }
            .find { it.getString("id") == id } ?: return null
        entry.put("id", fresh).put("origin", deviceId)
        prefs.saveDocument(document)
        fresh
    }

    private fun applyRecords(state: JSONObject, records: List<LocalRecord>, removals: Set<String>) {
        val shadow = state.getJSONObject("shadow")
        val origins = state.getJSONObject("origins")
        for (record in records.filter { it.category == "settings" }) {
            val key = record.data.getString("key")
            val outcome = adapter.apply(key, record.data.get("value"))
            if (outcome.applied) { shadow.remove(key); origins.put(record.id, record.origin) }
            else shadow.put(key, JSONObject().put("value", record.data.get("value")).put("origin", record.origin)
                .put("masked", StrictJson.canonical(adapter.nativeSetting(key))).put("reason", outcome.reason))
        }
        val dictionary = records.filter { it.category == "dictionary" }
        if (dictionary.isNotEmpty() || removals.isNotEmpty()) {
            val entries = LinkedHashMap(adapter.dictionary())
            removals.forEach(entries::remove)
            for (record in dictionary) entries[record.id] = record.origin to record.data
            for (kind in Contract.DICTIONARY_KINDS) {
                if (entries.values.count { it.second.getString("kind") == kind } > Contract.MAX_ENTRIES_PER_KIND) {
                    throw IllegalStateException("This would leave more than ${Contract.MAX_ENTRIES_PER_KIND} ${kind}s. Nothing was changed.")
                }
            }
            adapter.saveDictionary(entries)
        }
    }

    private fun snapshot(state: JSONObject) = JSONObject().put("id", UUID.randomUUID().toString()).put("native", adapter.snapshotNative())
        .put("shadow", StrictJson.copy(state.getJSONObject("shadow"))).put("origins", StrictJson.copy(state.getJSONObject("origins")))

    private fun finish(state: JSONObject, snapshot: JSONObject) {
        val current = JSONObject().apply { projection(state).forEach { (id, record) -> put(id, fingerprint(record)) } }
        state.put("observed", current).put("local_revision", state.getLong("local_revision") + 1)
        state.put("snapshot", snapshot.put("local_revision", state.getLong("local_revision")))
    }

    private fun notApplied(importId: String, reason: String) =
        JSONObject().put("schema_version", 1L).put("import_id", importId).put("state", "not_applied").put("reason", reason)

    companion object {
        /** Serializes every portable change in this process. */
        val LOCK = Any()

        /** Tests only: runs between the store writes and the commit point, to simulate process death there. */
        @Volatile internal var beforeCommit: (() -> Unit)? = null

        /** The immutable random installation ID. Never a hostname, serial, path or account name. */
        fun deviceId(context: Context): String = synchronized(LOCK) {
            val prefs = context.applicationContext.getSharedPreferences("portable-device", Context.MODE_PRIVATE)
            prefs.getString("id", null) ?: UUID.randomUUID().toString().also { check(prefs.edit().putString("id", it).commit()) }
        }

        private fun write(file: AtomicFile, value: JSONObject) {
            val out = file.startWrite()
            try { out.write(value.toString().toByteArray(Charsets.UTF_8)); file.finishWrite(out) } catch (error: Exception) { file.failWrite(out); throw error }
        }

        fun settingTitle(key: String) = when (key) {
            "ui_language" -> "Interface language"; "input_language" -> "Speech language"; "output_language" -> "Output language"
            "transcription_model" -> "Dictation engine"; "dictation_mode" -> "Dictation mode"; "cleanup" -> "Polishing"
            "delivery" -> "Inserted text"; "theme" -> "Theme"; "polish_instructions" -> "Polishing instructions"; else -> key
        }

        fun describe(key: String, value: Any?): String = when (key) {
            "ui_language", "input_language" -> CompanionUi.languageName(value as String)
            "output_language" -> (value as JSONObject).let { if (it.getString("mode") == "preserve") "Keep the spoken language" else "Translate to ${CompanionUi.languageName(it.getString("language"))}" }
            "transcription_model" -> (value as JSONObject).let { model -> if (model.getString("provider") == "local") "${model.getString("model")} on the phone"
                else modelLabel(model) }
            "cleanup" -> (value as JSONObject).optJSONObject("selection")?.let(::modelLabel) ?: "Off"
            "delivery" -> when (value) { "dictionary" -> "Saved corrections & snippets"; "cleaned" -> "Polished wording when available"; else -> "Spoken wording" }
            "dictation_mode" -> (value as String).replaceFirstChar { it.uppercase() }
            "theme" -> when (value) { "light" -> "Paper"; "dark" -> "Ink"; else -> "Follow system" }
            "polish_instructions" -> (value as JSONObject).let {
                val rewrite = when (it.optJSONObject("formatting")?.optString("rewrite")) { "light" -> "Light"; "concise" -> "Concise"; else -> "Polished" }
                if (PolishPrompts.fromJson(it) == PolishPrompts.defaults()) "Default instructions · $rewrite rewrite" else "Edited instructions · $rewrite rewrite"
            }
            else -> value.toString()
        }

        private fun modelLabel(model: JSONObject): String {
            val provider = CloudProviders.all.find { it.id == model.getString("provider") } ?: return "${model.getString("provider")} · ${model.getString("model")}"
            return "${provider.name} · ${CloudProviders.modelName(provider.id, model.getString("model"))}"
        }

        fun kindName(data: JSONObject) = when (data.getString("kind")) { "hint" -> "word"; "correction" -> "correction"; else -> "snippet" }

        fun entryTitle(data: JSONObject): String = when (data.getString("kind")) {
            "hint" -> data.getString("text")
            "correction" -> "${data.getString("from")} → ${data.getString("to")}"
            else -> "${data.getString("phrase")} → ${data.getString("text")}"
        } + if (data.getBoolean("enabled")) "" else " (off)"
    }
}

/**
 * Local portable changes, reported by the stores after they save. The sync engine listens; work
 * happens on one background thread, so a store write never waits on the account.
 */
object PortableChanges {
    private val listeners = CopyOnWriteArrayList<(Context) -> Unit>()
    private val worker = Executors.newSingleThreadExecutor { Thread(it, "writer-portable-changes").apply { isDaemon = true } }
    fun listen(listener: (Context) -> Unit): () -> Unit { listeners += listener; return { listeners -= listener } }
    /** Tests only: waits until every change reported so far has been handled. */
    internal fun settle() { worker.submit {}.get() }
    fun changed(context: Context) {
        val app = context.applicationContext
        worker.execute {
            try { PortableStore(app).observe() } catch (_: Exception) {}
            listeners.forEach { listener -> try { listener(app) } catch (_: Exception) {} }
        }
    }
}
