package com.metrix.writer.ui.settings

import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.lifecycle.compose.LifecycleResumeEffect
import com.metrix.writer.*
import com.metrix.writer.ui.*

/**
 * The Settings list: groups of rows whose second line is the live state, and
 * a search that replaces the list while it has a query. A row turns amber when
 * something needs attention (a key in use is missing, a permission is off).
 */
@Composable internal fun SettingsHome(vm: WriterViewModel, state: WriterState, navigate: (String) -> Unit) {
    state.revision
    val context = vm.context
    var resumed by remember { mutableIntStateOf(0) } // Permissions change outside the app.
    LifecycleResumeEffect(Unit) { resumed++; onPauseOrDispose { } }
    resumed
    Page {
        var query by rememberSaveable { mutableStateOf("") }
        OutlinedTextField(query, { query = it }, placeholder = { Text("Search settings") }, singleLine = true,
            leadingIcon = { WriterIcon("search") },
            trailingIcon = { if (query.isNotEmpty()) IconButton(onClick = { query = "" }, modifier = Modifier.semantics { contentDescription = "Clear search" }) { WriterIcon("close") } },
            shape = MaterialTheme.shapes.extraLarge, modifier = Modifier.fillMaxWidth().testTag("settings-search"))
        // Results replace the list; Back from a chosen page returns to them.
        if (query.isNotBlank()) {
            val entries = remember { SettingsSearch.index + SettingsSearch.catalog(PhoneModels(context).models().map { it.getString("id") }) }
            val results = SettingsSearch.search(query, entries)
            if (results.isEmpty()) PageIntro("No settings match “${query.trim()}”.")
            else SettingsGroup("Results") {
                results.forEach { entry ->
                    NavRow(entry.title, SettingsSearch.pageNames[entry.page] ?: entry.page, { SettingsSearch.aim(entry.anchor); navigate("Settings/${entry.page}") })
                }
            }
        } else HomeGroups(vm, state, navigate)
    }
}

@Composable private fun HomeGroups(vm: WriterViewModel, state: WriterState, navigate: (String) -> Unit) {
    val context = vm.context
    run {
        val steps = setupSteps(context, state.records.size)
        if (steps.any { !it }) SettingsGroup {
            NavRow("Finish setting up", "${steps.count { it }} of ${steps.size} steps done", { navigate("Settings/${SettingsRoutes.SETUP}") },
                icon = "check", tone = Tone.Attention, tag = "settings-setup")
        }
        val choice = DictationChoice(context)
        SettingsGroup("Dictation") {
            NavRow("Speech engine", choice.engineLabel(), { navigate("Settings/${SettingsRoutes.ENGINE}") },
                tone = if (engineReady(context)) Tone.Neutral else Tone.Attention, tag = "settings-engine")
            NavRow("Speech language", CompanionUi.languageName(choice.language()), { navigate("Settings/${SettingsRoutes.LANGUAGE}") })
            val (polishing, polishingTone) = Polishing.summary(context)
            NavRow("Polishing", polishing, { navigate("Settings/${SettingsRoutes.POLISHING}") }, tone = polishingTone, tag = "settings-polishing")
            val installed = installedPhoneModels(PhoneModels(context)).size
            NavRow("Phone models", if (installed == 0) "Download offline models" else "$installed installed", { navigate("Settings/${SettingsRoutes.MODELS}") })
        }
        SettingsGroup(if (Account.available) "Keys & account" else "Keys & backup") {
            val (keys, keysTone) = Keys.overview(context)
            NavRow(SettingsRoutes.KEYS, keys, { navigate("Settings/${SettingsRoutes.KEYS}") }, tone = keysTone, tag = "settings-keys")
            Account.SettingsEntry(navigate)
            NavRow("Export & import", "Settings and dictionary to a file · no account needed", { navigate("Settings/${SettingsRoutes.BACKUP}") })
        }
        SettingsGroup("Your phone") {
            NavRow("Floating control", floatingSummary(context), { navigate("Settings/${SettingsRoutes.FLOATING}") })
            val missing = missingPermissions(context)
            NavRow("Permissions", when (missing.size) { 0 -> "All allowed"; 1 -> "${missing.first()} is off"; else -> "${missing.size} are off" },
                { navigate("Settings/${SettingsRoutes.PERMISSIONS}") }, tone = if ("Microphone" in missing) Tone.Attention else Tone.Neutral)
            NavRow("Appearance", themeName(vm.appearance.theme), { navigate("Settings/${SettingsRoutes.APPEARANCE}") })
        }
        SettingsGroup("About") {
            NavRow("Storage & about", "${state.records.size} dictation${if (state.records.size == 1) "" else "s"} · version ${BuildConfig.VERSION_NAME}",
                { navigate("Settings/${SettingsRoutes.STORAGE}") })
            if (steps.all { it }) NavRow("Set up $PRODUCT_NAME", "Permissions, engine and first dictation", { navigate("Settings/${SettingsRoutes.SETUP}") })
        }
    }
}
