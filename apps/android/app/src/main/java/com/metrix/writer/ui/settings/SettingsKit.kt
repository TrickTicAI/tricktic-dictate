package com.metrix.writer.ui.settings

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.selection.toggleable
import androidx.compose.material3.*
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalLayoutDirection
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.LayoutDirection
import androidx.compose.ui.unit.dp
import com.metrix.writer.ui.WriterIcon
import com.metrix.writer.ui.rememberSearchReveal

/*
 * The settings kit (MW-SETTINGS-1). Every settings page is groups of rows:
 * a small header, then one rounded container whose rows are separated by
 * hairlines. Rows never nest another container, so there is no card inside a
 * card. A row's second line is its live value, not a description.
 */

/** How a status reads: neutral, fine, needs attention, or failed. */
internal enum class Tone { Neutral, Ok, Attention, Error }

@Composable internal fun Tone.color(): Color = when (this) {
    Tone.Neutral -> MaterialTheme.colorScheme.onSurfaceVariant
    Tone.Ok -> MaterialTheme.colorScheme.primary
    Tone.Attention -> MaterialTheme.colorScheme.tertiary
    Tone.Error -> MaterialTheme.colorScheme.error
}

/** A header and one container; its rows are separated by hairlines. [title] also anchors search. */
@Composable internal fun SettingsGroup(title: String? = null, footer: String? = null, content: @Composable ColumnScope.() -> Unit) {
    val shape = MaterialTheme.shapes.large
    Column(if (title != null) rememberSearchReveal(title, shape) else Modifier, verticalArrangement = Arrangement.spacedBy(8.dp)) {
        title?.let {
            Text(it, style = MaterialTheme.typography.titleSmall, color = MaterialTheme.colorScheme.primary,
                modifier = Modifier.padding(start = 4.dp))
        }
        Column(Modifier.fillMaxWidth().clip(shape).background(MaterialTheme.colorScheme.outlineVariant.copy(alpha = .45f)),
            verticalArrangement = Arrangement.spacedBy(1.dp), content = content)
        footer?.let {
            Text(it, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier.padding(horizontal = 4.dp))
        }
    }
}

/** Free content inside a group, on the row background. */
@Composable internal fun GroupBlock(modifier: Modifier = Modifier, content: @Composable ColumnScope.() -> Unit) {
    Column(modifier.fillMaxWidth().background(MaterialTheme.colorScheme.surfaceContainerLow).padding(horizontal = 18.dp, vertical = 14.dp),
        verticalArrangement = Arrangement.spacedBy(10.dp), content = content)
}

@Composable private fun RowFrame(modifier: Modifier, content: @Composable RowScope.() -> Unit) {
    Row(modifier.fillMaxWidth().heightIn(min = 56.dp).background(MaterialTheme.colorScheme.surfaceContainerLow)
        .padding(horizontal = 18.dp, vertical = 12.dp), verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(14.dp), content = content)
}

@Composable private fun RowText(title: String, value: String?, tone: Tone, modifier: Modifier, maxLines: Int = 2) {
    Column(modifier, verticalArrangement = Arrangement.spacedBy(2.dp)) {
        Text(title, style = MaterialTheme.typography.bodyLarge)
        value?.takeIf { it.isNotEmpty() }?.let {
            Text(it, style = MaterialTheme.typography.bodyMedium, color = tone.color(), maxLines = maxLines, overflow = TextOverflow.Ellipsis)
        }
    }
}

/** Opens another page. [value] is the live state; [tone] colours it when something needs attention. */
@Composable internal fun NavRow(title: String, value: String?, onClick: () -> Unit, icon: String? = null, tone: Tone = Tone.Neutral,
                                enabled: Boolean = true, tag: String? = null, trailing: (@Composable () -> Unit)? = null) {
    RowFrame(rememberSearchReveal(title, MaterialTheme.shapes.medium).clickable(enabled = enabled, role = Role.Button, onClick = onClick)
        .then(if (tag != null) Modifier.testTag(tag) else Modifier)) {
        icon?.let { WriterIcon(it, tint = MaterialTheme.colorScheme.primary) }
        RowText(title, value, tone, Modifier.weight(1f))
        trailing?.invoke()
        WriterIcon(if (LocalLayoutDirection.current == LayoutDirection.Rtl) "back" else "next", tint = MaterialTheme.colorScheme.onSurfaceVariant)
    }
}

/** A switch; the whole row toggles it. */
@Composable internal fun SwitchRow(title: String, value: String?, checked: Boolean, onChange: (Boolean) -> Unit, enabled: Boolean = true,
                                   description: String = title, tag: String? = null) {
    RowFrame(rememberSearchReveal(title, MaterialTheme.shapes.medium).toggleable(checked, enabled = enabled, role = Role.Switch, onValueChange = onChange)
        .then(if (description != title) Modifier.semantics { contentDescription = description } else Modifier)
        .then(if (tag != null) Modifier.testTag(tag) else Modifier)) {
        RowText(title, value, Tone.Neutral, Modifier.weight(1f))
        Switch(checked, onCheckedChange = null, enabled = enabled)
    }
}

/** One choice of several; the whole row selects it. [trailing] may hold a small action. */
@Composable internal fun RadioRow(title: String, value: String?, selected: Boolean, onClick: () -> Unit, enabled: Boolean = true,
                                  tone: Tone = Tone.Neutral, tag: String? = null, trailing: (@Composable () -> Unit)? = null) {
    RowFrame(rememberSearchReveal(title, MaterialTheme.shapes.medium).selectable(selected, enabled = enabled, role = Role.RadioButton, onClick = onClick)
        .then(if (tag != null) Modifier.testTag(tag) else Modifier)) {
        RadioButton(selected, onClick = null, enabled = enabled)
        RowText(title, value, tone, Modifier.weight(1f), maxLines = 3)
        trailing?.invoke()
    }
}

/** A command. [danger] colours it red; destructive commands still confirm in a dialog. */
@Composable internal fun ActionRow(title: String, onClick: () -> Unit, value: String? = null, danger: Boolean = false, enabled: Boolean = true,
                                   tag: String? = null) {
    RowFrame(rememberSearchReveal(title, MaterialTheme.shapes.medium).clickable(enabled = enabled, role = Role.Button, onClick = onClick)
        .then(if (tag != null) Modifier.testTag(tag) else Modifier)) {
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            val color = when { !enabled -> MaterialTheme.colorScheme.onSurface.copy(alpha = .38f); danger -> MaterialTheme.colorScheme.error; else -> MaterialTheme.colorScheme.primary }
            Text(title, style = MaterialTheme.typography.bodyLarge, color = color)
            value?.let { Text(it, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant) }
        }
    }
}

/** A label and a value that cannot be changed here. */
@Composable internal fun InfoRow(title: String, value: String, tone: Tone = Tone.Neutral, tag: String? = null) {
    RowFrame(rememberSearchReveal(title, MaterialTheme.shapes.medium).then(if (tag != null) Modifier.testTag(tag) else Modifier)) { RowText(title, value, tone, Modifier.weight(1f), maxLines = 6) }
}

@Composable internal fun StatusChip(label: String, tone: Tone) {
    val color = tone.color()
    Surface(shape = MaterialTheme.shapes.small, color = color.copy(alpha = .14f), contentColor = color) {
        Text(label, Modifier.padding(horizontal = 10.dp, vertical = 4.dp), style = MaterialTheme.typography.labelMedium)
    }
}

/** One or two sentences under the top bar saying what the page is for. */
@Composable internal fun PageIntro(text: String) {
    Text(text, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
}

/** A free-form group for settings pages that are not rows (account, backup, updates); same look as [SettingsGroup]. */
@Composable internal fun SettingsSection(title: String, subtitle: String? = null, content: @Composable ColumnScope.() -> Unit) {
    Column(rememberSearchReveal(title, MaterialTheme.shapes.large), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Text(title, style = MaterialTheme.typography.titleSmall, color = MaterialTheme.colorScheme.primary, modifier = Modifier.padding(start = 4.dp))
        subtitle?.let { Text(it, color = MaterialTheme.colorScheme.onSurfaceVariant, style = MaterialTheme.typography.bodySmall, modifier = Modifier.padding(horizontal = 4.dp)) }
        Surface(shape = MaterialTheme.shapes.large, color = MaterialTheme.colorScheme.surfaceContainerLow, modifier = Modifier.fillMaxWidth()) {
            Column(Modifier.padding(horizontal = 18.dp, vertical = 16.dp), verticalArrangement = Arrangement.spacedBy(12.dp), content = content)
        }
    }
}
