package com.metrix.writer.ui

import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Modifier
import androidx.compose.ui.Alignment
import androidx.compose.ui.unit.dp
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.*
import com.metrix.writer.CompanionPreferences
import org.json.JSONObject

// Both sides of a pair are searched, the way the desktop snippet search does it:
// you rarely remember the spoken trigger, you remember the text it inserts.
private fun searchable(kind: String, entry: JSONObject): String = when (kind) {
    "corrections" -> entry.optString("from") + "\n" + entry.optString("to")
    "snippets" -> entry.optString("phrase") + "\n" + entry.optString("text")
    else -> entry.optString("text")
}

@Composable fun DictionaryScreen(vm: WriterViewModel, revision: Int) {
    val prefs = remember { CompanionPreferences(vm.context) }
    var kind by rememberSaveable { mutableStateOf("hints") }
    val document = remember(revision) { prefs.document() }
    val array = document.getJSONArray(kind)
    val entries = (0 until array.length()).map { array.getJSONObject(it) }
    var query by rememberSaveable { mutableStateOf("") }
    val term = query.trim()
    // Keep each entry's own position in the saved document: editing, enabling and
    // deleting address the stored array, never the filtered view.
    val matches = entries.withIndex().filter { (_, entry) -> term.isEmpty() || searchable(kind, entry).contains(term, ignoreCase = true) }
    val noun = if (kind == "hints") "word" else if (kind == "snippets") "snippet" else "correction"
    var editing by rememberSaveable { mutableStateOf<Int?>(null) }
    var delete by rememberSaveable { mutableStateOf<Int?>(null) }
    Column(Modifier.fillMaxSize().padding(horizontal = 20.dp), verticalArrangement = Arrangement.spacedBy(14.dp)) {
        Text("A vocabulary that knows you.", style = MaterialTheme.typography.headlineSmall)
        FlowRow(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
            listOf("hints" to "Words", "corrections" to "Corrections", "snippets" to "Snippets").forEach { (value,title) -> FilterChip(kind == value, { kind = value }, label = { Text(title) }) }
        }
        Text(when(kind) { "hints" -> "Names and terms to help recognition."; "corrections" -> "Replace a recurring spelling with your preferred text."; else -> "Say a short trigger to insert exact text." }, color = MaterialTheme.colorScheme.onSurfaceVariant)
        OutlinedTextField(query, { query = it }, label = { Text("Search ${noun}s") }, singleLine = true, modifier = Modifier.fillMaxWidth().testTag("dictionary-search"),
            trailingIcon = { if (query.isNotEmpty()) IconButton(onClick = { query = "" }, modifier = Modifier.semantics { contentDescription = "Clear search" }) { WriterIcon("close") } })
        if (entries.isNotEmpty()) Text("${matches.size} of ${entries.size} ${noun}${if (entries.size == 1) "" else "s"}", style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
        Button(onClick = { editing = -1 }) { WriterIcon("add", tint = MaterialTheme.colorScheme.onPrimary); Text("Add entry") }
        LazyColumn(verticalArrangement = Arrangement.spacedBy(10.dp), contentPadding = PaddingValues(bottom = 24.dp)) {
            if (matches.isEmpty()) item {
                if (term.isEmpty()) EmptyState("Make it yours", "Add your first $noun.", "Dictionary")
                else Column(Modifier.fillMaxWidth(), horizontalAlignment = Alignment.CenterHorizontally) {
                    EmptyState("No matching entries", "No $noun here matches “$term”. The other half of a saved pair is searched too.", "Dictionary")
                    TextButton(onClick = { query = "" }) { Text("Clear search") }
                }
            }
            items(matches) { (index, entry) ->
                Surface(shape = MaterialTheme.shapes.medium, color = MaterialTheme.colorScheme.surfaceContainerLow) {
                    Column(Modifier.fillMaxWidth().padding(16.dp)) {
                        Text(entry.optString(if (kind == "corrections") "from" else if (kind == "snippets") "phrase" else "text"), style = MaterialTheme.typography.titleMedium)
                        if (kind != "hints") Text(entry.getString(if(kind == "corrections") "to" else "text"), color = MaterialTheme.colorScheme.onSurfaceVariant)
                        Row(verticalAlignment = Alignment.CenterVertically) {
                            TextButton(onClick = { editing = index }) { Text("Edit") }
                            TextButton(onClick = { delete = index }) { Text("Delete") }
                            Spacer(Modifier.weight(1f))
                            Switch(entry.optBoolean("enabled", true), { enabled -> vm.action { val fresh = prefs.document(); fresh.getJSONArray(kind).getJSONObject(index).put("enabled", enabled); prefs.saveDocument(fresh) } }, modifier = Modifier.semantics { contentDescription = "Enable " + entry.optString(if(kind == "corrections") "from" else if(kind == "snippets") "phrase" else "text") })
                        }
                    }
                }
            }
        }
    }
    editing?.let { index ->
        val entry = if (index < 0) JSONObject() else entries[index]
        val keys = when (kind) { "hints" -> listOf("text"); "corrections" -> listOf("from", "to"); else -> listOf("phrase", "text") }
        var first by rememberSaveable(kind,index) { mutableStateOf(entry.optString(keys[0])) }
        var second by rememberSaveable(kind,index) { mutableStateOf(if (keys.size == 2) entry.optString(keys[1]) else "") }
        var error by remember { mutableStateOf<String?>(null) }
        ProtectedAlertDialog(onDismissRequest = { editing = null }, title = { Text(if(index < 0) "Add entry" else "Edit entry") }, text = {
            Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
                OutlinedTextField(first, { first = it }, label = { Text(when(kind) { "hints" -> "Word or name"; "corrections" -> "Spoken text"; else -> "Spoken trigger" }) })
                if (keys.size == 2) OutlinedTextField(second, { second = it }, label = { Text(if(kind == "corrections") "Replacement" else "Exact inserted text") }, minLines = if(kind == "snippets") 3 else 1)
                error?.let { Text(it, color = MaterialTheme.colorScheme.error) }
            }
        }, confirmButton = { TextButton(onClick = {
            try {
                val fresh = prefs.document(); val values = fresh.getJSONArray(kind)
                val value = JSONObject().put(keys[0], first).put("enabled", entry.optBoolean("enabled", true))
                if(keys.size == 2) value.put(keys[1], second)
                // An edit keeps the entry's identity, so sync treats it as the same entry.
                if(index >= 0) value.put("id", entry.getString("id")).put("origin", entry.getString("origin"))
                if(index < 0) values.put(value) else values.put(index,value)
                prefs.saveDocument(fresh); editing = null; vm.notify("Dictionary saved.")
            } catch (e: Exception) { error = e.message }
        }) { Text("Save") } }, dismissButton = { TextButton(onClick = { editing = null }) { Text("Cancel") } })
    }
    delete?.let { index -> ProtectedAlertDialog(onDismissRequest = { delete = null }, title = { Text("Delete this entry?") }, text = { Text("Saved dictations keep their original dictionary snapshot.") },
        confirmButton = { TextButton(onClick = { vm.action { val fresh=prefs.document();fresh.getJSONArray(kind).remove(index);prefs.saveDocument(fresh) };delete=null }) { Text("Delete") } }, dismissButton = { TextButton(onClick = { delete = null }) { Text("Cancel") } }) }
}
