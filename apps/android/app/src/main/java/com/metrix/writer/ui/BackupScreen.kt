package com.metrix.writer.ui

import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.*
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.unit.dp
import com.metrix.writer.PRODUCT_NAME
import com.metrix.writer.sync.Contract
import com.metrix.writer.sync.ContractException
import com.metrix.writer.sync.ImportPlan
import com.metrix.writer.sync.PortableStore
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/** Fixed text for a refused import file. The file's contents are never shown or logged. */
internal fun importProblem(error: ContractException) = when (error.reason) {
    "unsupported_version" -> "This file was made by a newer version of $PRODUCT_NAME. Update to import it. Nothing was changed."
    "too_large" -> "This file is larger than a $PRODUCT_NAME export can be. Nothing was changed."
    "duplicate_id" -> "This file lists the same entry twice. Nothing was changed."
    else -> "This isn't a valid $PRODUCT_NAME settings export. Nothing was changed."
}

/** One preview row; the same rows serve file import and account restore. */
@Composable internal fun PreviewRow(title: String, detail: String, note: String?, trailing: @Composable () -> Unit = {}) {
    Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        Column(Modifier.weight(1f)) {
            Text(title, style = MaterialTheme.typography.bodyLarge)
            Text(detail, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            note?.let { Text(it, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.tertiary) }
        }
        trailing()
    }
}

/** Settings → Export & import: move settings and the dictionary between devices without an account. */
@Composable internal fun BackupPage(vm: WriterViewModel) {
    val context = vm.context
    val store = remember { PortableStore(context) }
    val scope = rememberCoroutineScope()
    var settings by rememberSaveable { mutableStateOf(true) }
    var dictionary by rememberSaveable { mutableStateOf(true) }
    var replace by rememberSaveable { mutableStateOf(false) }
    var file by remember { mutableStateOf<ByteArray?>(null) }
    var plan by remember { mutableStateOf<ImportPlan?>(null) }
    var keepLocal by remember { mutableStateOf(mapOf<String, Boolean>()) }
    var message by remember { mutableStateOf<String?>(null) }
    var busy by remember { mutableStateOf(false) }
    var undo by remember { mutableStateOf<Triple<String, String, Long>?>(null) }
    LaunchedEffect(Unit) { undo = withContext(Dispatchers.IO) { store.undoAvailable() } }

    fun run(block: suspend () -> String?) {
        if (busy) return
        busy = true; message = null
        scope.launch {
            message = try { withContext(Dispatchers.IO) { block() } } catch (error: ContractException) { importProblem(error) }
                catch (error: Exception) { error.message?.takeIf { error is IllegalStateException } ?: "That didn't work. Nothing was changed." }
            undo = withContext(Dispatchers.IO) { store.undoAvailable() }
            busy = false
        }
    }
    fun previewAgain() {
        val bytes = file ?: return
        val categories = setOfNotNull("settings".takeIf { settings }, "dictionary".takeIf { dictionary })
        if (categories.isEmpty()) { plan = null; return }
        run { plan = store.previewFile(bytes, categories, if (replace) "replace_selected" else "merge"); keepLocal = emptyMap(); null }
    }
    val exporter = rememberLauncherForActivityResult(ActivityResultContracts.CreateDocument("application/json")) { uri ->
        if (uri != null) run {
            val backup = store.exportBackup()
            context.contentResolver.openOutputStream(uri, "wt")!!.use { it.write(backup.toString(2).toByteArray(Charsets.UTF_8)) }
            "Exported ${backup.getJSONArray("settings").length()} settings and ${backup.getJSONArray("dictionary").length()} dictionary entries. No keys, recordings, history or account are in the file."
        }
    }
    val importer = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
        if (uri != null) run {
            val bytes = context.contentResolver.openInputStream(uri)!!.use { it.readNBytes(Contract.MAX_FILE_BYTES + 1) }
            if (bytes.size > Contract.MAX_FILE_BYTES) throw ContractException("too_large")
            val categories = setOfNotNull("settings".takeIf { settings }, "dictionary".takeIf { dictionary })
            check(categories.isNotEmpty()) { "Choose settings, dictionary or both first." }
            plan = store.previewFile(bytes, categories, if (replace) "replace_selected" else "merge")
            file = bytes; keepLocal = emptyMap()
            null
        }
    }

    Page {
        com.metrix.writer.ui.settings.SettingsSection("Export", "Save your settings and dictionary to a file you can import on another phone or computer. No account needed.") {
            Text("Included: speech language, dictation engine, polishing and wording choices, polishing instructions, theme, and every dictionary word, correction and snippet.")
            Text("Never included: API keys, recordings, transcripts, microphone, floating control, permissions and downloaded models.", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            Button(onClick = { exporter.launch("tricktic-dictate-settings.json") }, enabled = !busy, modifier = Modifier.testTag("backup-export")) { Text("Export to a file…") }
            Text("Anything you typed into a snippet is in the file as written. Keep it somewhere private.", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
        com.metrix.writer.ui.settings.SettingsSection("Import", "Nothing changes until you review the preview and tap Apply.") {
            Row(verticalAlignment = Alignment.CenterVertically) { Checkbox(settings, { settings = it; previewAgain() }, enabled = !busy); Text("Settings") }
            Row(verticalAlignment = Alignment.CenterVertically) { Checkbox(dictionary, { dictionary = it; previewAgain() }, enabled = !busy); Text("Dictionary") }
            Row(verticalAlignment = Alignment.CenterVertically) {
                Switch(replace, { replace = it; previewAgain() }, enabled = !busy && dictionary, modifier = Modifier.testTag("backup-replace"))
                Spacer(Modifier.width(12.dp))
                Text(if (replace) "Replace: remove dictionary entries that aren't in the file" else "Merge: add to what's on this phone")
            }
            OutlinedButton(onClick = { importer.launch(arrayOf("application/json", "text/plain", "application/octet-stream")) }, enabled = !busy && (settings || dictionary), modifier = Modifier.testTag("backup-import")) { Text("Choose a file…") }
        }
        plan?.let { current ->
            com.metrix.writer.ui.settings.SettingsSection("Preview") {
                val shown = current.items.filter { it.action != "unchanged" }
                val same = current.items.count { it.action == "unchanged" }
                if (shown.isEmpty()) Text("Everything in this file is already on this phone.")
                shown.forEach { item ->
                    when (item.action) {
                        "conflict" -> PreviewRow(item.title, item.detail, "Different on this phone. Choose which to keep.") {
                            Column {
                                FilterChip(keepLocal[item.id] == true, { keepLocal = keepLocal + (item.id to true) }, label = { Text("This phone") })
                                FilterChip(keepLocal[item.id] == false, { keepLocal = keepLocal + (item.id to false) }, label = { Text("File") })
                            }
                        }
                        "preserve" -> PreviewRow(item.title, item.detail, "Kept for export, not applied: ${item.reason}")
                        "remove" -> PreviewRow(item.title, item.detail, null)
                        else -> PreviewRow(item.title, item.detail, null)
                    }
                }
                if (same > 0) Text("$same already the same on this phone.", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                Text("Applying never downloads a model, adds a key, or starts a recording or cloud request.", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    Button(onClick = { run {
                        val receipt = store.apply(current.importId, keepLocal)
                        if (receipt.getString("state") == "applied") { plan = null; file = null; "Imported. You can undo this import below." }
                        else when (receipt.getString("reason")) {
                            "stale_preview" -> { plan = null; "Settings or the dictionary changed since this preview. Choose the file again. Nothing was changed." }
                            "conflict" -> "Choose which version to keep for each difference first."
                            else -> "Could not save the import. Nothing was changed."
                        }
                    } }, enabled = !busy && current.conflicts.all { it.id in keepLocal }, modifier = Modifier.testTag("backup-apply")) { Text("Apply") }
                    TextButton(onClick = { plan = null; file = null }, enabled = !busy) { Text("Cancel") }
                }
            }
        }
        undo?.let { (snapshot, importId, revision) ->
            com.metrix.writer.ui.settings.SettingsSection("Undo") {
                Text("Put back the settings and dictionary from before the last import or restore. Available until you discard it or import again.")
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    OutlinedButton(onClick = { run {
                        val receipt = store.rollback(importId, snapshot, revision)
                        if (receipt.getString("state") == "applied") "Undone. Your earlier settings and dictionary are back." else "Something changed after that import, so it can't be undone safely. Nothing was changed."
                    } }, enabled = !busy, modifier = Modifier.testTag("backup-undo")) { Text("Undo last import") }
                    TextButton(onClick = { run { store.discardUndo(); null } }, enabled = !busy) { Text("Discard") }
                }
            }
        }
        if (busy) LinearProgressIndicator(Modifier.fillMaxWidth())
        message?.let { Text(it, modifier = Modifier.testTag("backup-message")) }
    }
}
