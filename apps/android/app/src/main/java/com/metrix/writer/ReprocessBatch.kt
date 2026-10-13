package com.metrix.writer

import android.content.Context
import android.util.AtomicFile
import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.security.MessageDigest
import java.util.UUID
import java.util.concurrent.Executors

/** User-started, single-worker history queue. No target token, delivery ledger or insertion API. */
class ReprocessBatch internal constructor(private val context: Context, private val store: SessionStore,
    private val directory: File = File(context.filesDir, "reprocess-batches"),
    private val manager: SessionManager? = null,
    private val processor: ((JSONObject, JSONObject, String) -> JSONObject)? = null,
    private val cloudFactory: (JSONObject) -> CloudClient = { CloudProviders.client(context, it) }) {
    companion object {
        const val MAX_ITEMS = 32
        private const val MAX_BATCHES = 128
        private const val STAGE_VERSION = "mw113-android-stages-1"
        @Volatile private var instance: ReprocessBatch? = null
        fun get(context: Context): ReprocessBatch = instance ?: synchronized(this) {
            instance ?: SessionManager.get(context).let { ReprocessBatch(context.applicationContext, it.store, manager = it).also { instance = it } }
        }
        /** Sorted JSON keys make snapshots independent of JSONObject insertion order. */
        internal fun canonical(value: Any?): String = when (value) {
            is JSONObject -> value.keys().asSequence().toList().sorted().joinToString(",", "{", "}") { JSONObject.quote(it) + ":" + canonical(value.get(it)) }
            is JSONArray -> (0 until value.length()).joinToString(",", "[", "]") { canonical(value.get(it)) }
            null, JSONObject.NULL -> "null"
            is String -> JSONObject.quote(value)
            else -> value.toString()
        }
        internal fun hash(text: String): String = MessageDigest.getInstance("SHA-256").digest(text.toByteArray(Charsets.UTF_8)).joinToString("") { "%02x".format(it) }
        internal fun hash(file: File): String {
            val digest = MessageDigest.getInstance("SHA-256")
            file.inputStream().buffered().use { input -> val bytes = ByteArray(65536); while (true) { val count = input.read(bytes); if (count < 0) break; digest.update(bytes, 0, count) } }
            return digest.digest().joinToString("") { "%02x".format(it) }
        }
    }
    private val executor = Executors.newSingleThreadExecutor { Thread(it, "writer-historical-batch").apply { isDaemon = true } }
    @Volatile var active: String? = null; private set
    @Volatile private var cancelProvider: (() -> Unit)? = null
    init {
        check(directory.isDirectory || directory.mkdirs())
        // Durable completed items survive. A dead worker needs explicit review/resume.
        list().forEach { batch -> if (batch.optString("state") == "running") {
            batch.put("state", "paused")
            entries(batch).filter { it.getString("state") == "running" }.forEach { it.put("state", "interrupted").put("error", "Provider outcome may be unknown. Explicit resume may repeat paid work.") }
            save(batch)
        } }
    }
    private fun file(id: String) = AtomicFile(File(directory, "${UUID.fromString(id)}.json"))
    @Synchronized private fun save(batch: JSONObject) {
        val bytes = batch.toString().toByteArray(Charsets.UTF_8)
        check(bytes.size <= 16 * 1024 * 1024) { "Batch results exceed the storage budget; original recordings remain." }
        val retained = directory.listFiles().orEmpty().filter { it.name.endsWith(".json") && it.name != "${batch.getString("id")}.json" }.sumOf { it.length() }
        check(retained + bytes.size <= 32 * 1024 * 1024) { "Historical queue metadata is full (32 MiB). Delete completed plans; recording versions remain in History." }
        val atomic = file(batch.getString("id")); val stream = atomic.startWrite()
        try { stream.write(bytes); stream.fd.sync(); atomic.finishWrite(stream); check(atomic.readFully().contentEquals(bytes)) }
        catch (error: Exception) { atomic.failWrite(stream); throw error }
    }
    @Synchronized fun load(id: String) = JSONObject(String(file(id).readFully(), Charsets.UTF_8))
    @Synchronized fun list(): List<JSONObject> = directory.listFiles().orEmpty().map { it.name.removeSuffix(".bak") }.filter { it.endsWith(".json") }.distinct().map { load(it.removeSuffix(".json")) }.sortedByDescending { it.optLong("created_at_ms") }
    private fun entries(batch: JSONObject) = batch.getJSONArray("items").let { rows -> (0 until rows.length()).map { rows.getJSONObject(it) } }
    private fun lastRaw(item: JSONObject): JSONObject? = item.getJSONArray("attempts").let { rows -> (rows.length()-1 downTo 0).map { rows.getJSONObject(it) }.firstOrNull { it.getJSONObject("status").getString("state") == "succeeded" } }
    private fun input(item: JSONObject, operation: String, jobId: String?): String = if (operation == "transcribe") {
        check(item.optString("capture_state") in setOf("complete", "interrupted") && store.audio(item).isFile) { "Audio unavailable; use saved-text cleanup if raw text remains." }
        check(store.audio(item).length() == item.getLong("byte_length")) { "Saved audio changed." }; hash(store.audio(item))
    } else hash(CompanionProcessing.raw(CompanionProcessing.find(item, checkNotNull(jobId))))
    internal fun fingerprint(inputHash: String, choice: JSONObject, operation: String): String = hash(canonical(JSONObject()
        .put("stage", STAGE_VERSION).put("input_hash", inputHash).put("operation", operation).put("choice", choice)))
    private fun cached(key: String): JSONObject? = list().flatMap(::entries).firstOrNull {
        it.optString("state") == "succeeded" && it.optString("fingerprint") == key &&
            it.optString("version_session_id").isNotEmpty() && runCatching { lastRaw(store.load(it.getString("version_session_id"))) != null }.getOrDefault(false)
    }
    fun currentChoice(operation: String): JSONObject {
        require(operation in setOf("transcribe", "cleanup"))
        if (operation == "transcribe") return DictationChoice(context).capture()
        val prefs = CompanionPreferences(context)
        // Cleanup does not require a speech key, model or saved audio.
        val options = prefs.options("phone", "saved-text", DictationChoice(context).language())
        val selected = options.getJSONObject("cleanup").optJSONObject("selection")
        return JSONObject().put("options", options).put("policy", prefs.polishPolicy(selected))
    }
    @Synchronized fun plan(ids: List<String>, operation: String, choice: JSONObject = currentChoice(operation)): JSONObject {
        require(operation in setOf("transcribe", "cleanup")); require(ids.isNotEmpty() && ids.size <= MAX_ITEMS && ids.distinct().size == ids.size) { "Select 1–32 different saved recordings." }
        check(list().size < MAX_BATCHES) { "Batch history is full. Delete completed batches after exporting their versions." }
        check(choice.toString().toByteArray().size <= 128 * 1024)
        val frozen = JSONObject(choice.toString())
        val items = JSONArray()
        ids.forEach { id ->
            val entry = JSONObject().put("source_session_id", UUID.fromString(id).toString()).put("state", "queued").put("version_session_id", UUID.randomUUID().toString())
            try {
                val source = store.load(id)
                check(!source.has("pending") && !source.has("pending_postprocess")) { "Finish or cancel this item's earlier pending work first." }
                val job = lastRaw(source); val jobId = job?.getString("id")
                if (jobId != null) entry.put("source_job_id", jobId)
                val input = input(source, operation, jobId)
                val sourceOptions = if (operation == "cleanup") hash(canonical(job!!.getJSONObject("request").getJSONObject("options"))) else ""
                val key = fingerprint(if (operation == "cleanup") hash(input+sourceOptions) else input, frozen, operation)
                if (operation == "cleanup") entry.put("source_options_hash", sourceOptions)
                entry.put("input_hash", input).put("fingerprint", key).put("reuse_available", cached(key) != null)
            } catch (error: Exception) { entry.put("state", "unavailable").put("error", error.message ?: "Saved input unavailable.") }
            items.put(entry)
        }
        val policy = frozen.optJSONObject("policy") ?: frozen.optJSONObject("engine")?.optJSONObject("polish_policy") ?: frozen.optJSONObject("cloud")?.optJSONObject("polish_policy")
        val paid = if (operation == "transcribe") frozen.has("cloud") else policy?.optJSONObject("selection") != null
        val selected = if (operation == "transcribe") frozen.getJSONObject("options").getJSONObject("selection") else policy?.optJSONObject("selection")
        val batch = JSONObject().put("id", UUID.randomUUID().toString()).put("created_at_ms", System.currentTimeMillis()).put("state", "planned")
            .put("operation", operation).put("choice", frozen).put("items", items).put("paid_work", paid)
            .put("provider_model", selected?.let { "${it.getString("provider")} / ${it.getString("model")}" } ?: "Dictionary and snippets on this phone")
            .put("likely_paid_items", if (paid) entries(JSONObject().put("items",items)).count { it.getString("state") == "queued" && !it.optBoolean("reuse_available") } else 0)
        save(batch); return batch
    }
    @Synchronized fun start(id: String, acknowledgePaid: Boolean) {
        check(active == null) { "Another historical batch is active." }
        val batch = load(id); check(batch.getString("state") in setOf("planned", "paused")); check(!batch.getBoolean("paid_work") || acknowledgePaid) { "Confirm potential paid cloud work." }
        manager?.acquireHistorical()
        try {
            batch.put("state", "running")
            entries(batch).filter { it.getString("state") in setOf("running", "interrupted") }.forEach { it.put("state", "queued") }
            save(batch); active = id
            executor.execute {
                try { run(id) } catch (_: Exception) {
                    synchronized(this) { runCatching { load(id).also { if (it.getString("state") == "running") { it.put("state","paused"); save(it) } } } }
                } finally { cancelProvider = null; active = null; manager?.releaseHistorical() }
            }
        } catch (error: Exception) { manager?.releaseHistorical(); throw error }
    }
    @Synchronized fun cancel(id: String) {
        val batch = load(id); if (batch.getString("state") in setOf("complete", "canceled")) return
        batch.put("state", "canceled"); entries(batch).filter { it.getString("state") in setOf("queued", "running", "interrupted") }.forEach { it.put("state", "canceled") }
        save(batch); if (active == id) cancelProvider?.invoke()
    }
    @Synchronized fun delete(id: String) { check(active != id) { "Wait for cancellation before deleting batch versions." }; file(id).delete() }
    @Synchronized fun forgetSources(ids: Set<String>) {
        check(active == null) { "Cancel or finish historical processing before deleting saved data." }
        list().forEach { batch ->
            val kept = entries(batch).filter { it.getString("source_session_id") !in ids && it.getString("version_session_id") !in ids }
            if (kept.isEmpty()) file(batch.getString("id")).delete() else { batch.put("items",JSONArray(kept)); save(batch) }
        }
    }
    private fun running(id: String) = load(id).getString("state") == "running"
    private fun run(id: String) {
        while (true) {
            val batch: JSONObject; val entry: JSONObject
            synchronized(this) {
                batch = load(id); if (batch.getString("state") != "running") return
                entry = entries(batch).firstOrNull { it.getString("state") == "queued" } ?: run { batch.put("state", "complete"); save(batch); return }
                entry.put("state", "running"); save(batch)
            }
            var failure: String? = null
            try {
                val source = store.load(entry.getString("source_session_id")); val operation = batch.getString("operation"); val choice = batch.getJSONObject("choice")
                check(input(source, operation, entry.optString("source_job_id").takeIf { it.isNotEmpty() }) == entry.getString("input_hash")) { "Input changed since planning. Create a new batch." }
                if (operation == "cleanup") check(hash(canonical(CompanionProcessing.find(source,entry.getString("source_job_id")).getJSONObject("request").getJSONObject("options"))) == entry.getString("source_options_hash")) { "Source language/model settings changed; create a new plan." }
                val prior = synchronized(this) { cached(entry.getString("fingerprint")) }
                val version = createVersion(source, entry, operation)
                val result = if (prior != null) {
                    val old = lastRaw(store.load(prior.getString("version_session_id")))!!
                    val copied = JSONObject(old.toString()).put("id", UUID.randomUUID().toString())
                    copied.put("request", JSONObject(old.getJSONObject("request").toString()).put("recording_id",version.getString("id")).put("idempotency_key", copied.getString("id")))
                    if (operation == "cleanup") {
                        val original = CompanionProcessing.find(source,entry.getString("source_job_id"))
                        copied.getJSONObject("status").getJSONObject("result").put("transcript",JSONObject(original.getJSONObject("status").getJSONObject("result").getJSONObject("transcript").toString()))
                        copied.getJSONObject("request").put("options",processingOptions(original,choice.getJSONObject("options"),source.getString("input_language")))
                    }
                    for (key in listOf("client_timing", "polish_delivery", "delivery", "processing_attempts")) copied.remove(key)
                    copied.put("reused_from", prior.getString("version_session_id")); entry.put("reused_from", prior.getString("version_session_id")); copied
                } else processor?.invoke(version, choice, operation) ?: execute(version, choice, operation)
                check(running(id)) { "Batch canceled. Original data remains saved." }
                check(result.getJSONObject("status").getString("state") == "succeeded") { result.optJSONObject("status")?.optJSONObject("error")?.optString("message") ?: "Processing failed." }
                if (operation == "cleanup" && choice.getJSONObject("options").getJSONObject("cleanup").optJSONObject("selection") != null)
                    check(result.has("cleaned_text") && result.optJSONObject("polish_outcome")?.optBoolean("original_used") != true) { "Cleanup failed; original raw text remains saved." }
                val updated = store.load(version.getString("id")); updated.put("attempts",JSONArray().put(result)); store.save(updated)
                entry.put("state","succeeded")
            } catch (error: Exception) { failure = error.message ?: "Historical processing failed; original data remains." }
            synchronized(this) {
                if (!running(id)) return
                if (failure != null) entry.put("state","failed").put("error",failure)
                save(batch)
            }
        }
    }
    private fun createVersion(source: JSONObject, entry: JSONObject, operation: String): JSONObject {
        val id = entry.getString("version_session_id")
        // A crash may have saved the version before updating the queue. Keep it;
        // explicit resume reconciles its matching committed receipt below.
        runCatching { store.load(id) }.getOrNull()?.let { return it }
        val version = JSONObject().put("id",id).put("created_at_ms",System.currentTimeMillis()).put("duration_ms",source.optLong("duration_ms"))
            .put("mime_type","audio/wav").put("capture_state","complete").put("byte_length",if (operation == "transcribe") source.getLong("byte_length") else 0L)
            .put("input_language",source.getString("input_language")).put("attempts",JSONArray()).put("historical_source",JSONObject()
                .put("session_id",source.getString("id")).put("job_id",entry.opt("source_job_id") ?: JSONObject.NULL)
                .put("root_job_id",if (operation == "cleanup" && source.optJSONObject("historical_source")?.optString("operation") == "cleanup")
                    source.getJSONObject("historical_source").optString("root_job_id",source.getJSONObject("historical_source").getString("job_id")) else entry.opt("source_job_id") ?: JSONObject.NULL)
                .put("operation",operation).put("fingerprint",entry.getString("fingerprint")))
            .put("last_status","Historical $operation version of ${source.getString("id")} · never automatically inserted")
        if (operation == "transcribe") {
            check(android.os.StatFs(store.directory.path).availableBytes > source.getLong("byte_length") + 16 * 1024 * 1024) { "Not enough space for a separate audio version; originals retained." }
            val target = store.audio(version); val temporary = File(target.path + ".copy")
            try { store.audio(source).inputStream().use { input -> java.io.FileOutputStream(temporary).use { output -> input.copyTo(output); output.fd.sync() } }; check(temporary.renameTo(target)) }
            finally { temporary.delete() }
        }
        store.save(version); return version
    }
    private fun execute(version: JSONObject, choice: JSONObject, operation: String): JSONObject {
        val id = version.getString("id"); val options = JSONObject(choice.getJSONObject("options").toString())
        // A completed crash-window result is success only with its same frozen fingerprint.
        lastRaw(version)?.let { completed ->
            if (operation == "transcribe" || options.getJSONObject("cleanup").optJSONObject("selection") == null || completed.has("cleaned_text") && completed.optJSONObject("polish_outcome")?.optBoolean("original_used") != true) return completed
        }
        if (operation == "transcribe") {
            options.put("cleanup",JSONObject().put("mode","off"))
            if (!version.has("pending")) {
                if (choice.has("engine")) store.preparePhone(version,options,choice.getJSONObject("engine"),"raw")
                else store.prepareDirect(version,options,choice.getJSONObject("cloud"),"raw")
            }
            if (choice.has("engine")) {
                val engine = PhoneEngine(context,store); cancelProvider = engine::cancel
                if (!running(active!!)) engine.cancel()
                engine.execute(store.load(id),"phone")
            } else {
                val engine = DirectCloudEngine(context,store,cloudFactory); cancelProvider = engine::cancel
                if (!running(active!!)) engine.cancel()
                engine.execute(store.load(id),choice.getString("server"))
            }
            return lastRaw(store.load(id)) ?: error("Transcription failed or pending. Original audio retained; resume explicitly.")
        }
        if (version.has("pending_postprocess")) {
            val cleanup = DirectCloudProcessing(context,store,cloudFactory); cancelProvider = cleanup::cancel
            if (!running(active!!)) cleanup.cancel()
            cleanup.run(id); return lastRaw(store.load(id))!!
        }
        val source = version.getJSONObject("historical_source")
        val original = CompanionProcessing.find(store.load(source.getString("session_id")),source.getString("job_id"))
        val processingOptions = processingOptions(original,options,version.getString("input_language"))
        val jobId = UUID.randomUUID().toString()
        val job = JSONObject().put("id",jobId).put("execution_location",original.optString("execution_location","phone")).put("delivery_choice","raw")
            .put("request",JSONObject().put("recording_id",id).put("idempotency_key",jobId).put("options",processingOptions))
            .put("status",JSONObject().put("state","succeeded").put("result",JSONObject().put("transcript",JSONObject(original.getJSONObject("status").getJSONObject("result").getJSONObject("transcript").toString()))))
        version.put("attempts",JSONArray().put(job)); store.save(version)
        val cleanup = DirectCloudProcessing(context,store,cloudFactory); cancelProvider = cleanup::cancel
        cleanup.prepare(id,jobId,options.getJSONObject("cleanup").optJSONObject("selection"),policy=choice.getJSONObject("policy"))
        if (!running(active!!)) cleanup.cancel()
        cleanup.run(id); return lastRaw(store.load(id))!!
    }
    private fun processingOptions(original: JSONObject, options: JSONObject, language: String): JSONObject {
        val originalOptions = original.getJSONObject("request").getJSONObject("options")
        // The raw transcript still belongs to its original speech model/languages.
        return JSONObject(options.toString()).put("selection",originalOptions.getJSONObject("selection"))
            .put("input_language",originalOptions.optString("input_language",language))
            .put("output_language",originalOptions.optJSONObject("output_language") ?: JSONObject().put("mode","preserve"))
    }
}
