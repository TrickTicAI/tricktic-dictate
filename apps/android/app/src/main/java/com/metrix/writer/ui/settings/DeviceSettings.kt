package com.metrix.writer.ui.settings

import android.Manifest
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.provider.Settings
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.LifecycleResumeEffect
import com.metrix.writer.*
import com.metrix.writer.ui.*
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch

internal val THEMES = listOf("system" to "Follow system", "light" to "Paper · warm light", "dark" to "Ink · warm dark",
    "sage" to "Sage · calm green", "ocean" to "Ocean · deep blue")

internal fun themeName(key: String) = THEMES.find { it.first == key }?.second?.substringBefore(" ·") ?: "Follow system"

/** The Settings list row for Floating control. */
internal fun floatingSummary(context: Context): String {
    val prefs = FloatingPreferences(context)
    return when {
        !prefs.enabled -> "Off"
        prefs.snoozed() -> "Hidden until ${java.text.DateFormat.getTimeInstance(java.text.DateFormat.SHORT).format(java.util.Date(prefs.until))}"
        else -> "On · ${if (prefs.dock) "docked to an edge" else "free placement"} · ${prefs.size} dp"
    }
}

/** What is missing for recording and insertion, in the order Permissions lists it. */
internal fun missingPermissions(context: Context): List<String> {
    val grants = PermissionStatus.read(context)
    return listOfNotNull(
        if (!grants.microphone) "Microphone" else null,
        if (!grants.overlay) "Display over other apps" else null,
        if (!(grants.enabled && AccessibilityConsent(context).accepted)) "Text insertion" else null,
        if (!grants.notifications) "Recording notifications" else null,
    )
}

@OptIn(ExperimentalLayoutApi::class)
@Composable internal fun FloatingPage(vm: WriterViewModel, state: WriterState) {
    val prefs = remember { FloatingPreferences(vm.context) }
    var size by remember { mutableIntStateOf(prefs.size) }
    var enabled by remember { mutableStateOf(prefs.enabled) }
    var dock by remember { mutableStateOf(prefs.dock) }
    var minutes by remember { mutableIntStateOf(prefs.minutes) }
    var preview by remember { mutableStateOf("idle") }
    var snoozed by remember { mutableStateOf(prefs.snoozed()) }
    val feedback = remember { FeedbackPreferences(vm.context) }
    var haptic by remember { mutableStateOf(feedback.haptic) }
    var sound by remember { mutableStateOf(feedback.sound) }
    var timer by remember { mutableStateOf(feedback.timer) }
    Page {
        PageIntro("The microphone that appears beside supported text fields, next to your usual keyboard.")
        SettingsGroup("A small space for your voice", footer = "The recording preview is silent. During real capture, the waveform follows your microphone.") {
            GroupBlock {
                Box(Modifier.fillMaxWidth().height(100.dp), contentAlignment = Alignment.Center) {
                    if (preview == "idle") IdleControl(size, { preview = "recording" })
                    else RecordingPanel(preview, { 0f }, { preview = "processing" }, { preview = "idle" }, preview = true)
                }
                FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    listOf("idle", "recording", "processing").forEach { mode -> FilterChip(preview == mode, { preview = mode }, label = { Text(mode.replaceFirstChar(Char::uppercase)) }) }
                }
            }
        }
        SettingsGroup("Feedback", footer = "Feedback marks when recording really starts and when the result is in. Sound plays only when your phone's ringer is on.") {
            SwitchRow("Vibrate on start and stop", if (haptic) "On" else "Off", haptic, { haptic = it; feedback.haptic = it }, tag = "feedback-haptic")
            SwitchRow("Sound on start and stop", if (sound) "On · off in silent mode" else "Off in silent mode", sound, { sound = it; feedback.sound = it }, tag = "feedback-sound")
            SwitchRow("Show timer", if (timer) "Elapsed time while recording" else "Off", timer, { timer = it; feedback.timer = it }, tag = "feedback-timer")
        }
        SettingsGroup("Control size", footer = "The touch target stays at least 48 dp.") {
            SwitchRow("Show the floating control", if (enabled) "Beside supported text fields" else "Hidden everywhere", enabled,
                { enabled = it; prefs.enabled = it }, description = "Enable floating control")
            GroupBlock {
                Text("$size dp", style = MaterialTheme.typography.titleMedium)
                Slider(size.toFloat(), { size = it.toInt(); prefs.size = size }, valueRange = 32f..56f, steps = 2,
                    modifier = Modifier.semantics { contentDescription = "Floating control size" })
            }
        }
        SettingsGroup("Placement", choices = true) {
            RadioRow("Free placement · keep where I drag", "Your position is remembered, including the center. It adjusts to fit above the keyboard.", !dock,
                { dock = false; prefs.dock = false })
            RadioRow("Dock to nearest edge", "After dragging, the control snaps to the nearest side.", dock, { dock = true; prefs.dock = true })
            ActionRow("Reset position", { prefs.resetPosition(); vm.notify("Position reset.") })
        }
        SettingsGroup("Temporary hiding", footer = "Drag the idle control to Hide at the bottom of the screen; release elsewhere to cancel. Hiding never discards audio, and an active recording keeps its Finish and Cancel controls.") {
            listOf(10, 30, 60).forEach { value -> RadioRow("$value minutes", null, value == minutes, { minutes = value; prefs.minutes = value }) }
            if (snoozed) InfoRow("Hidden now", "Until ${java.text.DateFormat.getTimeInstance(java.text.DateFormat.SHORT).format(java.util.Date(prefs.until))}", Tone.Attention)
            ActionRow("Hide now", { prefs.snooze(); snoozed = true; vm.notify("Floating control hidden for $minutes minutes.") },
                value = "For $minutes minutes", enabled = state.capture == "idle" && !state.processing)
            ActionRow("Show now", { prefs.showNow(); enabled = true; snoozed = false; vm.notify("Floating control restored.") })
        }
    }
}

/**
 * Each permission with why it is asked for and its live state. Text insertion opens the app's one
 * consent flow through [LocalInsertionReview]; this page never hosts a disclosure of its own.
 */
@Composable internal fun PermissionsPage(vm: WriterViewModel) {
    val context = vm.context
    var grants by remember { mutableStateOf(PermissionStatus.read(context)) }
    val scope = rememberCoroutineScope()
    val consent = remember { AccessibilityConsent(context) }
    var choice by remember { mutableStateOf(consent.choice) }
    val review = LocalInsertionReview.current
    val grant = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { grants = PermissionStatus.read(context) }
    LifecycleResumeEffect(Unit) {
        val job = scope.launch { while (isActive) { grants = PermissionStatus.read(context); choice = consent.choice; delay(300) } }
        onPauseOrDispose { job.cancel() }
    }
    fun open(intent: Intent) { context.startActivity(intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)) }
    Page {
        PageIntro("Allow only what you want to use. Recording in $PRODUCT_NAME needs just the microphone.")
        PermissionRows(grants, choice, { grant.launch(it) }, ::open) { review { choice = consent.choice } }
        Distribution.InstallationHelp(::open)
    }
}

/** The permission checklist shared by Permissions and setup: reason, live state, and what still works without each. */
@Composable internal fun PermissionRows(grants: PermissionStatus, choice: String, request: (String) -> Unit, open: (Intent) -> Unit, reviewInsertion: () -> Unit) {
    val context = LocalContext.current
    fun tone(ok: Boolean) = if (ok) Tone.Ok else Tone.Attention
    SettingsGroup("Access", footer = "Without text insertion, $PRODUCT_NAME can still save text for you to copy. Without the overlay, record on Home instead of beside your keyboard.") {
        NavRow("Microphone", if (grants.microphone) "Allowed · record your voice" else "Not allowed · tap to let $PRODUCT_NAME record your voice",
            { request(Manifest.permission.RECORD_AUDIO) }, icon = "record", tone = tone(grants.microphone), tag = "permission-microphone")
        NavRow("Display over other apps", if (grants.overlay) "Allowed · floating control" else "Not allowed · needed to show the microphone beside your keyboard",
            { open(Intent(Settings.ACTION_MANAGE_OVERLAY_PERMISSION, Uri.parse("package:${context.packageName}"))) }, tone = tone(grants.overlay), tag = "permission-overlay")
        val insertion = when {
            grants.enabled && choice != "accepted" -> "On in Android Settings · paused until you accept. Tap to review."
            grants.bound -> "Connected · inserts into your editor"
            grants.enabled -> "Enabled · waiting to connect. Reopen the service if this persists."
            choice == "declined" -> "Declined · record in $PRODUCT_NAME and copy from History. Tap to review."
            else -> "Off · enable $PRODUCT_NAME · voice insertion"
        }
        NavRow("Text insertion", insertion, reviewInsertion, icon = "Dictionary", tone = tone(grants.bound && choice == "accepted"), tag = "permission-insertion")
        NavRow("Recording notifications", if (grants.notifications) "Allowed · Stop and Discard in the notification shade" else "Not allowed · show Stop and Discard outside $PRODUCT_NAME", {
            if (context.checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED) request(Manifest.permission.POST_NOTIFICATIONS)
            else open(Intent(Settings.ACTION_APP_NOTIFICATION_SETTINGS).putExtra(Settings.EXTRA_APP_PACKAGE, context.packageName))
        }, tone = tone(grants.notifications), tag = "permission-notifications")
    }
}

@Composable internal fun AppearancePage(vm: WriterViewModel, setTheme: (String) -> Unit) {
    var theme by remember { mutableStateOf(vm.appearance.theme) }
    Page {
        SettingsGroup("Theme", choices = true, footer = "The floating control follows the same theme.") {
            THEMES.forEach { (key, label) ->
                RadioRow(label, null, theme == key, { theme = key; setTheme(key) }, trailing = {
                    WriterTheme(key) {
                        Row(horizontalArrangement = Arrangement.spacedBy(4.dp)) {
                            listOf(MaterialTheme.colorScheme.background, MaterialTheme.colorScheme.primary, MaterialTheme.colorScheme.primaryContainer)
                                .forEach { color -> Box(Modifier.size(18.dp).clip(CircleShape).background(color).border(1.dp, MaterialTheme.colorScheme.outlineVariant, CircleShape)) }
                        }
                    }
                })
            }
        }
        SettingsGroup("Text size") {
            InfoRow("Follows Android", "Text size follows Android's font setting. $PRODUCT_NAME supports English, Hebrew and mixed text independently of the interface layout.")
        }
    }
}
