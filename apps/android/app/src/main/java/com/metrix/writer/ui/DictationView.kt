package com.metrix.writer.ui

import com.metrix.writer.PRODUCT_NAME
import com.metrix.writer.CloudProviders
import com.metrix.writer.ProcessingStage
import com.metrix.writer.ProcessingView

/*
 * What the renewed screens say about one dictation (MW-ANDROID-02 spec, "State mapping").
 * Everything here is derived from the saved session, its delivery ledger and the MW-ANDROID-01
 * processing view; nothing is guessed. Home, History, the entry and the floating control all use
 * these words, so a state reads the same everywhere.
 */

/** The text versions a person can choose between. Labels are the version chip names. */
enum class TextKind(val label: String) { POLISHED("Polished"), CORRECTED("Corrected"), ORIGINAL("Original") }

/** One readable text. [earlier] marks a polished text that a later polish replaced. */
data class TextVersion(val attempt: Attempt, val kind: TextKind, val text: String, val earlier: Boolean = false) {
    val key get() = "${attempt.id}:${kind.name}${if (earlier) ":" + text.hashCode() else ""}"
    val label get() = if (earlier) "Earlier polished" else kind.label
}

/** Where speech ran, in words: "On this phone" or "Cloud · Gemini". */
fun Attempt.whereLabel(): String = when {
    location == "phone" || provider == "phone" -> "On this phone"
    location == "server" -> "Writer server"
    else -> "Cloud" + (runCatching { " · " + CloudProviders.get(provider).name }.getOrNull() ?: "")
}

/**
 * The versions of [this] attempt that exist and differ, best first: Polished, Corrected, Original.
 * Earlier polished texts follow only when [withEarlier]; they stay reachable but are never on the page by default.
 */
fun Attempt.textVersions(withEarlier: Boolean = false): List<TextVersion> {
    if (state != "succeeded") return emptyList()
    val list = mutableListOf<TextVersion>()
    // Identical texts collapse into the better-named version, so a chip never repeats the same words.
    fun add(kind: TextKind, text: String?, earlier: Boolean = false) {
        if (!text.isNullOrBlank() && list.none { it.text == text }) list += TextVersion(this, kind, text, earlier)
    }
    add(TextKind.POLISHED, cleaned); add(TextKind.CORRECTED, dictionary); add(TextKind.ORIGINAL, raw)
    if (withEarlier) earlierPolished.asReversed().forEach { add(TextKind.POLISHED, it, earlier = true) }
    return list
}

/** The text an attempt holds for [kind]: Corrected falls back to the original when no correction applied. */
fun Attempt.textOf(kind: TextKind): String? = when (kind) {
    TextKind.POLISHED -> cleaned
    TextKind.CORRECTED -> dictionary ?: raw
    TextKind.ORIGINAL -> raw
}

/** The kind of text this attempt types (or typed): its delivery choice, after any polish fallback. */
fun Attempt.typedKind(): TextKind = when (deliveryChoice) {
    "cleaned" -> if (originalUsed || cleaned == null) { if (dictionary != null && dictionary != raw) TextKind.CORRECTED else TextKind.ORIGINAL } else TextKind.POLISHED
    "dictionary" -> if (dictionary != null && dictionary != raw) TextKind.CORRECTED else TextKind.ORIGINAL
    else -> TextKind.ORIGINAL
}

/** The shown version holding [attempt]'s text for [kind], whatever chip name it collapsed into. */
fun List<TextVersion>.of(attempt: Attempt, kind: TextKind): TextVersion? = attempt.textOf(kind)?.let { text -> firstOrNull { !it.earlier && it.text == text } }

/** The attempt whose text was inserted, when one was verified. */
val Dictation.insertedAttempt: Attempt? get() = attempts.lastOrNull { it.pasted } ?: latest?.takeIf { ledger?.state == "delivered" || delivery == "delivered" }

/** What was actually typed into another app, verified by read-back; null when nothing was. */
val Dictation.insertedKind: TextKind? get() = insertedAttempt?.typedKind()

/**
 * The version an entry opens on: what was typed when something was; otherwise the current usable
 * wording (polished, unless polishing fell back to the corrected original).
 */
fun Dictation.defaultVersion(): TextVersion? {
    insertedAttempt?.let { attempt -> val versions = attempt.textVersions(); return versions.of(attempt, attempt.typedKind()) ?: versions.firstOrNull() }
    val attempt = latest ?: return null
    return attempt.textVersions().let { versions -> versions.firstOrNull { it.text == attempt.selectedText } ?: versions.firstOrNull() }
}

/** Things a person has to deal with. Each has one inline next action in Needs attention. */
enum class Attention(val tag: String) {
    NOT_INSERTED("Not inserted"), UNCONFIRMED("Check the field"), FAILED("Transcription failed"),
    INTERRUPTED("Recording recovered"), POLISH_INCOMPLETE("Polishing incomplete"), NOT_TRANSCRIBED("Not transcribed")
}

/** True while capture or processing for this dictation is running now. */
val Dictation.busy: Boolean get() = view?.stage?.let { it == ProcessingStage.RECORDING || it.live } == true && view.error == null

fun Dictation.attention(): Attention? {
    val v = view ?: return null
    if (busy) return null
    val handled = ledger?.let { it.handled || it.recovered } == true
    return when (v.error?.code) {
        "delivery_recovery" -> Attention.NOT_INSERTED.takeIf { !handled }
        "delivery_unconfirmed" -> Attention.UNCONFIRMED.takeIf { !handled }
        "interrupted" -> Attention.INTERRUPTED
        "polish_incomplete" -> Attention.POLISH_INCOMPLETE
        "not_transcribed" -> Attention.NOT_TRANSCRIBED
        // Kept on purpose: a stopped dictation, not a problem.
        null, "cancelled", "kept" -> null
        else -> if (v.stage == ProcessingStage.FAILED || v.stage == ProcessingStage.RECOVERY) Attention.FAILED else null
    }
}

/** A content-free ledger reason in plain words. Unknown codes get a general sentence, never a guess. */
fun deliveryReason(code: String): String = when (code) {
    "no_focused_editor", "no_text_editor" -> "No text field was focused when the text was ready."
    "in_app" -> "Recorded in $PRODUCT_NAME with no text field focused. The text is saved here to copy."
    "lease_expired" -> "It took too long to insert automatically, so nothing was typed."
    "insertion_paused" -> "Text insertion is paused until you accept it in Settings → Permissions."
    "insertion_busy" -> "Another insertion was still being checked."
    "connection_missing" -> "The text field stopped accepting input."
    "empty_result" -> "Nothing was recognized."
    "text_limit" -> "The text is longer than that field allows."
    "readback_mismatch", "readback_unavailable", "readback_pending", "commit_failed" -> "Writer could not read the field back to confirm the text."
    "editor_changed" -> "The field changed while Writer was checking it."
    "editor_type_excluded", "private_editor", "node_type_excluded", "node_sensitive" -> "That field is private or excluded, so Writer does not type into it."
    "writer_own_editor" -> "Writer does not type into its own fields."
    else -> "The text field changed while processing."
}

/**
 * One line saying what was typed, separate from which version is on screen (D-3).
 * Returns the line and whether it is a problem.
 */
fun Dictation.deliveryLine(): Pair<String, Status> {
    val ledger = ledger
    val inserted = insertedKind
    return when {
        inserted != null && insertedAttempt?.originalUsed == true && insertedAttempt?.deliveryChoice == "cleaned" -> "${inserted.label} inserted · verified · polishing was not used" to Status.Ok
        inserted != null -> "Inserted · verified · ${inserted.label} version" to Status.Ok
        ledger == null -> (if (latest != null) "Recorded in Writer · ready to copy" else "Recorded in Writer") to Status.Neutral
        ledger.state == "unconfirmed" -> "Check the field · it may already be there" to Status.Warn
        ledger.state == "recovery" && ledger.reasonCode == "in_app" -> "Saved in $PRODUCT_NAME" to Status.Neutral
        ledger.state == "recovery" -> (if (ledger.recovered || ledger.handled) "Not inserted · handled" else "Saved, not inserted") to (if (ledger.recovered || ledger.handled) Status.Neutral else Status.Warn)
        else -> "Waiting to insert" to Status.Neutral
    }
}

/** The short tag a History row shows. */
fun Dictation.rowTag(): Pair<String, Status> {
    attention()?.let { return it.tag to if (it == Attention.UNCONFIRMED || it == Attention.NOT_INSERTED || it == Attention.POLISH_INCOMPLETE) Status.Warn else Status.Err }
    if (busy) return stageWords(view!!.stage) to Status.Neutral
    if (insertedKind != null) return "Inserted" to Status.Ok
    if (stoppedWithAudio) return "Stopped · audio kept" to Status.Neutral
    if (latest != null) return (if (ledger?.state == "recovery") "Not inserted" else "Ready to copy") to Status.Neutral
    return "Saved audio" to Status.Neutral
}

/** Stopped before any text: cancelled processing, or "Keep audio, insert nothing". The audio stays. */
val Dictation.stoppedWithAudio: Boolean get() = latest == null && view != null &&
    (view.stage == ProcessingStage.CANCELLED || view.error?.code == "kept") && view.versions.any { it.kind == com.metrix.writer.VersionKind.AUDIO }

/** A stage in words, the same in the floating control, Home and History. */
fun stageWords(stage: ProcessingStage): String = when (stage) {
    ProcessingStage.RECORDING -> "Recording"
    ProcessingStage.SAVED -> "Saved · waiting"
    ProcessingStage.PREPARING -> "Preparing audio"
    ProcessingStage.RECOGNIZING -> "Transcribing"
    ProcessingStage.RAW_READY -> "Original ready"
    ProcessingStage.POLISHING -> "Polishing"
    ProcessingStage.READY_TO_DELIVER -> "Inserting"
    ProcessingStage.INSERTED -> "Inserted"
    ProcessingStage.RECOVERY -> "Needs attention"
    ProcessingStage.CANCELLED -> "Stopped"
    ProcessingStage.FAILED -> "Failed"
}

/** Elapsed time in the current live stage, ticking from the projection time. Null when unknown. */
fun ProcessingView.elapsedNow(projectedAt: Long, now: Long): Long? = stageElapsedMs?.let { it + (now - projectedAt).coerceAtLeast(0) }

/**
 * Why polishing produced no usable text, from the saved outcome reason (G-1). Null when polishing
 * was not attempted or succeeded. The reason codes are DirectCloudProcessing's.
 */
fun Attempt.polishNotice(): Pair<String, String>? {
    val reason = polishReason ?: return null
    if (late == "succeeded") return null // latePolish says it instead.
    val afterwards = if (late == "failed") "Polishing kept going afterwards but did not produce a usable version." else "A polished version that arrives later is saved here and never typed."
    return when (reason) {
        "user_skipped" -> "Original inserted · you skipped polishing" to afterwards
        "provider_timeout" -> "Original kept · polishing took longer than your wait limit" to afterwards
        "invalid_provider_response" -> "Polishing not used" to "The polished text changed protected words, numbers or meaning, so Writer kept your original."
        "rate_limited" -> "Polishing failed" to "The polishing provider is busy (rate limited). Your original is kept."
        "authentication_failed" -> "Polishing failed" to "The polishing provider rejected the API key. Check it in Settings → API keys."
        "provider_unavailable" -> "Polishing failed" to "No polishing provider is set up or reachable. Your original is kept."
        else -> "Polishing failed" to "The polishing provider could not be reached. Your original is kept."
    }
}

/** A polished version that arrived after the original was typed: shown, copyable, never typed (A4). */
val Attempt.latePolish: Boolean get() = cleaned != null && (late == "succeeded" || originalDelivered && !originalUsed)
