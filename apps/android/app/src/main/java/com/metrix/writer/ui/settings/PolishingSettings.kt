package com.metrix.writer.ui.settings

import android.content.Context
import androidx.compose.foundation.layout.*
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.unit.dp
import com.metrix.writer.*
import com.metrix.writer.ui.Page
import com.metrix.writer.ui.ProtectedAlertDialog
import com.metrix.writer.ui.WriterState
import com.metrix.writer.ui.WriterViewModel
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

/**
 * What gets inserted, as one choice. It maps exactly onto the stored
 * preferences: As spoken = `raw`, With my corrections = `dictionary`,
 * Polished = `cleaned` with a polishing model. Polished text falls back to the
 * corrected, then the spoken, wording, so corrections and snippets are kept.
 */
internal enum class Wording(val delivery: String, val title: String) {
    Spoken("raw", "As spoken"), Corrected("dictionary", "With my corrections"), Polished("cleaned", "Polished");

    companion object {
        fun of(prefs: CompanionPreferences): Wording = when (prefs.delivery()) {
            "raw" -> Spoken
            "dictionary" -> Corrected
            else -> if (prefs.cleanup() != "off") Polished else Corrected // cleaned without a model inserts the corrected text
        }
    }
}

internal object Polishing {
    /** The model Polished uses, or would use when turned on: current, else last chosen, else none. */
    fun target(context: Context): String? {
        val prefs = CompanionPreferences(context)
        return prefs.cleanup().takeIf { it != "off" } ?: prefs.lastCleanup()?.takeIf {
            runCatching { CloudProviders.cleanupAllowed(context, it.substringBefore('/'), it.substringAfter('/')) }.getOrDefault(false)
        }
    }

    fun modelLabel(choice: String) = "${CloudProviders.modelName(choice.substringBefore('/'), choice.substringAfter('/'))} · ${CloudProviders.get(choice.substringBefore('/')).name}"

    fun usable(context: Context, choice: String?) = choice != null && DirectCloud(context, choice.substringBefore('/')).configured()

    /** The Settings list row. */
    fun summary(context: Context): Pair<String, Tone> {
        val prefs = CompanionPreferences(context)
        return when (Wording.of(prefs)) {
            Wording.Spoken -> "As spoken" to Tone.Neutral
            Wording.Corrected -> "With my corrections" to Tone.Neutral
            Wording.Polished -> {
                val choice = prefs.cleanup()
                if (usable(context, choice)) "Polished · ${CloudProviders.modelName(choice.substringBefore('/'), choice.substringAfter('/'))}" to Tone.Neutral
                else "Polished · ${CloudProviders.get(choice.substringBefore('/')).name} key needed" to Tone.Attention
            }
        }
    }

    fun sample(language: String) = if (language == "he") "שלום מה שלומך רציתי לשאול על הפגישה מחר בשלוש אם זה עדיין מתאים"
        else "hi how are you i wanted to ask about the meeting tomorrow at three is that still ok"

    fun instructionsSummary(context: Context): String {
        val prefs = CompanionPreferences(context)
        return if (prefs.polishStyle() == "light") "Light · punctuation only"
        else if (prefs.isPolishDefault()) "Like the computer"
        else "Like the computer · edited"
    }
}

@Composable internal fun PolishingPage(vm: WriterViewModel, state: WriterState, navigate: (String) -> Unit) {
    state.revision
    val context = vm.context
    val prefs = remember { CompanionPreferences(context) }
    val wording = Wording.of(prefs)
    val cleanup = prefs.cleanup()
    var trial by rememberSaveable { mutableStateOf<List<String>?>(null) } // sample, result
    var backupPicker by rememberSaveable { mutableStateOf(false) }
    fun choose(next: Wording) {
        if (state.actionBusy) return
        when (next) {
            Wording.Spoken, Wording.Corrected -> vm.action {
                prefs.save("off", next.delivery, prefs.cleanupReasoning()); vm.notify("${next.title}: saved.")
            }
            Wording.Polished -> {
                val target = Polishing.target(context)
                if (target != null && Polishing.usable(context, target)) vm.action {
                    prefs.save(target, "cleaned", prefs.cleanupReasoning()); vm.notify("Polished with ${Polishing.modelLabel(target)}.")
                } else navigate("Settings/${SettingsRoutes.POLISHING_MODEL}")
            }
        }
    }
    Page {
        PageIntro("Choose the words $PRODUCT_NAME inserts. Your spoken words are always kept in History.")
        SettingsGroup("What gets inserted", footer = "If the chosen wording is unavailable, $PRODUCT_NAME inserts the saved original.") {
            RadioRow(Wording.Spoken.title, "“hello sam how are you”", wording == Wording.Spoken, { choose(Wording.Spoken) }, tag = "wording-raw")
            RadioRow(Wording.Corrected.title, "Your dictionary's corrections and snippets are applied", wording == Wording.Corrected, { choose(Wording.Corrected) }, tag = "wording-dictionary")
            RadioRow(Wording.Polished.title, "“Hello Sam, how are you?” Grammar, sentences and paragraphs fixed online, as on the computer", wording == Wording.Polished, { choose(Wording.Polished) }, tag = "wording-cleaned")
        }
        if (cleanup != "off" && wording != Wording.Polished) SettingsGroup {
            InfoRow("Polishing still runs in the background", "${Polishing.modelLabel(cleanup)} polishes each dictation's text; the result is kept in History but not inserted.", Tone.Attention)
            ActionRow("Stop background polishing", { vm.action { prefs.save("off", prefs.delivery(), prefs.cleanupReasoning()); vm.notify("Background polishing stopped.") } }, enabled = !state.actionBusy)
        }
        if (wording == Wording.Polished) {
            val provider = CloudProviders.get(cleanup.substringBefore('/'))
            val model = cleanup.substringAfter('/')
            val usable = Polishing.usable(context, cleanup)
            SettingsGroup("Polishing", footer = "Only text is sent for polishing, never audio. That includes dictations your phone transcribes offline: their text goes to ${Polishing.modelLabel(cleanup)}.") {
                NavRow("Model", Polishing.modelLabel(cleanup), { navigate("Settings/${SettingsRoutes.POLISHING_MODEL}") }, tag = "polishing-model")
                NavRow("Instructions", Polishing.instructionsSummary(context), { navigate("Settings/${SettingsRoutes.POLISHING_INSTRUCTIONS}") }, tag = "polishing-instructions")
                NavRow(SettingsRoutes.keyPage(provider), Keys.summary(context, provider), { navigate("Settings/${SettingsRoutes.keyPage(provider)}") },
                    tone = if (usable) Keys.summaryTone(context, provider) else Tone.Attention)
                ActionRow("Try polishing", {
                    val sample = Polishing.sample(DictationChoice(context).language())
                    vm.action {
                        val result = DirectCloud(context, provider.id).tryPolishing(model, sample, prefs.cleanupReasoning())
                        withContext(Dispatchers.Main) { trial = listOf(sample, result) }
                        vm.notify("Polishing works with ${Polishing.modelLabel(cleanup)}.")
                    }
                }, value = "Sends one sample sentence to ${provider.name} and shows the result. Costs a tiny amount.",
                    enabled = usable && !state.actionBusy, tag = "polishing-try")
                trial?.let { (before, after) ->
                    GroupBlock(Modifier.testTag("polishing-trial")) {
                        Text("Before", style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
                        Text(before, style = MaterialTheme.typography.bodyMedium)
                        Text("After", style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.primary)
                        Text(after, style = MaterialTheme.typography.bodyLarge)
                    }
                }
                NavRow("Polishing options", "Thinking effort: ${CustomTextModels.reasoningLabel(prefs.cleanupReasoning())} · " +
                    CustomTextModels(context).list().size.let { if (it == 1) "1 model added by you" else "$it models added by you" },
                    { navigate("Settings/${SettingsRoutes.POLISHING_OPTIONS}") })
            }
            SettingsGroup("When polishing is unavailable", footer = "After both models fail or the wait ends, use your original with corrections and snippets. A later response never inserts again. These controls are saved on this phone.") {
                val backup = prefs.fallbackCleanup()
                ActionRow("Fallback model", { backupPicker = !backupPicker }, value = if (backup == "off") "None" else Polishing.modelLabel(backup), tag = "polishing-fallback")
                if (backupPicker) {
                    RadioRow("No fallback", null, backup == "off", { vm.action { prefs.saveReliability("off"); backupPicker = false; vm.notify("Fallback off.") } })
                    CloudProviders.all.filter { it.text }.forEach { option ->
                        CloudProviders.cleanupModels(context, option.id).forEach { candidate ->
                            val choice = "${option.id}/${candidate.id}"
                            if (choice != cleanup) RadioRow(candidate.name, "${option.name} · ${if (DirectCloud(context, option.id).configured()) "key saved" else "key needed"}", backup == choice, {
                                vm.action { prefs.saveReliability(choice); backupPicker = false; vm.notify("Fallback saved. Check a sample to verify it.") }
                            }, tag = "fallback-${option.id}-${candidate.id}")
                        }
                    }
                }
                if (backup != "off") {
                    val backupProvider = CloudProviders.get(backup.substringBefore('/'))
                    NavRow(SettingsRoutes.keyPage(backupProvider), Keys.summary(context, backupProvider), { navigate("Settings/${SettingsRoutes.keyPage(backupProvider)}") })
                    ActionRow("Check fallback", {
                        vm.action {
                            val sample = Polishing.sample(DictationChoice(context).language())
                            val result = DirectCloud(context, backupProvider.id).tryPolishing(backup.substringAfter('/'), sample, "provider_default")
                            withContext(Dispatchers.Main) { trial = listOf(sample, result) }
                            vm.notify("Fallback sample succeeded with ${Polishing.modelLabel(backup)}.")
                        }
                    }, value = "Sends one sample sentence. A saved key alone does not verify inference.", enabled = DirectCloud(context, backupProvider.id).configured() && !state.actionBusy, tag = "polishing-check-fallback")
                    if (backupProvider.id == provider.id) InfoRow("Shared provider", "Both models use the same provider account. A quota or provider outage can affect both.", Tone.Attention)
                    InfoRow("Backup starts after ${prefs.fallbackAfterMs() / 1000.0} seconds", "A failure or invalid response starts it immediately. Both requests can be charged.")
                }
                InfoRow("Maximum wait: ${prefs.polishTimeoutMs() / 1000.0} seconds", "Measured after transcription. The backup gets at least two seconds.")
                FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    listOf(3000L, 5000L, 10000L).forEach { wait -> FilterChip(prefs.polishTimeoutMs() == wait,
                        { vm.action { prefs.saveReliability(backup, 1000L, wait); vm.notify("Polishing wait saved.") } }, label = { Text("${wait / 1000}s") }) }
                }
            }
        }
    }
}

/** Polishing models grouped by provider. A provider without a key offers to add one, then selects the tapped model. */
@Composable internal fun PolishingModelPage(vm: WriterViewModel, state: WriterState, navigate: (String) -> Unit) {
    state.revision
    val context = vm.context
    val prefs = remember { CompanionPreferences(context) }
    val current = if (Wording.of(prefs) == Wording.Polished) prefs.cleanup() else null
    Page {
        PageIntro("Choose the model that polishes your text. Only the text of a dictation is sent, never audio.")
        CloudProviders.all.filter { it.text }.forEach { provider ->
            val configured = DirectCloud(context, provider.id).configured()
            val added = CustomTextModels(context).forProvider(provider.id).map { it.model }.toSet()
            SettingsGroup(provider.name, footer = provider.note.takeIf { !provider.transcribes && it.isNotEmpty() }) {
                if (!configured) NavRow("Add ${provider.name} key", "Needed before its models can polish", {
                    PendingChoice.polishing = null; navigate("Settings/${SettingsRoutes.keyPage(provider)}")
                }, tone = Tone.Attention)
                CloudProviders.cleanupModels(context, provider.id).forEach { model ->
                    val id = "${provider.id}/${model.id}"
                    RadioRow(model.name, if (model.id in added) "Added by you" else null, current == id, {
                        if (state.actionBusy) return@RadioRow
                        if (configured) vm.action {
                            prefs.save(id, "cleaned", prefs.cleanupReasoning()); vm.notify("Polished with ${Polishing.modelLabel(id)}.")
                        } else { PendingChoice.polishing = id; navigate("Settings/${SettingsRoutes.keyPage(provider)}") }
                    }, enabled = true, tone = Tone.Neutral, tag = "polish-${provider.id}-${model.id}")
                }
            }
        }
        SettingsGroup {
            NavRow("Your own models", "Add a model by the ID its provider publishes", { navigate("Settings/${SettingsRoutes.POLISHING_OPTIONS}") })
        }
    }
}

/** The advanced part of polishing: thinking effort and models added by ID. */
@Composable internal fun PolishingOptionsPage(vm: WriterViewModel, state: WriterState) {
    state.revision
    val context = vm.context
    val prefs = remember { CompanionPreferences(context) }
    val store = remember { CustomTextModels(context) }
    val textProviders = CloudProviders.all.filter { it.text }
    var provider by rememberSaveable { mutableStateOf(prefs.cleanup().takeIf { it != "off" }?.substringBefore('/') ?: textProviders.first().id) }
    var custom by rememberSaveable { mutableStateOf("") }
    var customError by rememberSaveable { mutableStateOf<String?>(null) }
    var removing by remember { mutableStateOf<CustomTextModel?>(null) }
    var revision by remember { mutableIntStateOf(0) }
    val reasoning = prefs.cleanupReasoning()
    Page {
        SettingsGroup("Thinking effort", footer = "A model that does not accept the chosen level fails that polishing request; the saved transcript is kept.") {
            CustomTextModels.REASONING.forEach { level ->
                RadioRow(CustomTextModels.reasoningLabel(level), when (level) {
                    "provider_default" -> "Sends nothing; the provider decides"
                    "off" -> "Asks the model not to think"
                    else -> null
                }, reasoning == level, {
                    if (!state.actionBusy) vm.action { prefs.save(prefs.cleanup(), prefs.delivery(), level); vm.notify("Thinking effort: ${CustomTextModels.reasoningLabel(level)}.") }
                })
            }
        }
        key(revision) {
            SettingsGroup("Your own models", footer = "Add a model by the ID its provider publishes. $PRODUCT_NAME cannot check it in advance; use Try polishing after choosing it.") {
                val entries = store.list()
                entries.forEach { entry ->
                    ActionRow(entry.model, { removing = entry }, value = "${CloudProviders.get(entry.provider).name} · tap to remove")
                }
                if (entries.isEmpty()) InfoRow("None added", "Built-in models are listed on Polishing model.")
                GroupBlock {
                    FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp), modifier = Modifier.fillMaxWidth()) {
                        textProviders.forEach { option -> FilterChip(provider == option.id, { provider = option.id; customError = null }, label = { Text(option.name) }) }
                    }
                    OutlinedTextField(custom, { custom = it; customError = null }, label = { Text("Model ID") },
                        placeholder = { Text(if (provider == "openrouter") "publisher/model" else "model-id") },
                        singleLine = true, isError = customError != null, modifier = Modifier.fillMaxWidth().testTag("custom-model-id"))
                    customError?.let { Text(it, color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodySmall) }
                    Button(onClick = {
                        val entry = custom.trim()
                        val problem = CustomTextModels.validate(store.list() + CustomTextModel(provider, entry))
                        if (problem != null) customError = problem else {
                            store.add(provider, entry); custom = ""; revision++
                            vm.notify("Added $entry for ${CloudProviders.get(provider).name}. Choose it on Polishing model, then try polishing.")
                        }
                    }, enabled = custom.isNotBlank()) { Text("Add model") }
                }
            }
        }
    }
    removing?.let { entry -> ProtectedAlertDialog(onDismissRequest = { removing = null }, title = { Text("Remove ${entry.model}?") },
        text = { Text("Saved transcripts and history are unchanged. Polishing requests naming this model need it added again.") },
        confirmButton = { TextButton(onClick = {
            store.remove(entry.provider, entry.model)
            if (prefs.cleanup() == "${entry.provider}/${entry.model}") prefs.save("${entry.provider}/${CloudProviders.get(entry.provider).cleanup.first().id}", prefs.delivery(), prefs.cleanupReasoning())
            removing = null; revision++
        }, colors = ButtonDefaults.textButtonColors(contentColor = MaterialTheme.colorScheme.error)) { Text("Remove") } },
        dismissButton = { TextButton(onClick = { removing = null }) { Text("Cancel") } }) }
}

/** The instructions the polishing model follows: Super like the computer, or Light punctuation-only. */
@Composable internal fun PolishingInstructionsPage(vm: WriterViewModel, state: WriterState) {
    state.revision
    val context = vm.context
    val prefs = remember { CompanionPreferences(context) }
    val style = prefs.polishStyle()
    val formatting = prefs.polish().formatting
    val stored = prefs.polish().instructions
    var draft by rememberSaveable { mutableStateOf(stored) }
    var lastStored by rememberSaveable { mutableStateOf(stored) }
    if (stored != lastStored) {
        if (draft == lastStored) draft = stored // Unedited: follow the synced value.
        lastStored = stored
    }
    var error by rememberSaveable { mutableStateOf<String?>(null) }
    fun update(next: PolishFormatting) {
        if (!state.actionBusy) vm.action { prefs.savePolish(style, prefs.polish().instructions, next); vm.notify("Writing style saved.") }
    }
    Page {
        PageIntro("The instructions your polishing model follows, shared with your computer.")
        SettingsGroup("Style", footer = "Light only fixes punctuation, capitals and spacing. The style stays on this phone.") {
            RadioRow("Like the computer", "Full polish: grammar, sentences and paragraphs", style == "super", {
                if (!state.actionBusy) vm.action { prefs.savePolish("super", prefs.polish().instructions, prefs.polish().formatting); vm.notify("Polishes like the computer.") }
            }, tag = "polish-style-super")
            RadioRow("Light", "Punctuation, capitals and spacing only", style == "light", {
                if (!state.actionBusy) vm.action { prefs.savePolish("light", prefs.polish().instructions, prefs.polish().formatting); vm.notify("Light polishing.") }
            }, tag = "polish-style-light")
        }
        SettingsGroup("Instructions", footer = "Changes apply to the next dictation and sync to your computer. Up to 16,384 bytes.") {
            GroupBlock {
                OutlinedTextField(draft, { next ->
                    draft = next
                    val problem = PolishPrompts.validate(next)
                    error = problem
                    if (problem == null) prefs.savePolish(style, next, formatting)
                }, label = { Text("Polishing instructions") }, minLines = 4, maxLines = 12, isError = error != null,
                    supportingText = { Text(error ?: "${draft.toByteArray(Charsets.UTF_8).size} / 16,384 bytes") },
                    modifier = Modifier.fillMaxWidth().testTag("polish-instructions"))
            }
            ActionRow("Reset to default", {
                if (!state.actionBusy) vm.action { prefs.resetPolish(); draft = PolishPrompts.SUPER; error = null; vm.notify("Polishing instructions reset.") }
            }, value = "The computer's Super instructions",
                enabled = (style != "super" || draft != PolishPrompts.SUPER || formatting != PolishFormatting()) && !state.actionBusy, tag = "polish-reset")
        }
        if (style == "super") SettingsGroup("Writing style", footer = "The same preferences as the computer's Super mode.") {
            GroupBlock {
                Text("Rewrite strength", style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
                FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp), modifier = Modifier.fillMaxWidth()) {
                    listOf("light" to "Light", "polished" to "Polished", "concise" to "Concise").forEach { (id, label) ->
                        FilterChip(formatting.rewrite == id, { update(formatting.copy(rewrite = id)) }, label = { Text(label) },
                            enabled = !state.actionBusy, modifier = Modifier.testTag("polish-rewrite-$id"))
                    }
                }
                Text("Layout", style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
                FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp), modifier = Modifier.fillMaxWidth()) {
                    listOf("automatic" to "Natural paragraphs", "single_line" to "One line").forEach { (id, label) ->
                        FilterChip(formatting.layout == id, { update(formatting.copy(layout = id)) }, label = { Text(label) },
                            enabled = !state.actionBusy, modifier = Modifier.testTag("polish-layout-$id"))
                    }
                }
            }
            SwitchRow("Remove fillers and false starts", null, formatting.removeFillers, { update(formatting.copy(removeFillers = it)) },
                enabled = !state.actionBusy, tag = "polish-fillers")
            SwitchRow("Resolve spoken corrections", null, formatting.selfCorrections, { update(formatting.copy(selfCorrections = it)) },
                enabled = !state.actionBusy, tag = "polish-corrections")
            SwitchRow("Format lists automatically", if (formatting.layout == "single_line") "One line keeps lists inline" else null, formatting.lists,
                { update(formatting.copy(lists = it)) }, enabled = formatting.layout != "single_line" && !state.actionBusy, tag = "polish-lists")
            SwitchRow("Recognize spoken punctuation", null, formatting.spokenPunctuation, { update(formatting.copy(spokenPunctuation = it)) },
                enabled = !state.actionBusy, tag = "polish-punctuation")
        }
    }
}
