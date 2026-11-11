@file:OptIn(androidx.compose.material3.ExperimentalMaterial3Api::class)
package com.metrix.writer.ui

import android.content.Intent
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.*
import androidx.compose.ui.unit.dp
import com.metrix.writer.*
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import java.text.DateFormat
import java.util.Date

/*
 * Entry (MW-ANDROID-02 "Entry"): one result area. The selected version's text, one Copy, and a
 * delivery line that always says what was typed, whichever version is shown (D-3). Every other
 * version and attempt is one tap away in Versions & attempts; retries appear only in the notice
 * for what failed, and say their consequence.
 */

@Composable internal fun EntryScreen(vm: WriterViewModel, state: WriterState, id: String, navigate: (String) -> Unit) {
    val row = state.records.firstOrNull { it.id == id }
    val context = LocalContext.current
    if (row == null) { EmptyState("Dictation removed", "Return to History to see your saved recordings.", "History"); return }
    val attempt = row.insertedAttempt ?: row.latest
    val versions = attempt?.textVersions().orEmpty()
    var selectedKey by rememberSaveable(id) { mutableStateOf<String?>(null) }
    val selected = versions.firstOrNull { it.key == selectedKey } ?: row.defaultVersion()?.let { d -> versions.firstOrNull { it.key == d.key } } ?: versions.firstOrNull()
    var sheet by rememberSaveable(id) { mutableStateOf(false) }
    var compare by rememberSaveable(id) { mutableStateOf(false) }
    var confirm by rememberSaveable(id) { mutableStateOf<String?>(null) }
    var more by remember { mutableStateOf(false) }
    var exportText by rememberSaveable { mutableStateOf<String?>(null) }
    val idle = !state.processing && state.capture == "idle" && !row.busy
    val saveAudio = rememberLauncherForActivityResult(ActivityResultContracts.CreateDocument("audio/wav")) { uri ->
        if (uri != null) vm.action { context.contentResolver.openOutputStream(uri, "wt")!!.use { out -> vm.manager.store.audio(vm.manager.store.load(id)).inputStream().use { it.copyTo(out) } }; vm.notify("Audio saved.") }
    }
    val saveText = rememberLauncherForActivityResult(ActivityResultContracts.CreateDocument("text/plain")) { uri ->
        val text = exportText
        if (uri != null && text != null) vm.action { context.contentResolver.openOutputStream(uri, "wt")!!.use { it.write(text.toByteArray()) }; vm.notify("Text exported.") }
        exportText = null
    }
    Page {
        // Date, duration and speech language; the location of the shown version follows in the delivery line.
        Text(listOfNotNull(DateFormat.getDateTimeInstance(DateFormat.MEDIUM, DateFormat.SHORT).format(Date(row.created)), formatClock(row.duration),
            row.language.takeIf { it.isNotEmpty() }?.let(CompanionUi::languageName)).joinToString(" · "),
            style = MaterialTheme.typography.labelLarge, color = MaterialTheme.colorScheme.onSurfaceVariant)
        row.historicalSource?.let { source ->
            Notice("Separate version", "Reprocessed from an earlier recording. Separate versions are never inserted automatically.", Status.Neutral) {
                TextButton(onClick = { navigate("History/$source") }, modifier = Modifier.heightIn(min = 48.dp)) { Text("Open source recording") }
            }
        }
        EntryNotices(vm, row, idle, navigate, select = { kind -> versions.of(attempt!!, kind)?.let { selectedKey = it.key } }) { confirm = it }
        if (selected != null) {
            if (versions.size > 1) ChoiceChips("Version", versions, selected, { it.label }, { selectedKey = it.key }, tag = { "version-${it.kind.name.lowercase()}" })
            SelectionContainer { SpokenText(selected.text, Modifier.fillMaxWidth().testTag("entry-text"), large = true) }
            FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(4.dp), itemVerticalAlignment = Alignment.CenterVertically) {
                Button(onClick = { copyToClipboard(context, "$PRODUCT_NAME ${selected.label}", selected.text); vm.notify("${selected.label} copied.") },
                    modifier = Modifier.heightIn(min = 48.dp).testTag("entry-copy").semantics { contentDescription = "Copy ${selected.label} text" }) {
                    WriterIcon("copy", tint = MaterialTheme.colorScheme.onPrimary); Spacer(Modifier.width(6.dp)); Text("Copy")
                }
                if (versions.size > 1) FilledTonalButton(onClick = { compare = true }, modifier = Modifier.heightIn(min = 48.dp)) { Text("Compare") }
                Box {
                    IconButton(onClick = { more = true }, modifier = Modifier.testTag("entry-more").semantics { contentDescription = "More actions for this dictation" }) { WriterIcon("more") }
                    EntryMenu(more, { more = false }, row, selected, idle, vm, navigate, onSaveAudio = { saveAudio.launch("$PRODUCT_NAME recording.wav") },
                        onExport = { exportText = selected.text; saveText.launch("$PRODUCT_NAME ${selected.label}.txt") }) { confirm = it }
                }
            }
            DeliveryLine(row, selected)
        } else if (!row.busy && row.attention() == null) Text(if (row.audioAvailable) "No text yet. The audio is saved." else "No text or audio is saved.", color = MaterialTheme.colorScheme.onSurfaceVariant)
        if (row.audioAvailable) AudioPlayer(id, { vm.manager.store.audio(vm.manager.store.load(id)) }, row.duration, row.capture != "capturing") { vm.notify("Playback failed. Saved audio remains available.") }
        else Text("Audio unavailable · saved text remains available.", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        if (selected == null) Box { // Audio-only entries still reach Share, Save and Delete.
            TextButton(onClick = { more = true }, modifier = Modifier.heightIn(min = 48.dp).testTag("entry-more")) { Text("More actions") }
            EntryMenu(more, { more = false }, row, null, idle, vm, navigate, onSaveAudio = { saveAudio.launch("$PRODUCT_NAME recording.wav") }, onExport = {}) { confirm = it }
        }
        val allVersions = row.attempts.sumOf { it.textVersions(withEarlier = true).size } + row.attempts.count { it.state != "succeeded" }
        if (allVersions > 0) Surface(onClick = { sheet = true }, shape = MaterialTheme.shapes.medium, color = MaterialTheme.colorScheme.surfaceContainerLow,
            modifier = Modifier.fillMaxWidth().testTag("entry-versions")) {
            Row(Modifier.heightIn(min = 56.dp).padding(horizontal = 16.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                WriterIcon("layers", tint = MaterialTheme.colorScheme.primary)
                Text("Versions & attempts · $allVersions", Modifier.weight(1f), style = MaterialTheme.typography.bodyLarge)
                WriterIcon(if (androidx.compose.ui.platform.LocalLayoutDirection.current == androidx.compose.ui.unit.LayoutDirection.Rtl) "back" else "next")
            }
        }
    }
    if (sheet) VersionsSheet(vm, row, idle, { sheet = false }) { confirm = it }
    if (compare && selected != null) {
        val other = versions.firstOrNull { it.kind == TextKind.ORIGINAL && it.key != selected.key } ?: versions.firstOrNull { it.key != selected.key }
        ProtectedAlertDialog(onDismissRequest = { compare = false }, title = { Text("Compare versions") },
            text = { Column(Modifier.verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(selected.label, style = MaterialTheme.typography.titleSmall); SelectionContainer { SpokenText(selected.text) }
                other?.let { HorizontalDivider(); Text(it.label, style = MaterialTheme.typography.titleSmall); SelectionContainer { SpokenText(it.text) } }
            } },
            confirmButton = { TextButton(onClick = { compare = false }) { Text("Close") } })
    }
    confirm?.let { action -> EntryConfirm(vm, row, action) { confirm = null } }
}

/** What was typed, separate from what is shown: "Showing: Original · Inserted: Polished". */
@Composable private fun DeliveryLine(row: Dictation, selected: TextVersion) {
    val inserted = row.insertedKind
    val (line, status) = row.deliveryLine()
    val text = if (inserted != null && row.insertedAttempt?.textOf(inserted) != selected.text) "Showing: ${selected.label} · Inserted: ${inserted.label} · verified" else line
    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp), modifier = Modifier.testTag("delivery-line").semantics(mergeDescendants = true) { }) {
        WriterIcon(if (inserted != null) "check" else if (status == Status.Warn) "alert" else "info", Modifier.size(18.dp), status.textColor())
        Text("$text · ${selected.attempt.whereLabel()}", style = MaterialTheme.typography.bodyMedium, color = status.textColor())
    }
}

/** The current engine in words, so a retry says where it will run. */
@Composable private fun retryLabel(): String {
    val choice = DictationChoice(LocalContext.current)
    return if (choice.isPhone()) "On this phone · ${choice.model()}" else DirectCloud.selectedProvider(LocalContext.current).let { "Cloud · ${CloudProviders.get(it).name}" }
}

/** Contextual notices: each failure states its consequence and offers only its own next action. */
@Composable private fun EntryNotices(vm: WriterViewModel, row: Dictation, idle: Boolean, navigate: (String) -> Unit, select: (TextKind) -> Unit, confirm: (String) -> Unit) {
    val view = row.view
    val engine = retryLabel()
    if (row.busy && view != null) {
        Notice(stageWords(view.stage), listOfNotNull(row.pendingWhere, "Your audio is saved.").joinToString(" · "), Status.Neutral) {
            TextButton(onClick = vm::stopProcessing, modifier = Modifier.heightIn(min = 48.dp)) { Text(if (view.stage == ProcessingStage.POLISHING) "Stop polishing · keep original" else "Stop processing · keep audio") }
        }
    }
    when (row.attention()) {
        Attention.NOT_INSERTED -> Notice("Saved, not inserted", deliveryReason(row.ledger?.reasonCode.orEmpty()) + " Nothing was typed. To type it into a field, focus the field and use Insert here on the floating button.", Status.Warn, Modifier.testTag("entry-recovery")) {
            Button(onClick = { vm.copySaved(row.id) }, modifier = Modifier.heightIn(min = 48.dp)) { Text("Copy") }
            TextButton(onClick = { vm.markHandled(row.id) }, modifier = Modifier.heightIn(min = 48.dp)) { Text("Mark handled") }
        }
        Attention.UNCONFIRMED -> Notice("Check the field", "Writer typed into the field but could not read it back. It may already be there, so check before copying. Nothing will be typed again automatically.", Status.Warn, Modifier.testTag("entry-recovery")) {
            Button(onClick = { vm.copySaved(row.id) }, modifier = Modifier.heightIn(min = 48.dp)) { Text("Copy") }
            TextButton(onClick = { vm.markHandled(row.id) }, modifier = Modifier.heightIn(min = 48.dp)) { Text("It's there") }
        }
        Attention.FAILED -> Notice("Transcription failed · audio saved", ((view?.error?.message ?: row.note).ifBlank { "The engine did not return text." }) + " Nothing was inserted.", Status.Err, Modifier.testTag("entry-failed")) {
            Button(onClick = { vm.retry(row.id) }, enabled = idle && row.audioAvailable, modifier = Modifier.heightIn(min = 48.dp)) { Text("Retry · $engine") }
            when (view?.error?.action) {
                ProcessingAction.OPEN_CLOUD_SETTINGS -> TextButton(onClick = { navigate("Settings/${com.metrix.writer.ui.settings.SettingsRoutes.KEYS}") }, modifier = Modifier.heightIn(min = 48.dp)) { Text("Check API key") }
                ProcessingAction.OPEN_MODELS, ProcessingAction.CHOOSE_SMALLER_MODEL -> TextButton(onClick = { navigate("Settings/${com.metrix.writer.ui.settings.SettingsRoutes.MODELS}") }, modifier = Modifier.heightIn(min = 48.dp)) { Text("Phone models") }
                else -> {}
            }
            if (row.pending) TextButton(onClick = { confirm("replace") }, enabled = idle, modifier = Modifier.heightIn(min = 48.dp)) { Text("New cloud attempt with current settings") }
        }
        Attention.INTERRUPTED -> Notice("Recording recovered", "Writer stopped while this was in progress. The audio is saved. It will not be typed anywhere.", Status.Warn) {
            Button(onClick = { vm.retry(row.id) }, enabled = idle && row.audioAvailable, modifier = Modifier.heightIn(min = 48.dp)) { Text("Transcribe again · $engine") }
            if (row.pending) TextButton(onClick = { confirm("replace") }, enabled = idle, modifier = Modifier.heightIn(min = 48.dp)) { Text("New cloud attempt with current settings") }
        }
        Attention.NOT_TRANSCRIBED -> Notice("Not transcribed", "Audio saved on this phone.", Status.Warn) {
            Button(onClick = { vm.retry(row.id) }, enabled = idle && row.audioAvailable, modifier = Modifier.heightIn(min = 48.dp)) { Text("Transcribe now · $engine") }
        }
        Attention.POLISH_INCOMPLETE -> Notice("Polishing did not finish", "Your original and corrections are saved. Finishing keeps the request's saved settings.", Status.Warn) {
            Button(onClick = { vm.resume(row.id) }, enabled = idle, modifier = Modifier.heightIn(min = 48.dp)) { Text("Finish polishing") }
            TextButton(onClick = { confirm("cleanup") }, enabled = idle, modifier = Modifier.heightIn(min = 48.dp)) { Text("Use current polishing settings") }
        }
        null -> {}
    }
    if (row.stoppedWithAudio && row.audioAvailable)
        Notice("Stopped without transcribing", "Audio saved on this phone. Nothing was inserted.", Status.Neutral) {
            Button(onClick = { vm.retry(row.id) }, enabled = idle, modifier = Modifier.heightIn(min = 48.dp)) { Text("Transcribe now · $engine") }
        }
    val attempt = row.insertedAttempt ?: row.latest ?: return
    if (attempt.latePolish) Notice("Polished version ready · not typed anywhere", "The original was typed. This polished version arrived later and is only saved here.", Status.Neutral) {
        TextButton(onClick = { select(TextKind.POLISHED) }, modifier = Modifier.heightIn(min = 48.dp)) { Text("View polished") }
    }
    attempt.polishNotice()?.let { (title, body) ->
        Notice(title, body, if (attempt.polishReason == "user_skipped" || attempt.polishReason == null) Status.Neutral else Status.Warn, Modifier.testTag("entry-polish-notice")) {
            if (CompanionPreferences(LocalContext.current).cleanup() != "off")
                TextButton(onClick = { confirm("process:${attempt.id}") }, enabled = idle && !row.pending && !row.postprocessing, modifier = Modifier.heightIn(min = 48.dp)) { Text("Polish again") }
        }
    }
}

@Composable private fun EntryMenu(open: Boolean, close: () -> Unit, row: Dictation, selected: TextVersion?, idle: Boolean, vm: WriterViewModel, navigate: (String) -> Unit,
                                  onSaveAudio: () -> Unit, onExport: () -> Unit, confirm: (String) -> Unit) {
    val context = LocalContext.current
    val audio = row.audioAvailable && row.capture != "capturing"
    DropdownMenu(expanded = open, onDismissRequest = close) {
        if (selected != null) {
            DropdownMenuItem(text = { Text("Share text") }, onClick = { close(); context.startActivity(Intent.createChooser(Intent(Intent.ACTION_SEND).setType("text/plain").putExtra(Intent.EXTRA_TEXT, selected.text), "Share text")) })
            DropdownMenuItem(text = { Text("Export text") }, onClick = { close(); onExport() })
            DropdownMenuItem(text = { Text("Add a correction") }, onClick = { close(); navigate("Settings/Words") })
        }
        DropdownMenuItem(text = { Text("Share audio") }, enabled = audio, onClick = { close(); vm.action {
            val uri = AudioExportProvider.snapshot(context, vm.manager.store, row.id)
            withContext(Dispatchers.Main) { context.startActivity(Intent.createChooser(Intent(Intent.ACTION_SEND).setType("audio/wav").putExtra(Intent.EXTRA_STREAM, uri).addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION), "Share audio")) }
        } })
        DropdownMenuItem(text = { Text("Save audio as…") }, enabled = audio, onClick = { close(); onSaveAudio() })
        DropdownMenuItem(text = { Text(if (row.pending) "Resume transcription" else "Transcribe again") }, enabled = audio && idle && !row.postprocessing, onClick = { close(); vm.retry(row.id) })
        DropdownMenuItem(text = { Text("Delete dictation", color = MaterialTheme.colorScheme.error) }, enabled = idle && !row.pending && !row.postprocessing,
            onClick = { close(); confirm("delete") }, modifier = Modifier.testTag("entry-delete"))
    }
}

/** Every version and attempt on one line each, with its model, location, status and Copy. Failed attempts keep their error. */
@Composable private fun VersionsSheet(vm: WriterViewModel, row: Dictation, idle: Boolean, close: () -> Unit, confirm: (String) -> Unit) {
    val context = LocalContext.current
    val inserted = row.insertedAttempt
    ModalBottomSheet(onDismissRequest = close, modifier = Modifier.testTag("versions-sheet")) {
        Column(Modifier.verticalScroll(rememberScrollState()).padding(horizontal = 16.dp).padding(bottom = 24.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            Text("Versions & attempts", style = MaterialTheme.typography.titleLarge, modifier = Modifier.semantics { heading() })
            row.attempts.asReversed().forEachIndexed { index, attempt ->
                val title = "Attempt ${row.attempts.size - index} · ${attempt.model} · ${attempt.whereLabel()}" + (attempt.latency?.let { " · ${seconds(it)}" } ?: "")
                Text(title, style = MaterialTheme.typography.titleSmall, color = MaterialTheme.colorScheme.primary)
                if (attempt.state != "succeeded") Text(listOfNotNull(if (attempt.state == "failed") "Failed" else attempt.state.replaceFirstChar(Char::uppercase), attempt.error).joinToString(" · "),
                    style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.error)
                attempt.textVersions(withEarlier = true).forEach { version ->
                    val typed = inserted == attempt && !version.earlier && attempt.textOf(attempt.typedKind()) == version.text
                    Surface(shape = MaterialTheme.shapes.small, color = MaterialTheme.colorScheme.surfaceContainerLow, modifier = Modifier.fillMaxWidth()) {
                        Row(Modifier.padding(start = 12.dp).heightIn(min = 56.dp), verticalAlignment = Alignment.CenterVertically) {
                            Column(Modifier.weight(1f).padding(vertical = 8.dp)) {
                                Text(version.label + if (typed) " · inserted" else "", style = MaterialTheme.typography.labelLarge)
                                SpokenText(version.text, maxLines = 1)
                            }
                            IconButton(onClick = { copyToClipboard(context, "$PRODUCT_NAME ${version.label}", version.text); vm.notify("${version.label} copied.") },
                                modifier = Modifier.semantics { contentDescription = "Copy ${version.label} from attempt ${row.attempts.size - index}" }) { WriterIcon("copy") }
                        }
                    }
                }
                if (attempt.state == "succeeded" && CompanionPreferences(context).cleanup() != "off")
                    TextButton(onClick = { close(); confirm("process:${attempt.id}") }, enabled = idle && !row.pending && !row.postprocessing, modifier = Modifier.heightIn(min = 48.dp)) { Text("Polish this attempt again…") }
            }
        }
    }
}

/** Confirms actions with a consequence: delete, a new paid request, or a new polish. */
@Composable private fun EntryConfirm(vm: WriterViewModel, row: Dictation, action: String, close: () -> Unit) {
    val context = LocalContext.current
    val polish = action.startsWith("process")
    val prefs = CompanionPreferences(context)
    val model = prefs.cleanup().let { if (it == "off") null else runCatching { "${CloudProviders.get(it.substringBefore('/')).name} · ${it.substringAfter('/')}" }.getOrDefault(it) }
    ProtectedAlertDialog(onDismissRequest = close,
        title = { Text(when { action == "delete" -> "Delete this dictation?"; polish -> "Polish again?"; else -> "Use current settings?" }) },
        text = { Text(when {
            action == "delete" -> "This removes the audio, transcripts, insertion record and $PRODUCT_NAME’s cached shares on this phone. Recipient copies, files you exported elsewhere, cloud provider retention and OS backups are outside this deletion. Already-open shares may remain readable. This is not secure storage erasure."
            polish -> "Sends the transcript text (not audio) to ${model ?: "your polishing provider"} in the cloud. This may cost money. The result is saved as a new version and is not typed anywhere. The original stays available."
            else -> "The previous outcome may be unknown. A new cloud request can incur another charge. Earlier audio and text stay saved."
        }) },
        confirmButton = { TextButton(onClick = {
            when { action == "delete" -> vm.delete(row.id); action == "replace" -> vm.replacePending(row.id); action == "cleanup" -> vm.replaceCleanup(row.id); else -> vm.cleanup(row.id, action.substringAfter(':')) }
            close()
        }, modifier = Modifier.testTag("confirm-$action".substringBefore(':'))) { Text(if (action == "delete") "Delete" else "Continue", color = if (action == "delete") MaterialTheme.colorScheme.error else androidx.compose.ui.graphics.Color.Unspecified) } },
        dismissButton = { TextButton(onClick = close) { Text("Cancel") } })
}
