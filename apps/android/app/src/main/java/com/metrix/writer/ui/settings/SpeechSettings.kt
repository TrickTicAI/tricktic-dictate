package com.metrix.writer.ui.settings

import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import com.metrix.writer.*
import com.metrix.writer.ui.Page
import com.metrix.writer.ui.ProtectedAlertDialog
import com.metrix.writer.ui.WriterState
import com.metrix.writer.ui.WriterViewModel

/** Phone models on disk that pass a quick file check; the selected engine may still verify them again. */
internal fun installedPhoneModels(models: PhoneModels): List<String> =
    models.models().map { it.getString("id") }.filter { runCatching { models.file(it).isFile }.getOrDefault(false) }

/** Whether new dictations can run: a cloud engine needs a key, a phone engine an installed model. */
internal fun engineReady(context: android.content.Context): Boolean {
    val choice = DictationChoice(context)
    return if (choice.isPhone()) runCatching { PhoneModels(context).file(choice.model()).exists() }.getOrDefault(false)
    else DirectCloud(context).configured()
}

/** One page for where speech is recognised: each cloud provider and each installed phone model. */
@Composable internal fun SpeechEnginePage(vm: WriterViewModel, state: WriterState, navigate: (String) -> Unit) {
    state.revision
    val context = vm.context
    val choice = DictationChoice(context)
    val phone = choice.isPhone()
    val selected = DirectCloud.selectedProvider(context)
    val models = remember { PhoneModels(context) }
    val installed = installedPhoneModels(models)
    Page {
        PageIntro("New dictations use the engine chosen here. Saved dictations keep the engine they were recorded with. There is no automatic fallback between cloud and phone.")
        SettingsGroup("Active") {
            InfoRow("New dictations use", choice.engineLabel(), if (engineReady(context)) Tone.Ok else Tone.Attention, tag = "active-engine")
        }
        SettingsGroup("Cloud", footer = (listOf("Audio goes directly to the provider, with your own key. Charges may apply.") +
            CloudProviders.transcribers.map { it.note }.filter { it.isNotEmpty() }).joinToString(" ")) {
            CloudProviders.transcribers.forEach { provider ->
                val cloud = DirectCloud(context, provider.id)
                val active = !phone && selected == provider.id
                val value = if (cloud.configured()) CloudProviders.modelName(provider.id, cloud.model()) else "No key · tap to add one"
                RadioRow(provider.name, value, active, {
                    if (state.actionBusy) return@RadioRow
                    if (cloud.configured()) vm.useCloudEngine(provider.id)
                    else { PendingChoice.engine = provider.id; navigate("Settings/${SettingsRoutes.keyPage(provider)}") }
                }, tone = if (!cloud.configured() && active) Tone.Attention else Tone.Neutral, tag = "engine-${provider.id}")
            }
            if (!phone) CloudProviders.transcribers.find { it.id == selected }?.let { provider ->
                NavRow("${provider.name} speech model", CloudProviders.modelName(provider.id, DirectCloud(context, provider.id).model()),
                    { navigate("Settings/${SettingsRoutes.SPEECH_MODEL}") })
            }
        }
        SettingsGroup("On this phone", footer = "Runs offline, without uploading audio. Speed and accuracy vary by model and language.") {
            installed.forEach { id ->
                RadioRow(id, "Offline", phone && choice.model() == id, {
                    if (!state.actionBusy && state.transfer == null) vm.usePhoneEngine(id)
                }, tag = "engine-phone-$id")
            }
            if (installed.isEmpty()) InfoRow("No phone model yet", "Download one to dictate offline.")
            NavRow("Phone models", if (installed.isEmpty()) "Download and manage" else "${installed.size} installed · download and manage",
                { navigate("Settings/${SettingsRoutes.MODELS}") })
        }
        SettingsGroup("Language") {
            NavRow("Speech language", CompanionUi.languageName(choice.language()), { navigate("Settings/${SettingsRoutes.LANGUAGE}") })
        }
    }
}

/** The selected cloud provider's speech models. */
@Composable internal fun SpeechModelPage(vm: WriterViewModel, state: WriterState, navigate: (String) -> Unit) {
    state.revision
    val context = vm.context
    val provider = CloudProviders.get(DirectCloud.selectedProvider(context))
    val cloud = DirectCloud(context, provider.id)
    val current = cloud.model()
    Page {
        PageIntro("${provider.name} models for new dictations. Saved dictations keep the model they used.")
        SettingsGroup(provider.name, footer = listOfNotNull(provider.note.ifEmpty { null },
            if (provider.id != "gemini") "Audio upload: up to 25 MB per file. Longer recordings stay saved for export or another engine." else null).joinToString(" ").ifEmpty { null }) {
            provider.speech.forEach { model ->
                RadioRow(model.name, if (model.id == current) "In use" else null, model.id == current, {
                    if (!state.actionBusy) vm.action {
                        cloud.saveSelection(model.id, DictationChoice(context).language())
                        if (cloud.configured()) DictationChoice(context).useCloud(provider.id)
                        vm.notify("New dictations use ${model.name}.")
                    }
                })
            }
            if (provider.speech.none { it.id == current }) InfoRow("Previously selected", "$current. Choose a supported model for new dictations; saved attempts keep their settings.", Tone.Attention)
        }
        SettingsGroup("Access") {
            ActionRow("Check model access", {
                vm.action { cloud.checkAccess(current); vm.notify("Model access checked. A dictation is the final test.") }
            }, value = cloud.accessStatus(), enabled = cloud.configured() && !state.actionBusy && provider.speech.any { it.id == current })
            NavRow(SettingsRoutes.keyPage(provider), Keys.summary(context, provider), { navigate("Settings/${SettingsRoutes.keyPage(provider)}") },
                tone = Keys.summaryTone(context, provider))
        }
    }
}

@Composable internal fun SpeechLanguagePage(vm: WriterViewModel) {
    val choice = DictationChoice(vm.context)
    var language by rememberSaveable { mutableStateOf(choice.language()) }
    val choices = if (choice.isPhone()) PhoneModels(vm.context).languages(choice.model()) else listOf("en", "he", "mixed")
    Page {
        PageIntro("One choice for new dictations. Saved dictations keep their original language.")
        SettingsGroup("Speech language", footer = "$PRODUCT_NAME keeps the language you speak. Mixed speech keeps both Hebrew and English; it does not translate. Available choices depend on your selected model.") {
            choices.forEach { code ->
                RadioRow(CompanionUi.languageName(code), null, language == code, {
                    language = code
                    vm.action { if (choice.isPhone()) choice.save(true, choice.model(), code) else choice.saveLanguage(code) }
                })
            }
        }
    }
}

/** Download, import, verify and remove phone models; choosing the engine lives on Speech engine. */
@Composable internal fun PhoneModelsPage(vm: WriterViewModel, state: WriterState, navigate: (String) -> Unit) {
    val models = remember { PhoneModels(vm.context) }
    val choice = DictationChoice(vm.context)
    var imported by rememberSaveable { mutableStateOf<String?>(null) }
    var removing by rememberSaveable { mutableStateOf<String?>(null) }
    val picker = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { uri -> val id = imported; imported = null; if (uri != null && id != null) vm.transfer(id, uri) }
    val idle = state.transfer == null && !state.processing && state.capture == "idle"
    Page {
        PageIntro("Models run on this phone without uploading audio. Phone inference is experimental; speed and accuracy vary by language and model.")
        state.transfer?.let {
            SettingsGroup("Model transfer") {
                GroupBlock { LinearProgressIndicator(Modifier.fillMaxWidth()); Text(it) }
                ActionRow("Pause transfer", vm::pauseTransfer)
            }
        }
        if (choice.isPhone() && models.models().none { it.getString("id") == choice.model() }) SettingsGroup {
            InfoRow("The selected model was removed", "Choose another engine on Speech engine. Your saved audio and transcripts are kept.", Tone.Attention)
        }
        models.models().forEach { model ->
            val id = model.getString("id"); val installed = models.file(id).isFile
            val inUse = choice.isPhone() && choice.model() == id
            SettingsGroup(id, footer = models.limits(id)) {
                InfoRow(when { inUse -> "Installed · in use"; installed -> "Installed"; models.partial(id).exists() -> "Partly downloaded"; else -> "Not installed" },
                    "${model.getLong("bytes") / 1048576} MB · ${model.optString("license", "See model source")}" + (if (installed) " · verified again before use" else ""),
                    if (inUse) Tone.Ok else Tone.Neutral)
                if (installed) {
                    if (!inUse) ActionRow("Use this model", { vm.usePhoneEngine(id) }, enabled = !state.actionBusy && state.transfer == null)
                    ActionRow("Verify file", { vm.action { models.verify(id); vm.notify("SHA-256 verified: $id") } }, enabled = !state.actionBusy && state.transfer == null)
                    ActionRow("Remove model…", { removing = id }, danger = true, enabled = idle)
                } else {
                    ActionRow(if (models.partial(id).exists()) "Resume download" else "Download", { vm.transfer(id) }, enabled = state.transfer == null,
                        tag = "download-$id")
                }
                ActionRow("Import file…", { imported = id; picker.launch(arrayOf("application/octet-stream", "*/*")) }, enabled = idle)
            }
        }
        SettingsGroup {
            NavRow("Speech engine", choice.engineLabel(), { navigate("Settings/${SettingsRoutes.ENGINE}") }, tone = if (engineReady(vm.context)) Tone.Neutral else Tone.Attention)
        }
    }
    removing?.let { id -> ProtectedAlertDialog(onDismissRequest = { removing = null }, title = { Text("Remove $id?") },
        text = { Text("Recordings and transcripts stay saved. Requests using this model need it reinstalled before retrying.") },
        confirmButton = { TextButton(onClick = { vm.action { models.delete(id) }; removing = null },
            colors = ButtonDefaults.textButtonColors(contentColor = MaterialTheme.colorScheme.error)) { Text("Remove") } },
        dismissButton = { TextButton(onClick = { removing = null }) { Text("Cancel") } }) }
}
