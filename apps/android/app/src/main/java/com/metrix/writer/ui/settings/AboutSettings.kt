package com.metrix.writer.ui.settings

import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.semantics
import androidx.lifecycle.compose.LifecycleResumeEffect
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.unit.dp
import com.metrix.writer.*
import com.metrix.writer.ui.*

/**
 * Storage and content-free diagnostics. The timing report is the allowlisted latency export:
 * ids, provider, a hash of the model id, mode, warm/cold, outcomes and stage durations; never
 * audio, transcripts, keys or editor contents.
 */
@Composable internal fun StoragePage(vm: WriterViewModel, state: WriterState, navigate: (String) -> Unit) {
    var eraseAll by rememberSaveable { mutableStateOf(false) }
    val context = vm.context
    val saveReport = rememberLauncherForActivityResult(ActivityResultContracts.CreateDocument("application/json")) { uri ->
        if (uri != null) vm.action {
            val report = LatencyMeasurements.read(java.io.File(context.filesDir, "recordings")).toString(2)
            context.contentResolver.openOutputStream(uri, "wt")!!.use { it.write(report.toByteArray()) }
            vm.notify("Timing report exported. It holds stage timings and outcome codes only.")
        }
    }
    Page {
        SettingsGroup("Your data", footer = "Phone models work offline. Cloud dictation sends audio to your chosen provider; polishing sends transcript text. Uninstalling $PRODUCT_NAME or clearing app data removes local recordings and keys.") {
            InfoRow("Saved on this phone", "${state.records.size} dictation${if (state.records.size == 1) "" else "s"} in private app storage. Export or delete single dictations from History; deleting also changes Statistics.")
            ActionRow("Delete all audio and transcripts…", { eraseAll = true }, danger = true,
                enabled = !state.actionBusy && !state.processing && state.capture == "idle" && state.records.isNotEmpty())
        }
        SettingsGroup("Diagnostics", footer = "The report contains stage timings, engine ids, warm or cold starts and outcome codes. It never contains audio, transcripts, keys or the contents of other apps' fields.") {
            ActionRow("Export timing report", { saveReport.launch("$PRODUCT_NAME timing report.json") }, value = "Save a content-free JSON file to share when something is slow",
                enabled = !state.actionBusy, tag = "export-timing")
            val latest = state.records.maxByOrNull { it.created }?.view
            InfoRow("Latest dictation", latest?.timingsMs?.takeIf { it.isNotEmpty() }?.let { timings ->
                timingOrder.filter { it in timings }.joinToString(" · ") { "${timingNames[it]} ${formatStage(timings.getValue(it))}" } +
                    (if (latest.warm != "unknown") " · ${latest.warm} start" else "")
            } ?: "No measured timings yet", tag = "latest-timing")
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

/** The latency export's stage names in reading order, with the words this page uses for them. */
private val timingOrder = listOf("capture_readiness", "queue", "preparation", "upload", "model_load", "asr", "cloud_request", "raw_availability", "cleanup", "insertion", "stop_to_insertion")
private val timingNames = mapOf("capture_readiness" to "mic start", "queue" to "queue", "preparation" to "audio prep", "upload" to "upload",
    "model_load" to "model load", "asr" to "speech", "cloud_request" to "cloud request", "raw_availability" to "original ready",
    "cleanup" to "polishing", "insertion" to "insertion", "stop_to_insertion" to "stop to inserted")
private fun formatStage(ms: Long) = if (ms < 1000) "$ms ms" else if (ms < 60000) "${"%.1f".format(ms / 1000.0)} s" else "${ms / 60000} min ${(ms % 60000) / 1000} s"

/** Version, updates (dev build), legal links and licenses. */
@Composable internal fun AboutPage(vm: WriterViewModel, state: WriterState, navigate: (String) -> Unit) {
    Page {
        SettingsGroup(PRODUCT_NAME) {
            GroupBlock {
                WriterIcon("wave", Modifier.size(40.dp), MaterialTheme.colorScheme.primary)
                Text("Your voice. Your words.", style = MaterialTheme.typography.titleMedium)
                Text("Version ${BuildConfig.VERSION_NAME} (${BuildConfig.VERSION_CODE})", style = MaterialTheme.typography.bodyMedium)
                Distribution.InstallationNote()
            }
        }
        Distribution.UpdatesSection(vm, state)
        SettingsGroup("Legal", footer = "Phone inference uses whisper.cpp (MIT). Open-source licenses lists every bundled component and phone model license.") {
            GroupBlock { LegalLinkRow(vm::notify) }
            NavRow("Open-source licenses", "Bundled components and phone models", { navigate("Settings/${SettingsRoutes.LICENSES}") }, tag = "licenses-link")
        }
    }
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

/**
 * First run: permissions with their reasons, then the engine (on this phone recommended, so a first
 * dictation needs no key; Cloud stays an explicit choice with its consent), then a test dictation
 * into a practice field inside $PRODUCT_NAME. The test proves the microphone and the engine; typing
 * into other apps is checked separately with the floating button.
 */
@Composable internal fun SetupPage(vm: WriterViewModel, state: WriterState, navigate: (String) -> Unit) {
    val context = vm.context
    val (microphone, engine, dictated) = setupSteps(context, state.records.size)
    var step by rememberSaveable { mutableIntStateOf(if (!microphone) 0 else if (!engine) 1 else 2) }
    val titles = listOf("Allow what you want to use", "Choose where speech runs", "Try a test dictation")
    Page {
        PageIntro("Step ${step + 1} of 3 · ${titles[step]}")
        SettingsGroup("Steps") {
            listOf(microphone, engine, dictated).forEachIndexed { index, done ->
                NavRow("${index + 1}. ${titles[index]}", if (done) "Done" else if (index == step) "In progress" else "Not done yet", { step = index },
                    icon = if (done) "check" else null, tone = if (done) Tone.Ok else if (index == step) Tone.Neutral else Tone.Attention, tag = "setup-step-$index")
            }
        }
        when (step) {
            0 -> SetupPermissions(vm)
            1 -> SetupEngine(vm, state, navigate)
            else -> SetupTest(vm, state)
        }
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            if (step > 0) OutlinedButton(onClick = { step-- }, modifier = Modifier.heightIn(min = 48.dp).testTag("setup-back")) { Text("Back") }
            Spacer(Modifier.weight(1f))
            if (step < 2) Button(onClick = { step++ }, modifier = Modifier.heightIn(min = 48.dp).testTag("setup-next")) { Text("Next") }
            else Button(onClick = { navigate("Home") }, modifier = Modifier.heightIn(min = 48.dp).testTag("setup-done")) { Text("Done") }
        }
        PageIntro("The floating control appears beside supported text fields after overlay and accessibility access are enabled. You can keep using your usual keyboard.")
        LegalLinkRow(vm::notify)
    }
}

@Composable private fun SetupPermissions(vm: WriterViewModel) {
    val context = vm.context
    var grants by remember { mutableStateOf(PermissionStatus.read(context)) }
    val consent = remember { AccessibilityConsent(context) }
    var choice by remember { mutableStateOf(consent.choice) }
    val review = LocalInsertionReview.current
    val scope = rememberCoroutineScope()
    val grant = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { grants = PermissionStatus.read(context) }
    LifecycleResumeEffect(Unit) {
        val job = scope.launch { while (isActive) { grants = PermissionStatus.read(context); choice = consent.choice; delay(300) } }
        onPauseOrDispose { job.cancel() }
    }
    PermissionRows(grants, choice, { grant.launch(it) }, { context.startActivity(it.addFlags(android.content.Intent.FLAG_ACTIVITY_NEW_TASK)) }) { review { choice = consent.choice } }
}

@Composable private fun SetupEngine(vm: WriterViewModel, state: WriterState, navigate: (String) -> Unit) {
    val context = vm.context
    val choice = DictationChoice(context)
    val models = remember { PhoneModels(context) }
    val installed = installedPhoneModels(models)
    SettingsGroup("Where speech runs", choices = true, footer = "You can change this at any time in Settings → Speech engine. $PRODUCT_NAME never switches by itself.") {
        RadioRow("On this phone · recommended to start", if (installed.isEmpty()) "Works offline, no key needed. Download a model first (about ${models.models().firstOrNull()?.optLong("bytes")?.div(1048576) ?: 0} MB)."
            else "Works offline, no key needed. Audio stays on this phone.", choice.isPhone(), {
                val model = choice.model().takeIf { it in installed } ?: installed.firstOrNull()
                if (model != null) vm.usePhoneEngine(model) else navigate("Settings/${SettingsRoutes.MODELS}")
            }, tag = "setup-engine-phone")
        RadioRow("Cloud", "Audio is sent to a provider with your own key. Charges may apply. Choose a provider on the next page.", !choice.isPhone(),
            { navigate("Settings/${SettingsRoutes.ENGINE}") }, tag = "setup-engine-cloud")
    }
    state.transfer?.let { SettingsGroup("Model transfer") { GroupBlock { LinearProgressIndicator(Modifier.fillMaxWidth()); Text(it) } } }
    SettingsGroup {
        InfoRow("New dictations use", engineSummary(context), if (engineReady(context)) Tone.Ok else Tone.Attention)
    }
}

/** A practice field inside $PRODUCT_NAME: records here and shows the transcript; no other app is touched. */
@Composable private fun SetupTest(vm: WriterViewModel, state: WriterState) {
    var startedAt by rememberSaveable { mutableLongStateOf(0L) }
    var practice by rememberSaveable { mutableStateOf("") }
    val result = state.records.filter { startedAt > 0 && it.created >= startedAt }.maxByOrNull { it.created }
    LaunchedEffect(result?.latest?.raw) { result?.latest?.let { practice = it.selectedText } }
    val recording = state.capture in listOf("starting", "recording")
    val status = when {
        startedAt == 0L -> "Tap Start test, say a sentence, then tap Stop."
        recording -> "Recording · tap Stop when you are done"
        state.capture == "saving" -> "Saving your recording…"
        result?.latest != null -> "Your words arrived. The microphone and speech engine work."
        result?.view?.error != null -> "Transcription did not finish: ${result.view.error.message}. Your audio is saved in History."
        result?.busy == true -> stageWords(result.view!!.stage) + "…"
        else -> "Waiting for the result…"
    }
    SettingsGroup("Test dictation", footer = "This proves the microphone and the speech engine inside $PRODUCT_NAME. Typing into other apps is checked separately with the floating button.") {
        GroupBlock {
            Text(status, style = MaterialTheme.typography.bodyMedium, modifier = Modifier.semantics { liveRegion = LiveRegionMode.Polite }.testTag("setup-test-status"))
            OutlinedTextField(practice, { practice = it }, label = { Text("Practice field") }, minLines = 3, modifier = Modifier.fillMaxWidth().testTag("setup-practice"))
            if (recording) Button(onClick = vm::finish, modifier = Modifier.heightIn(min = 48.dp).testTag("setup-test-stop")) { Text("Stop") }
            else Button(onClick = { startedAt = System.currentTimeMillis(); practice = ""; vm.start() },
                enabled = !state.processing && state.capture == "idle" && PermissionStatus.read(vm.context).microphone,
                modifier = Modifier.heightIn(min = 48.dp).testTag("setup-test-start")) { Text(if (startedAt == 0L) "Start test" else "Test again") }
        }
    }
}
