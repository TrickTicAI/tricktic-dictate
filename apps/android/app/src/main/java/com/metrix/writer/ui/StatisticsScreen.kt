package com.metrix.writer.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.*
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Modifier
import androidx.compose.ui.Alignment
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.semantics.*
import androidx.compose.ui.unit.dp

@Composable fun StatisticsScreen(records: List<Dictation>) {
    var range by rememberSaveable { mutableIntStateOf(7) }
    var exact by rememberSaveable { mutableStateOf(false) }
    var exactModels by rememberSaveable { mutableStateOf(false) }
    val stats = remember(records, range) { WriterStatistics.compute(records, range) }
    Page {
        Text("Your words, in perspective.", style = MaterialTheme.typography.headlineSmall)
        Text("Based on dictations saved on this phone.", color = MaterialTheme.colorScheme.onSurfaceVariant)
        FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            listOf(7 to "7 days", 30 to "30 days", 0 to "All history").forEach { (value,label) -> FilterChip(selected = range == value, onClick = { range = value }, label = { Text(label) }) }
        }
        Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
            Metric("Words dictated", stats.words.toString(), Modifier.weight(1f))
            Metric("Dictation time", formatDuration(stats.duration), Modifier.weight(1f))
        }
        Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
            Metric("Dictations", stats.sessions.toString(), Modifier.weight(1f))
            Metric("Speaking pace", stats.pace?.let { "$it wpm" } ?: "—", Modifier.weight(1f))
        }
        Section("Daily activity", if (range == 0) "Latest 30 days · totals above use all retained history" else "Words from successful original transcripts") {
            val maximum = stats.days.maxOfOrNull { it.second }?.coerceAtLeast(1) ?: 1
            Row(Modifier.fillMaxWidth().height(110.dp), verticalAlignment = Alignment.Bottom, horizontalArrangement = Arrangement.spacedBy(4.dp)) {
                stats.days.forEach { (date, words) ->
                    Box(Modifier.weight(1f).fillMaxHeight().semantics { contentDescription = "$date: $words words" }, contentAlignment = Alignment.BottomCenter) {
                        Box(Modifier.fillMaxWidth().height((if (words == 0) 2f else words * 105f / maximum).dp).background(if (words == 0) MaterialTheme.colorScheme.outlineVariant else MaterialTheme.colorScheme.primary, MaterialTheme.shapes.small))
                    }
                }
            }
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween) { Text(stats.days.first().first.toString(), style = MaterialTheme.typography.labelSmall); Text(stats.days.last().first.toString(), style = MaterialTheme.typography.labelSmall) }
            TextButton(onClick = { exact = !exact }) { Text(if (exact) "Hide daily totals" else "Show daily totals") }
            if (exact) stats.days.forEach { (day, words) -> Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween) { Text(day.toString()); Text("$words words") } }
        }
        Section("Reliability") {
            val finished = stats.successes + stats.failures
            StatRow("Successful attempts", if (finished == 0) "—" else "${stats.successes} / $finished")
            StatRow("Retries", stats.retries.toString())
            StatRow("Original used after polishing", stats.originalsUsed.toString())
            StatRow("Original inserted", stats.originalsInserted.toString())
            StatRow("Median stop-to-result", stats.medianLatency?.let { "${it / 1000.0}s" } ?: "—")
            Text("Timing includes work after Stop until a saved result. It does not measure insertion. Older records without timing are excluded.", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
        val series = stats.speechModels + stats.textModels
        if (stats.audioPreparation.isNotEmpty()) Section("Audio preparation", "Measured transcription attempts, including retries") {
            StatRow("Silence shortened", formatDuration(stats.audioPreparation.sumOf { it.removedMs }))
            val before = stats.audioPreparation.sumOf { it.inputBytes } / 1_000_000.0
            val after = stats.audioPreparation.sumOf { it.outputBytes } / 1_000_000.0
            StatRow("Audio size before / after", "${"%.2f".format(before)} / ${"%.2f".format(after)} MB")
            StatRow("Typical preparation", formatTiming(WriterStatistics.percentile(stats.audioPreparation.map { it.preparationMs }, 0.5)))
            Text("Saved recordings and playback keep every original frame. Only the audio sent to the model is shortened.", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
        Section("Your speech models", "Transcription attempts, including retries") {
            if (stats.speechModels.isEmpty()) Text("Model usage will appear after your first attempt.", color = MaterialTheme.colorScheme.onSurfaceVariant)
            stats.speechModels.forEach { ModelRow(it, "attempt", seriesColor(series, it.key)) }
            if (stats.speechModels.any { it.timings.isEmpty() }) Text("Typical and 90% cover successful attempts that saved a stop-to-result measurement. Attempts without one are excluded, never counted as zero.", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
        Section("Your text models", "Cleanup and polishing edits by model") {
            if (stats.textModels.isEmpty()) Text("Text model use appears after your first cleanup with a model selected.", color = MaterialTheme.colorScheme.onSurfaceVariant)
            stats.textModels.forEach { ModelRow(it, "attempt", seriesColor(series, it.key)) }
            if (stats.textModels.isNotEmpty()) Text("Primary and fallback attempts count under the model that ran. Using the original does not count as successful polishing. Each attempt counts on its recording's date; missing times and abandoned requests are excluded from successful timing.", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
        Section("Daily model use", if (range == 0) "Latest 30 days · attempts and edits by model" else "Last $range days · attempts and edits by model") {
            val maximum = stats.modelDays.maxOfOrNull { day -> day.second.values.sum() }?.coerceAtLeast(1) ?: 1
            if (series.isEmpty()) Text("No model attempts in this period.", color = MaterialTheme.colorScheme.onSurfaceVariant)
            else {
                Row(Modifier.fillMaxWidth().height(110.dp), verticalAlignment = Alignment.Bottom, horizontalArrangement = Arrangement.spacedBy(4.dp)) {
                    stats.modelDays.forEach { (date, counts) ->
                        val total = counts.values.sum()
                        Box(Modifier.weight(1f).fillMaxHeight().semantics { contentDescription = "$date: ${describe(series, counts)}" }, contentAlignment = Alignment.BottomCenter) {
                            Column(Modifier.fillMaxWidth().height((if (total == 0) 2f else total * 105f / maximum).dp).clip(MaterialTheme.shapes.small)) {
                                if (total == 0) Box(Modifier.fillMaxSize().background(MaterialTheme.colorScheme.outlineVariant))
                                else series.filter { counts.containsKey(it.key) }.forEach { model ->
                                    Box(Modifier.fillMaxWidth().weight(counts.getValue(model.key).toFloat()).background(seriesColor(series, model.key)))
                                }
                            }
                        }
                    }
                }
                Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween) { Text(stats.modelDays.first().first.toString(), style = MaterialTheme.typography.labelSmall); Text(stats.modelDays.last().first.toString(), style = MaterialTheme.typography.labelSmall) }
                TextButton(onClick = { exactModels = !exactModels }) { Text(if (exactModels) "Hide daily model use" else "Show daily model use") }
                if (exactModels) {
                    val used = stats.modelDays.filter { it.second.values.sum() > 0 }
                    if (used.isEmpty()) Text("No attempts or edits in the chart's days.", color = MaterialTheme.colorScheme.onSurfaceVariant)
                    used.forEach { (date, counts) -> Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(12.dp)) { Text(date.toString()); Text(describe(series, counts), Modifier.weight(1f), style = MaterialTheme.typography.labelLarge) } }
                }
            }
        }
        Text("Words count once per dictation. Speaking pace includes pauses. Deleting history changes these totals.", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
    }
}
@Composable private fun Metric(label: String, value: String, modifier: Modifier) {
    Surface(modifier, shape = MaterialTheme.shapes.large, color = MaterialTheme.colorScheme.surfaceContainerLow) {
        Column(Modifier.padding(18.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text(value, style = MaterialTheme.typography.headlineMedium, color = MaterialTheme.colorScheme.primary)
            Text(label, style = MaterialTheme.typography.labelMedium)
        }
    }
}
/** Stable colour per model row, so a bar segment and its row read as the same series. */
@Composable private fun seriesColor(series: List<ModelUse>, key: String): Color {
    val palette = listOf(MaterialTheme.colorScheme.primary, MaterialTheme.colorScheme.tertiary, MaterialTheme.colorScheme.secondary,
        MaterialTheme.colorScheme.primaryContainer, MaterialTheme.colorScheme.tertiaryContainer, MaterialTheme.colorScheme.secondaryContainer)
    return palette[series.indexOfFirst { it.key == key }.coerceAtLeast(0) % palette.size]
}
private fun describe(series: List<ModelUse>, counts: Map<String, Int>): String {
    val named = series.filter { counts.containsKey(it.key) }.joinToString(" · ") { model ->
        // Two providers can offer the same model name; only then is the qualifier needed.
        val name = if (series.count { it.label == model.label } > 1) "${model.label} · ${model.detail}" else model.label
        "$name ${counts.getValue(model.key)}"
    }
    return if (named.isEmpty()) "no attempts" else named
}
/** Millisecond-scale timing; an unmeasured value stays "—" and is never shown as zero. */
private fun formatTiming(ms: Long?) = when {
    ms == null -> "—"
    ms < 1000 -> "$ms ms"
    ms < 60000 -> "${"%.1f".format(ms / 1000.0)} s"
    else -> "${ms / 60000}m ${(ms % 60000) / 1000}s"
}
@Composable private fun ModelRow(model: ModelUse, noun: String, color: Color) {
    Column(verticalArrangement = Arrangement.spacedBy(4.dp), modifier = Modifier.fillMaxWidth()) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Box(Modifier.size(10.dp).clip(MaterialTheme.shapes.small).background(color))
            Text(model.label, style = MaterialTheme.typography.titleSmall)
            Text(model.detail, style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
        Text("${model.successes} successful / ${model.attempts} ${noun}${if (model.attempts == 1) "" else "s"}" +
            (model.rate?.let { " · $it% of ${model.finished} finished" } ?: " · none finished yet"),
            style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        if (model.timings.isEmpty()) Text("Timing unavailable for this model in this period.", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        else Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(12.dp)) {
            Text("Typical ${formatTiming(model.median)}", Modifier.weight(1f), style = MaterialTheme.typography.labelLarge)
            Text("90% within ${formatTiming(model.p90)}", style = MaterialTheme.typography.labelLarge)
            Text("${model.timings.size} measured", style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
    }
}
@Composable private fun StatRow(label: String, value: String) {
    Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(12.dp)) { Text(label, Modifier.weight(1f), style = MaterialTheme.typography.bodyMedium); Text(value, style = MaterialTheme.typography.labelLarge) }
}
