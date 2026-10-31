package com.metrix.writer.ui

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.*
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalLayoutDirection
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.*
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.LayoutDirection
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp

/*
 * The renewal kit (MW-ANDROID-02 "Visual system"). One primary button per surface, tonal for
 * secondary actions, text buttons for tertiary ones, red text for destructive ones (which always
 * confirm). Every control is at least 48 × 48 dp, every size is in sp, and spoken text resolves
 * its direction from its content while times and media stay left-to-right.
 */

/** How a state reads. Each has a text colour and a container colour with at least 4.5:1 contrast. */
enum class Status { Neutral, Ok, Warn, Err }

private data class StatusColors(val content: Color, val container: Color)

@Composable private fun Status.colors(): StatusColors {
    val dark = MaterialTheme.colorScheme.background.luminance() < .5f
    return when (this) {
        Status.Neutral -> StatusColors(MaterialTheme.colorScheme.onSurfaceVariant, MaterialTheme.colorScheme.surfaceContainerHigh)
        Status.Ok -> if (dark) StatusColors(Color(0xFF9FD8AE), Color(0xFF1F3A28)) else StatusColors(Color(0xFF1E5B32), Color(0xFFDDF0E1))
        Status.Warn -> if (dark) StatusColors(Color(0xFFF2C77B), Color(0xFF41321A)) else StatusColors(Color(0xFF7A4D00), Color(0xFFFBEBCB))
        Status.Err -> if (dark) StatusColors(Color(0xFFFFB4A8), Color(0xFF4A2420)) else StatusColors(Color(0xFF9C2A1D), Color(0xFFFADBD5))
    }
}
private fun Color.luminance() = .2126f * red + .7152f * green + .0722f * blue

@Composable fun Status.textColor(): Color = colors().content

/** A small status label: "Inserted", "Not inserted", "Check the field". */
@Composable fun StatusPill(text: String, status: Status, modifier: Modifier = Modifier) {
    val c = status.colors()
    Surface(shape = RoundedCornerShape(50), color = c.container, contentColor = c.content, modifier = modifier) {
        Text(text, Modifier.padding(horizontal = 10.dp, vertical = 4.dp), style = MaterialTheme.typography.labelMedium, maxLines = 2)
    }
}

/**
 * A contextual notice: what happened, what it means, and the actions for it. Retry and recovery
 * actions live only here, next to what failed, never as a page-wide button stack.
 */
@Composable fun Notice(title: String, body: String?, status: Status, modifier: Modifier = Modifier, actions: (@Composable FlowRowScope.() -> Unit)? = null) {
    val c = status.colors()
    Surface(shape = MaterialTheme.shapes.medium, color = c.container, modifier = modifier.fillMaxWidth().semantics(mergeDescendants = false) { liveRegion = LiveRegionMode.Polite }) {
        Column(Modifier.padding(horizontal = 16.dp, vertical = 14.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
            Text(title, style = MaterialTheme.typography.titleSmall, color = c.content, modifier = Modifier.semantics { heading() })
            body?.let { Text(it, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurface) }
            actions?.let { FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(4.dp), content = it) }
        }
    }
}

/** A wrapping, single-select chip group with one group label, so names never clip at 150 % text. */
@Composable fun <T> ChoiceChips(label: String, options: List<T>, selected: T?, name: (T) -> String, onSelect: (T) -> Unit, modifier: Modifier = Modifier, tag: (T) -> String? = { null }) {
    FlowRow(modifier.semantics { contentDescription = label; isTraversalGroup = true }, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        options.forEach { option ->
            FilterChip(selected = option == selected, onClick = { onSelect(option) }, label = { Text(name(option), maxLines = 2) },
                modifier = Modifier.heightIn(min = 48.dp).then(tag(option)?.let { Modifier.testTag(it) } ?: Modifier))
        }
    }
}

/** Spoken text: its direction comes from its content, independently of the layout direction. */
@Composable fun SpokenText(text: String, modifier: Modifier = Modifier, large: Boolean = false, maxLines: Int = Int.MAX_VALUE) {
    Text(text, modifier, style = if (large) MaterialTheme.typography.bodyLarge.copy(fontSize = 18.sp, lineHeight = 26.sp)
        else MaterialTheme.typography.bodyLarge.copy(fontSize = 16.sp, lineHeight = 23.sp), maxLines = maxLines, overflow = TextOverflow.Ellipsis)
}

/** Times, timers, durations and the audio player read left to right in every layout (D-6). */
@Composable fun LeftToRight(content: @Composable () -> Unit) {
    CompositionLocalProvider(LocalLayoutDirection provides LayoutDirection.Ltr, content = content)
}

/** A time or count in tabular figures, so a running timer does not jitter. */
@Composable fun Figures(text: String, modifier: Modifier = Modifier, style: androidx.compose.ui.text.TextStyle = MaterialTheme.typography.labelLarge, color: Color = Color.Unspecified) {
    Text(text, modifier, style = style.copy(fontFeatureSettings = "tnum", textDirection = androidx.compose.ui.text.style.TextDirection.Ltr), color = color, maxLines = 1)
}

/** m:ss for timers and elapsed stage time; h:mm:ss past an hour. */
fun formatClock(ms: Long): String {
    val s = ms.coerceAtLeast(0) / 1000
    return if (s >= 3600) "${s / 3600}:${((s % 3600) / 60).toString().padStart(2, '0')}:${(s % 60).toString().padStart(2, '0')}"
    else "${s / 60}:${(s % 60).toString().padStart(2, '0')}"
}

/** Copies only when the person taps Copy. */
fun copyToClipboard(context: Context, label: String, text: String) {
    context.getSystemService(ClipboardManager::class.java).setPrimaryClip(ClipData.newPlainText(label, text))
}

/** The page column: a single column, capped at 600 dp on large screens and centred. */
@Composable fun PageColumn(modifier: Modifier = Modifier, content: @Composable ColumnScope.() -> Unit) {
    Box(Modifier.fillMaxWidth(), contentAlignment = Alignment.TopCenter) {
        Column(modifier.widthIn(max = 600.dp).fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(12.dp), content = content)
    }
}
