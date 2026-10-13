package com.metrix.writer

import android.util.AtomicFile
import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.io.RandomAccessFile
import java.util.UUID
import java.net.URI

/** Existing WAV + JSON format is retained in place; complete WAVs are never rewritten. */
class SessionStore(val directory: File, private val context: android.content.Context? = null) {
    init { check(directory.isDirectory || directory.mkdirs()) { "Cannot open recording storage" } }
    private fun id(item: JSONObject) = UUID.fromString(item.getString("id")).toString()
    fun audio(item: JSONObject) = File(directory, "${id(item)}.wav")
    @Synchronized fun save(item: JSONObject) {
        // Commit the repair intent before the authoritative sidecar; a crash between
        // sidecar and projection replays just this ID when History opens.
        indexDatabase().use { db -> db.execSQL("INSERT OR IGNORE INTO repairs VALUES (?)", arrayOf(id(item))) }
        val atomic = AtomicFile(File(directory, "${id(item)}.json"))
        val out = atomic.startWrite()
        val bytes = item.toString().toByteArray(Charsets.UTF_8)
        try {
            out.write(bytes); out.fd.sync(); atomic.finishWrite(out)
            // AtomicFile logs some rename failures instead of throwing. Verify before announcing save.
            check(atomic.readFully().contentEquals(bytes)) { "Session metadata did not commit" }
        } catch (e: Exception) { atomic.failWrite(out); throw e }
        LatencyMeasurements.capture(directory, item)
        revision++
        indexDatabase().use { db ->
            db.beginTransaction()
            try { indexItem(db, item); db.execSQL("DELETE FROM repairs WHERE id=?", arrayOf(id(item))); db.setTransactionSuccessful() }
            finally { db.endTransaction() }
        }
        context?.let { ctx ->
            // The native sidecar is authoritative; a failed projection is repaired on the next sync.
            runCatching { com.metrix.writer.sync.TranscriptStore(ctx).capture(item) }
            com.metrix.writer.sync.PortableChanges.changed(ctx)
        }
    }
    /** End raw availability only after the original transcript's atomic save. */
    @Synchronized fun saveRaw(item: JSONObject, job: JSONObject) {
        save(item)
        if (job.optJSONObject("status")?.optString("state") != "succeeded") return
        val timing = job.optJSONObject("client_timing") ?: return
        val now = android.os.SystemClock.elapsedRealtime()
        timing.put("raw_durable_elapsed_ms", now)
        if (timing.has("capture_stop_elapsed_ms")) {
            val stopped = timing.getLong("capture_stop_elapsed_ms")
            if (now >= stopped) timing.put("stop_to_raw_ms", now - stopped)
        }
        // Raw is already durable. Optional timing persistence must not turn it
        // into a reported transcription failure if the second metadata write fails.
        runCatching { save(item) }
    }
    @Synchronized fun load(id: String): JSONObject = JSONObject(String(
        AtomicFile(File(directory, "${UUID.fromString(id)}.json")).readFully(), Charsets.UTF_8))

    @JvmOverloads @Synchronized fun create(language: String, configure: (JSONObject) -> Unit = {}, reserveCapture: Boolean = false): JSONObject {
        if (reserveCapture) check(activeCaptureId == null) { "Capture already reserved" }
        val item = JSONObject().put("id", UUID.randomUUID().toString())
            .put("created_at_ms", System.currentTimeMillis()).put("mime_type", "audio/wav")
            .put("byte_length", 44).put("duration_ms", 0).put("input_language", language)
            .put("capture_state", "capturing").put("attempts", JSONArray())
            .put("transcript_owner", com.metrix.writer.sync.TranscriptStore.owner?.invoke()?.let(com.metrix.writer.sync.TranscriptStore::ownerKey).orEmpty())
        RandomAccessFile(audio(item), "rw").use { header(it, 0); it.fd.sync() }
        configure(item)
        save(item)
        if (reserveCapture) activeCaptureId = id(item)
        return item
    }

    @Volatile var activeCaptureId: String? = null

    /** Terminal state and the durable completion event share one atomic sidecar write. */
    @Synchronized fun finishCapture(item: JSONObject, reason: String?) {
        if (load(id(item)).optString("capture_state") == "discarded") { discard(item); return }
        check(audio(item).isFile) { "Saved audio is missing" }
        RandomAccessFile(audio(item), "rw").use { wav ->
            check(wav.length() >= 44) { "Incomplete WAV header" }
            wav.setLength(wav.length() - (wav.length() - 44) % 2)
            header(wav, wav.length() - 44); wav.fd.sync()
        }
        val state = if (reason == null) "complete" else "interrupted"
        item.put("capture_state", state).put("byte_length", audio(item).length())
            .put("duration_ms", (audio(item).length() - 44) * 1000 / 32000)
            .put("capture_completion", JSONObject().put("event_id", id(item))
                .put("session_id", id(item)).put("state", state)
                .put("saved_at_ms", System.currentTimeMillis()))
        if (reason != null) item.put("capture_error", reason)
        save(item)
    }

    /** A durable discard intent wins over recovery even if deletion or the process fails. */
    @Synchronized fun discard(item: JSONObject) {
        item.put("capture_state", "discarded").remove("capture_completion")
        save(item)
        context?.let { UploadCheckpointStore.delete(it,id(item)) }
        check(!audio(item).exists() || audio(item).delete()) { "Discard saved; audio deletion needs retry" }
    }

    @Synchronized fun deleteSaved(id: String) {
        val item = load(id)
        check(id != activeCaptureId && item.optString("capture_state") != "capturing")
        check(!item.has("pending") && !item.has("pending_postprocess"))
        if (context != null) PhoneResultCache.deleting(context) { deleteItem(item) }
        else deleteItem(item)
    }
    private fun deleteItem(item: JSONObject) = synchronized(AudioExportProvider.guard) {
        val id = id(item)
        if (item.optString("capture_state") == "discarded") {
            finishDeletion(item)
            return@synchronized
        }
        context?.let { com.metrix.writer.sync.TranscriptStore(it).deleteSession(id) }
        // Redacted tombstone is durable before deletion, so interrupted deletion never restores text.
        val tombstone = JSONObject().put("id", id).put("capture_state", "discarded")
        if (item.has("original_target")) tombstone.put("delete_delivery", UUID.fromString(item.getString("original_target")).toString())
        save(tombstone)
        finishDeletion(tombstone)
    }
    /**
     * Deletes every saved dictation, one durable tombstone at a time. A dictation that is
     * still capturing or has work pending is left alone and reported, so nothing in flight
     * is silently destroyed. Models, dictionary, settings and keys are untouched.
     */
    @Synchronized fun deleteAllSaved(): Int {
        var kept = 0
        val items = restore()
        check(restoreFailures == 0) { "Some records or cached copies could not be erased. Reopen History to retry." }
        for (item in items) {
            val id = item.optString("id").ifEmpty { continue }
            if (item.optString("capture_state") == "discarded") continue
            if (id == activeCaptureId || item.optString("capture_state") == "capturing" ||
                item.has("pending") || item.has("pending_postprocess")) { kept += 1; continue }
            deleteSaved(id)
        }
        return kept
    }
    private fun finishDeletion(item: JSONObject) = synchronized(AudioExportProvider.guard) {
        context?.let { AudioExportProvider.erase(it, id(item)) }
        context?.let { UploadCheckpointStore.delete(it,id(item)) }
        check(!audio(item).exists() || audio(item).delete()) { "Deletion will resume when History reopens." }
        if (item.has("delete_delivery")) {
            val token = UUID.fromString(item.getString("delete_delivery"))
            val delivery = File(directory.parentFile, "deliveries/$token.json")
            AtomicFile(delivery).delete()
            check(listOf(delivery, File("${delivery.path}.bak"), File("${delivery.path}.new")).none { it.exists() }) {
                "Insertion record deletion needs retry. Reopen History."
            }
        }
    }

    var restoreFailures: Int = 0
        private set

    /** Repair inactive writers only. Discover AtomicFile backups left by interrupted writes too. */
    @Synchronized private fun restoreSidecars(): List<JSONObject> {
        val ids = directory.listFiles().orEmpty()
            .filter { it.name.endsWith(".json") || it.name.endsWith(".json.bak") }
            .sortedByDescending { it.lastModified() }
            .map { it.name.removeSuffix(".bak").removeSuffix(".json") }.distinct()
        restoreFailures = 0
        context?.let {
            try { AudioExportProvider.sweep(it); PhoneResultCache.sweep(it) }
            catch (_: Exception) { restoreFailures++ }
        }
        return ids.mapNotNull { id ->
          try {
            val item = load(id)
            if (item.optString("capture_state") == "discarded") {
                if (id == activeCaptureId) return@mapNotNull null
                // The deletion intent was committed while no phone writer held a lease.
                // Retry its unlinks even if another recording now has an active worker.
                finishDeletion(item)
                return@mapNotNull null
            }
            if (item.getString("capture_state") == "capturing" && id != activeCaptureId) {
                // SessionManager transcribes it once for History; only newly recovered captures are marked.
                if (item.optString("capture_origin") in listOf("bubble", "app")) item.put("auto_submit", true)
                finishCapture(item, "Recording interrupted by process or service loss. Saved complete frames recovered.")
            }
            item
          } catch (_: Exception) { restoreFailures++; null }
        }
    }

    @Volatile var revision: Long = 0
        private set
    data class Cursor(val created: Long, val id: String)
    data class Page(val records: List<JSONObject>, val next: Cursor?, val revision: Long)
    private fun indexDatabase(): android.database.sqlite.SQLiteDatabase {
        val db = android.database.sqlite.SQLiteDatabase.openOrCreateDatabase(File(directory, "history-index.sqlite3"), null)
        db.execSQL("PRAGMA synchronous=FULL")
        db.rawQuery("PRAGMA secure_delete=ON", null).use { it.moveToFirst() }
        db.execSQL("CREATE TABLE IF NOT EXISTS meta (id INTEGER PRIMARY KEY CHECK(id=1), ready INTEGER NOT NULL)")
        db.execSQL("CREATE TABLE IF NOT EXISTS sessions (id TEXT PRIMARY KEY, created INTEGER NOT NULL, data TEXT NOT NULL, state TEXT NOT NULL, needs_work INTEGER NOT NULL, search TEXT NOT NULL)")
        db.execSQL("CREATE INDEX IF NOT EXISTS sessions_page ON sessions(created DESC,id DESC)")
        db.execSQL("CREATE INDEX IF NOT EXISTS sessions_recovery ON sessions(needs_work)")
        db.execSQL("CREATE TABLE IF NOT EXISTS repairs (id TEXT PRIMARY KEY)")
        return db
    }
    private fun indexItem(db: android.database.sqlite.SQLiteDatabase, item: JSONObject) {
        val state = item.optString("capture_state")
        // Redact index copies along with the authoritative deletion tombstone.
        if (state == "discarded") db.execSQL("DELETE FROM sessions WHERE id=?", arrayOf(id(item)))
        else db.execSQL("INSERT INTO sessions VALUES (?,?,?,?,?,?) ON CONFLICT(id) DO UPDATE SET created=excluded.created,data=excluded.data,state=excluded.state,needs_work=excluded.needs_work,search=excluded.search",
            arrayOf<Any>(id(item), item.optLong("created_at_ms"), item.toString(), state,
                if (state == "capturing" || item.has("pending") || item.has("pending_postprocess") || item.optBoolean("auto_submit")) 1 else 0, item.toString().lowercase()))
    }
    private var indexing = false
    private fun ensureIndex() {
        if (indexing) return
        indexing = true
        try {
            val ready = indexDatabase().use { db -> db.rawQuery("SELECT ready FROM meta WHERE id=1", null).use { it.moveToFirst() && it.getInt(0) == 1 } }
            if (!ready) {
                // One-time import leaves all sidecars/audio intact and retries after interruption.
                val items = restoreSidecars()
                indexDatabase().use { db ->
                    db.beginTransaction()
                    try {
                        db.execSQL("DELETE FROM sessions")
                        items.forEach { indexItem(db, it) }
                        if (restoreFailures == 0) db.execSQL("INSERT OR REPLACE INTO meta VALUES (1,1)")
                        db.setTransactionSuccessful()
                    } finally { db.endTransaction() }
                }
            }
            val repair = indexDatabase().use { db -> db.rawQuery("SELECT id FROM repairs", null).use { rows -> buildList { while (rows.moveToNext()) add(rows.getString(0)) } } }
            for (id in repair) {
                // Missing/corrupt sidecars remain retryable; never erase their saved audio.
                try {
                    val item = load(id)
                    if (item.optString("capture_state") == "discarded") finishDeletion(item)
                    indexDatabase().use { db ->
                        db.beginTransaction()
                        try { indexItem(db, item); db.execSQL("DELETE FROM repairs WHERE id=?", arrayOf(id)); db.setTransactionSuccessful() }
                        finally { db.endTransaction() }
                    }
                } catch (_: Exception) { restoreFailures++ }
            }
        } finally { indexing = false }
    }
    /** Immutable recording-time/ID keyset: edits cannot move a row across pages. */
    @Synchronized fun page(after: Cursor? = null, limit: Int = 50, query: String = ""): Page {
        ensureIndex()
        val values = indexDatabase().use { db ->
            val cursor = after?.let { " AND (created,id) < (?,?)" }.orEmpty()
            val search = if (query.isEmpty()) "1=1" else "instr(search,?)>0"
            val args = mutableListOf<String>()
            if (query.isNotEmpty()) args.add(query.lowercase())
            after?.let { args.add(it.created.toString()); args.add(it.id) }
            args.add((limit.coerceIn(1,100) + 1).toString())
            db.rawQuery("SELECT data FROM sessions WHERE $search" + cursor + " ORDER BY created DESC,id DESC LIMIT ?", args.toTypedArray()).use { rows ->
                buildList { while (rows.moveToNext()) add(JSONObject(rows.getString(0))) }
            }
        }
        val records = values.take(limit.coerceIn(1,100))
        return Page(records, if (values.size > records.size) records.last().let { Cursor(it.optLong("created_at_ms"), it.getString("id")) } else null, revision)
    }
    @Synchronized fun allIndexed(): List<JSONObject> {
        ensureIndex()
        return indexDatabase().use { db -> db.rawQuery("SELECT data FROM sessions ORDER BY created DESC,id DESC", null).use { rows -> buildList { while (rows.moveToNext()) add(JSONObject(rows.getString(0))) } } }
    }
    /** Recovery reads only unfinished records after the one-time legacy import. */
    @Synchronized fun recover(): List<JSONObject> {
        ensureIndex()
        val items = indexDatabase().use { db -> db.rawQuery("SELECT data FROM sessions WHERE needs_work=1", null).use { rows -> buildList { while (rows.moveToNext()) add(JSONObject(rows.getString(0))) } } }
        return items.mapNotNull { item ->
            try {
                if (item.optString("capture_state") == "capturing" && item.getString("id") != activeCaptureId) {
                    if (item.optString("capture_origin") in listOf("bubble", "app")) item.put("auto_submit", true)
                    finishCapture(item, "Recording interrupted by process or service loss. Saved complete frames recovered.")
                }
                item
            } catch (_: Exception) { restoreFailures++; null }
        }
    }
    /** Explicit bulk operations retain the existing full recovery semantics. */
    @Synchronized fun restore(): List<JSONObject> {
        val items = restoreSidecars()
        indexDatabase().use { db ->
            db.beginTransaction()
            try {
                db.execSQL("DELETE FROM sessions")
                items.forEach { indexItem(db, it) }
                if (restoreFailures == 0) db.execSQL("INSERT OR REPLACE INTO meta VALUES (1,1)")
                db.setTransactionSuccessful()
            } finally { db.endTransaction() }
        }
        return items
    }

    /** Persist before I/O. An uncertain request never adopts new options or a new server. */
    @Synchronized fun prepare(item: JSONObject, server: String, provider: String, model: String, language: String) {
        val uri = URI(server)
        require(uri.host != null && uri.userInfo == null && uri.query == null && uri.fragment == null &&
            (uri.scheme == "https" || uri.scheme == "http" && uri.host == "10.0.2.2")) { "Use a valid HTTPS endpoint" }
        if (item.has("pending")) {
            check(server == item.getString("pending_server")) { "Resume on the original server" }; return
        }
        check(item.getString("capture_state") in setOf("complete", "interrupted")) { "Save capture before submitting" }
        val options = JSONObject().put("selection", JSONObject().put("provider", provider).put("model", model))
            .put("input_language", language).put("output_language", JSONObject().put("mode", "preserve"))
            .put("vocabulary", JSONObject().put("hints", JSONArray()).put("corrections", JSONArray()).put("snippets", JSONArray()))
            .put("cleanup", JSONObject().put("mode", "off"))
        prepareOptions(item, server, options, "raw")
    }
    @Synchronized fun prepareOptions(item: JSONObject, server: String, options: JSONObject, delivery: String) {
        if (item.has("pending")) { check(server == item.getString("pending_server")); return }
        check(item.getString("capture_state") in setOf("complete", "interrupted"))
        check(!item.has("pending_postprocess")) { "Resume text processing before starting a new transcription" }
        val uri = URI(server)
        require(uri.host != null && uri.userInfo == null && uri.query == null && uri.fragment == null &&
            uri.path.orEmpty().isEmpty() && (uri.scheme == "https" || uri.scheme == "http" && uri.host == "10.0.2.2"))
        val pending = JSONObject().put("recording_id", id(item)).put("idempotency_key", UUID.randomUUID().toString()).put("options", JSONObject(options.toString()))
        val attempts = item.getJSONArray("attempts")
        if (attempts.length() > 0 && attempts.getJSONObject(attempts.length() - 1).optString("execution_location") != "phone") pending.put("retry_of_job_id", attempts.getJSONObject(attempts.length() - 1).getString("id"))
        item.put("pending", pending).put("pending_server", server).put("pending_delivery", delivery)
        item.remove("pending_timing")
        save(item)
    }
    @Synchronized fun prepareDirect(item: JSONObject, options: JSONObject, cloud: JSONObject, delivery: String) {
        if (item.has("pending")) return
        check(item.getString("capture_state") in setOf("complete", "interrupted"))
        check(!item.has("pending_postprocess")) { "Finish text processing before a new transcription." }
        require(options.getJSONObject("selection").getString("provider") == cloud.getString("provider"))
        CloudProviders.get(cloud.getString("provider"))
        require(cloud.getInt("transport_version") == 1)
        UUID.fromString(cloud.getString("credential_id"))
        item.put("pending", JSONObject().put("recording_id", id(item)).put("idempotency_key", UUID.randomUUID().toString())
            .put("options", JSONObject(options.toString())))
            .put("pending_server", CloudProviders.get(cloud.getString("provider")).origin).put("pending_cloud", JSONObject(cloud.toString()))
            .put("pending_delivery", delivery)
        item.remove("pending_timing"); item.remove("cloud_upload"); item.remove("pending_cloud_error")
        save(item)
    }
    @Synchronized fun preparePhone(item: JSONObject, options: JSONObject, engine: JSONObject, delivery: String) {
        if (item.has("pending")) return
        check(item.getString("capture_state") in setOf("complete", "interrupted"))
        check(!item.has("pending_postprocess")) { "Resume text processing before a new transcription" }
        require(options.getJSONObject("selection").getString("provider") == "phone")
        val pending = JSONObject().put("recording_id", id(item)).put("idempotency_key", UUID.randomUUID().toString())
            .put("options", JSONObject(options.toString()))
        item.put("pending", pending).put("pending_server", "phone").put("pending_engine", JSONObject(engine.toString()))
            .put("pending_delivery", delivery)
        item.remove("pending_timing"); save(item)
    }
    companion object {
        @JvmStatic fun header(file: RandomAccessFile, bytes: Long) {
            fun little(value: Long, size: Int) { repeat(size) { file.write((value shr (8 * it)).toInt() and 255) } }
            file.seek(0); file.writeBytes("RIFF"); little(bytes + 36, 4); file.writeBytes("WAVEfmt ")
            little(16, 4); little(1, 2); little(1, 2); little(16000, 4); little(32000, 4)
            little(2, 2); little(16, 2); file.writeBytes("data"); little(bytes, 4)
        }
    }
}
