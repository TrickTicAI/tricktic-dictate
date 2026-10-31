package com.metrix.writer.ui

import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.*
import androidx.compose.ui.unit.dp
import com.metrix.writer.PRODUCT_NAME
import java.text.DateFormat
import java.util.Date

/*
 * History (MW-ANDROID-02 "History"): two lines of text per row, then time, duration, a delivery
 * tag and the version. Rows never repeat transcripts or per-version buttons. Needs attention
 * replaces "Text to recover"; secondary flows live behind ⋮.
 */

private enum class Filter(val label: String) { ALL("All"), ATTENTION("Needs attention"), POLISHED("Polished"), HEBREW("Hebrew") }

private val hebrew = Regex("[\\u0590-\\u05FF]")

@Composable internal fun HistoryScreen(vm: WriterViewModel, state: WriterState, navigate: (String) -> Unit, attentionOnly: Boolean) {
    var query by rememberSaveable { mutableStateOf("") }
    var filter by rememberSaveable { mutableStateOf(if (attentionOnly) Filter.ATTENTION else Filter.ALL) }
    var menu by remember { mutableStateOf(false) }
    val list = rememberLazyListState()
    val rows = state.records.filter { row ->
        when (filter) {
            Filter.ALL -> true
            Filter.ATTENTION -> row.attention() != null
            Filter.POLISHED -> row.attempts.any { it.cleaned != null }
            Filter.HEBREW -> row.language in setOf("he", "mixed") || row.latest?.raw?.let(hebrew::containsMatchIn) == true
        }
    }
    Column(Modifier.fillMaxSize().padding(horizontal = 16.dp), horizontalAlignment = Alignment.CenterHorizontally) {
        Column(Modifier.widthIn(max = 600.dp).fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(4.dp)) {
                OutlinedTextField(query, { query = it; vm.searchHistory(it) }, placeholder = { Text("Search dictations") }, singleLine = true,
                    leadingIcon = { WriterIcon("search") }, shape = MaterialTheme.shapes.extraLarge, modifier = Modifier.weight(1f).testTag("history-search"))
                Box {
                    IconButton(onClick = { menu = true }, modifier = Modifier.testTag("history-menu").semantics { contentDescription = "More History actions" }) { WriterIcon("more") }
                    DropdownMenu(expanded = menu, onDismissRequest = { menu = false }) {
                        DropdownMenuItem(text = { Text("Select and reprocess") }, onClick = { menu = false; navigate("History/Select") })
                        DropdownMenuItem(text = { Text("Statistics") }, onClick = { menu = false; navigate("History/Statistics") })
                        DropdownMenuItem(text = { Text("Export & import") }, onClick = { menu = false; navigate("Settings/${com.metrix.writer.ui.settings.SettingsRoutes.BACKUP}") })
                        DropdownMenuItem(text = { Text("Diagnostics") }, onClick = { menu = false; navigate("Settings/${com.metrix.writer.ui.settings.SettingsRoutes.STORAGE}") })
                        if (Account.available) DropdownMenuItem(text = { Text("Synced text") }, onClick = { menu = false; navigate("History/Synced text") })
                    }
                }
            }
            // Chip names stay fixed; the History tab badge carries the attention count.
            ChoiceChips("Show", Filter.entries, filter, { it.label }, { filter = it }, tag = { "filter-${it.name.lowercase()}" })
        }
        LazyColumn(Modifier.widthIn(max = 600.dp).fillMaxWidth(), state = list, verticalArrangement = Arrangement.spacedBy(12.dp), contentPadding = PaddingValues(top = 8.dp, bottom = 24.dp)) {
            if (rows.isEmpty() && (filter != Filter.ATTENTION || state.deliveries.none { d -> d.state != "delivered" && state.records.none { it.ledger?.token == d.token } })) item {
                if (filter == Filter.ATTENTION) EmptyState("Nothing needs attention", "Saved results that were not inserted, and failed transcriptions, appear here.", "check")
                else EmptyState(if (query.isBlank()) "No dictations yet" else "No matching dictations", "Record something worth keeping.", "History")
            }
            if (filter == Filter.ATTENTION) {
                // Saved results whose recording is gone stay copyable here (the old Text to recover page),
                // newest first together with the dictations that need attention.
                val orphans = state.deliveries.filter { d -> d.state != "delivered" && state.records.none { it.ledger?.token == d.token } }
                val entries = (rows.map { it.created to (it as Any) } + orphans.map { it.created to (it as Any) }).sortedByDescending { it.first }.map { it.second }
                items(entries, key = { (it as? Dictation)?.id ?: (it as SavedDelivery).token }) { entry ->
                    if (entry is Dictation) AttentionCard(vm, entry, navigate) else OrphanResult(vm, entry as SavedDelivery)
                }
            } else items(rows, key = { it.id }) { row -> DictationRow(row) { navigate("History/${row.id}") } }
            if (state.historyNext != null) item {
                TextButton(onClick = vm::loadMoreHistory, modifier = Modifier.heightIn(min = 48.dp)) { Text("Load more dictations") }
            }
        }
    }
}

/** A History row: two lines of the text, then time · duration · tag · version. */
@Composable internal fun DictationRow(row: Dictation, click: () -> Unit) {
    val version = row.defaultVersion()
    val (tag, status) = row.rowTag()
    val time = DateFormat.getDateTimeInstance(DateFormat.SHORT, DateFormat.SHORT).format(Date(row.created))
    Surface(onClick = click, shape = MaterialTheme.shapes.medium, color = MaterialTheme.colorScheme.surfaceContainerLow,
        modifier = Modifier.fillMaxWidth().testTag("dictation-${row.id}").semantics(mergeDescendants = true) { }) {
        Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            SpokenText(version?.text ?: if (row.audioAvailable) "Saved audio" else "No saved text", maxLines = 2)
            FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(4.dp), itemVerticalAlignment = Alignment.CenterVertically) {
                Figures(time, style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
                Figures(formatClock(row.duration), style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
                StatusPill(tag, status)
                version?.let { Text("${it.label} · ${it.attempt.whereLabel()}", style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.onSurfaceVariant) }
                if (row.historicalSource != null) Text("Separate version", style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
        }
    }
}

/** A Needs attention card with its one inline next action. "Check the field" offers Copy only. */
@Composable private fun AttentionCard(vm: WriterViewModel, row: Dictation, navigate: (String) -> Unit) {
    val attention = row.attention() ?: return
    val busy = vm.manager.isBusy() || row.busy
    val body = when (attention) {
        Attention.NOT_INSERTED -> deliveryReason(row.ledger?.reasonCode.orEmpty())
        Attention.UNCONFIRMED -> "Writer could not read the field back. It may already be there; check before copying."
        Attention.FAILED -> (row.view?.error?.message ?: row.note).ifBlank { "Transcription failed." } + " Audio saved. Nothing was inserted."
        Attention.INTERRUPTED -> "Writer stopped while this was in progress. Audio saved. It will not be typed anywhere."
        Attention.POLISH_INCOMPLETE -> "The original is saved. Polishing did not finish."
        Attention.NOT_TRANSCRIBED -> "Audio saved on this phone. It has not been transcribed."
    }
    Notice(attention.tag, body, if (attention == Attention.FAILED) Status.Err else Status.Warn, Modifier.testTag("attention-${row.id}")) {
        row.defaultVersion()?.let { Box(Modifier.fillMaxWidth()) { SpokenText(it.text, maxLines = 2) } }
        when (attention) {
            Attention.NOT_INSERTED -> {
                Button(onClick = { vm.copySaved(row.id) }, modifier = Modifier.heightIn(min = 48.dp)) { Text("Copy") }
                TextButton(onClick = { vm.markHandled(row.id) }, modifier = Modifier.heightIn(min = 48.dp)) { Text("Mark handled") }
            }
            Attention.UNCONFIRMED -> {
                Button(onClick = { vm.copySaved(row.id) }, modifier = Modifier.heightIn(min = 48.dp)) { Text("Copy") }
                TextButton(onClick = { vm.markHandled(row.id) }, modifier = Modifier.heightIn(min = 48.dp)) { Text("It's there") }
            }
            Attention.FAILED, Attention.INTERRUPTED, Attention.NOT_TRANSCRIBED ->
                Button(onClick = { vm.retry(row.id) }, enabled = row.audioAvailable && !busy, modifier = Modifier.heightIn(min = 48.dp)) { Text("Transcribe again") }
            Attention.POLISH_INCOMPLETE -> Button(onClick = { vm.resume(row.id) }, enabled = !busy, modifier = Modifier.heightIn(min = 48.dp)) { Text("Finish polishing") }
        }
        TextButton(onClick = { navigate("History/${row.id}") }, modifier = Modifier.heightIn(min = 48.dp)) { Text("Open") }
    }
}

@Composable private fun OrphanResult(vm: WriterViewModel, delivery: SavedDelivery) {
    val context = LocalContext.current
    Notice(if (delivery.fixture) "Delivery test" else if (delivery.state == "unconfirmed") "Check the field" else "Saved result",
        if (delivery.state == "unconfirmed") "Check the field you were in before copying." else deliveryReason(delivery.reasonCode), Status.Warn) {
        Box(Modifier.fillMaxWidth()) { SpokenText(delivery.text, maxLines = 3) }
        Button(onClick = { copyToClipboard(context, "$PRODUCT_NAME saved result", delivery.text); vm.notify("Copied.") }, modifier = Modifier.heightIn(min = 48.dp)) { Text("Copy saved result") }
    }
}
