package com.metrix.writer.ui

import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import com.metrix.writer.*
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.withContext
import org.json.JSONObject

/** The visible plan is the exact saved choice the queue executes. */
@Composable internal fun ReprocessHistory(vm: WriterViewModel, state: WriterState, navigate: (String) -> Unit, batchQueue: ReprocessBatch? = null) {
    val context = LocalContext.current
    var open by remember { mutableStateOf(false) }
    var operation by remember { mutableStateOf("transcribe") }
    var selected by remember { mutableStateOf(setOf<String>()) }
    var batches by remember { mutableStateOf(emptyList<JSONObject>()) }
    var paid by remember { mutableStateOf(setOf<String>()) }
    var error by remember { mutableStateOf<String?>(null) }
    var compare by remember { mutableStateOf<Pair<String,String>?>(null) }
    val queue = remember(context, batchQueue) { batchQueue ?: ReprocessBatch.get(context) }
    LaunchedEffect(open) {
        while (open) {
            try { batches = withContext(Dispatchers.IO) { queue.list() } }
            catch (failure: Exception) { error = failure.message }
            delay(650)
        }
    }
    TextButton(onClick = { open = true }) { Text("Reprocess selected recordings") }
    if (open) AlertDialog(onDismissRequest = { open = false }, title = { Text("Reprocess saved recordings") },
        confirmButton = { TextButton(onClick = { open = false }) { Text("Close") } },
        text = { Column(Modifier.fillMaxWidth().heightIn(max=560.dp).verticalScroll(rememberScrollState()), verticalArrangement=Arrangement.spacedBy(12.dp)) {
            Text("Select up to 32 saved items. Review the model and possible charges before starting. Results become separate linked versions and are never inserted automatically. Interrupted batches wait for explicit resume.")
            Row {
                TextButton(onClick = { operation = "transcribe" }) { Text(if(operation=="transcribe") "✓ Saved audio" else "Saved audio") }
                TextButton(onClick = { operation = "cleanup" }) { Text(if(operation=="cleanup") "✓ Saved raw text" else "Saved raw text") }
            }
            Text(if(operation=="transcribe") "Transcribe saved audio using current speech settings" else "Clean up saved raw text using current dictionary, snippets and polishing settings")
            state.records.forEach { row ->
                Row(Modifier.fillMaxWidth()) {
                    Checkbox(checked=row.id in selected, enabled=row.id in selected || selected.size<ReprocessBatch.MAX_ITEMS,
                        onCheckedChange={ checked -> selected = if(checked) selected+row.id else selected-row.id })
                    Column { Text(java.text.DateFormat.getDateTimeInstance().format(java.util.Date(row.created)))
                        Text("${row.latest?.model ?: "No saved text"} · ${row.capture}",style=MaterialTheme.typography.bodySmall)
                        if (!row.audioAvailable) Text("Audio unavailable · saved-text cleanup remains possible",style=MaterialTheme.typography.bodySmall)
                    }
                }
            }
            Text("Audio availability is checked per item in the plan. Missing audio does not prevent cleanup of saved raw text.",style=MaterialTheme.typography.bodySmall)
            if (state.historyNext != null) TextButton(onClick=vm::loadMoreHistory) { Text("Load more saved items") }
            Button(enabled=selected.isNotEmpty() && !state.actionBusy, onClick={ vm.action { queue.plan(selected.toList(), operation) } }) { Text("Review batch plan (${selected.size})") }
            error?.let { Text(it, color=MaterialTheme.colorScheme.error) }
            batches.forEach { batch ->
                val id=batch.getString("id"); val rows=batch.getJSONArray("items"); val status=batch.getString("state")
                HorizontalDivider(); Text("${batch.getString("operation")} · $status",style=MaterialTheme.typography.titleSmall)
                Text(batch.getString("provider_model"))
                val choice=batch.getJSONObject("choice"); val options=choice.getJSONObject("options")
                Text("Language: ${options.getString("input_language")} · ${options.getJSONObject("output_language").getString("mode")}")
                val policy=choice.optJSONObject("policy")
                policy?.optJSONObject("fallback")?.let { Text("Fallback: ${it.getString("provider")} / ${it.getString("model")}") }
                if(batch.getBoolean("paid_work")) Text("${batch.getInt("likely_paid_items")} likely paid items; up to ${rows.length()} selected items may use cloud models. Fallbacks can add requests. Provider rates apply; interrupted requests may already be billed.")
                if(status in setOf("planned","paused")) {
                    if(batch.getBoolean("paid_work")) Row {
                        Checkbox(checked=id in paid,onCheckedChange={ paid=if(it) paid+id else paid-id })
                        Text("I understand this batch may incur cloud charges")
                    }
                    Button(enabled=!state.actionBusy && (id in paid || !batch.getBoolean("paid_work")),onClick={vm.action {queue.start(id,id in paid)}}) { Text(if(status=="paused") "Resume batch explicitly" else "Start reviewed batch") }
                }
                if(status in setOf("planned","paused","running")) TextButton(onClick={vm.action {queue.cancel(id)}}) { Text("Cancel batch") }
                if(status!="running") TextButton(onClick={vm.action {queue.delete(id)}}) { Text("Delete batch plan") }
                (0 until rows.length()).forEach { index ->
                    val item=rows.getJSONObject(index)
                    Text("Item ${index+1} · ${item.getString("state")}${if(item.optBoolean("reuse_available")) " · matching stage available" else ""}")
                    item.optString("error").takeIf{it.isNotEmpty()}?.let{Text(it,color=MaterialTheme.colorScheme.error)}
                    if(item.getString("state")=="succeeded") {
                        TextButton(onClick={
                            vm.action {
                                val original = runCatching { CompanionProcessing.find(vm.manager.store.load(item.getString("source_session_id")),item.getString("source_job_id")) }.getOrNull()
                                val jobs=vm.manager.store.load(item.getString("version_session_id")).getJSONArray("attempts")
                                val version=jobs.getJSONObject(jobs.length()-1)
                                val text=version.optString("cleaned_text",version.optString("dictionary_text",CompanionProcessing.raw(version)))
                                withContext(Dispatchers.Main) { compare=(original?.let(CompanionProcessing::deliveryText) ?: "No earlier saved text") to ("Raw text:\n${CompanionProcessing.raw(version)}\n\nNew result:\n$text") }
                            }
                        }) { Text("Compare version${if(item.has("reused_from")) " · reused successful stage" else ""}") }
                        TextButton(onClick={open=false;navigate("History/${item.getString("version_session_id")}")}) { Text("Open separate version") }
                    }
                }
            }
        } })
    compare?.let { texts -> AlertDialog(onDismissRequest={compare=null},title={Text("Compare historical version")},confirmButton={TextButton(onClick={compare=null}){Text("Close")}},
        text={Column(Modifier.verticalScroll(rememberScrollState())){Text("Earlier result",style=MaterialTheme.typography.titleSmall);Text(texts.first);HorizontalDivider();Text(texts.second)}}) }
}
