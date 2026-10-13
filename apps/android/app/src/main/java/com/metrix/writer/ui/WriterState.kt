package com.metrix.writer.ui

import android.app.Application
import android.content.Context
import android.net.Uri
import android.os.SystemClock
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import com.metrix.writer.*
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import org.json.JSONObject
import java.time.Instant
import java.time.LocalDate
import java.time.ZoneId

class AppearancePreferences(private val context: Context) {
    private val prefs = context.getSharedPreferences("writer-appearance", Context.MODE_PRIVATE)
    var theme: String
        get() = prefs.getString("theme", "system")!!
        set(value) { require(value in listOf("system", "light", "dark", "sage", "ocean")); check(prefs.edit().putString("theme", value).commit()); com.metrix.writer.sync.PortableChanges.changed(context) }
    var onboarding: Boolean
        get() = prefs.getBoolean("onboarding", false)
        set(value) { prefs.edit().putBoolean("onboarding", value).apply() }
}

/** One saved text (cleanup) receipt: its model, outcome and, when recorded, its own measured run. */
data class TextEdit(val provider: String, val model: String, val state: String, val cleanup: String?, val elapsed: Long?) {
    val succeeded get() = state == "complete" && cleanup == "succeeded"
    val failed get() = state == "incomplete" || (state == "complete" && cleanup != "succeeded")
}
data class AudioPreparation(val inputBytes: Long, val outputBytes: Long, val removedMs: Long, val preparationMs: Long)
data class Attempt(val id: String, val state: String, val raw: String, val dictionary: String?, val cleaned: String?, val model: String, val location: String, val error: String?, val latency: Long? = null, val edits: List<TextEdit> = emptyList(), val originalUsed: Boolean = false, val originalDelivered: Boolean = false, val audioPreparation: AudioPreparation? = null) {
    val selectedText get() = if (originalUsed) dictionary ?: raw else cleaned ?: dictionary ?: raw
}
data class Dictation(val id: String, val created: Long, val duration: Long, val capture: String, val pending: Boolean, val postprocessing: Boolean, val attempts: List<Attempt>, val latency: Long?, val delivery: String?, val note: String, val audioAvailable: Boolean = true, val historicalSource: String? = null, val historicalSourceJob: String? = null) {
    val latest get() = attempts.lastOrNull { it.state == "succeeded" }
    companion object {
        /** Saved cleanup receipts that named a text model. Receipts without one did dictionary work only. */
        private fun edits(job: JSONObject): List<TextEdit> {
            val rows = job.optJSONArray("processing_attempts") ?: return emptyList()
            return (0 until rows.length()).flatMap { index ->
                val receipt = rows.getJSONObject(index)
                val models = receipt.optJSONObject("outcome")?.optJSONArray("model_attempts")
                if (models != null) return@flatMap (0 until models.length()).map { i ->
                    val call = models.getJSONObject(i); val model = call.getJSONObject("selection"); val state = call.getString("state")
                    TextEdit(model.getString("provider"), model.getString("model"), if (state == "abandoned" || state == "pending") "pending" else "complete",
                        if (state == "succeeded") "succeeded" else "failed", call.optLong("elapsed_ms", -1).takeIf { it >= 0 })
                }
                val selection = receipt.optJSONObject("selection") ?: return@flatMap emptyList()
                val started = receipt.optLong("started_at_ms", -1)
                val finished = receipt.optLong("finished_at_ms", -1)
                listOf(TextEdit(selection.optString("provider"), selection.optString("model"), receipt.optString("state"),
                    receipt.optJSONObject("cleanup")?.optString("state"),
                    // Older receipts saved no completion time; they are excluded from timing rather than read as zero.
                    if (started > 0 && finished >= started) finished - started else null))
            }
        }
        fun from(item: JSONObject, delivery: String? = null): Dictation {
            val rows = item.optJSONArray("attempts")
            val attempts = (0 until (rows?.length() ?: 0)).map { index ->
                val job = rows!!.getJSONObject(index); val status = job.getJSONObject("status")
                val selection = job.optJSONObject("request")?.optJSONObject("options")?.optJSONObject("selection")
                Attempt(job.getString("id"), status.getString("state"), if (status.optString("state") == "succeeded") CompanionProcessing.raw(job) else "",
                    if (job.has("dictionary_text")) job.getString("dictionary_text") else null,
                    if (job.has("cleaned_text")) job.getString("cleaned_text") else null,
                    selection?.optString("model") ?: "Unknown model", job.optString("execution_location", if (selection?.optString("provider") == "phone") "phone" else "server"),
                    status.optJSONObject("error")?.optString("message"),
                    // This attempt's own measured stop-to-result. Attempts without it stay unmeasured, never zero.
                    job.optJSONObject("client_timing")?.takeIf { it.has("stop_to_result_ms") }?.optLong("stop_to_result_ms", -1)?.takeIf { it >= 0 },
                    edits(job), job.optJSONObject("polish_outcome")?.optBoolean("original_used") == true,
                    job.optJSONObject("polish_delivery")?.optBoolean("original_used") == true && job.getJSONObject("polish_delivery").optBoolean("pasted"),
                    job.optJSONObject("client_timing")?.takeIf { t -> listOf("silence_input_bytes", "silence_output_bytes", "silence_removed_ms", "silence_prepare_ms").all { t.has(it) } }?.let { t ->
                        AudioPreparation(t.getLong("silence_input_bytes"), t.getLong("silence_output_bytes"), t.getLong("silence_removed_ms"), t.getLong("silence_prepare_ms"))
                    })
            }
            val timing = item.optJSONObject("last_timing")
            val latency = timing?.takeIf { it.has("stop_to_result_ms") }?.optLong("stop_to_result_ms", -1)?.takeIf { it >= 0 }
            return Dictation(item.getString("id"), item.optLong("created_at_ms"), item.optLong("duration_ms"), item.optString("capture_state"), item.has("pending"), item.has("pending_postprocess"), attempts, latency, delivery, item.optString("last_status"), item.optLong("byte_length") > 0,
                item.optJSONObject("historical_source")?.optString("session_id"), item.optJSONObject("historical_source")?.optString("job_id"))
        }
    }
}

/**
 * One model's record over the selected range. [attempts] counts every attempt, including
 * ones still unfinished; [finished] counts only terminal ones, so the rate never treats
 * pending work as a failure. [timings] holds measured runs only: unmeasured work is absent,
 * not zero, and an empty list reports as unavailable.
 */
data class ModelUse(val key: String, val label: String, val detail: String, val attempts: Int, val successes: Int, val finished: Int, val timings: List<Long>) {
    val rate: Int? get() = if (finished == 0) null else Math.round(successes * 100f / finished)
    val median: Long? get() = WriterStatistics.percentile(timings, 0.5)
    val p90: Long? get() = WriterStatistics.percentile(timings, 0.9)
}
data class Statistics(val sessions: Int, val words: Int, val duration: Long, val pace: Int?, val successes: Int, val failures: Int, val retries: Int, val medianLatency: Long?, val days: List<Pair<LocalDate, Int>>,
    val speechModels: List<ModelUse>, val textModels: List<ModelUse>, val modelDays: List<Pair<LocalDate, Map<String, Int>>>, val originalsUsed: Int = 0, val originalsInserted: Int = 0, val audioPreparation: List<AudioPreparation> = emptyList())
object WriterStatistics {
    /** Nearest-rank percentile over measured values only, matching the desktop statistics page. */
    fun percentile(values: List<Long>, fraction: Double): Long? {
        val sorted = values.sorted()
        if (sorted.isEmpty()) return null
        return sorted[Math.ceil(sorted.size * fraction).toInt().coerceIn(1, sorted.size) - 1]
    }
    // Letters/numbers with internal apostrophes; Hebrew combining marks stay attached to their word.
    fun words(text: String) = Regex("[\\p{L}\\p{N}][\\p{L}\\p{N}\\p{M}]*(?:['’][\\p{L}\\p{N}\\p{M}]+)*").findAll(text).count()
    fun compute(records: List<Dictation>, range: Int, today: LocalDate = LocalDate.now(), zone: ZoneId = ZoneId.systemDefault()): Statistics {
        fun day(row: Dictation) = Instant.ofEpochMilli(row.created).atZone(zone).toLocalDate()
        val selected = records.distinctBy { it.id }.filter { range == 0 || !day(it).isBefore(today.minusDays(range.toLong() - 1)) }.filter { !day(it).isAfter(today) }
        // Attempts and text edits count on their recording's day, as retries do elsewhere on this page.
        val speech = selected.flatMap { row -> row.attempts.map { day(row) to it } }.distinctBy { it.second.id }
        val edits = speech.flatMap { (date, job) -> job.edits.map { date to it } }
        val attempts = speech.map { it.second }
        val paced = selected.filter { it.duration > 0 && it.latest != null }
        val time = paced.sumOf { it.duration }
        val latencies = selected.mapNotNull { it.latency }.sorted()
        val chartDays = if (range == 7) 7 else 30
        fun speechKey(row: Attempt) = "speech:${row.model} · ${row.location}"
        fun textKey(row: TextEdit) = "text:${row.provider}/${row.model}"
        val order = compareByDescending<ModelUse> { it.attempts }.thenBy { it.key }
        val speechModels = attempts.groupBy(::speechKey).map { (key, group) ->
            val succeeded = group.filter { it.state == "succeeded" }
            ModelUse(key, group.first().model, group.first().location, group.size, succeeded.size,
                group.count { it.state == "succeeded" || it.state == "failed" }, succeeded.mapNotNull { it.latency })
        }.sortedWith(order)
        val textModels = edits.map { it.second }.groupBy(::textKey).map { (key, group) ->
            val succeeded = group.filter { it.succeeded }
            ModelUse(key, group.first().model, group.first().provider, group.size, succeeded.size,
                group.count { it.succeeded || it.failed }, succeeded.mapNotNull { it.elapsed })
        }.sortedWith(order)
        return Statistics(selected.size, selected.sumOf { words(it.latest?.raw.orEmpty()) }, selected.sumOf { it.duration.coerceAtLeast(0) },
            if (time > 0) (paced.sumOf { words(it.latest!!.raw) } * 60000L / time).toInt() else null,
            attempts.count { it.state == "succeeded" }, attempts.count { it.state == "failed" }, selected.sumOf { (it.attempts.distinctBy { a -> a.id }.size - 1).coerceAtLeast(0) },
            if (latencies.isEmpty()) null else latencies[(latencies.size - 1) / 2].let { lower -> lower + (latencies[latencies.size / 2] - lower) / 2 },
            (chartDays - 1 downTo 0).map { offset -> val date = today.minusDays(offset.toLong()); date to selected.filter { day(it) == date }.sumOf { words(it.latest?.raw.orEmpty()) } },
            speechModels, textModels,
            (chartDays - 1 downTo 0).map { offset ->
                val date = today.minusDays(offset.toLong())
                date to (speech.filter { it.first == date }.groupingBy { speechKey(it.second) }.eachCount() +
                    edits.filter { it.first == date }.groupingBy { textKey(it.second) }.eachCount())
            }, attempts.count { it.originalUsed }, attempts.count { it.originalDelivered }, attempts.mapNotNull { it.audioPreparation })
    }
}

data class SavedDelivery(val token: String, val text: String, val state: String, val fixture: Boolean)

data class WriterState(val deliveries: List<SavedDelivery> = emptyList(), val records: List<Dictation> = emptyList(), val capture: String = "idle", val processing: Boolean = false, val elapsed: Long = 0, val message: String? = null, val actionBusy: Boolean = false, val transfer: String? = null, val revision: Int = 0, val historyNext: SessionStore.Cursor? = null, val statisticsRecords: List<Dictation> = emptyList())

class WriterViewModel(application: Application) : AndroidViewModel(application) {
    val context: Context = application
    val manager = SessionManager.get(context)
    val appearance = AppearancePreferences(context)
    private val mutable = MutableStateFlow(WriterState())
    val state = mutable.asStateFlow()
    private var transferJob: Job? = null
    @Volatile private var cancelTransfer = false
    private var captureJob: Job? = null
    private var refreshJob: Job? = null
    private var messageJob: Job? = null
    private val messageGeneration = java.util.concurrent.atomic.AtomicLong()
    private val actionMessageScope = ThreadLocal<Long?>()
    @Volatile private var messageScope = 0L
    fun clearScreenMessage() { messageScope++; notify(null) }
    fun observe(visible: Boolean) {
        refreshJob?.cancel(); refreshJob = null
        captureJob?.cancel(); captureJob = null
        if (visible) {
            Distribution.onVisible(context)
            Account.onVisible(context)
            captureJob = viewModelScope.launch {
                while (isActive) {
                    mutable.update { it.copy(capture = if (CaptureService.isActive()) CaptureService.state else "idle", processing = manager.isBusy(), elapsed = if (CaptureService.isActive()) (SystemClock.elapsedRealtime() - CaptureService.startedAtElapsed).coerceAtLeast(0) else 0) }
                    delay(50)
                }
            }
            refreshJob = viewModelScope.launch {
                while (isActive) {
                    try { refresh() } catch (error: Exception) {
                        if (error is CancellationException) throw error
                        notify("Some saved records could not be read. Their audio is retained.", true)
                    }
                    delay(650)
                }
            }
        }
    }
    private val historyMutex = kotlinx.coroutines.sync.Mutex()
    private var listedRevision = -1L
    private var historyRoute = "Home"
    private var statisticsRevision = -1L
    private var historyQuery = ""
    private fun dictation(item: JSONObject): Dictation {
        val delivery = if (item.has("original_target")) runCatching { DeliveryStore(java.io.File(context.filesDir, "deliveries")).load(item.getString("original_target")).optString("state") }.getOrNull() else null
        return Dictation.from(item, delivery).copy(audioAvailable = manager.store.audio(item).isFile)
    }
    fun historyRoute(route: String) {
        historyRoute = route
        if (route != "History") historyQuery = ""
        listedRevision = -1L
        viewModelScope.launch { refresh() }
    }
    fun searchHistory(query: String) {
        historyQuery = query; listedRevision = -1L
        viewModelScope.launch { refresh() }
    }
    fun loadMoreHistory() {
        val cursor = mutable.value.historyNext ?: return
        val query = historyQuery
        viewModelScope.launch { historyMutex.lock()
            try {
            if (query != historyQuery || cursor != mutable.value.historyNext) return@launch
            val page = withContext(Dispatchers.IO) { manager.store.page(cursor, query = query) }
            val rows = withContext(Dispatchers.IO) { page.records.map(::dictation) }
            mutable.update { it.copy(records = (it.records + rows).distinctBy { row -> row.id }, historyNext = page.next) }
            } finally { historyMutex.unlock() }
        }
    }
    suspend fun refresh() {
        historyMutex.lock()
        try {
        var next = mutable.value.historyNext
        val records = if (listedRevision == manager.store.revision) mutable.value.records else withContext(Dispatchers.IO) {
            val page = manager.store.page(query = historyQuery)
            next = page.next
            listedRevision = page.revision
            val items = page.records.toMutableList()
            if (historyRoute.startsWith("History/") && historyRoute.substringAfter("History/") !in setOf("Synced text", "Text to recover")) {
                val id = historyRoute.substringAfter("History/")
                if (items.none { it.optString("id") == id }) runCatching { manager.store.load(id) }.getOrNull()?.takeIf { it.optString("capture_state") != "discarded" }?.let(items::add)
            }
            items.map { item ->
                val delivery = if (item.has("original_target")) runCatching { DeliveryStore(java.io.File(context.filesDir, "deliveries")).load(item.getString("original_target")).optString("state") }.getOrNull() else null
                Dictation.from(item, delivery)
            }
        }
        if (manager.store.restoreFailures > 0 && mutable.value.message == null) {
            notify("Some records or cached copies need recovery or deletion retry. Finish active work and reopen History; deletion is not complete.", true)
        }
        // Recovery receipts are read only when their own page is opened.
        val deliveries = if (historyRoute == "History/Text to recover") withContext(Dispatchers.IO) {
            DeliveryStore(java.io.File(context.filesDir, "deliveries")).all().filter { it.has("text") }.map {
                SavedDelivery(it.getString("token"), it.getString("text"), it.optString("state"), it.optBoolean("fixture"))
            }
        } else mutable.value.deliveries
        val statistics = if (historyRoute == "Statistics" && statisticsRevision != manager.store.revision) withContext(Dispatchers.IO) {
            manager.store.allIndexed().map(::dictation).also { statisticsRevision = manager.store.revision }
        } else mutable.value.statisticsRecords
        mutable.update { it.copy(records = records, statisticsRecords = statistics, historyNext = next, deliveries = deliveries, capture = if (CaptureService.isActive()) CaptureService.state else "idle", processing = manager.isBusy(), elapsed = if (CaptureService.isActive()) (SystemClock.elapsedRealtime() - CaptureService.startedAtElapsed).coerceAtLeast(0) else 0) }
        } finally { historyMutex.unlock() }
    }
    fun notify(message: String?, persistent: Boolean = false) {
        val scope = actionMessageScope.get()
        // Serialize both publication and expiry with navigation/capture on the main thread.
        viewModelScope.launch(Dispatchers.Main.immediate) {
            if (scope != null && scope != messageScope) return@launch
            val generation = messageGeneration.incrementAndGet()
            messageJob?.cancel()
            mutable.update { it.copy(message = message, revision = it.revision + 1) }
            if (message != null && !persistent) messageJob = viewModelScope.launch {
                delay(4500)
                if (messageGeneration.get() == generation) mutable.update { it.copy(message = null) }
            }
        }
    }
    fun action(block: suspend () -> Unit) {
        if (mutable.value.actionBusy) return
        mutable.update { it.copy(actionBusy = true, message = null) }
        val scope = messageScope
        messageGeneration.incrementAndGet(); messageJob?.cancel()
        viewModelScope.launch {
            try { withContext(Dispatchers.IO + actionMessageScope.asContextElement(scope)) { block() }; refresh() }
            catch (error: Exception) { if (error is CancellationException) throw error; if (scope == messageScope) notify(GeminiClient.explain(error), true) }
            finally { mutable.update { it.copy(actionBusy = false, revision = it.revision + 1) } }
        }
    }
    fun start() { clearScreenMessage(); if (!CaptureService.start(context, "app")) notify(CaptureService.message, true) }
    fun finish() = CaptureService.stop(context, false)
    fun cancelRecording() = CaptureService.stop(context, true)
    fun retry(id: String) = action { DictationChoice(context).prepare(manager.store, manager.store.load(id)); manager.submit(id, 0) }
    fun resume(id: String) = action { manager.submit(id, 0) }
    fun delete(id: String) = action {
        check(!manager.isBusy() && !CaptureService.isActive()) { "Finish current work before deleting a dictation." }
        ReprocessBatch.get(context).forgetSources(setOf(id))
        manager.store.deleteSaved(id)
        notify("Dictation and $PRODUCT_NAME’s cached copies deleted from this phone.")
    }
    fun cleanup(id: String, job: String) = action {
        val prefs = CompanionPreferences(context); val choice = prefs.cleanup()
        check(choice != "off") { "Choose a cleanup model in Settings first." }
        DirectCloudProcessing(context, manager.store).prepare(id, job, JSONObject().put("provider", choice.substringBefore('/'))
            .put("model", choice.substringAfter('/')).put("reasoning", prefs.cleanupReasoning()))
        manager.submit(id, 0)
    }
    /** Bulk erase from Storage & about. Anything still recording or waiting for a result is kept and reported. */
    fun deleteAllDictations() = action {
        check(!manager.isBusy() && !CaptureService.isActive()) { "Finish current work before deleting every dictation." }
        val kept = manager.store.deleteAllSaved()
        if (kept == 0) ReprocessBatch.get(context).list().forEach { ReprocessBatch.get(context).delete(it.getString("id")) }
        notify(if (kept == 0) "Saved dictations and $PRODUCT_NAME’s cached copies deleted from this phone." else "Deleted saved dictations. $kept kept: still recording or waiting for a result.")
    }
    fun replacePending(id: String) = action { DirectCloud(context).replacePending(manager.store, id); manager.submit(id, 0) }
    fun replaceCleanup(id: String) = action {
        val prefs = CompanionPreferences(context); val choice = prefs.cleanup()
        val selection = if (choice == "off") null else JSONObject().put("provider", choice.substringBefore('/'))
            .put("model", choice.substringAfter('/')).put("reasoning", prefs.cleanupReasoning())
        DirectCloudProcessing(context, manager.store).replacePending(id, selection); manager.submit(id, 0)
    }
    fun configureCloud(provider: String, model: String) = action {
        val cloud = DirectCloud(context, provider)
        require(CloudProviders.get(provider).speech.any { it.id == model })
        cloud.saveSelection(model, DictationChoice(context).language())
        DictationChoice(context).useCloud(provider)
        notify("${CloudProviders.get(provider).name} selected. Try a dictation to check recognition.")
    }
    fun useCloudEngine(provider: String) = action {
        DictationChoice(context).useCloud(provider)
        notify("New dictations use ${DictationChoice(context).engineLabel()}.")
    }
    fun usePhoneEngine(model: String) = action {
        DictationChoice(context).usePhone(model)
        notify("New dictations use ${DictationChoice(context).engineLabel()}. No automatic cloud fallback.")
    }
    fun transfer(id: String, uri: Uri? = null) {
        if (transferJob?.isActive == true) return
        cancelTransfer = false
        val scope = messageScope
        transferJob = viewModelScope.launch {
            mutable.update { it.copy(transfer = "Preparing model…") }
            try {
                withContext(Dispatchers.IO) {
                    val models = PhoneModels(context)
                    val progress: (Long, Long) -> Unit = { current, total -> mutable.update { it.copy(transfer = "${current * 100 / total}% · ${current / 1048576} / ${total / 1048576} MB") } }
                    if (uri == null) models.download(id, { cancelTransfer }, progress)
                    else context.contentResolver.openInputStream(uri)!!.use { models.import(id, it, { cancelTransfer }, progress) }
                }
                if (scope == messageScope) notify("Model verified and installed.")
            } catch (error: Exception) { if (scope == messageScope) notify(error.message ?: "Transfer paused. Resume to continue.", true) }
            finally { mutable.update { it.copy(transfer = null, revision = it.revision + 1) } }
        }
    }
    fun pauseTransfer() { cancelTransfer = true }
    override fun onCleared() { cancelTransfer = true; super.onCleared() }
}
