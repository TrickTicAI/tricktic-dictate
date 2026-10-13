@file:OptIn(androidx.compose.material3.ExperimentalMaterial3Api::class)
package com.metrix.writer.ui

import android.Manifest
import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.media.MediaPlayer
import android.os.Build
import android.provider.Settings
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.*
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.relocation.BringIntoViewRequester
import androidx.compose.foundation.relocation.bringIntoViewRequester
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.*
import androidx.compose.runtime.snapshots.SnapshotStateList
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalLayoutDirection
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.*
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.LayoutDirection
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.LifecycleResumeEffect
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.navigation3.runtime.NavEntry
import androidx.navigation3.ui.NavDisplay
import com.metrix.writer.*
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import java.text.DateFormat
import java.util.Date

private val tabs = listOf("Home", "History", "Dictionary", "Statistics", "Settings")

@Composable fun WriterApp(vm: WriterViewModel, initial: String) {
    val state by vm.state.collectAsStateWithLifecycle()
    val stack = rememberSaveable(saver = listSaver<SnapshotStateList<String>, String>(save = { it.toList() }, restore = { it.toMutableStateList() })) { mutableStateListOf(initial.substringBefore('/')).apply { if ('/' in initial) add(initial) } }
    LaunchedEffect(stack.lastOrNull()) { vm.clearScreenMessage(); vm.historyRoute(stack.lastOrNull() ?: "Home") }
    val navigate: (String) -> Unit = { if (it in tabs) { stack.clear(); stack.add(it) } else if (stack.lastOrNull() != it) stack.add(it) }
    var theme by remember { mutableStateOf(vm.appearance.theme) }
    var welcome by rememberSaveable { mutableStateOf(!vm.appearance.onboarding && initial == "Home") }
    // Separate from the welcome/legal dialog: both setup and exploring encounter the disclosure.
    var discloseInsertion by rememberSaveable { mutableStateOf(initial == "Home" && AccessibilityConsent(vm.context).choice == "unset") }
    val context = LocalContext.current
    val activity = androidx.activity.compose.LocalActivity.current
    // A recording keeps the screen awake while Writer is in front; screen-off no longer ends it either.
    val recording = state.capture in listOf("starting", "recording")
    DisposableEffect(activity, recording) {
        if (recording) activity?.window?.addFlags(android.view.WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        onDispose { activity?.window?.clearFlags(android.view.WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON) }
    }
    val permission = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { if (it) vm.start() else vm.notify("Microphone access is needed to record. Enable it in Permissions.") }
    val start: () -> Unit = { if (context.checkSelfPermission(Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED) vm.start() else permission.launch(Manifest.permission.RECORD_AUDIO) }
    LifecycleResumeEffect(Unit) { vm.observe(true); theme = vm.appearance.theme; onPauseOrDispose { vm.observe(false) } }
    WriterTheme(theme) {
        val dark = MaterialTheme.colorScheme.background.red < .5f
        SideEffect {
            val window = activity?.window ?: return@SideEffect
            androidx.core.view.WindowCompat.getInsetsController(window, window.decorView).apply {
                isAppearanceLightStatusBars = !dark
                isAppearanceLightNavigationBars = !dark
            }
        }
        Scaffold(
            topBar = { TopAppBar(title = { Text(if (stack.size > 1) com.metrix.writer.ui.settings.SettingsRoutes.title(stack.last()) ?: stack.last().substringAfterLast('/').let { if (it.length > 30) "Dictation" else it } else stack.first()) },
                navigationIcon = { if (stack.size > 1) IconButton(onClick = { stack.removeAt(stack.lastIndex) }, modifier = Modifier.semantics { contentDescription = "Back" }) {
                    WriterIcon(if (LocalLayoutDirection.current == LayoutDirection.Rtl) "next" else "back")
                } else Box(Modifier.padding(start = 20.dp, end = 10.dp)) { WriterIcon("wave", tint = MaterialTheme.colorScheme.primary) } },
                colors = TopAppBarDefaults.topAppBarColors(containerColor = MaterialTheme.colorScheme.background)) },
            bottomBar = { NavigationBar(containerColor = MaterialTheme.colorScheme.surfaceContainerLow) {
                tabs.forEach { tab -> NavigationBarItem(selected = stack.first() == tab, onClick = { stack.clear(); stack.add(tab) },
                    icon = { WriterIcon(tab, tint = if (stack.first() == tab) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.onSurfaceVariant) },
                    label = { Text(when(tab) { "Statistics" -> "Stats"; "Dictionary" -> "Words"; else -> tab }, maxLines = 1, style = MaterialTheme.typography.labelSmall) }, modifier = Modifier.testTag("nav-$tab").semantics { contentDescription = tab }) }
            } }
        ) { padding ->
            Column(Modifier.fillMaxSize().padding(padding)) {
                state.message?.let { message -> Surface(color = MaterialTheme.colorScheme.secondaryContainer, modifier = Modifier.fillMaxWidth()) {
                    Row(Modifier.padding(horizontal = 20.dp, vertical = 6.dp), verticalAlignment = Alignment.CenterVertically) {
                        Text(message, Modifier.weight(1f), style = MaterialTheme.typography.bodyMedium)
                        IconButton(onClick = { vm.notify(null) }, modifier = Modifier.semantics { contentDescription = "Dismiss message" }) { WriterIcon("close") }
                    }
                } }
                if (state.actionBusy) LinearProgressIndicator(Modifier.fillMaxWidth())
                NavDisplay(backStack = stack, onBack = { if (stack.size > 1) stack.removeAt(stack.lastIndex) }, entryProvider = { route ->
                    NavEntry(route) { when {
                        route == "Home" -> HomeScreen(vm, state, start, navigate) { discloseInsertion = true }
                        route == "History" -> HistoryScreen(vm, state, navigate)
                        route == "History/Synced text" -> Account.TranscriptPage()
                        route == "History/Text to recover" -> DeliveryScreen(state.deliveries)
                        route.startsWith("History/") -> DetailScreen(vm, state, route.substringAfter('/'), navigate)
                        route == "Dictionary" -> DictionaryScreen(vm, state.revision)
                        route == "Statistics" -> StatisticsScreen(state.statisticsRecords)
                        else -> SettingsScreen(vm, state, route, navigate) { mode -> vm.appearance.theme = mode; theme = mode }
                    } }
                })
            }
        }
        if (welcome) ProtectedAlertDialog(onDismissRequest = { welcome = false; vm.appearance.onboarding = true },
            icon = { WriterIcon("wave", Modifier.size(40.dp), MaterialTheme.colorScheme.primary) }, title = { Text("Your voice. Your words.") },
            text = { Column {
                Text("Welcome to $PRODUCT_NAME. Record here, or use the floating control beside your usual keyboard. Choose cloud or a model on this phone, then try your first dictation.")
                LegalLinkRow(vm::notify)
            } },
            confirmButton = { TextButton(onClick = { welcome = false; vm.appearance.onboarding = true; stack.clear(); stack.add("Settings"); navigate("Settings/Setup") }) { Text("Set up $PRODUCT_NAME") } },
            dismissButton = { TextButton(onClick = { welcome = false; vm.appearance.onboarding = true }) { Text("Explore first") } })
        if (!welcome && discloseInsertion) AccessibilitySetupFlow(vm) { discloseInsertion = false }
    }
}

@Composable internal fun Page(content: @Composable ColumnScope.() -> Unit) {
    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(horizontal = 20.dp, vertical = 12.dp), verticalArrangement = Arrangement.spacedBy(18.dp), content = content)
}
/**
 * Scrolls to and briefly outlines the element titled [title] when Settings
 * search opened its page for it (see [SettingsSearch]). Otherwise it does nothing.
 */
@Composable internal fun rememberSearchReveal(title: String, shape: androidx.compose.ui.graphics.Shape): Modifier {
    val requester = remember { BringIntoViewRequester() }
    val mark = remember { androidx.compose.animation.core.Animatable(0f) }
    LaunchedEffect(Unit) {
        if (!SettingsSearch.consume(title)) return@LaunchedEffect
        kotlinx.coroutines.delay(250) // Let the page lay out and its transition settle.
        requester.bringIntoView()
        mark.snapTo(1f); kotlinx.coroutines.delay(1200)
        mark.animateTo(0f, androidx.compose.animation.core.tween(1200))
    }
    val color = MaterialTheme.colorScheme.primary
    return Modifier.bringIntoViewRequester(requester)
        .then(if (mark.value > 0f) Modifier.border(2.dp, color.copy(alpha = mark.value), shape) else Modifier)
}
@Composable internal fun Section(title: String, subtitle: String? = null, content: @Composable ColumnScope.() -> Unit) {
    Column(rememberSearchReveal(title, MaterialTheme.shapes.large), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        Text(title, style = MaterialTheme.typography.titleMedium)
        subtitle?.let { Text(it, color = MaterialTheme.colorScheme.onSurfaceVariant, style = MaterialTheme.typography.bodyMedium) }
        Surface(shape = MaterialTheme.shapes.large, color = MaterialTheme.colorScheme.surfaceContainerLow, modifier = Modifier.fillMaxWidth()) {
            Column(Modifier.padding(18.dp), verticalArrangement = Arrangement.spacedBy(12.dp), content = content)
        }
    }
}
@Composable internal fun SettingRow(title: String, subtitle: String, onClick: () -> Unit, icon: String = "Settings") {
    Surface(onClick = onClick, shape = MaterialTheme.shapes.medium, color = MaterialTheme.colorScheme.surfaceContainerLow, modifier = rememberSearchReveal(title, MaterialTheme.shapes.medium)) {
        Row(Modifier.fillMaxWidth().padding(16.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(14.dp)) {
            WriterIcon(icon, tint = MaterialTheme.colorScheme.primary)
            Column(Modifier.weight(1f)) { Text(title, style = MaterialTheme.typography.titleSmall); Text(subtitle, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant) }
            WriterIcon(if (LocalLayoutDirection.current == LayoutDirection.Rtl) "back" else "next")
        }
    }
}
@Composable internal fun EmptyState(title: String, text: String, icon: String) {
    Column(Modifier.fillMaxWidth().padding(vertical = 36.dp), horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(12.dp)) {
        WriterIcon(icon, Modifier.size(44.dp), MaterialTheme.colorScheme.primary)
        Text(title, style = MaterialTheme.typography.titleLarge)
        Text(text, color = MaterialTheme.colorScheme.onSurfaceVariant)
    }
}
@Composable private fun HomeScreen(vm: WriterViewModel, state: WriterState, start: () -> Unit, navigate: (String) -> Unit, reviewInsertion: () -> Unit) {
    val choice = DictationChoice(vm.context)
    Page {
        Text("Make room for\nyour thoughts.", style = MaterialTheme.typography.headlineLarge)
        Text("Speak naturally. Keep the words that matter.", color = MaterialTheme.colorScheme.onSurfaceVariant)
        if ((!choice.isPhone() && !DirectCloud(vm.context).configured()) || (choice.isPhone() && !runCatching { PhoneModels(vm.context).file(choice.model()).exists() }.getOrDefault(false))) {
            SettingRow("Finish setting up", "Choose an available transcription model", { navigate("Settings/Setup") }, "record")
        }
        Surface(color = MaterialTheme.colorScheme.primaryContainer, shape = MaterialTheme.shapes.large, modifier = Modifier.fillMaxWidth()) {
            Column(Modifier.padding(24.dp), horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(18.dp)) {
                Text(if (state.capture == "starting") "Starting microphone…" else if (state.capture == "saving") "Finishing recording…" else if (state.capture == "recording") "Listening to you" else if (state.processing) "Turning speech into text" else "Ready when you are", style = MaterialTheme.typography.titleLarge)
                if (state.capture != "idle") {
                    RecordingPanel(state.capture, { vm.manager.capture.level }, vm::finish, vm::cancelRecording)
                    Text(formatDuration(state.elapsed), style = MaterialTheme.typography.labelLarge)
                } else if (state.processing) {
                    CircularProgressIndicator(Modifier.size(32.dp))
                    Text("Your audio is saved on this phone.", style = MaterialTheme.typography.bodyMedium)
                    TextButton(onClick = { vm.manager.cancelWork() }) { Text("Stop processing · keep audio") }
                } else Button(onClick = start, modifier = Modifier.heightIn(min = 56.dp).testTag("start-recording")) {
                    WriterIcon("record", tint = MaterialTheme.colorScheme.onPrimary); Spacer(Modifier.width(10.dp)); Text("Start recording")
                }
                AssistChip(onClick = { navigate("Settings/Engine") }, label = { Text("${choice.engineLabel()} · ${CompanionUi.languageName(choice.language())}", style = MaterialTheme.typography.labelMedium) },
                    modifier = Modifier.testTag("home-engine").semantics { contentDescription = "Active engine ${choice.engineLabel()}. Change engine" })
            }
        }
        val insertion = PermissionStatus.read(vm.context)
        if (!insertion.enabled || !AccessibilityConsent(vm.context).accepted) {
            SettingRow(if (insertion.enabled) "Text insertion is paused" else "Type into other apps",
                "Review Accessibility access · optional; recording here still works", reviewInsertion, "Dictionary")
        }
        val overlay = FloatingPreferences(vm.context)
        if (overlay.snoozed()) SettingRow("Floating control is hidden", "Tap to show it now", { overlay.showNow(); vm.notify("Floating control restored beside supported text fields.") }, "record")
        Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
            Text("Recent dictations", Modifier.weight(1f), style = MaterialTheme.typography.titleMedium)
            TextButton(onClick = { navigate("History") }) { Text("View all") }
        }
        if (state.records.isEmpty()) EmptyState("A fresh page", "Your first dictation will appear here.", "History")
        state.records.take(3).forEach { row -> DictationRow(row) { navigate("History/${row.id}") } }
    }
}
@Composable private fun DictationRow(row: Dictation, click: () -> Unit) {
    Surface(onClick = click, shape = MaterialTheme.shapes.medium, color = MaterialTheme.colorScheme.surfaceContainerLow, modifier = Modifier.fillMaxWidth().testTag("dictation-${row.id}")) {
        Column(Modifier.padding(18.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween) {
                Text(DateFormat.getDateTimeInstance(DateFormat.SHORT, DateFormat.SHORT).format(Date(row.created)), style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
                Text(formatDuration(row.duration), style = MaterialTheme.typography.labelMedium)
            }
            Text(row.latest?.raw?.takeIf { it.isNotBlank() } ?: "Saved audio", maxLines = 3, overflow = TextOverflow.Ellipsis, style = MaterialTheme.typography.bodyLarge)
            Text(if (row.pending) "Waiting for transcription" else if (row.postprocessing) "Text processing needs attention" else if (row.latest != null) "Transcript ready${if(row.capture == "interrupted") " · audio interrupted" else ""}" else if (row.capture == "interrupted") "Interrupted · audio retained" else "Ready to transcribe", style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.primary)
        }
    }
}
@Composable private fun HistoryScreen(vm: WriterViewModel, state: WriterState, navigate: (String) -> Unit) {
    var query by rememberSaveable { mutableStateOf("") }
    Column(Modifier.fillMaxSize().padding(horizontal = 20.dp), verticalArrangement = Arrangement.spacedBy(16.dp)) {
        OutlinedTextField(query, { query = it; vm.searchHistory(it) }, label = { Text("Search transcripts") }, singleLine = true, modifier = Modifier.fillMaxWidth())
        TextButton(onClick = { navigate("History/Text to recover") }) { Text("Text to recover") }
        Text("Recordings and transcripts · tap a dictation to play, copy, or retry", color = MaterialTheme.colorScheme.onSurfaceVariant, style = MaterialTheme.typography.bodySmall)
        Account.HistoryEntry(navigate)
        ReprocessHistory(vm, state, navigate)
        val filtered = state.records.filter { query.isBlank() || it.latest?.raw?.contains(query, true) == true }
        LazyColumn(verticalArrangement = Arrangement.spacedBy(12.dp), contentPadding = PaddingValues(bottom = 24.dp)) {
            if (filtered.isEmpty()) item { EmptyState(if (query.isBlank()) "No dictations yet" else "No matching dictations", "Record something worth keeping.", "History") }
            items(filtered, key = { it.id }) { row -> DictationRow(row) { navigate("History/${row.id}") } }
            if (state.historyNext != null) item { TextButton(onClick = vm::loadMoreHistory) { Text("Load more dictations") } }
        }
    }
}
@Composable private fun DetailScreen(vm: WriterViewModel, state: WriterState, id: String, navigate: (String) -> Unit) {
    val row = state.records.firstOrNull { it.id == id }
    val context = LocalContext.current
    var confirm by remember { mutableStateOf<String?>(null) }
    var player by remember { mutableStateOf<MediaPlayer?>(null) }
    var export by rememberSaveable { mutableStateOf<String?>(null) }
    val saveAudio = rememberLauncherForActivityResult(ActivityResultContracts.CreateDocument("audio/wav")) { uri ->
        if (uri != null && export == id) vm.action { context.contentResolver.openOutputStream(uri, "wt")!!.use { out -> vm.manager.store.audio(vm.manager.store.load(id)).inputStream().use { it.copyTo(out) } }; vm.notify("Audio exported.") }
        export = null
    }
    DisposableEffect(Unit) { onDispose { player?.release() } }
    LifecycleResumeEffect(Unit) { onPauseOrDispose { player?.release(); player = null } }
    if (row == null) { EmptyState("Dictation removed", "Return to History to see your saved recordings.", "History"); return }
    Page {
        Text(DateFormat.getDateTimeInstance().format(Date(row.created)), style = MaterialTheme.typography.titleMedium)
        Text("${formatDuration(row.duration)} · ${row.capture}", color = MaterialTheme.colorScheme.onSurfaceVariant)
        row.historicalSource?.let { source -> Section("Separate historical version") {
            Text("Source recording: $source")
            row.historicalSourceJob?.takeIf { it != "null" }?.let { Text("Source raw version: $it") }
            Text("Historical results are never automatically inserted.")
            TextButton(onClick = { navigate("History/$source") }) { Text("Open source recording") }
        } }
        Section("Audio") {
            if (!row.audioAvailable) Text("Audio unavailable · saved raw text remains available for cleanup.")
            FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                OutlinedButton(onClick = {
                    if (player != null) { player?.release(); player = null }
                    else {
                        val audio = MediaPlayer()
                        try {
                            audio.setDataSource(vm.manager.store.audio(vm.manager.store.load(id)).path)
                            audio.setOnPreparedListener { if (player === it) it.start() }
                            audio.setOnCompletionListener { it.release(); player = null }
                            audio.setOnErrorListener { value, _, _ -> value.release(); player = null; vm.notify("Playback failed. Saved audio remains available."); true }
                            player = audio; audio.prepareAsync()
                        } catch (_: Exception) { audio.release(); player = null; vm.notify("Playback failed. Saved audio remains available.") }
                    }
                }, enabled = row.audioAvailable && row.capture != "capturing") { Text(if (player == null) "Play" else "Stop") }
                TextButton(onClick = { export = id; saveAudio.launch("$PRODUCT_NAME recording.wav") }, enabled = row.audioAvailable && row.capture != "capturing") { Text("Export") }
                TextButton(onClick = { vm.action {
                    val uri = AudioExportProvider.snapshot(context, vm.manager.store, id)
                    withContext(Dispatchers.Main) { context.startActivity(Intent.createChooser(Intent(Intent.ACTION_SEND).setType("audio/wav").putExtra(Intent.EXTRA_STREAM, uri).addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION), "Share audio")) }
                } }, enabled = row.audioAvailable && row.capture != "capturing") { Text("Share") }
            }
            Button(onClick = { vm.retry(id) }, enabled = row.audioAvailable && !state.processing && state.capture == "idle" && !row.postprocessing) { Text(if (row.pending) "Resume transcription" else "Transcribe / retry") }
            if (row.pending) TextButton(onClick = { confirm = "replace" }) { Text("New cloud attempt with current settings") }
            if (row.postprocessing) {
                Button(onClick = { vm.resume(id) }, enabled = !state.processing) { Text("Resume text processing") }
                TextButton(onClick = { confirm = "cleanup" }) { Text("Use current cleanup settings") }
            }
            if (row.pending || row.postprocessing) Text("Resume keeps the request's saved settings. Earlier audio and text remain available.", style = MaterialTheme.typography.bodySmall)
        }
        row.attempts.asReversed().forEach { attempt ->
            Section(if (attempt.state == "succeeded") "Transcript" else "Attempt · ${attempt.state}", "${attempt.model} · ${attempt.location}") {
                if (attempt.state == "succeeded") {
                    TranscriptText(if (attempt.originalUsed) "Current wording · original" else "Current wording", attempt.selectedText, vm)
                    if (attempt.originalUsed) Text("Polishing did not return usable text. Your corrections and snippets are kept.", style = MaterialTheme.typography.bodySmall)
                    if (attempt.originalDelivered) Text("The original was inserted. Retried polishing stays here for you to copy.", style = MaterialTheme.typography.bodySmall)
                    TranscriptText("Spoken wording", attempt.raw, vm)
                    attempt.dictionary?.let { TranscriptText("With saved corrections & snippets", it, vm) }
                    attempt.cleaned?.takeIf { it != attempt.selectedText }?.let { TranscriptText("Earlier polished wording", it, vm) }
                    TextButton(onClick = { confirm = "process:${attempt.id}" }, enabled = !state.processing && !row.pending && !row.postprocessing) { Text("Retry polishing…") }
                } else attempt.error?.let { Text(it, color = MaterialTheme.colorScheme.error) }
            }
        }
        row.delivery?.let { Text(when(it) { "delivered" -> "Inserted and verified"; "unconfirmed" -> "Insertion unconfirmed. Check the field you were in before copying."; else -> "Text retained. Copy when you are ready." }, style = MaterialTheme.typography.bodyMedium) }
        OutlinedButton(onClick = { confirm = "delete" }, enabled = !state.processing && state.capture == "idle" && !row.pending && !row.postprocessing) { Text("Delete dictation", color = MaterialTheme.colorScheme.error) }
    }
    confirm?.let { action -> ProtectedAlertDialog(onDismissRequest = { confirm = null }, title = { Text(if (action == "delete") "Delete this dictation?" else if (action.startsWith("process")) "Polish wording online?" else "Use current settings?") },
        text = { Text(when { action == "delete" -> "This removes the audio, transcripts, insertion record and $PRODUCT_NAME’s cached shares on this phone. Recipient copies, files you exported elsewhere, cloud provider retention and OS backups are outside this deletion. Already-open shares may remain readable. This is not secure storage erasure."
            action.startsWith("process") -> "Send editable transcript text to your selected polishing provider. Audio is not uploaded. The original stays available."
            else -> "The previous outcome may be unknown. A new cloud request can incur another charge. Earlier audio and text stay saved." }) },
        confirmButton = { TextButton(onClick = { when { action == "delete" -> { player?.release(); player = null; vm.delete(id) }; action == "replace" -> vm.replacePending(id); action == "cleanup" -> vm.replaceCleanup(id); else -> vm.cleanup(id, action.substringAfter(':')) }; confirm = null }) { Text("Continue") } },
        dismissButton = { TextButton(onClick = { confirm = null }) { Text("Cancel") } }) }
}
@Composable private fun TranscriptText(title: String, text: String, vm: WriterViewModel) {
    val context = LocalContext.current
    // Each variant exports itself, so the file holds exactly the wording on screen
    // and its name says which one: spoken, corrected or polished.
    var exporting by rememberSaveable { mutableStateOf(false) }
    val saveTranscript = rememberLauncherForActivityResult(ActivityResultContracts.CreateDocument("text/plain")) { uri ->
        if (uri != null && exporting) vm.action { context.contentResolver.openOutputStream(uri, "wt")!!.use { out -> out.write(text.toByteArray()) }; vm.notify("$title exported.") }
        exporting = false
    }
    Text(title, style = MaterialTheme.typography.labelLarge, color = MaterialTheme.colorScheme.primary)
    androidx.compose.foundation.text.selection.SelectionContainer { Text(text, style = MaterialTheme.typography.bodyLarge) }
    FlowRow {
        TextButton(onClick = { context.getSystemService(ClipboardManager::class.java).setPrimaryClip(ClipData.newPlainText(title, text)); vm.notify("Copied.") }) { WriterIcon("copy"); Spacer(Modifier.width(6.dp)); Text("Copy $title") }
        TextButton(onClick = { exporting = true; saveTranscript.launch("$PRODUCT_NAME $title.txt") }) { Text("Export $title") }
        TextButton(onClick = { context.startActivity(Intent.createChooser(Intent(Intent.ACTION_SEND).setType("text/plain").putExtra(Intent.EXTRA_TEXT, text), "Share text")) }) { Text("Share") }
    }
}

@Composable private fun DeliveryScreen(deliveries: List<SavedDelivery>) {
    val context = LocalContext.current
    LazyColumn(contentPadding = PaddingValues(20.dp), verticalArrangement = Arrangement.spacedBy(16.dp)) {
        item { Text("Saved text is also available here if its recording is missing. Check the field you were in before copying an unconfirmed insertion. Copy changes the clipboard only when you tap it.") }
        items(deliveries, key = { it.token }) { delivery ->
            Section(if (delivery.fixture) "Delivery test" else "Saved result") {
                Text(when(delivery.state) { "delivered" -> "Inserted and verified"; "unconfirmed" -> "Check the field you were in"; else -> "Ready to copy" }, color = MaterialTheme.colorScheme.primary)
                androidx.compose.foundation.text.selection.SelectionContainer { Text(delivery.text) }
                TextButton(onClick = { context.getSystemService(ClipboardManager::class.java).setPrimaryClip(ClipData.newPlainText("$PRODUCT_NAME saved result", delivery.text)) }) { Text("Copy saved result") }
            }
        }
    }
}
