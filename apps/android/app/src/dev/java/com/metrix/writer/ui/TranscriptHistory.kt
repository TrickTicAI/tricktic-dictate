package com.metrix.writer.ui

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.util.AtomicFile
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.unit.dp
import com.metrix.writer.account.AuthState
import com.metrix.writer.sync.TranscriptStore
import com.metrix.writer.sync.TranscriptSync
import com.metrix.writer.sync.objects
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.io.File

@Composable internal fun TranscriptConsent(engineOverride: TranscriptSync? = null) {
    val context = LocalContext.current
    val engine = remember { engineOverride ?: WriterAccounts.transcripts(context) }
    val status by engine.status.collectAsState()
    val scope = rememberCoroutineScope()
    var mode by rememberSaveable { mutableStateOf("future_only") }
    var understood by rememberSaveable { mutableStateOf(false) }
    var busy by remember { mutableStateOf(false) }
    var message by remember { mutableStateOf<String?>(null) }
    Section("Transcript sync", "Separate from settings and dictionary. Signing in never uploads History.") {
        Text("Text only: original wording, corrections, polishing and your edits. Audio, saved keys and retry requests stay on this phone.")
        Text("Firebase storage is encrypted in transit and at rest, but service administrators can read it. It is not end-to-end encrypted.")
        if (!engine.uploadsAllowed) Text("Transcript uploads are disabled in this build.", modifier = Modifier.testTag("transcripts-privacy-hold"))
        if (!status.enabled) {
            Row { RadioButton(mode == "future_only", { mode = "future_only" }); Text("Only future recordings", Modifier.padding(top = 12.dp)) }
            Row { RadioButton(mode == "include_existing", { mode = "include_existing" }); Text("Include existing history on this phone", Modifier.padding(top = 12.dp)) }
            Row { Checkbox(understood, { understood = it }, modifier = Modifier.testTag("transcripts-consent")); Text("I agree to store transcript text with my account.", Modifier.padding(top = 12.dp)) }
        }
        fun act(block: suspend () -> Unit) {
            busy = true; message = null
            scope.launch {
                message = try { withContext(Dispatchers.IO) { block() }; null } catch (_: Exception) { "Could not complete this change. Your saved text remains on this phone." }
                busy = false
            }
        }
        if (status.enabled) {
            Text("Transcript sync is on. ${status.pending} changes waiting.")
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                OutlinedButton({ act { engine.runOnce() } }, enabled = !busy) { Text("Sync text now") }
                TextButton({ act { engine.disable(); engine.runOnce() } }, enabled = !busy) { Text("Turn off transcript sync") }
            }
        } else Button({ act { engine.enable(mode); engine.runOnce() } }, enabled = understood && engine.uploadsAllowed && !busy,
            modifier = Modifier.testTag("transcripts-enable")) { Text("Turn on transcript sync") }
        Text("Turning off stops new transfers. Already uploaded text remains in your account; use Delete cloud data to remove it.", style = MaterialTheme.typography.bodySmall)
        (message ?: status.problem?.let(::transcriptProblem))?.let { Text(it) }
    }
}

private fun transcriptProblem(kind: String) = when (kind) {
    "offline" -> "Offline. Text changes are saved and retry when this phone reconnects."
    "not_set_up" -> "Cloud storage is not set up for this build."
    "turned_off" -> "Transcript consent changed or cloud data was deleted. Review consent before syncing again."
    "retained" -> "Some versions could not upload. They are retained on this phone."
    "privacy_hold" -> "Transcript uploads are disabled in this build."
    "quota" -> "Cloud storage is full or busy. Saved text will retry later."
    else -> "Text sync needs attention. Saved text remains on this phone; check your sign-in and try again."
}

@Composable internal fun TranscriptHistory(engineOverride: TranscriptSync? = null) {
    val context = LocalContext.current
    val engine = remember { engineOverride ?: WriterAccounts.transcripts(context) }
    val status by engine.status.collectAsState()
    val snapshot by WriterAccounts.get(context).snapshots.collectAsState()
    val scope = rememberCoroutineScope()
    var query by rememberSaveable { mutableStateOf("") }
    var editing by remember { mutableStateOf<String?>(null) }
    var deleting by remember { mutableStateOf<String?>(null) }
    var text by remember { mutableStateOf("") }
    var notice by remember { mutableStateOf<String?>(null) }
    var kept by remember(snapshot.generation) { mutableStateOf(TranscriptStore.read(AtomicFile(File(context.filesDir, "kept-transcripts.json")))?.objects()?.values.orEmpty().toList()) }
    var deleteKept by remember { mutableStateOf<String?>(null) }
    val accountRows = if (status.account == snapshot.account) status.records else emptyList()
    val rows = (accountRows + kept).distinctBy { it.getString("id") }
    Column(Modifier.fillMaxSize().padding(20.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        Text("Text-only copies. Audio is not downloaded; playback, transcription retry and automatic insertion are unavailable.", modifier = Modifier.testTag("transcripts-no-audio"))
        if (snapshot.state == AuthState.GUEST && kept.isEmpty()) Text("Sign in to see this account’s copies. Your local recordings remain in History.")
        OutlinedTextField(query, { query = it }, label = { Text("Search synced text") }, modifier = Modifier.fillMaxWidth())
        notice?.let { Text(it) }
        LazyColumn(verticalArrangement = Arrangement.spacedBy(12.dp)) {
            if (status.next != null) item { TextButton({ scope.launch { withContext(Dispatchers.IO) { engine.loadMore() } } }) { Text("Load more text copies") } }
            if (rows.isEmpty()) item { Text("No synced text on this phone yet.") }
            items(rows.filter { it.getJSONObject("content").getJSONObject("data").getString("text").contains(query, true) }, key = { it.getString("id") }) { record ->
                val id = record.getString("id"); val data = record.getJSONObject("content").getJSONObject("data")
                val value = data.getString("text")
                Card(Modifier.fillMaxWidth()) { Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    Text(when(data.getString("kind")) { "original" -> "Original wording"; "corrected" -> "Corrected wording"; "cleaned" -> "Polished wording"; else -> "Edited version" }, style = MaterialTheme.typography.titleSmall)
                    Text(value)
                    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        TextButton({ context.getSystemService(ClipboardManager::class.java).setPrimaryClip(ClipData.newPlainText("Transcript", value)) }) { Text("Copy") }
                        if (accountRows.any { it.getString("id") == id }) {
                            TextButton({ editing = id; text = value }) { Text("Save an edit") }
                            TextButton({ deleting = id }) { Text("Delete") }
                        } else TextButton({ deleteKept = id }) { Text("Delete local copy") }
                    }
                } }
            }
        }
    }
    deleteKept?.let { id -> ProtectedAlertDialog(onDismissRequest = { deleteKept = null }, title = { Text("Delete the kept copy?") },
        text = { Text("Removes this text and its versions from this phone. Your account and recorded audio are unchanged.") },
        confirmButton = { TextButton({
            synchronized(TranscriptStore.LOCK) {
                val file = AtomicFile(File(context.filesDir, "kept-transcripts.json"))
                val all = TranscriptStore.read(file) ?: org.json.JSONObject()
                all.objects().filter { (key, value) -> key == id || value.getJSONObject("content").getJSONObject("data").optString("original_record_id") == id }
                    .keys.forEach(all::remove)
                TranscriptStore.write(file, all); kept = all.objects().values.toList()
            }
            deleteKept = null
        }) { Text("Delete local copy") } }, dismissButton = { TextButton({ deleteKept = null }) { Text("Cancel") } }) }
    editing?.let { id -> ProtectedAlertDialog(onDismissRequest = { editing = null }, title = { Text("Save a separate edited version") },
        text = { OutlinedTextField(text, { text = it }, modifier = Modifier.fillMaxWidth()) },
        confirmButton = { TextButton({ scope.launch { try { withContext(Dispatchers.IO) { engine.edit(id, text); engine.runOnce() }; editing = null } catch (_: Exception) { notice = "Could not save this edit." } } }) { Text("Save version") } },
        dismissButton = { TextButton({ editing = null }) { Text("Cancel") } }) }
    deleting?.let { id -> ProtectedAlertDialog(onDismissRequest = { deleting = null }, title = { Text("Delete this text?") },
        text = { Text("When sync is on, deletion also reaches your account. Deleting an original removes all its synced versions. Local recorded audio is kept.") },
        confirmButton = { TextButton({ scope.launch { try { withContext(Dispatchers.IO) { engine.delete(id); engine.runOnce() }; deleting = null } catch (_: Exception) { notice = "Could not save this deletion." } } }) { Text("Delete text") } },
        dismissButton = { TextButton({ deleting = null }) { Text("Cancel") } }) }
}
