package com.metrix.writer.sync

import android.content.Context
import android.util.AtomicFile
import com.metrix.writer.CompanionProcessing
import com.metrix.writer.account.AccountKey
import org.json.JSONObject
import java.io.File
import java.io.FileNotFoundException
import java.util.UUID

/** Text-only journal. Never contains requests, credentials, audio paths or insertion targets. */
class TranscriptStore(private val context: Context) {
    private val file get() = AtomicFile(File(context.filesDir, "transcript-journal.json"))
    private val database = TranscriptDatabase(file, setOf("records", "deleted", "scanned"))
    private fun load(): JSONObject = database.load { JSONObject().put("records", JSONObject()).put("deleted", JSONObject()).put("scanned", JSONObject()) }

    fun capture(item: JSONObject): Boolean = synchronized(LOCK) {
        if (item.optString("capture_state") !in setOf("complete", "interrupted")) return@synchronized false
        val state = load()
        val rows = state.getJSONObject("records")
        val attempts = item.optJSONArray("attempts") ?: return@synchronized false
        var unresolvedHistoricalRoot = false
        for (i in 0 until attempts.length()) {
            val attempt = attempts.getJSONObject(i)
            if (attempt.optJSONObject("status")?.optString("state") != "succeeded") continue
            val attemptId = attempt.optString("id")
            if (!Contract.isUuid(attemptId)) continue
            val historical = item.optJSONObject("historical_source")
            if (historical?.optString("operation") == "cleanup") {
                val sourceAttempt = historical.optString("root_job_id", historical.optString("job_id"))
                if (!Contract.isUuid(sourceAttempt)) continue
                val root = stable("original:$sourceAttempt")
                if (state.getJSONObject("deleted").has(root)) continue
                if (!rows.has(root)) { unresolvedHistoricalRoot = true; continue }
                var parent = root
                for ((field, kind) in listOf("dictionary_text" to "corrected", "cleaned_text" to "cleaned")) {
                    val text = attempt.opt(field) as? String ?: continue
                    val id = stable("historical:$root:$attemptId:$kind")
                    val data = derived(kind, root, parent, text, item.optLong("created_at_ms"))
                    if (!Contract.isTranscript(id, data)) continue
                    if (!rows.has(id)) rows.put(id, JSONObject().put("id", id).put("origin", PortableStore.deviceId(context))
                        .put("data", data).put("session", item.getString("id"))
                        .put("owner", rows.optJSONObject(root)?.optString("owner") ?: item.optString("transcript_owner", owner?.invoke()?.let(::ownerKey).orEmpty())))
                    parent = id
                }
                continue
            }
            val originalId = stable("original:$attemptId")
            if (state.getJSONObject("deleted").has(originalId)) continue
            val options = attempt.optJSONObject("request")?.optJSONObject("options") ?: continue
            val selection = options.optJSONObject("selection")?.let(StrictJson::copy) ?: continue
            if (selection.optString("provider") == "phone") selection.put("provider", "local")
            val original = JSONObject().put("kind", "original").put("attempt_id", attemptId)
                .put("recorded_at_ms", item.optLong("created_at_ms"))
                .put("input_language", options.optString("input_language", item.optString("input_language")))
                .put("output_language", options.optJSONObject("output_language") ?: JSONObject().put("mode", "preserve"))
                .put("selection", selection).put("text", CompanionProcessing.raw(attempt))
            if (!Contract.isTranscript(originalId, original)) continue // Unrepresentable models/languages stay local.
            fun add(id: String, data: JSONObject) {
                if (!rows.has(id)) rows.put(id, JSONObject().put("id", id).put("origin", PortableStore.deviceId(context))
                    .put("data", data).put("session", item.getString("id"))
                    .put("owner", rows.optJSONObject(originalId)?.optString("owner") ?: item.optString("transcript_owner", owner?.invoke()?.let(::ownerKey).orEmpty())))
            }
            add(originalId, original)
            for ((field, kind) in listOf("dictionary_text" to "corrected", "cleaned_text" to "cleaned")) {
                val text = attempt.opt(field) as? String ?: continue
                val id = stable("$originalId:$kind:$text")
                val data = derived(kind, originalId, originalId, text, item.optLong("created_at_ms"))
                if (Contract.isTranscript(id, data)) { add(id, data) }
            }
        }
        val sidecar = File(context.filesDir, "recordings/${item.getString("id")}.json")
        if (unresolvedHistoricalRoot) state.getJSONObject("scanned").remove(item.getString("id"))
        else state.getJSONObject("scanned").put(item.getString("id"), fingerprint(sidecar))
        database.save(state)
        !unresolvedHistoricalRoot
    }

    private fun fingerprint(file: File) = "${file.lastModified()}:${file.length()}"

    /** Repairs the gap after a native sidecar commit and before its journal projection. No audio I/O. */
    fun scan() = synchronized(LOCK) {
        val state = load()
        val scanned = state.getJSONObject("scanned")
        File(context.filesDir, "recordings").listFiles().orEmpty()
            .filter { it.name.endsWith(".json") || it.name.endsWith(".json.bak") }
            .map { it.name.removeSuffix(".bak").removeSuffix(".json") }.distinct().forEach { id ->
                if (!Contract.isUuid(id)) return@forEach
                val source = File(context.filesDir, "recordings/$id.json")
                if (!File(source.path + ".bak").exists() && scanned.optString(id) == fingerprint(source)) return@forEach
                val item = read(AtomicFile(source)) ?: return@forEach
                val projected = if (item.optString("capture_state") == "discarded") { deleteSession(id); true } else capture(item)
                // Include incomplete attempts: their next sidecar commit changes the fingerprint.
                // capture owns the projection receipt; unresolved historical
                // roots must be retried after their retained original is scanned.
                if (projected || item.optString("capture_state") !in setOf("complete", "interrupted")) scanned.put(id, fingerprint(source)) else scanned.remove(id)
            }
        database.save(state)
    }

    internal fun changes(after: Long) = synchronized(LOCK) { database.changes("records", after) }
    internal fun deletions(after: Long) = synchronized(LOCK) { database.changes("deleted", after) }

    fun records(): Map<String, JSONObject> = synchronized(LOCK) { load().getJSONObject("records").objects() }
    fun deleted(): Set<String> = synchronized(LOCK) { load().getJSONObject("deleted").keys().asSequence().toSet() }

    /** Durable deletion before the native tombstone. This wins even if unlinking audio is interrupted. */
    fun deleteSession(sessionId: String) = synchronized(LOCK) {
        val state = load(); val rows = state.getJSONObject("records")
        val pending = database.pendingKeys("records").filter { rows.optJSONObject(it)?.optString("session") == sessionId }
        (database.session("records", sessionId) + pending).forEach { id ->
            state.getJSONObject("deleted").put(id, true); rows.remove(id)
        }
        database.save(state)
    }

    companion object {
        internal val LOCK = Any()
        @Volatile var owner: (() -> AccountKey?)? = null
        fun ownerKey(account: AccountKey) = "${account.projectId}/${account.uid}"
        fun stable(value: String): String = UUID.nameUUIDFromBytes(value.toByteArray(Charsets.UTF_8)).toString()
        fun derived(kind: String, original: String, parent: String, text: String, time: Long = System.currentTimeMillis()) =
            JSONObject().put("kind", kind).put("original_record_id", original).put("parent_record_id", parent).put("created_at_ms", time).put("text", text)
        internal fun read(file: AtomicFile): JSONObject? = try { JSONObject(String(file.readFully(), Charsets.UTF_8)) } catch (_: FileNotFoundException) { null }
        internal fun write(file: AtomicFile, state: JSONObject) {
            file.baseFile.parentFile!!.mkdirs()
            val bytes = state.toString().toByteArray(Charsets.UTF_8)
            val stream = file.startWrite()
            try { stream.write(bytes); stream.fd.sync(); file.finishWrite(stream); check(file.readFully().contentEquals(bytes)) }
            catch (error: Exception) { file.failWrite(stream); throw error }
        }
    }
}

internal fun JSONObject.objects(): Map<String, JSONObject> = keys().asSequence().associateWith { getJSONObject(it) }
