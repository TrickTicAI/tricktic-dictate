package com.metrix.writer.ui

import android.os.SystemClock
import androidx.compose.animation.animateContentSize
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.*
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.*
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.dp
import com.metrix.writer.*
import kotlinx.coroutines.delay

/*
 * Home (MW-ANDROID-02 "Home"): one primary action, the engine and output in words, the live
 * dictation with its stage and elapsed time, the original as soon as it exists, and a saved-but-
 * not-inserted banner that stays until it is handled.
 */

@Composable internal fun HomeScreen(vm: WriterViewModel, state: WriterState, start: () -> Unit, navigate: (String) -> Unit) {
    val context = LocalContext.current
    val choice = DictationChoice(context)
    val review = LocalInsertionReview.current
    val latest = state.records.firstOrNull()
    Page {
        // Recovery first: it stays until copied, inserted, dismissed or marked handled (D-4).
        state.records.firstOrNull { it.attention() == Attention.NOT_INSERTED || it.attention() == Attention.UNCONFIRMED }?.let { row ->
            RecoveryBanner(vm, row, navigate)
        }
        state.records.firstOrNull { it.attention() == Attention.INTERRUPTED }?.let { row ->
            Notice("Recording recovered", "Writer stopped while this recording was in progress. The audio is saved. It will not be typed anywhere.", Status.Warn,
                Modifier.testTag("home-recovered")) {
                Button(onClick = { navigate("History/${row.id}") }) { Text("Open") }
            }
        }
        DictationCard(vm, state, latest, start)
        FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
            EngineChip(choice) { navigate("Settings/Engine") }
            OutputChip { navigate("Settings/Polishing") }
        }
        if ((!choice.isPhone() && !DirectCloud(context).configured()) || (choice.isPhone() && !runCatching { PhoneModels(context).file(choice.model()).exists() }.getOrDefault(false))) {
            SettingRow("Finish setting up", "Choose an available transcription model", { navigate("Settings/Setup") }, "record")
        }
        val insertion = PermissionStatus.read(context)
        if (!insertion.enabled || !AccessibilityConsent(context).accepted) {
            SettingRow(if (insertion.enabled) "Text insertion is paused" else "Type into other apps",
                "Review Accessibility access · optional; recording here still works", { review { } }, "Dictionary")
        }
        val overlay = FloatingPreferences(context)
        if (overlay.snoozed()) SettingRow("Floating control is hidden", "Tap to show it now", { overlay.showNow(); vm.notify("Floating control restored beside supported text fields.") }, "record")
        if (latest != null && !latest.busy) {
            Text("Last dictation", style = MaterialTheme.typography.titleMedium, modifier = Modifier.semantics { heading() })
            LastDictation(latest, { vm.notify("Copied.") }) { navigate("History/${latest.id}") }
        }
        val earlier = state.records.drop(1).take(2)
        if (earlier.isNotEmpty() || latest == null) Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
            Text("Earlier", Modifier.weight(1f), style = MaterialTheme.typography.titleMedium)
            TextButton(onClick = { navigate("History") }, modifier = Modifier.heightIn(min = 48.dp)) { Text("View all") }
        }
        if (state.records.isEmpty()) EmptyState("A fresh page", "Your first dictation will appear here.", "History")
        earlier.forEach { row -> DictationRow(row) { navigate("History/${row.id}") } }
    }
}

/** The live card: start, record, save, process, with the original shown as soon as it exists. */
@Composable private fun DictationCard(vm: WriterViewModel, state: WriterState, latest: Dictation?, start: () -> Unit) {
    var cancel by rememberSaveable { mutableStateOf(false) }
    val working = latest?.takeIf { it.busy && it.view?.stage != ProcessingStage.RECORDING }
    Surface(color = MaterialTheme.colorScheme.primaryContainer, shape = MaterialTheme.shapes.large, modifier = Modifier.fillMaxWidth().animateContentSize()) {
        Column(Modifier.padding(20.dp), verticalArrangement = Arrangement.spacedBy(14.dp)) {
            when {
                state.capture == "starting" -> {
                    Text("Starting microphone…", style = MaterialTheme.typography.titleLarge, modifier = Modifier.semantics { liveRegion = LiveRegionMode.Polite })
                    LinearProgressIndicator(Modifier.fillMaxWidth())
                }
                state.capture == "recording" -> {
                    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                        Text("Recording", Modifier.weight(1f).semantics { liveRegion = LiveRegionMode.Polite }, style = MaterialTheme.typography.titleLarge)
                        Figures(formatClock(state.elapsed), Modifier.semantics { contentDescription = "Recording time ${formatClock(state.elapsed)}" }, MaterialTheme.typography.titleLarge)
                    }
                    LeftToRight { HomeLevel({ vm.manager.capture.level }) }
                    LaunchedEffect(Unit) { CaptureService.visibleRecording(CaptureService.activeSessionId, false) }
                    FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        Button(onClick = vm::finish, modifier = Modifier.heightIn(min = 48.dp).testTag("stop-recording").semantics { contentDescription = "Stop and transcribe" }) {
                            WriterIcon("stop", tint = MaterialTheme.colorScheme.onPrimary); Spacer(Modifier.width(8.dp)); Text("Stop")
                        }
                        TextButton(onClick = { cancel = true }, modifier = Modifier.heightIn(min = 48.dp).semantics { contentDescription = "Cancel: continue, stop, keep or discard" }) { Text("Cancel") }
                    }
                    Text("No time limit. Audio is saved as you speak.", style = MaterialTheme.typography.bodySmall)
                }
                state.capture == "saving" -> {
                    Text("Saving recording…", style = MaterialTheme.typography.titleLarge, modifier = Modifier.semantics { liveRegion = LiveRegionMode.Polite })
                    LinearProgressIndicator(Modifier.fillMaxWidth())
                }
                working != null -> ProcessingCard(vm, working)
                state.processing -> {
                    Text("Working on a saved recording", style = MaterialTheme.typography.titleLarge)
                    LinearProgressIndicator(Modifier.fillMaxWidth())
                    TextButton(onClick = vm::stopProcessing, modifier = Modifier.heightIn(min = 48.dp)) { Text("Stop processing · keep audio") }
                }
                else -> {
                    Text("Ready when you are", style = MaterialTheme.typography.titleLarge)
                    Button(onClick = start, modifier = Modifier.fillMaxWidth().heightIn(min = 56.dp).testTag("start-recording")) {
                        WriterIcon("mic", tint = MaterialTheme.colorScheme.onPrimary); Spacer(Modifier.width(10.dp)); Text("Start dictation")
                    }
                    Text("Records here, with no text field to type into. The text is saved for you to copy.", style = MaterialTheme.typography.bodySmall)
                }
            }
        }
    }
    if (cancel) ProtectedAlertDialog(onDismissRequest = { cancel = false },
        title = { Text("Stop this recording?") },
        text = { Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
            Text("Recording continues until you choose.")
            TextButton(onClick = { cancel = false; vm.finish() }, modifier = Modifier.fillMaxWidth().heightIn(min = 48.dp)) { Text("Stop & transcribe") }
            TextButton(onClick = { cancel = false; vm.keepRecording() }, modifier = Modifier.fillMaxWidth().heightIn(min = 48.dp).testTag("keep-recording")) { Text("Keep audio, insert nothing") }
            TextButton(onClick = { cancel = false; vm.cancelRecording() }, modifier = Modifier.fillMaxWidth().heightIn(min = 48.dp).testTag("discard-recording")) {
                Text("Discard recording", color = MaterialTheme.colorScheme.error)
            }
        } },
        confirmButton = { Button(onClick = { cancel = false }) { Text("Continue recording") } })
    // The dialog closes by itself if the recording ends meanwhile.
    if (cancel && state.capture != "recording") cancel = false
}

/** Stage, elapsed time and location; the original appears, copyable, as soon as it is saved. */
@Composable private fun ProcessingCard(vm: WriterViewModel, row: Dictation) {
    val view = row.view!!
    val context = LocalContext.current
    var now by remember { mutableLongStateOf(SystemClock.elapsedRealtime()) }
    LaunchedEffect(row.id) { while (true) { now = SystemClock.elapsedRealtime(); delay(500) } }
    val elapsed = view.elapsedNow(row.projectedAt, now)
    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
        Text(stageWords(view.stage), Modifier.weight(1f).semantics { liveRegion = LiveRegionMode.Polite }, style = MaterialTheme.typography.titleLarge)
        elapsed?.let { Figures(formatClock(it), style = MaterialTheme.typography.titleMedium) }
    }
    // Only a measured fraction is drawn as progress; otherwise the bar is indeterminate (D-8).
    view.progress?.let { LinearProgressIndicator(progress = { it }, modifier = Modifier.fillMaxWidth()) } ?: LinearProgressIndicator(Modifier.fillMaxWidth())
    Text(listOfNotNull(row.pendingWhere ?: row.latest?.whereLabel(), "Your audio is saved on this phone").joinToString(" · "), style = MaterialTheme.typography.bodyMedium)
    StepList(view)
    val original = row.latest?.takeIf { view.stage == ProcessingStage.POLISHING }
    if (original != null) {
        Surface(shape = MaterialTheme.shapes.medium, color = MaterialTheme.colorScheme.surfaceContainerLow) {
            Column(Modifier.padding(14.dp).fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text("Original · ready now", style = MaterialTheme.typography.labelLarge, color = MaterialTheme.colorScheme.primary)
                SpokenText(original.dictionary ?: original.raw, maxLines = 6)
                FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    Button(onClick = { copyToClipboard(context, "$PRODUCT_NAME original", original.dictionary ?: original.raw); vm.notify("Original copied.") },
                        modifier = Modifier.heightIn(min = 48.dp).testTag("copy-original-now")) { WriterIcon("copy", tint = MaterialTheme.colorScheme.onPrimary); Spacer(Modifier.width(6.dp)); Text("Copy original") }
                    TextButton(onClick = vm::stopProcessing, modifier = Modifier.heightIn(min = 48.dp)) { Text("Stop polishing · keep original") }
                }
            }
        }
    } else TextButton(onClick = vm::stopProcessing, modifier = Modifier.heightIn(min = 48.dp).semantics { contentDescription = "Stop processing, keep audio" }) { Text("Stop processing · keep audio") }
}

private val stepNames = linkedMapOf("queue" to "Waited to start", "preparation" to "Audio prepared", "model_load" to "Model loaded", "upload" to "Uploaded",
    "asr" to "Transcribed", "cloud_request" to "Transcribed in the cloud", "raw_availability" to "Original ready", "cleanup" to "Polished")

/** Completed stage timings, measured only. */
@Composable private fun StepList(view: ProcessingView) {
    val steps = stepNames.mapNotNull { (key, name) -> view.timingsMs[key]?.let { name to it } }
    if (steps.isEmpty()) return
    Column(verticalArrangement = Arrangement.spacedBy(2.dp), modifier = Modifier.semantics(mergeDescendants = true) { }) {
        steps.forEach { (name, ms) ->
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                WriterIcon("check", Modifier.size(18.dp), MaterialTheme.colorScheme.primary)
                Text(name, Modifier.weight(1f), style = MaterialTheme.typography.bodySmall)
                Figures(seconds(ms), style = MaterialTheme.typography.bodySmall)
            }
        }
    }
}

internal fun seconds(ms: Long) = if (ms < 60_000) "%.1f s".format(java.util.Locale.ROOT, ms / 1000.0) else formatClock(ms)

@Composable private fun HomeLevel(level: () -> Float) {
    var samples by remember { mutableStateOf(List(28) { 0f }) }
    LaunchedEffect(Unit) { while (true) { samples = samples.drop(1) + (level() * 5f).coerceIn(0f, 1f); delay(60) } }
    val color = MaterialTheme.colorScheme.primary
    Canvas(Modifier.fillMaxWidth().height(40.dp).semantics { contentDescription = "Live microphone level" }) {
        val step = size.width / samples.size
        samples.forEachIndexed { i, value ->
            val h = 3.dp.toPx() + value * (size.height - 3.dp.toPx()); val x = step * (i + .5f)
            drawLine(color, Offset(x, (size.height - h) / 2), Offset(x, (size.height + h) / 2), 3.dp.toPx(), StrokeCap.Round)
        }
    }
}

/** "On this phone · base · English" or "Cloud · Gemini · English", with the location in bold (D-10). */
@Composable private fun EngineChip(choice: DictationChoice, open: () -> Unit) {
    val language = CompanionUi.languageName(choice.language())
    val (where, detail) = if (choice.isPhone()) "On this phone" to "${choice.model()} · $language"
        else DirectCloud.selectedProvider(LocalContext.current).let { "Cloud" to "${CloudProviders.get(it).name} · $language" }
    AssistChip(onClick = open, label = { Text(buildAnnotatedString { withStyle(SpanStyle(fontWeight = FontWeight.Bold)) { append(where) }; append(" · $detail") }, maxLines = 2) },
        leadingIcon = { WriterIcon(if (choice.isPhone()) "phone" else "cloud", Modifier.size(18.dp)) },
        modifier = Modifier.heightIn(min = 48.dp).testTag("home-engine").semantics { contentDescription = "Speech: $where, $detail. Change engine" })
}

/** What gets typed: "As spoken", "With my corrections" or "Polished · Cloud · wait 3 s". */
@Composable private fun OutputChip(open: () -> Unit) {
    val prefs = CompanionPreferences(LocalContext.current)
    val label = when (prefs.delivery()) {
        "cleaned" -> if (prefs.cleanup() == "off") "Polished · choose a model" else "Polished · Cloud · wait ${prefs.polishTimeoutMs() / 1000} s"
        "dictionary" -> "With my corrections"
        else -> "As spoken"
    }
    AssistChip(onClick = open, label = { Text(label, maxLines = 2) }, leadingIcon = { WriterIcon("insert", Modifier.size(18.dp)) },
        modifier = Modifier.heightIn(min = 48.dp).testTag("home-output").semantics { contentDescription = "Typed text: $label. Change polishing" })
}

/** Saved, not inserted (or unconfirmed): reason, preview and the next action, until handled. */
@Composable private fun RecoveryBanner(vm: WriterViewModel, row: Dictation, navigate: (String) -> Unit) {
    val unconfirmed = row.attention() == Attention.UNCONFIRMED
    val preview = row.defaultVersion()?.text
    Notice(if (unconfirmed) "Check the field" else "Saved, not inserted",
        if (unconfirmed) "Writer typed into a field but could not read it back. It may already be there, so check before copying."
        else deliveryReason(row.ledger?.reasonCode.orEmpty()) + " Nothing was typed.", Status.Warn, Modifier.testTag("home-attention")) {
        preview?.let { Box(Modifier.fillMaxWidth()) { SpokenText(it, maxLines = 2) } }
        Button(onClick = { vm.copySaved(row.id) }, modifier = Modifier.heightIn(min = 48.dp)) { Text("Copy") }
        if (!unconfirmed) TextButton(onClick = { vm.markHandled(row.id) }, modifier = Modifier.heightIn(min = 48.dp)) { Text("Mark handled") }
        else TextButton(onClick = { vm.markHandled(row.id) }, modifier = Modifier.heightIn(min = 48.dp)) { Text("It's there") }
        TextButton(onClick = { navigate("History/${row.id}") }, modifier = Modifier.heightIn(min = 48.dp)) { Text("Open") }
    }
}

/** The last result, text first, with its delivery line and one Copy. */
@Composable private fun LastDictation(row: Dictation, copied: () -> Unit, open: () -> Unit) {
    val context = LocalContext.current
    val version = row.defaultVersion()
    Surface(onClick = open, shape = MaterialTheme.shapes.medium, color = MaterialTheme.colorScheme.surfaceContainerLow, modifier = Modifier.fillMaxWidth().testTag("dictation-${row.id}")) {
        Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
            SpokenText(version?.text ?: row.rowTag().first, maxLines = 4)
            val (line, status) = row.deliveryLine()
            Text(line, style = MaterialTheme.typography.labelMedium, color = status.textColor())
            if (version != null) Button(onClick = { copyToClipboard(context, "$PRODUCT_NAME ${version.label}", version.text); copied() }, modifier = Modifier.heightIn(min = 48.dp).semantics { contentDescription = "Copy ${version.label} text" }) {
                WriterIcon("copy", tint = MaterialTheme.colorScheme.onPrimary); Spacer(Modifier.width(6.dp)); Text("Copy")
            }
        }
    }
}
