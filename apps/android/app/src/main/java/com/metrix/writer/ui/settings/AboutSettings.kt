package com.metrix.writer.ui.settings

import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.unit.dp
import com.metrix.writer.*
import com.metrix.writer.ui.*

@Composable internal fun StoragePage(vm: WriterViewModel, state: WriterState, navigate: (String) -> Unit) {
    var eraseAll by rememberSaveable { mutableStateOf(false) }
    Page {
        SettingsGroup("Your data", footer = "Phone models work offline. Cloud dictation sends audio to your chosen provider; polishing sends transcript text. Uninstalling $PRODUCT_NAME or clearing app data removes local recordings and keys.") {
            InfoRow("Saved on this phone", "${state.records.size} dictation${if (state.records.size == 1) "" else "s"} in private app storage. Export or delete single dictations from History; deleting also changes Statistics.")
            ActionRow("Delete all audio and transcripts…", { eraseAll = true }, danger = true,
                enabled = !state.actionBusy && !state.processing && state.capture == "idle" && state.records.isNotEmpty())
        }
        Distribution.UpdatesSection(vm, state)
        SettingsGroup(PRODUCT_NAME) {
            GroupBlock {
                WriterIcon("wave", Modifier.size(40.dp), MaterialTheme.colorScheme.primary)
                Text("Your voice. Your words.", style = MaterialTheme.typography.titleMedium)
                Text("Version ${BuildConfig.VERSION_NAME} (${BuildConfig.VERSION_CODE})", style = MaterialTheme.typography.bodyMedium)
                Distribution.InstallationNote()
            }
        }
        SettingsGroup("Legal", footer = "Phone inference uses whisper.cpp (MIT). Open-source licenses lists every bundled component and phone model license.") {
            GroupBlock { LegalLinkRow(vm::notify) }
            NavRow("Open-source licenses", "Bundled components and phone models", { navigate("Settings/${SettingsRoutes.LICENSES}") }, tag = "licenses-link")
        }
        SettingsGroup("Advanced") {
            NavRow("Legacy server connection", "Recover requests from earlier $PRODUCT_NAME versions", { navigate("Settings/${SettingsRoutes.LEGACY}") })
        }
    }
    if (eraseAll) ProtectedAlertDialog(onDismissRequest = { eraseAll = false }, title = { Text("Delete all audio and transcripts?") },
        text = { Text("Every saved dictation on this phone is deleted, including its audio, transcripts, polished text and $PRODUCT_NAME’s cached shares. This cannot be undone. Phone models, your dictionary, settings and saved keys are kept. A dictation that is still recording or waiting for a result is kept and reported. Recipient copies, files exported elsewhere, cloud provider retention and OS backups are outside this deletion. Already-open shares may remain readable. This is not secure storage erasure.") },
        confirmButton = { TextButton(onClick = { eraseAll = false; vm.deleteAllDictations() },
            colors = ButtonDefaults.textButtonColors(contentColor = MaterialTheme.colorScheme.error)) { Text("Delete everything") } },
        dismissButton = { TextButton(onClick = { eraseAll = false }) { Text("Cancel") } })
}

@Composable internal fun LegacyConnectionPage(vm: WriterViewModel) {
    SecureScreen()
    val client = remember { MobileConnection(vm.context) }
    var endpoint by rememberSaveable { mutableStateOf(client.endpoint()) }
    var token by remember { mutableStateOf("") }
    Page {
        PageIntro("Use the original server connection to resume recordings created by earlier $PRODUCT_NAME versions. New dictation uses your chosen cloud provider or a phone model.")
        SettingsGroup("Recover an older server request", footer = "Those recordings keep their saved model and endpoint.") {
            GroupBlock {
                OutlinedTextField(endpoint, { endpoint = it }, label = { Text("HTTPS endpoint") }, modifier = Modifier.fillMaxWidth())
                OutlinedTextField(token, { token = it }, label = { Text("Device credential") }, visualTransformation = PasswordVisualTransformation(),
                    keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password), modifier = Modifier.fillMaxWidth())
                Button(onClick = { val candidate = token; token = ""; vm.action { client.check(endpoint, candidate); vm.notify("Legacy server connection checked.") } }) { Text("Check connection") }
            }
        }
    }
}

/** Setup steps with their live state; the Settings list shows this only while a step is open. */
internal fun setupSteps(context: android.content.Context, records: Int): List<Boolean> =
    listOf(PermissionStatus.read(context).microphone, engineReady(context), records > 0)

@Composable internal fun SetupPage(vm: WriterViewModel, state: WriterState, navigate: (String) -> Unit) {
    val (microphone, engine, dictated) = setupSteps(vm.context, state.records.size)
    fun tone(done: Boolean) = if (done) Tone.Ok else Tone.Attention
    Page {
        PageIntro("Three steps to your first dictation.")
        SettingsGroup("Steps") {
            NavRow("1. Allow recording and insertion", if (microphone) "Microphone allowed · review insertion and overlay" else "Allow the microphone first",
                { navigate("Settings/${SettingsRoutes.PERMISSIONS}") }, icon = "record", tone = tone(microphone))
            NavRow("2. Choose where speech is processed", DictationChoice(vm.context).engineLabel(), { navigate("Settings/${SettingsRoutes.ENGINE}") },
                icon = "Settings", tone = tone(engine))
            NavRow("3. Try a dictation", if (dictated) "Done" else "Record on Home, then check your words in History", { navigate("Home") }, icon = "check", tone = tone(dictated))
        }
        PageIntro("The floating control appears beside supported text fields after overlay and accessibility access are enabled. You can keep using your usual keyboard.")
        LegalLinkRow(vm::notify)
    }
}
