package com.metrix.writer.ui.settings

import android.content.ClipboardManager
import android.content.Context
import android.content.Intent
import android.net.Uri
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.unit.dp
import com.metrix.writer.*
import com.metrix.writer.ui.Page
import com.metrix.writer.ui.ProtectedAlertDialog
import com.metrix.writer.ui.WriterState
import com.metrix.writer.ui.WriterViewModel
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

/**
 * API keys: every provider's key in one place. A key is typed on a full page,
 * never in a sheet or dialog: those are separate windows that FLAG_SECURE and
 * the Activity's touch guard do not cover.
 */
internal object Keys {
    fun state(context: Context, provider: CloudProvider) = DirectCloud(context, provider.id).keyState()

    fun label(state: DirectCloud.KeyState) = when (state) {
        DirectCloud.KeyState.Absent -> "No key"
        DirectCloud.KeyState.Unchecked -> "Not checked"
        DirectCloud.KeyState.Checked -> "Checked"
        DirectCloud.KeyState.Failed -> "Check failed"
    }

    fun tone(state: DirectCloud.KeyState) = when (state) {
        DirectCloud.KeyState.Absent -> Tone.Neutral
        DirectCloud.KeyState.Unchecked -> Tone.Attention
        DirectCloud.KeyState.Checked -> Tone.Ok
        DirectCloud.KeyState.Failed -> Tone.Error
    }

    /** One line for a provider's key: its state, and what uses it. */
    fun summary(context: Context, provider: CloudProvider): String {
        val state = when (val key = state(context, provider)) {
            DirectCloud.KeyState.Absent -> "No key"
            else -> "Key saved · ${label(key).lowercase()}"
        }
        val uses = CloudProviders.uses(context, provider.id)
        return if (uses.isEmpty()) state else "$state · used for ${uses.joinToString(" and ") { it.lowercase() }}"
    }

    /** Needs attention: a key in use is missing, or a saved key is unchecked or failing. */
    fun summaryTone(context: Context, provider: CloudProvider): Tone {
        val state = state(context, provider)
        return if (state == DirectCloud.KeyState.Absent && CloudProviders.uses(context, provider.id).isNotEmpty()) Tone.Attention else tone(state).takeIf { it != Tone.Ok } ?: Tone.Neutral
    }

    /** The Settings list row. */
    fun overview(context: Context): Pair<String, Tone> {
        val missing = CloudProviders.all.firstOrNull { state(context, it) == DirectCloud.KeyState.Absent && CloudProviders.uses(context, it.id).isNotEmpty() }
        if (missing != null) return "${missing.name} key needed" to Tone.Attention
        val saved = CloudProviders.all.filter { state(context, it) != DirectCloud.KeyState.Absent }
        if (saved.isEmpty()) return "Add a key for cloud speech or polishing" to Tone.Neutral
        val count = if (saved.size == 1) "1 key saved" else "${saved.size} keys saved"
        val failed = saved.firstOrNull { state(context, it) == DirectCloud.KeyState.Failed }
        val unchecked = saved.firstOrNull { state(context, it) == DirectCloud.KeyState.Unchecked }
        return when {
            failed != null -> "$count · ${failed.name} check failed" to Tone.Error
            unchecked != null -> "$count · ${unchecked.name} not checked" to Tone.Attention
            else -> "$count · all checked" to Tone.Neutral
        }
    }
}

/**
 * What to finish once a key is added: a speech engine or polishing model that
 * was tapped while its provider had no key. In memory only; a recreated
 * activity simply forgets it.
 */
internal object PendingChoice {
    var engine: String? = null
    var polishing: String? = null // provider/model

    fun forget(provider: String) {
        if (engine == provider) engine = null
        if (polishing?.substringBefore('/') == provider) polishing = null
    }

    fun applyAfterKey(context: Context, provider: String): String? {
        var done: String? = null
        if (engine == provider) { DictationChoice(context).useCloud(provider); done = "New dictations use ${DictationChoice(context).engineLabel()}." }
        polishing?.takeIf { it.substringBefore('/') == provider }?.let { choice ->
            val prefs = CompanionPreferences(context)
            if (CloudProviders.cleanupAllowed(context, provider, choice.substringAfter('/'))) {
                prefs.save(choice, "cleaned", prefs.cleanupReasoning())
                done = "Polishing uses ${CloudProviders.modelName(provider, choice.substringAfter('/'))}."
            }
        }
        engine = null; polishing = null
        return done
    }
}

@Composable internal fun KeysPage(vm: WriterViewModel, state: WriterState, navigate: (String) -> Unit) {
    state.revision // Recompose after saves and checks; key state lives in preferences.
    Page {
        PageIntro("Your own keys for cloud speech and polishing. Each is encrypted on this phone, excluded from backups and never shown again.")
        SettingsGroup("Providers") {
            CloudProviders.all.forEach { provider ->
                val key = Keys.state(vm.context, provider)
                val uses = CloudProviders.uses(vm.context, provider.id)
                NavRow(provider.name,
                    (if (provider.transcribes) "Speech and polishing" else "Polishing only") + (if (uses.isEmpty()) "" else " · in use for ${uses.joinToString(" and ") { it.lowercase() }}"),
                    { navigate("Settings/${SettingsRoutes.keyPage(provider)}") },
                    tone = if (key == DirectCloud.KeyState.Absent && uses.isNotEmpty()) Tone.Attention else Tone.Neutral,
                    tag = "key-${provider.id}",
                    trailing = { StatusChip(Keys.label(key), Keys.tone(key)) })
            }
        }
    }
}

@Composable internal fun ProviderKeyPage(vm: WriterViewModel, state: WriterState, provider: CloudProvider) {
    com.metrix.writer.ui.SecureScreen()
    state.revision
    val context = vm.context
    val cloud = remember(provider.id) { DirectCloud(context, provider.id) }
    var key by remember(provider.id) { mutableStateOf("") } // Never saved or populated with a stored secret.
    var forget by remember { mutableStateOf(false) }
    val importKey = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
        if (uri != null) vm.action {
            val candidate = context.contentResolver.openInputStream(uri)!!.use { it.readNBytes(513).toString(Charsets.UTF_8).trim() }
            require(candidate.length in 20..512) { "Choose a text file containing only your API key." }
            withContext(Dispatchers.Main) { key = candidate }
        }
    }
    // A tapped engine or polishing model waits only while this page is open; leaving without a key forgets it.
    DisposableEffect(provider.id) { onDispose { PendingChoice.forget(provider.id) } }
    val uses = CloudProviders.uses(context, provider.id)
    val keyState = cloud.keyState()
    val configured = keyState != DirectCloud.KeyState.Absent
    val checkModel = CloudProviders.checkModel(context, provider.id)
    Page {
        PageIntro(when {
            uses.isNotEmpty() -> "${uses.joinToString(" and ")} use${if (uses.size == 1) "s" else ""} this key."
            provider.transcribes -> "Not in use yet. Add a key, then choose ${provider.name} for speech or polishing."
            else -> "Not in use yet. ${provider.name} polishes text only; add a key, then choose it under Polishing."
        })
        SettingsGroup("Status") {
            InfoRow(if (configured) "Key saved" else "No key saved",
                if (configured) cloud.accessStatus() else "Paste your own ${provider.name} key below.", Keys.tone(keyState))
        }
        SettingsGroup(if (configured) "Replace key" else "Add your key", footer = "Encrypted on this phone, excluded from backups and never shown again.") {
            GroupBlock {
                OutlinedTextField(key, { key = it }, label = { Text("Paste API key") },
                    singleLine = true, visualTransformation = PasswordVisualTransformation(),
                    keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password),
                    modifier = Modifier.fillMaxWidth().testTag("key-input"), enabled = !state.actionBusy)
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    OutlinedButton(onClick = {
                        val clip = (context.getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager).primaryClip
                        val text = clip?.takeIf { it.itemCount > 0 }?.getItemAt(0)?.coerceToText(context)?.toString()?.trim().orEmpty()
                        if (text.isEmpty()) vm.notify("The clipboard has no text to paste.") else key = text
                    }, enabled = !state.actionBusy, modifier = Modifier.testTag("key-paste")) { Text("Paste") }
                    if (provider.keyUrl.isNotEmpty()) TextButton(onClick = {
                        context.startActivity(Intent(Intent.ACTION_VIEW, Uri.parse(provider.keyUrl)).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
                    }) { Text("Get a key") }
                }
                Button(onClick = {
                    val candidate = key; key = ""
                    vm.action {
                        cloud.saveKey(candidate)
                        val applied = PendingChoice.applyAfterKey(context, provider.id)
                        val model = CloudProviders.checkModel(context, provider.id)
                        try {
                            cloud.checkAccess(model)
                            vm.notify("${provider.name} key saved and working with ${CloudProviders.modelName(provider.id, model)}." + (applied?.let { " $it" } ?: ""))
                        } catch (error: Exception) {
                            vm.notify("${provider.name} key saved, but the check failed: ${GeminiClient.explain(error)}", true)
                        }
                    }
                }, enabled = key.isNotBlank() && !state.actionBusy, modifier = Modifier.fillMaxWidth().testTag("key-save")) { Text("Save and check") }
                TextButton(onClick = { importKey.launch(arrayOf("text/plain", "application/octet-stream")) }, enabled = !state.actionBusy) { Text("Import key from file…") }
            }
        }
        if (configured) SettingsGroup("Saved key") {
            ActionRow("Check key", {
                vm.action { cloud.checkAccess(checkModel); vm.notify("${provider.name} key works with ${CloudProviders.modelName(provider.id, checkModel)}.") }
            }, value = "Checks access to ${CloudProviders.modelName(provider.id, checkModel)}. A dictation is the final test.",
                enabled = !state.actionBusy, tag = "key-check")
            ActionRow("Remove key…", { forget = true }, danger = true, enabled = !state.actionBusy, tag = "key-remove")
        }
    }
    if (forget) ProtectedAlertDialog(onDismissRequest = { forget = false }, title = { Text("Remove ${provider.name} key?") },
        text = { Text(buildString {
            if (uses.isNotEmpty()) append("${uses.joinToString(" and ")} will stop working until you add a key again. ")
            append("Earlier key revisions are removed too, so pending requests using them need a new attempt. Audio and transcripts stay saved.")
        }) },
        confirmButton = { TextButton(onClick = { forget = false; vm.action { cloud.forget(); vm.notify("${provider.name} key removed.") } },
            colors = ButtonDefaults.textButtonColors(contentColor = MaterialTheme.colorScheme.error)) { Text("Remove key") } },
        dismissButton = { TextButton(onClick = { forget = false }) { Text("Cancel") } })
}
