package com.metrix.writer.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.semantics.*
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import com.metrix.writer.PRODUCT_NAME
import com.metrix.writer.ProcessingStage
import kotlinx.coroutines.delay

/*
 * The floating control (MW-ANDROID-02 "Floating control" table and state mapping). The state is
 * derived purely from capture, the session manager and the tracked session's processing view, so
 * every permutation is testable without an overlay (FloatingStateTest). Nothing here inserts text:
 * automatic insertion stays with SessionManager, and the only manual insert is the explicit
 * "Insert here" / "Insert anyway" action.
 */

/** What the floating control shows. [name] is the content-free label used in overlay diagnostics. */
sealed interface FloatingState {
    val name: String
    /** The polite announcement for this state, or null when it is not announced. */
    val announcement: String? get() = null

    data object Idle : FloatingState { override val name = "idle" }
    /** A saved result still needs attention but its card was collapsed: idle button plus a badge. */
    data class Attention(val sessionId: String, val kind: com.metrix.writer.ui.Attention) : FloatingState { override val name = "attention" }
    data object Starting : FloatingState { override val name = "starting"; override val announcement = "Starting microphone" }
    data class Recording(val elapsedMs: Long) : FloatingState { override val name = "recording"; override val announcement = "Recording" }
    /** ✕ while recording asks first; nothing is discarded by a single tap. */
    data class CancelConfirm(val elapsedMs: Long) : FloatingState { override val name = "cancel" }
    data object Saving : FloatingState { override val name = "saving"; override val announcement = "Saving recording" }
    /**
     * Saved, preparing, recognizing or inserting. [elapsedMs] is time in the live stage when known;
     * [progress] is shown only when the engine measured it (never invented).
     */
    data class Working(val stage: ProcessingStage?, val elapsedMs: Long?, val phone: Boolean, val progress: Float?) : FloatingState {
        override val name = "processing"
        override val announcement get() = if (stage == ProcessingStage.READY_TO_DELIVER) "Inserting" else "Transcribing"
    }
    /**
     * Polishing, with the wait limit after which the original is typed instead. [insertable]: the
     * original is still waiting to be typed, so "Insert original now" can end the wait.
     */
    data class Polishing(val elapsedMs: Long?, val limitMs: Long, val insertable: Boolean = false) : FloatingState { override val name = "polishing"; override val announcement = "Polishing" }
    /** Shown only after read-back verification. */
    data class Inserted(val kind: TextKind, val originalUsed: Boolean) : FloatingState {
        override val name = "success"
        override val announcement get() = if (originalUsed) "Original inserted, verified" else "Inserted, ${kind.label.lowercase()}, verified"
    }
    /** Persistent until copied, inserted or marked handled (D-4). */
    data class NotInserted(val sessionId: String, val reason: String, val preview: String?) : FloatingState { override val name = "recovery"; override val announcement = "Saved, not inserted" }
    /** The edit may already be in the field, so there is no primary insert (D-5). */
    data class Unconfirmed(val sessionId: String, val preview: String?) : FloatingState { override val name = "uncertain"; override val announcement = "Check the field" }
    data class Failed(val sessionId: String, val reason: String, val retryable: Boolean) : FloatingState { override val name = "failure"; override val announcement = "Transcription failed, audio saved" }
    /** Recording could not start (for example the microphone is not allowed). Nothing was recorded. */
    data class Blocked(val message: String) : FloatingState { override val name = "blocked"; override val announcement = "Recording did not start" }
}

/** Everything the floating state depends on, read from production state by WriterBubble. */
data class FloatingInput(
    val captureActive: Boolean,
    /** CaptureService.state while active: starting, recording, saving. */
    val capture: String,
    val captureElapsedMs: Long,
    /** SessionManager.state: idle, processing, recovery, failure, verifying, uncertain. */
    val manager: String,
    val managerBusy: Boolean,
    /** The session the control is following (the latest capture), with its projected view. */
    val tracked: Dictation?,
    /** The saved result's text from the delivery ledger, for a one-line preview. */
    val savedText: String? = null,
    val phone: Boolean = false,
    val polishLimitMs: Long = 3000,
    val now: Long = 0,
    /** When the control first saw the tracked session become inserted, or null. */
    val insertedSince: Long? = null,
    /** The person collapsed the card for the tracked session. */
    val collapsed: Boolean = false,
    val confirmingCancel: Boolean = false,
    val blocked: String? = null,
)

object FloatingView {
    /** How long the green "Inserted" pill stays before the control returns to idle. */
    const val INSERTED_MS = 1500L

    fun from(input: FloatingInput): FloatingState {
        input.blocked?.let { return FloatingState.Blocked(it) }
        if (input.captureActive) return when (input.capture) {
            "starting" -> FloatingState.Starting
            "recording" -> if (input.confirmingCancel) FloatingState.CancelConfirm(input.captureElapsedMs) else FloatingState.Recording(input.captureElapsedMs)
            else -> FloatingState.Saving
        }
        val d = input.tracked
        val v = d?.view
        if (d != null && v != null) {
            val elapsed = v.elapsedNow(d.projectedAt, input.now)
            when {
                v.error == null && v.stage in setOf(ProcessingStage.SAVED, ProcessingStage.PREPARING, ProcessingStage.RECOGNIZING, ProcessingStage.READY_TO_DELIVER) ->
                    return FloatingState.Working(v.stage, elapsed, input.phone, v.progress)
                v.error == null && v.stage == ProcessingStage.POLISHING -> return FloatingState.Polishing(elapsed, input.polishLimitMs, d.ledger?.state == "awaiting_result")
                v.stage == ProcessingStage.INSERTED -> {
                    val since = input.insertedSince
                    return if (since != null && input.now - since < INSERTED_MS)
                        FloatingState.Inserted(d.insertedKind ?: TextKind.ORIGINAL, d.insertedAttempt?.originalUsed == true && d.insertedAttempt?.deliveryChoice == "cleaned")
                    else FloatingState.Idle
                }
            }
            when (val attention = d.attention()) {
                Attention.NOT_INSERTED -> return if (input.collapsed) FloatingState.Attention(d.id, attention)
                    else FloatingState.NotInserted(d.id, deliveryReason(d.ledger?.reasonCode.orEmpty()), input.savedText?.preview())
                Attention.UNCONFIRMED -> return if (input.collapsed) FloatingState.Attention(d.id, attention)
                    else FloatingState.Unconfirmed(d.id, input.savedText?.preview())
                Attention.FAILED -> return if (input.collapsed) FloatingState.Attention(d.id, attention)
                    else FloatingState.Failed(d.id, v.error?.message ?: "Transcription failed.", v.error?.retryable != false)
                // Interrupted, not transcribed and unfinished polishing are History's to show: never typed (sys-recreate).
                else -> {}
            }
        }
        if (input.managerBusy || input.manager == "processing") return FloatingState.Working(null, null, input.phone, null)
        return FloatingState.Idle
    }

    private fun String.preview(): String? = lineSequence().map { it.trim() }.firstOrNull { it.isNotEmpty() }
}

/** The control's actions. Each one is an explicit user action; none inserts on its own. */
class FloatingActions(
    val start: () -> Unit = {},
    val stop: () -> Unit = {},
    val requestCancel: () -> Unit = {},
    val continueRecording: () -> Unit = {},
    val discard: () -> Unit = {},
    /** Stop and keep the audio without transcribing or typing anything. */
    val keep: () -> Unit = {},
    /** End the polishing wait and type the original now; polishing continues to a saved version. */
    val insertOriginal: () -> Unit = {},
    val stopProcessing: () -> Unit = {},
    val insertHere: (anyway: Boolean) -> Unit = {},
    val copy: () -> Unit = {},
    val markHandled: () -> Unit = {},
    val open: () -> Unit = {},
    val retry: () -> Unit = {},
    val collapse: () -> Unit = {},
    val expand: () -> Unit = {},
    val dismiss: () -> Unit = {},
    val openSettings: () -> Unit = {},
)

/**
 * The overlay content for [state]. [edge] draws the slim tab used when the keyboard leaves no room
 * (D-9): it never disappears while recording or processing. [maxHeight] and [maxWidth] bound cards so they
 * stay on screen; [message] is the outcome of the last action on a card (for example "Tap the text field…").
 */
@Composable fun FloatingContent(state: FloatingState, size: Int, level: () -> Float, timer: Boolean, edge: Boolean, maxHeight: Dp, maxWidth: Dp = 320.dp, message: String? = null, actions: FloatingActions) {
    var now by remember { mutableLongStateOf(android.os.SystemClock.elapsedRealtime()) }
    val base = remember(state) { android.os.SystemClock.elapsedRealtime() }
    LaunchedEffect(state) { while (true) { now = android.os.SystemClock.elapsedRealtime(); delay(250) } }
    fun ticking(elapsed: Long?) = elapsed?.let { it + (now - base).coerceAtLeast(0) }
    val announce = Modifier.semantics { state.announcement?.let { liveRegion = LiveRegionMode.Polite; stateDescription = it } }
    if (edge && state !is FloatingState.Idle) { EdgeTab(state, actions.expand); return }
    when (state) {
        FloatingState.Idle -> IdleControl(size, actions.start)
        is FloatingState.Attention -> Row(verticalAlignment = Alignment.CenterVertically) {
            IdleControl(size, actions.start)
            IconButton(onClick = actions.expand, modifier = Modifier.size(48.dp).semantics {
                contentDescription = if (state.kind == Attention.FAILED) "Transcription failed; show what happened" else "Saved result not inserted; show it"
            }) { Badge(if (state.kind == Attention.FAILED) Status.Err else Status.Warn) }
        }
        FloatingState.Starting -> Pill(announce) {
            Box(Modifier.size(48.dp), contentAlignment = Alignment.Center) { WriterIcon("mic", tint = MaterialTheme.colorScheme.primary) }
            Text("Starting…", style = MaterialTheme.typography.labelLarge, modifier = Modifier.padding(end = 16.dp))
        }
        is FloatingState.Recording -> Pill(announce) {
            IconButton(onClick = actions.requestCancel, modifier = Modifier.size(48.dp).semantics { contentDescription = "Cancel: keep or discard audio" }) { WriterIcon("close") }
            LeftToRight {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Box(Modifier.width(56.dp).height(48.dp), contentAlignment = Alignment.Center) { LiveWaveform(level) }
                    if (timer) Figures(formatClock(ticking(state.elapsedMs) ?: 0), Modifier.padding(horizontal = 4.dp), color = MaterialTheme.colorScheme.onSurface)
                }
            }
            IconButton(onClick = actions.stop, modifier = Modifier.size(48.dp).semantics { contentDescription = "$PRODUCT_NAME stop recording" }) { WriterIcon("stop", tint = MaterialTheme.colorScheme.primary) }
        }
        is FloatingState.CancelConfirm -> Card(maxHeight, maxWidth, message, "Stop this recording?", null, actions.continueRecording, "Continue recording") {
            Text("Your audio so far is saved. Stop & transcribe turns it into text. Keep audio saves it without transcribing or typing anything. Discard deletes it.", style = MaterialTheme.typography.bodyMedium)
            Buttons {
                Button(onClick = actions.continueRecording, modifier = Modifier.heightIn(min = 48.dp)) { Text("Continue recording") }
                OutlinedButton(onClick = actions.stop, modifier = Modifier.heightIn(min = 48.dp)) { Text("Stop & transcribe") }
                OutlinedButton(onClick = actions.keep, modifier = Modifier.heightIn(min = 48.dp)) { Text("Keep audio, insert nothing") }
                TextButton(onClick = actions.discard, modifier = Modifier.heightIn(min = 48.dp).semantics { contentDescription = "Discard recording; deletes this audio" }) { Text("Discard recording", color = MaterialTheme.colorScheme.error) }
            }
        }
        FloatingState.Saving -> Pill(announce) {
            Box(Modifier.size(48.dp), contentAlignment = Alignment.Center) { CircularProgressIndicator(Modifier.size(20.dp), strokeWidth = 2.dp) }
            Text("Saving…", style = MaterialTheme.typography.labelLarge, modifier = Modifier.padding(end = 16.dp))
        }
        is FloatingState.Working -> Pill(announce) {
            IconButton(onClick = actions.stopProcessing, modifier = Modifier.size(48.dp).semantics { contentDescription = "Stop processing, keep audio" }) { WriterIcon("close") }
            Box(Modifier.size(40.dp), contentAlignment = Alignment.Center) {
                val progress = state.progress
                if (progress != null) CircularProgressIndicator(progress = { progress.coerceIn(0f, 1f) }, modifier = Modifier.size(20.dp), strokeWidth = 2.dp)
                else CircularProgressIndicator(Modifier.size(20.dp), strokeWidth = 2.dp)
            }
            Text(if (state.stage == ProcessingStage.READY_TO_DELIVER) "Inserting" else if (state.stage == ProcessingStage.SAVED) "Saved · waiting" else "Transcribing",
                style = MaterialTheme.typography.labelLarge, maxLines = 1)
            ticking(state.elapsedMs)?.let { LeftToRight { Figures(" · " + formatClock(it), color = MaterialTheme.colorScheme.onSurfaceVariant) } }
            Box(Modifier.size(40.dp).semantics { contentDescription = if (state.phone) "On this phone" else "Cloud" }, contentAlignment = Alignment.Center) {
                WriterIcon(if (state.phone) "phone" else "cloud", Modifier.size(20.dp))
            }
        }
        is FloatingState.Polishing -> Pill(announce) {
            IconButton(onClick = actions.stopProcessing, modifier = Modifier.size(48.dp).semantics { contentDescription = "Stop polishing, keep original" }) { WriterIcon("close") }
            val elapsed = ticking(state.elapsedMs)
            Box(Modifier.size(40.dp).semantics {
                contentDescription = if (elapsed == null) "Waiting up to ${state.limitMs / 1000} seconds for polishing"
                    else "${((state.limitMs - elapsed).coerceAtLeast(0) + 999) / 1000} seconds until the original is typed"
            }, contentAlignment = Alignment.Center) {
                // A countdown of the configured wait limit, not a processing percentage.
                if (elapsed != null) CircularProgressIndicator(progress = { 1f - (elapsed.toFloat() / state.limitMs).coerceIn(0f, 1f) }, modifier = Modifier.size(22.dp), strokeWidth = 3.dp)
                else CircularProgressIndicator(Modifier.size(20.dp), strokeWidth = 2.dp)
            }
            Text("Polishing", style = MaterialTheme.typography.labelLarge, maxLines = 1)
            // The ring already counts down, so the button takes the elapsed time's place.
            if (state.insertable) TextButton(onClick = actions.insertOriginal, modifier = Modifier.heightIn(min = 48.dp).padding(end = 4.dp)
                .semantics { contentDescription = "Insert original now; polishing continues and is only saved" }) { Text("Insert original", maxLines = 1) }
            else elapsed?.let { LeftToRight { Figures(" · " + formatClock(it), Modifier.padding(end = 12.dp), color = MaterialTheme.colorScheme.onSurfaceVariant) } }
        }
        is FloatingState.Inserted -> {
            val c = Status.Ok
            Surface(shape = RoundedCornerShape(28.dp), color = MaterialTheme.colorScheme.surfaceContainerHigh, shadowElevation = 3.dp, modifier = announce) {
                Row(Modifier.heightIn(min = 56.dp).padding(horizontal = 14.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    WriterIcon("check", tint = c.textColor())
                    Text(if (state.originalUsed) "Original inserted · polishing not used" else "Inserted · ${state.kind.label}", color = c.textColor(),
                        style = MaterialTheme.typography.labelLarge, maxLines = 2)
                }
            }
        }
        is FloatingState.NotInserted -> Card(maxHeight, maxWidth, message, "Saved, not inserted", state.reason, actions.collapse, "Hide this card; the result stays saved", announce) {
            state.preview?.let { SpokenText(it, maxLines = 2) }
            Buttons {
                Button(onClick = { actions.insertHere(false) }, modifier = Modifier.heightIn(min = 48.dp).semantics { contentDescription = "Insert into the field focused now" }) {
                    WriterIcon("insert", tint = MaterialTheme.colorScheme.onPrimary); Spacer(Modifier.width(6.dp)); Text("Insert here")
                }
                OutlinedButton(onClick = actions.copy, modifier = Modifier.heightIn(min = 48.dp)) { Text("Copy") }
                TextButton(onClick = actions.open, modifier = Modifier.heightIn(min = 48.dp).semantics { contentDescription = "Open in $PRODUCT_NAME" }) { Text("Open") }
            }
        }
        is FloatingState.Unconfirmed -> Card(maxHeight, maxWidth, message, "Check the field", "It may already be there. Writer could not read the field back, so nothing will be typed again on its own.", actions.collapse, "Hide this card; the result stays saved", announce) {
            state.preview?.let { SpokenText(it, maxLines = 2) }
            Buttons {
                OutlinedButton(onClick = actions.copy, modifier = Modifier.heightIn(min = 48.dp)) { Text("Copy") }
                OutlinedButton(onClick = actions.markHandled, modifier = Modifier.heightIn(min = 48.dp).semantics { contentDescription = "Looks right; mark handled" }) { Text("Looks right") }
                TextButton(onClick = { actions.insertHere(true) }, modifier = Modifier.heightIn(min = 48.dp).semantics { contentDescription = "Insert anyway; may duplicate" }) { Text("Insert anyway") }
            }
        }
        is FloatingState.Failed -> Card(maxHeight, maxWidth, message, "Transcription failed · audio saved", state.reason, actions.collapse, "Hide this card; the audio stays saved", announce) {
            Text("Nothing was inserted.", style = MaterialTheme.typography.bodyMedium)
            Buttons {
                if (state.retryable) Button(onClick = actions.retry, modifier = Modifier.heightIn(min = 48.dp).semantics { contentDescription = "Retry transcription with the current engine" }) { Text("Retry") }
                TextButton(onClick = actions.open, modifier = Modifier.heightIn(min = 48.dp).semantics { contentDescription = "Open in $PRODUCT_NAME" }) { Text("Open") }
            }
        }
        is FloatingState.Blocked -> Card(maxHeight, maxWidth, message, "Recording did not start", state.message, actions.dismiss, "Dismiss", announce) {
            Text("Nothing was recorded.", style = MaterialTheme.typography.bodyMedium)
            Buttons { Button(onClick = actions.openSettings, modifier = Modifier.heightIn(min = 48.dp)) { Text("Open permissions") } }
        }
    }
}

@Composable private fun Badge(status: Status) {
    Box(Modifier.size(22.dp).clip(CircleShape).background(MaterialTheme.colorScheme.surfaceContainerHigh), contentAlignment = Alignment.Center) {
        WriterIcon("alert", Modifier.size(18.dp), tint = status.textColor())
    }
}

@Composable private fun Pill(modifier: Modifier = Modifier, content: @Composable RowScope.() -> Unit) {
    Surface(shape = RoundedCornerShape(28.dp), color = MaterialTheme.colorScheme.surfaceContainerHigh, shadowElevation = 3.dp, modifier = modifier) {
        Row(Modifier.heightIn(min = 56.dp).padding(horizontal = 4.dp), verticalAlignment = Alignment.CenterVertically, content = content)
    }
}

@Composable private fun Buttons(content: @Composable FlowRowScope.() -> Unit) {
    FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(4.dp), content = content)
}

/** A card that grows upward from the control, at most [maxWidth] (≤ 320 dp) wide and [maxHeight] tall; its body scrolls. */
@Composable private fun Card(maxHeight: Dp, maxWidth: Dp, message: String?, title: String, body: String?, close: () -> Unit, closeLabel: String, modifier: Modifier = Modifier, content: @Composable ColumnScope.() -> Unit) {
    Surface(shape = RoundedCornerShape(24.dp), color = MaterialTheme.colorScheme.surfaceContainerHigh, shadowElevation = 4.dp,
        modifier = modifier.widthIn(max = maxWidth).heightIn(max = maxHeight)) {
        Column(Modifier.verticalScroll(rememberScrollState()).padding(start = 16.dp, end = 4.dp, top = 4.dp, bottom = 14.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(title, Modifier.weight(1f).padding(top = 8.dp).semantics { heading() }, style = MaterialTheme.typography.titleSmall)
                IconButton(onClick = close, modifier = Modifier.size(48.dp).semantics { contentDescription = closeLabel }) { WriterIcon("close") }
            }
            Column(Modifier.padding(end = 12.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                body?.let { Text(it, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant, overflow = TextOverflow.Ellipsis) }
                content()
                message?.let { Text(it, style = MaterialTheme.typography.bodyMedium, color = Status.Warn.textColor(), modifier = Modifier.semantics { liveRegion = LiveRegionMode.Polite }) }
            }
        }
    }
}

/** The slim edge tab while the keyboard leaves no room (D-9). A 48 dp touch target around a narrow pill. */
@Composable private fun EdgeTab(state: FloatingState, expand: () -> Unit) {
    Surface(onClick = expand, color = androidx.compose.ui.graphics.Color.Transparent, modifier = Modifier.size(48.dp, 64.dp).semantics {
        contentDescription = "Show $PRODUCT_NAME controls · ${state.announcement ?: "needs attention"}"
    }) {
        Box(contentAlignment = Alignment.Center) {
            Surface(shape = RoundedCornerShape(12.dp), color = MaterialTheme.colorScheme.primary, shadowElevation = 3.dp, modifier = Modifier.size(24.dp, 56.dp)) {
                Box(contentAlignment = Alignment.Center) {
                    WriterIcon(if (state is FloatingState.Recording) "mic" else if (state is FloatingState.NotInserted || state is FloatingState.Unconfirmed || state is FloatingState.Failed) "alert" else "wave",
                        Modifier.size(18.dp), tint = MaterialTheme.colorScheme.onPrimary)
                }
            }
        }
    }
}
