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

private val tabs = listOf("Home", "History", "Settings")

/** The back stack an entry route opens: its tab, then the page. Old routes keep working (U-1: Words and Stats moved down). */
internal fun initialStack(route: String): List<String> {
    val mapped = when (route) {
        "Dictionary" -> "Settings/Words"
        "Statistics" -> "History/Statistics"
        "History/Text to recover" -> "History/Needs attention"
        else -> route
    }
    val tab = mapped.substringBefore('/').takeIf { it in tabs } ?: "Home"
    return if ('/' in mapped && mapped.substringBefore('/') in tabs) listOf(tab, mapped) else listOf(tab)
}

/** Top-bar title for a pushed route. */
private fun routeTitle(route: String): String = com.metrix.writer.ui.settings.SettingsRoutes.title(route) ?: when (val page = route.substringAfter('/')) {
    "Statistics", "Needs attention", "Synced text" -> page
    "Select" -> "Select and reprocess"
    else -> if (route.startsWith("History/")) "Dictation" else page
}

/**
 * The app shell. [request] carries a notification's route or Copy request (MainActivity extras);
 * it is handled once per new intent.
 */
@Composable fun WriterApp(vm: WriterViewModel, initial: String, request: AppRequest? = null) {
    val state by vm.state.collectAsStateWithLifecycle()
    val stack = rememberSaveable(saver = listSaver<SnapshotStateList<String>, String>(save = { it.toList() }, restore = { it.toMutableStateList() })) { initialStack(initial).toMutableStateList() }
    LaunchedEffect(stack.lastOrNull()) { vm.clearScreenMessage(); vm.historyRoute(stack.lastOrNull() ?: "Home") }
    val navigate: (String) -> Unit = { route ->
        if (route in tabs) { stack.clear(); stack.add(route) }
        else if (stack.lastOrNull() != route) {
            // A page from another tab opens on that tab, so Back returns within it.
            val tab = route.substringBefore('/')
            if (tab in tabs && stack.first() != tab) { stack.clear(); stack.add(tab) }
            stack.add(route)
        }
    }
    LaunchedEffect(request) {
        request ?: return@LaunchedEffect
        request.route?.let { route -> stack.clear(); stack.addAll(initialStack(route)) }
        request.copySession?.let { vm.copySaved(it) }
    }
    var theme by remember { mutableStateOf(vm.appearance.theme) }
    var welcome by rememberSaveable { mutableStateOf(!vm.appearance.onboarding && initial == "Home") }
    // Separate from the welcome/legal dialog: both setup and exploring encounter the disclosure.
    // This is the app's only consent flow; every screen asks for it through LocalInsertionReview.
    var discloseInsertion by rememberSaveable { mutableStateOf(initial == "Home" && AccessibilityConsent(vm.context).choice == "unset") }
    var onDisclosureClosed by remember { mutableStateOf<(() -> Unit)?>(null) }
    val reviewInsertion: (() -> Unit) -> Unit = { closed -> onDisclosureClosed = closed; discloseInsertion = true }
    val context = LocalContext.current
    val activity = androidx.activity.compose.LocalActivity.current
    // A recording keeps the screen awake while Writer is in front; screen-off no longer ends it either.
    val recording = state.capture in listOf("starting", "recording")
    DisposableEffect(activity, recording) {
        if (recording) activity?.window?.addFlags(android.view.WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        onDispose { activity?.window?.clearFlags(android.view.WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON) }
    }
    val permission = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { if (it) vm.start() else vm.notify("Microphone access is needed to record. Enable it in Settings → Permissions.", true) }
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
        val attention = state.records.count { it.attention() != null }
        CompositionLocalProvider(LocalInsertionReview provides reviewInsertion) {
        Scaffold(
            topBar = { TopAppBar(title = { Text(if (stack.size > 1) routeTitle(stack.last()) else stack.first(), maxLines = 2) },
                navigationIcon = { if (stack.size > 1) IconButton(onClick = { stack.removeAt(stack.lastIndex) }, modifier = Modifier.semantics { contentDescription = "Back" }) {
                    WriterIcon(if (LocalLayoutDirection.current == LayoutDirection.Rtl) "next" else "back")
                } else Box(Modifier.padding(start = 20.dp, end = 10.dp)) { WriterIcon("wave", tint = MaterialTheme.colorScheme.primary) } },
                colors = TopAppBarDefaults.topAppBarColors(containerColor = MaterialTheme.colorScheme.background)) },
            bottomBar = { NavigationBar(containerColor = MaterialTheme.colorScheme.surfaceContainerLow) {
                tabs.forEach { tab -> NavigationBarItem(selected = stack.first() == tab, onClick = { stack.clear(); stack.add(tab) },
                    icon = {
                        if (tab == "History" && attention > 0) BadgedBox(badge = { Badge { Text("$attention") } }) { WriterIcon(tab, tint = if (stack.first() == tab) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.onSurfaceVariant) }
                        else WriterIcon(tab, tint = if (stack.first() == tab) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.onSurfaceVariant)
                    },
                    label = { Text(tab, maxLines = 1, style = MaterialTheme.typography.labelMedium) },
                    modifier = Modifier.testTag("nav-$tab").semantics { contentDescription = if (tab == "History" && attention > 0) "History, $attention need attention" else tab }) }
            } }
        ) { padding ->
            Column(Modifier.fillMaxSize().padding(padding).consumeWindowInsets(padding).imePadding()) {
                state.message?.let { message -> Surface(color = MaterialTheme.colorScheme.secondaryContainer, modifier = Modifier.fillMaxWidth().semantics { liveRegion = LiveRegionMode.Polite }) {
                    Row(Modifier.padding(horizontal = 16.dp, vertical = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                        Text(message, Modifier.weight(1f), style = MaterialTheme.typography.bodyMedium)
                        IconButton(onClick = { vm.notify(null) }, modifier = Modifier.semantics { contentDescription = "Dismiss message" }) { WriterIcon("close") }
                    }
                } }
                if (state.actionBusy) LinearProgressIndicator(Modifier.fillMaxWidth())
                NavDisplay(backStack = stack, onBack = { if (stack.size > 1) stack.removeAt(stack.lastIndex) }, entryProvider = { route ->
                    NavEntry(route) { when {
                        route == "Home" -> HomeScreen(vm, state, start, navigate)
                        route == "History" -> HistoryScreen(vm, state, navigate, attentionOnly = false)
                        route == "History/Needs attention" -> HistoryScreen(vm, state, navigate, attentionOnly = true)
                        route == "History/Synced text" -> Account.TranscriptPage()
                        route == "History/Statistics" -> StatisticsScreen(state.statisticsRecords)
                        route == "History/Select" -> ReprocessPage(vm, state, navigate) { if (stack.size > 1) stack.removeAt(stack.lastIndex) }
                        route.startsWith("History/") -> EntryScreen(vm, state, route.substringAfter('/'), navigate)
                        else -> SettingsScreen(vm, state, route, navigate) { mode -> vm.appearance.theme = mode; theme = mode }
                    } }
                })
            }
        }
        if (welcome) ProtectedAlertDialog(onDismissRequest = { welcome = false; vm.appearance.onboarding = true },
            icon = { WriterIcon("wave", Modifier.size(40.dp), MaterialTheme.colorScheme.primary) }, title = { Text("Your voice. Your words.") },
            text = { Column {
                Text("Welcome to $PRODUCT_NAME. Record here, or use the floating control beside your usual keyboard. Speech runs on this phone unless you choose a cloud engine. Then try your first dictation.")
                LegalLinkRow(vm::notify)
            } },
            confirmButton = { TextButton(onClick = { welcome = false; vm.appearance.onboarding = true; stack.clear(); stack.add("Settings"); navigate("Settings/Setup") }) { Text("Set up $PRODUCT_NAME") } },
            dismissButton = { TextButton(onClick = { welcome = false; vm.appearance.onboarding = true }) { Text("Explore first") } })
        if (!welcome && discloseInsertion) AccessibilitySetupFlow(vm) { discloseInsertion = false; onDisclosureClosed?.invoke(); onDisclosureClosed = null }
        }
    }
}

/** A notification's request: open [route], or copy [copySession]'s saved result. */
data class AppRequest(val route: String?, val copySession: String?, val serial: Long)

/** A scrolling page: 16 dp gutters, one column capped at 600 dp and centred on large screens. */
@Composable internal fun Page(content: @Composable ColumnScope.() -> Unit) {
    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(horizontal = 16.dp, vertical = 12.dp), horizontalAlignment = Alignment.CenterHorizontally) {
        Column(Modifier.widthIn(max = 600.dp).fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(16.dp), content = content)
    }
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
