package com.metrix.writer

import android.os.SystemClock
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import org.json.JSONArray
import org.json.JSONObject
import java.io.File

/**
 * Processing/view contract (MW-ANDROID-01). One content-free view of a dictation, projected from the
 * existing session sidecar and delivery ledger plus an in-process live hint. No persisted format
 * changes: older sessions project without migration. Ownership and ordering are specified in
 * docs/testing/android-renewal/processing-contract.md.
 */
enum class ProcessingStage(val wire: String, val rank: Int) {
    RECORDING("recording", 0),
    SAVED("saved", 1), // Audio is durable; the request is queued or waiting to be retried.
    PREPARING("preparing", 2),
    RECOGNIZING("recognizing", 3),
    RAW_READY("raw_ready", 4), // A durable original exists; nothing further is in flight.
    POLISHING("polishing", 5),
    READY_TO_DELIVER("ready_to_deliver", 6),
    INSERTED("inserted", 7),
    RECOVERY("recovery", 8),
    CANCELLED("cancelled", 8),
    FAILED("failed", 8);
    /** Only these may come from the live hint; every other stage is a durable fact. */
    val live get() = this in setOf(SAVED, PREPARING, RECOGNIZING, POLISHING, READY_TO_DELIVER)
}

enum class ProcessingAction(val wire: String) {
    NONE("none"), RETRY_SAVED_AUDIO("retry_saved_audio"), RETRY_WHEN_ONLINE("retry_when_online"),
    CHOOSE_SMALLER_MODEL("choose_smaller_model"), OPEN_MODELS("open_models"), OPEN_CLOUD_SETTINGS("open_cloud_settings"),
    RETRY_POLISH("retry_polish"), COPY_FROM_HISTORY("copy_from_history"), CHECK_FIELD_THEN_COPY("check_field_then_copy"),
    TRANSCRIBE_SAVED_AUDIO("transcribe_saved_audio")
}

/** [code] and [action] are stable and loggable; [message] is the existing human text and is never logged. */
data class ProcessingError(val code: String, val action: ProcessingAction, val retryable: Boolean, val message: String)

enum class VersionKind(val wire: String) { AUDIO("audio"), RAW("raw"), DICTIONARY("dictionary"), POLISHED("polished") }
data class ProcessingVersion(val kind: VersionKind, val attemptId: String?)

enum class DeliveryState(val wire: String) {
    NOT_REQUESTED("not_requested"), AWAITING_RESULT("awaiting_result"), UNCONFIRMED("unconfirmed"),
    DELIVERED("delivered"), RECOVERY("recovery")
}
/** Named apart from DeliveryStore's [DeliveryOutcome] codes (MW-ANDROID-03), which merged under the same name. */
data class DeliveryView(val state: DeliveryState, val originalUsed: Boolean,
    /** raw, dictionary or polished: the version handed to insertion; null before delivery was decided. */
    val version: String? = null)

/**
 * Content-free polishing state of the latest attempt. [failure] is one of timeout, unavailable or
 * validation_rejected; [late] means a polished version arrived after the original was used.
 */
/** [rejection] is the content-free validation check that refused the polish, when one did. */
data class PolishView(val state: String, val failure: String?, val late: Boolean, val rejection: String? = null) {
    fun toJson(): JSONObject = JSONObject().put("state", state).put("failure", failure ?: JSONObject.NULL).put("late", late)
        .put("rejection", rejection ?: JSONObject.NULL)
}

data class ProcessingView(
    val sessionId: String,
    val attemptId: String?,
    val stage: ProcessingStage,
    /** Null means indeterminate. Only chunked recognition reports measured progress (finished/total ranges); none is invented. */
    val progress: Float?,
    /** Elapsed time in the current live stage, when known from this boot. */
    val stageElapsedMs: Long?,
    /** Completed stage durations, the same names and bounds as the latency export. */
    val timingsMs: Map<String, Long>,
    val warm: String,
    val error: ProcessingError?,
    val versions: List<ProcessingVersion>,
    val delivery: DeliveryView,
    val polish: PolishView = PolishView("not_requested", null, false),
) {
    /** Content-free: no transcript, editor text, key or provider message. */
    fun toJson(): JSONObject = JSONObject().put("session_id", sessionId).put("attempt_id", attemptId ?: JSONObject.NULL)
        .put("stage", stage.wire).put("progress", progress ?: JSONObject.NULL).put("stage_elapsed_ms", stageElapsedMs ?: JSONObject.NULL)
        .put("timings_ms", JSONObject(timingsMs)).put("warm", warm)
        .put("error", error?.let { JSONObject().put("code", it.code).put("action", it.action.wire).put("retryable", it.retryable) } ?: JSONObject.NULL)
        .put("versions", JSONArray(versions.map { JSONObject().put("kind", it.kind.wire).put("attempt_id", it.attemptId ?: JSONObject.NULL) }))
        .put("delivery", JSONObject().put("state", delivery.state.wire).put("original_used", delivery.originalUsed).put("version", delivery.version ?: JSONObject.NULL))
        .put("polish", polish.toJson())
}

/**
 * In-process hint for work the sidecar cannot show while it runs. Per attempt it only moves forward;
 * a different attempt replaces it; [finish] clears it so the durable projection decides the outcome.
 * Process death loses it by design: unfinished work then projects as recovery, never as running.
 */
object ProcessingEvents {
    /** [progress] is a measured fraction (finished/total chunked ranges) or null when not countable. */
    data class Event(val sessionId: String, val attemptId: String, val stage: ProcessingStage, val atElapsedMs: Long, val sequence: Long, val progress: Float? = null)
    private val current = HashMap<String, Event>()
    private var sequence = 0L
    private val revisionFlow = MutableStateFlow(0L)
    /** Changes on every accepted event; observers re-project. */
    val revision: StateFlow<Long> = revisionFlow.asStateFlow()

    /** Returns false for an out-of-order or non-live event, which is ignored. */
    @Synchronized fun emit(sessionId: String, attemptId: String, stage: ProcessingStage, at: Long = SystemClock.elapsedRealtime()): Boolean {
        if (!stage.live) return false
        val previous = current[sessionId]
        if (previous != null && previous.attemptId == attemptId && stage.rank <= previous.stage.rank) return false
        current[sessionId] = Event(sessionId, attemptId, stage, at, ++sequence)
        revisionFlow.value = sequence
        return true
    }
    /** Updates measured progress of the current live stage of the same attempt; never moves the stage. */
    @Synchronized fun progress(sessionId: String, attemptId: String, done: Int, total: Int) {
        val previous = current[sessionId] ?: return
        if (previous.attemptId != attemptId || total <= 0) return
        current[sessionId] = previous.copy(progress = (done.toFloat() / total).coerceIn(0f, 1f), sequence = ++sequence)
        revisionFlow.value = sequence
    }
    @Synchronized fun finish(sessionId: String, attemptId: String? = null) {
        val previous = current[sessionId] ?: return
        if (attemptId != null && previous.attemptId != attemptId) return
        current.remove(sessionId); revisionFlow.value = ++sequence
    }
    @Synchronized fun get(sessionId: String): Event? = current[sessionId]
    @Synchronized internal fun reset() { current.clear(); revisionFlow.value = ++sequence }
}

object ProcessingState {
    private data class Durable(val stage: ProcessingStage, val attemptId: String?, val error: ProcessingError? = null,
        /** True when the stage only reflects that no live work is known (it may still be running). */
        val provisional: Boolean = false,
        /** Furthest in-flight stage the sidecar itself proves (engines persist some markers while running). */
        val reached: ProcessingStage? = null)

    private fun key(job: JSONObject?) = job?.optJSONObject("request")?.optString("idempotency_key")?.takeIf { it.isNotEmpty() }

    /** Prefers an explicit error code; otherwise classifies today's fixed engine messages. */
    fun classify(error: JSONObject?, fallback: String): ProcessingError {
        val message = error?.optString("message")?.takeIf { it.isNotEmpty() } ?: fallback
        val explicit = error?.optString("code")?.takeIf { it.isNotEmpty() }
        val text = message.lowercase()
        val (code, action) = when {
            explicit != null -> explicit to when (explicit) {
                "cancelled" -> ProcessingAction.NONE
                "cloud_credential" -> ProcessingAction.OPEN_CLOUD_SETTINGS
                "network", "waiting_for_network" -> ProcessingAction.RETRY_WHEN_ONLINE
                "model_memory" -> ProcessingAction.CHOOSE_SMALLER_MODEL
                "model_unavailable" -> ProcessingAction.OPEN_MODELS
                else -> ProcessingAction.RETRY_SAVED_AUDIO
            }
            "cancel" in text -> "cancelled" to ProcessingAction.NONE
            "memory" in text || "smaller model" in text -> "model_memory" to ProcessingAction.CHOOSE_SMALLER_MODEL
            "model" in text && ("unavailable" in text || "could not load" in text || "version" in text || "reinstall" in text) -> "model_unavailable" to ProcessingAction.OPEN_MODELS
            "key" in text || "credential" in text || "unauthori" in text || "401" in text || "403" in text -> "cloud_credential" to ProcessingAction.OPEN_CLOUD_SETTINGS
            "network" in text || "offline" in text || "connect" in text || "timed out" in text || "timeout" in text -> "network" to ProcessingAction.RETRY_WHEN_ONLINE
            else -> "recognition_failed" to ProcessingAction.RETRY_SAVED_AUDIO
        }
        return ProcessingError(code, action, action != ProcessingAction.NONE, message)
    }

    private fun durable(item: JSONObject, ledger: JSONObject?): Durable {
        when (item.optString("capture_state")) {
            "capturing" -> return Durable(ProcessingStage.RECORDING, null)
            "discarded" -> return Durable(ProcessingStage.CANCELLED, null)
        }
        val attempts = item.optJSONArray("attempts") ?: JSONArray()
        val last = if (attempts.length() > 0) attempts.getJSONObject(attempts.length() - 1) else null
        item.optJSONObject("pending")?.let { pending ->
            val attempt = pending.optString("idempotency_key").takeIf { it.isNotEmpty() }
            val timing = item.optJSONObject("pending_timing")
            if (item.has("pending_cloud_error") || item.optBoolean("pending_latency_failed"))
                return Durable(ProcessingStage.RECOVERY, attempt, classify(JSONObject().put("message", item.optString("pending_cloud_error"))
                    .apply { item.optString("pending_cloud_error_code").takeIf { it.isNotEmpty() }?.let { put("code", it) } },
                    item.optString("last_status", "Request failed; saved audio retained")))
            // Queued again automatically for a connection; nothing billable repeats (see pipeline.md).
            when (item.optString("pending_automatic_rerun")) {
                "network" -> return Durable(ProcessingStage.SAVED, attempt,
                    ProcessingError("waiting_for_network", ProcessingAction.RETRY_WHEN_ONLINE, true, item.optString("last_status", "Waiting for a connection.")), provisional = true)
                "system_stop" -> return Durable(ProcessingStage.SAVED, attempt, provisional = true)
            }
            val recognizing = timing != null && listOf("upload_started_elapsed_ms", "asr_started_elapsed_ms", "cloud_request_started_elapsed_ms").any(timing::has)
            // Started work with no live owner is interrupted; queued work is shown as saved.
            return if (timing?.has("request_elapsed_ms") == true) Durable(ProcessingStage.RECOVERY, attempt,
                ProcessingError("interrupted", ProcessingAction.RETRY_SAVED_AUDIO, true, item.optString("last_status", "Audio saved. Retry this session.")), provisional = true,
                reached = if (recognizing) ProcessingStage.RECOGNIZING else ProcessingStage.PREPARING)
            else Durable(ProcessingStage.SAVED, attempt, provisional = true)
        }
        if (last == null) return Durable(ProcessingStage.RECOVERY, null,
            ProcessingError(if (item.optJSONObject("capture_completion")?.optBoolean("kept") == true) "kept" else "not_transcribed",
                ProcessingAction.TRANSCRIBE_SAVED_AUDIO, true, item.optString("last_status", "Audio saved.")), provisional = true)
        val attempt = key(last)
        val status = last.optJSONObject("status")
        if (status?.optString("state") != "succeeded") {
            val error = classify(status?.optJSONObject("error"), "Transcription failed; saved audio retained")
            return Durable(if (error.code == "cancelled") ProcessingStage.CANCELLED else ProcessingStage.FAILED, attempt, error)
        }
        if (last.optJSONObject("polish_delivery")?.optBoolean("pasted") == true || ledger?.optString("state") == "delivered")
            return Durable(ProcessingStage.INSERTED, attempt)
        // Polishing past the insertion budget in this process is still running, not interrupted.
        val polishing = item.optJSONObject("pending_postprocess")?.optBoolean("continuing") == true && DirectCloudProcessing.isContinuing(item.getString("id"))
        if (item.has("pending_postprocess") && !polishing) return Durable(ProcessingStage.RECOVERY, attempt,
            ProcessingError("polish_incomplete", ProcessingAction.RETRY_POLISH, true, item.optString("last_status", "Text processing incomplete; original retained.")), provisional = true)
        return when (ledger?.optString("state")) {
            "recovery" -> Durable(ProcessingStage.RECOVERY, attempt,
                ProcessingError("delivery_recovery", ProcessingAction.COPY_FROM_HISTORY, true, ledger.optString("reason", "Saved. Copy from History.")))
            "unconfirmed" -> Durable(ProcessingStage.RECOVERY, attempt,
                ProcessingError("delivery_unconfirmed", ProcessingAction.CHECK_FIELD_THEN_COPY, true, ledger.optString("reason", "Check the field before copying.")), provisional = true)
            else -> Durable(ProcessingStage.RAW_READY, attempt, provisional = ledger != null)
        }
    }

    fun view(item: JSONObject, ledger: JSONObject?, live: ProcessingEvents.Event?, now: Long = SystemClock.elapsedRealtime()): ProcessingView {
        val id = item.getString("id")
        val persisted = durable(item, ledger)
        val terminal = persisted.stage in setOf(ProcessingStage.INSERTED, ProcessingStage.CANCELLED, ProcessingStage.FAILED, ProcessingStage.RECORDING)
        // Live applies only to the same attempt, never over a durable terminal fact.
        val applies = live != null && live.sessionId == id && live.attemptId == persisted.attemptId && !terminal &&
            (persisted.provisional || live.stage.rank > persisted.stage.rank)
        val stage = if (!applies) persisted.stage
            else persisted.reached?.takeIf { it.rank > live!!.stage.rank } ?: live!!.stage
        val attempts = item.optJSONArray("attempts") ?: JSONArray()
        val last = if (attempts.length() > 0) attempts.getJSONObject(attempts.length() - 1) else null
        val measuredJob = if (item.has("pending")) JSONObject().put("request", item.getJSONObject("pending")).put("client_timing", item.opt("pending_timing"))
            .put("status", JSONObject().put("state", "pending")) else last
        val row = measuredJob?.let { runCatching { LatencyMeasurements.project(item, it) }.getOrNull() }
        val timings = row?.getJSONObject("stages_ms")?.let { s -> s.keys().asSequence().associateWith { s.getLong(it) } } ?: emptyMap()
        val versions = mutableListOf<ProcessingVersion>()
        if (item.optLong("byte_length") > 44 && item.optString("capture_state") != "discarded") versions += ProcessingVersion(VersionKind.AUDIO, null)
        for (i in 0 until attempts.length()) {
            val job = attempts.getJSONObject(i)
            if (job.optJSONObject("status")?.optString("state") != "succeeded") continue
            val attempt = key(job)
            versions += ProcessingVersion(VersionKind.RAW, attempt)
            if (job.has("dictionary_text")) versions += ProcessingVersion(VersionKind.DICTIONARY, attempt)
            if (job.has("cleaned_text")) versions += ProcessingVersion(VersionKind.POLISHED, attempt)
        }
        val delivered = last?.optJSONObject("polish_delivery")?.optBoolean("pasted") == true
        val delivery = when {
            delivered || ledger?.optString("state") == "delivered" -> DeliveryState.DELIVERED
            ledger == null -> DeliveryState.NOT_REQUESTED
            ledger.optString("state") == "recovery" -> DeliveryState.RECOVERY
            ledger.optString("state") == "unconfirmed" -> DeliveryState.UNCONFIRMED
            else -> DeliveryState.AWAITING_RESULT
        }
        val originalUsed = ledger?.optBoolean("original_used") == true || last?.optJSONObject("polish_delivery")?.optBoolean("original_used") == true
        val polish = polishView(item, last)
        return ProcessingView(id, persisted.attemptId, stage, if (applies) live.progress else null,
            if (applies) (now - live!!.atElapsedMs).takeIf { it >= 0 } else null,
            timings, row?.optString("warm") ?: "unknown", if (applies) null else persisted.error, versions,
            DeliveryView(delivery, originalUsed, ledger?.optString("version")?.takeIf { it in setOf("raw", "dictionary", "polished") }), polish)
    }

    private fun polishView(item: JSONObject, last: JSONObject?): PolishView {
        if (last == null || last.optJSONObject("status")?.optString("state") != "succeeded" || item.has("pending")) return PolishView("not_requested", null, false)
        val requested = last.optJSONObject("request")?.optJSONObject("options")?.optJSONObject("cleanup")?.optString("mode", "off") != "off"
        val late = last.optJSONObject("late_polish_outcome")
        val outcome = late ?: last.optJSONObject("polish_outcome")
        fun failure(o: JSONObject?) = o?.optString("failure")?.takeIf { it.isNotEmpty() && it != "null" }
            ?: o?.optString("reason")?.takeIf { it.isNotEmpty() && it != "null" }?.let(DirectCloudProcessing::failure)
        return when {
            item.optJSONObject("pending_postprocess")?.let { it.optString("job_id") == last.optString("id") } == true ->
                if (DirectCloudProcessing.isContinuing(item.getString("id"))) PolishView("running", null, false) else PolishView("incomplete", null, false)
            outcome?.optJSONObject("selected_model") != null -> PolishView("succeeded", null, late != null)
            outcome != null -> PolishView("failed", failure(outcome), late != null, (0 until (outcome.optJSONArray("model_attempts")?.length() ?: 0))
                .firstNotNullOfOrNull { outcome.getJSONArray("model_attempts").getJSONObject(it).optString("rejection").takeIf { r -> r.isNotEmpty() && r != "null" } })
            last.has("cleaned_text") -> PolishView("succeeded", null, false) // Receipts older than polish outcomes.
            requested -> PolishView("not_started", null, false)
            else -> PolishView("not_requested", null, false)
        }
    }

    /** Reads the sidecar, its delivery ledger and the live hint. Throws only if the session cannot be read. */
    fun view(filesDir: File, sessionId: String): ProcessingView {
        val item = SessionStore(File(filesDir, "recordings")).load(sessionId)
        val ledger = item.optString("original_target").takeIf { it.isNotEmpty() }?.let { token ->
            runCatching { DeliveryStore(File(filesDir, "deliveries")).load(token) }.getOrNull()
        }
        return view(item, ledger, ProcessingEvents.get(sessionId))
    }
}
