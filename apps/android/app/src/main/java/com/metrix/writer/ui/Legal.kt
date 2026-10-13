package com.metrix.writer.ui

import android.content.ActivityNotFoundException
import android.content.Context
import android.content.Intent
import android.net.Uri
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.DialogProperties
import com.metrix.writer.PhoneModels
import com.metrix.writer.R

/**
 * The public legal pages compiled into the app. These URLs are permanent once shipped.
 * Earlier builds carry https://tricktic.com/automation/legal/tricktic-dictate/..., which
 * answers a permanent redirect to the same page here.
 */
object LegalLinks {
    const val BASE = "https://legal.tricktic.com/tricktic-dictate"
    const val PRIVACY = "$BASE/privacy"
    const val TERMS = "$BASE/terms"
}

internal fun openLink(context: Context, url: String, notify: (String) -> Unit) {
    try { context.startActivity(Intent(Intent.ACTION_VIEW, Uri.parse(url)).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)) }
    catch (_: ActivityNotFoundException) { notify("No browser is available. The page is at $url") }
}

@Composable internal fun LegalLinkRow(notify: (String) -> Unit) {
    val context = LocalContext.current
    FlowRow(horizontalArrangement = Arrangement.spacedBy(4.dp)) {
        TextButton(onClick = { openLink(context, LegalLinks.PRIVACY, notify) }, modifier = Modifier.testTag("privacy-link")) { Text("Privacy policy") }
        TextButton(onClick = { openLink(context, LegalLinks.TERMS, notify) }, modifier = Modifier.testTag("terms-link")) { Text("Terms of use") }
    }
}

/**
 * Prominent disclosure shown before Writer opens Android's Accessibility settings. Each claim
 * matches WriterInputService and editor_accessibility.xml; update both together.
 */
@Composable internal fun AccessibilityDisclosure(onAccept: () -> Unit, onDecline: () -> Unit, onDismiss: () -> Unit) {
    val app = stringResource(R.string.app_name)
    val service = stringResource(R.string.editor_service_label)
    AlertDialog(onDismissRequest = onDismiss, properties = DialogProperties(dismissOnClickOutside = false),
        modifier = Modifier.testTag("accessibility-disclosure"),
        title = { Text("Accessibility access for voice typing") },
        text = { Column(Modifier.verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(10.dp)) {
            Text("$app uses the Android AccessibilityService API (\"$service\") to insert your dictation into the focused text field and show the floating microphone beside your keyboard.")
            Text("What it accesses", style = MaterialTheme.typography.titleSmall)
            Text("It reads the focused field's text, cursor position and view ID, and receives focus, text and window-change events. It checks the foreground app window and keyboard position to place the microphone, insert your words in the right place and verify insertion.")
            Text("Where the data goes", style = MaterialTheme.typography.titleSmall)
            Text("The text and window information it reads are used only on this phone. They are not saved, collected by us, or sent to anyone.")
            Text("It skips password, number and private fields, and never presses Send. $app is not an accessibility tool.")
            Text("You can turn the service off at any time in Android Settings. If you decline, you can still record in $app and copy your text from History.")
        } },
        confirmButton = { TextButton(onClick = onAccept, modifier = Modifier.testTag("accessibility-accept")) { Text("Accept") } },
        dismissButton = { TextButton(onClick = onDecline, modifier = Modifier.testTag("accessibility-decline")) { Text("Decline") } })
}

/** One consent flow used by first launch, Home and Settings. Only Accept can open OS settings. */
@Composable internal fun AccessibilitySetupFlow(vm: WriterViewModel, onClose: () -> Unit) {
    val context = vm.context
    val consent = remember { com.metrix.writer.AccessibilityConsent(context) }
    var explain by rememberSaveable { mutableStateOf(false) }
    if (!explain) AccessibilityDisclosure(
        onAccept = {
            consent.record(true)
            if (com.metrix.writer.PermissionStatus.read(context).enabled) {
                vm.notify("Text insertion is on.")
                onClose()
            } else explain = true
        },
        onDecline = {
            consent.record(false)
            vm.notify("Text insertion stays off. You can still record and copy your text from History.")
            onClose()
        },
        onDismiss = onClose)
    else ProtectedAlertDialog(onDismissRequest = onClose,
        title = { Text("Enable ${stringResource(R.string.app_name)} · voice insertion") },
        text = { Text("In Android Settings:\n\n1. Open Installed apps (or Downloaded apps).\n2. Choose ${stringResource(R.string.editor_service_label)}.\n3. Turn on the service switch and confirm.\n\nLeave the optional accessibility shortcut off. You can return here and record even if you leave the service off.") },
        confirmButton = { TextButton(onClick = {
            onClose()
            context.startActivity(Intent(android.provider.Settings.ACTION_ACCESSIBILITY_SETTINGS).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
        }) { Text("Continue") } },
        dismissButton = { TextButton(onClick = onClose) { Text("Cancel") } })
}

private data class LicenseText(val id: String, val title: String, val asset: String)
private val licenseTexts = listOf(
    LicenseText("Apache-2.0", "Apache License 2.0", "licenses/Apache-2.0.txt"),
    LicenseText("LLVM-exception", "LLVM exceptions to the Apache License 2.0", "licenses/LLVM-exception.txt"),
    LicenseText("whisper.cpp", "whisper.cpp and ggml · MIT License", "whisper-LICENSE.txt"),
    LicenseText("libfvad", "libfvad / WebRTC VAD · BSD 3-Clause License", "licenses/libfvad-BSD-3-Clause.txt"),
    LicenseText("libfvad-patents", "WebRTC patent grant", "licenses/libfvad-PATENTS.txt"),
    LicenseText("Whisper-models", "OpenAI Whisper models · MIT License", "licenses/openai-whisper-MIT.txt"),
    // Only builds that bundle Google's sign-in libraries show this; see [googleTerms].
    LicenseText("Google-SDK-terms", "Google SDK terms", "licenses/google-sdk-terms.txt"),
)
/** Dependency license ids that are Google's SDK terms rather than an open-source license. */
private val googleTerms = mapOf("Android-SDK-License" to "Android SDK License", "Play-Core-Terms" to "Play Core SDK Terms", "Play-Integrity-Terms" to "Play Integrity API Terms")

/** Offline notices: every text is packaged in the APK's assets. */
@Composable internal fun LicensesScreen() {
    val context = LocalContext.current
    val dependencies = remember { context.assets.open("licenses/dependencies.tsv").bufferedReader().use { it.readLines() }.filter { it.isNotBlank() }.map { it.substringBefore('\t') to it.substringAfter('\t') } }
    val google = dependencies.any { it.second in googleTerms }
    val phone = remember { PhoneModels(context) }
    val runtime = phone.catalog.getJSONObject("runtime")
    val models = phone.models()
    var open by rememberSaveable { mutableStateOf<String?>(null) }
    Page {
        Text("Built with open source.", style = MaterialTheme.typography.headlineSmall)
        Text("${stringResource(R.string.app_name)} includes the components below. Their full license texts are part of the app, so this page works offline.", color = MaterialTheme.colorScheme.onSurfaceVariant)
        Section("Phone speech runtime", "Included in the app") {
            Text("whisper.cpp ${runtime.getString("tag")}, including ggml · ${runtime.getString("license")}", style = MaterialTheme.typography.bodyMedium)
            Text(runtime.getString("repository"), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            Text("LLVM libc++ from the Android NDK, linked into the runtime · Apache-2.0 with LLVM exceptions", style = MaterialTheme.typography.bodyMedium)
            Text("libfvad / WebRTC voice activity detection · BSD 3-Clause · github.com/dpirch/libfvad", style = MaterialTheme.typography.bodyMedium)
        }
        Section("Phone models", "Downloaded only when you choose one in Phone models") {
            models.forEach { model ->
                Text("${model.getString("id")} · ${model.getString("filename")} from huggingface.co/${model.getString("repository")} · ${model.optString("license", "See model source")}", style = MaterialTheme.typography.bodyMedium)
            }
            Text("OpenAI Whisper model weights, converted to ggml by the whisper.cpp project.", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
        Section("App libraries", if (google) "${dependencies.size} AndroidX, Jetpack Compose, Kotlin, Firebase, Google Play services and supporting libraries · Apache-2.0 unless marked with Google SDK terms"
            else "${dependencies.size} AndroidX, Jetpack Compose, Kotlin and supporting libraries · Apache-2.0") {
            SelectionContainer { Text(dependencies.joinToString("\n") { (coordinate, license) -> googleTerms[license]?.let { "$coordinate · $it" } ?: coordinate }, style = MaterialTheme.typography.bodySmall, modifier = Modifier.testTag("license-dependencies")) }
        }
        Section("License texts") {
            licenseTexts.filter { google || it.id != "Google-SDK-terms" }.forEach { license ->
                TextButton(onClick = { open = if (open == license.id) null else license.id }, modifier = Modifier.testTag("license-${license.id}")) {
                    Text((if (open == license.id) "Hide " else "Show ") + license.title)
                }
                if (open == license.id) {
                    val text = remember(license.asset) { context.assets.open(license.asset).bufferedReader().use { it.readText() } }
                    SelectionContainer { Text(text.trim(), style = MaterialTheme.typography.bodySmall.copy(fontFamily = FontFamily.Monospace), modifier = Modifier.testTag("license-text-${license.id}")) }
                }
            }
        }
    }
}
