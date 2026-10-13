package com.metrix.writer.ui

import android.content.Context
import android.text.format.DateUtils
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.selection.toggleable
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.unit.dp
import com.metrix.writer.PRODUCT_NAME
import com.metrix.writer.account.*
import com.metrix.writer.sync.AccountSync
import com.metrix.writer.sync.CloudFailure
import com.metrix.writer.sync.FirestoreRest
import com.metrix.writer.sync.MergePreview
import com.metrix.writer.sync.SyncStatus
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.io.File

/** The dev build's one account session, its controls and its sync engine. Created on first use; a guest never starts Firebase. */
internal object WriterAccounts {
    @Volatile private var session: AccountSession? = null
    @Volatile private var controls: AccountControls? = null
    @Volatile private var sync: AccountSync? = null
    @Volatile private var transcripts: com.metrix.writer.sync.TranscriptSync? = null
    fun get(context: Context): AccountSession = session ?: synchronized(this) {
        session ?: AccountSession(FirebaseGoogleIdentity(context), CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)).also { session = it }
    }
    fun controls(context: Context): AccountControls = controls ?: synchronized(this) {
        controls ?: AccountControls(get(context), AccountServiceHttp(), CleanupTracker(AccountWorkspace(context.applicationContext.filesDir))).also { controls = it }
    }
    /** Settings/dictionary sync (MW-OUATH-7). It sends nothing until a category is turned on for the signed-in account. */
    fun sync(context: Context): AccountSync = sync ?: synchronized(this) {
        sync ?: AccountSync(context, get(context), { project -> FirestoreRest(project) }).also { sync = it; it.start() }
    }
    fun transcripts(context: Context): com.metrix.writer.sync.TranscriptSync = transcripts ?: synchronized(this) {
        transcripts ?: com.metrix.writer.sync.TranscriptSync(context.applicationContext, get(context), { FirestoreRest(it) })
            .also { transcripts = it; it.start() }
    }
    /** Resumes sync after a restart, only when this phone has turned it on for some account before. */
    fun onVisible(context: Context) {
        if (File(context.filesDir, "accounts").isDirectory) transcripts(context)
        if (File(context.filesDir, "account-sync").listFiles().orEmpty().any { it.name.endsWith(".json") }) sync(context)
    }
}

/**
 * Optional Google sign-in, in the dev build only until MW-RELEASE-12 makes it store-compliant.
 * The Play build's [Account] shows nothing, so sign-in is unreachable there.
 */
internal object Account {
    /** Whether this build offers sign-in; the Settings list names its group after it. */
    const val available = true
    val searchEntries = listOf(
        SettingEntry("Account", "Google account", "Google account", "Optional. Dictation, History, your dictionary and settings work without an account. Sign in with Google. Continue without an account. Signing in uploads nothing. Signed in with Google. Check sign-in. Sign out. Sign in again. Cancel sign-in. Switch account. login log in logout log out google firebase profile"),
        SettingEntry("Account", "Backup & sync", "Backup & sync", "Sync settings and dictionary with your account. Set up backup & sync. Compare with your account. Restore once. Restore and keep syncing. Turn on sync. Pause on this phone. Sync now. Turn off. Keep this phone's. Use the account's. Changes waiting to upload. Last synced. backup restore new phone cloud firestore transfer"),
        SettingEntry("Account", "Account and cloud data", "Account and cloud data", "Sign out on all devices. Delete cloud data. Delete everywhere. Delete Writer account. Keep a copy on this phone. Remove from this phone. Your Google account is not deleted. revoke sessions privacy erase remove account"),
    )

    fun onVisible(context: Context) = WriterAccounts.onVisible(context)
    @Composable fun HistoryEntry(navigate: (String) -> Unit) {
        TextButton(onClick = { navigate("History/Synced text") }) { Text("Synced text") }
    }
    @Composable fun TranscriptPage() = TranscriptHistory()


    @Composable fun SettingsEntry(navigate: (String) -> Unit) {
        val context = LocalContext.current
        val snapshot by remember { WriterAccounts.get(context) }.snapshots.collectAsState()
        com.metrix.writer.ui.settings.NavRow("Account", when (snapshot.state) {
            AuthState.GUEST -> "Optional · not signed in"
            AuthState.SIGNING_IN -> "Signing in…"
            AuthState.SIGNED_IN -> snapshot.display?.email ?: "Signed in with Google"
            AuthState.REAUTH_REQUIRED -> "Sign in again to keep this account connected"
        }, { navigate("Settings/Account") }, tone = if (snapshot.state == AuthState.REAUTH_REQUIRED) com.metrix.writer.ui.settings.Tone.Attention else com.metrix.writer.ui.settings.Tone.Neutral)
    }

    private enum class Dialog { SIGN_OUT, EVERYWHERE, CLOUD, ACCOUNT }

    @Composable fun Page() {
        val context = LocalContext.current
        val session = remember { WriterAccounts.get(context) }
        val controls = remember { WriterAccounts.controls(context) }
        val snapshot by session.snapshots.collectAsState()
        var attemptId by rememberSaveable { mutableStateOf<String?>(null) }
        var notice by rememberSaveable { mutableStateOf<String?>(null) }
        var checking by remember { mutableStateOf(false) }
        var busy by remember { mutableStateOf(false) }
        var dialog by rememberSaveable { mutableStateOf<Dialog?>(null) }
        // A sensitive action waiting for the same-account sign-in it asked for.
        var pending by remember { mutableStateOf<Pair<String, AccountAction>?>(null) }
        var jobsVersion by remember { mutableIntStateOf(0) }
        val scope = rememberCoroutineScope()
        val account = snapshot.account
        val jobs = remember(account, jobsVersion) { account?.let { runCatching { controls.jobs(it) }.getOrDefault(emptyList()) }.orEmpty() }
        val job = jobs.lastOrNull { !it.completed } ?: jobs.lastOrNull()

        fun report(action: AccountAction?, result: ControlResult) {
            jobsVersion++
            when (result) {
                is ControlResult.NeedsReauth -> if (action == null) notice = "Sign in again with the same Google account, then try again." else {
                    val id = session.reauthenticateGoogle(context)
                    if (id == null) notice = message(AuthError.UNAVAILABLE) else { attemptId = id; pending = id to action; notice = "To confirm it's you, sign in again with the same Google account." }
                }
                is ControlResult.SignedOutEverywhere -> notice = "Signed out on every device. Other devices stop at their next sign-in check, within about an hour. Nothing was deleted." +
                    if (result.localCopyApplied) "" else " The copies on this phone couldn't all be kept or removed; they're still here."
                is ControlResult.Job -> notice = jobText(result.job) + if (result.localCopyApplied) "" else " The copies on this phone couldn't all be kept or removed; they're still here."
                is ControlResult.Dropped -> notice = "The account changed before this finished, so nothing was applied on this phone."
                is ControlResult.Failed -> notice = controlMessage(result.error, job?.accountScope == true && action == null)
            }
        }
        fun perform(action: AccountAction) {
            busy = true; notice = null
            scope.launch { val result = withContext(Dispatchers.IO) { controls.run(action) }; busy = false; report(action, result) }
        }
        fun follow(account: AccountKey, job: CleanupJob, retry: Boolean) {
            busy = true; notice = null
            scope.launch {
                val result = withContext(Dispatchers.IO) { if (retry) controls.retry(account, job.operationId) else controls.check(account, job.operationId) }
                busy = false; report(null, result)
            }
        }

        DisposableEffect(session) {
            val unsubscribe = session.subscribe { event ->
                if (event is AuthEvent.SignInFinished) scope.launch {
                    if (event.attemptId == attemptId) attemptId = null
                    val waiting = pending?.takeIf { it.first == event.attemptId }
                    if (waiting != null) {
                        pending = null
                        if (event.error == null) perform(waiting.second) else notice = message(event.error)
                    } else notice = if (event.error == null) "Signed in. Nothing has been uploaded." else message(event.error)
                }
            }
            onDispose { unsubscribe() }
        }
        // A deletion the service is still working on: look again a few times while this page is open.
        LaunchedEffect(job?.operationId, job?.state) {
            val current = job ?: return@LaunchedEffect
            if (account == null || current.state !in setOf("pending", "running")) return@LaunchedEffect
            repeat(10) {
                delay(3_000)
                val result = withContext(Dispatchers.IO) { controls.check(account, current.operationId) }
                if (result !is ControlResult.Job || result.job.state != current.state) { report(null, result); return@LaunchedEffect }
            }
        }
        Page {
            com.metrix.writer.ui.settings.SettingsSection("Google account", "Optional. Dictation, History, your dictionary and settings work without an account.") {
                when (snapshot.state) {
                    AuthState.GUEST -> {
                        Text("Signing in with Google connects an account to $PRODUCT_NAME on this phone. It uploads nothing: backup & sync is a separate choice that shows you what it will send first.")
                        Button(onClick = { notice = null; attemptId = session.signInGoogle(context) }, enabled = session.configured, modifier = Modifier.testTag("account-sign-in")) { Text("Sign in with Google") }
                        if (!session.configured) Text("Sign-in isn't set up in this build.", color = MaterialTheme.colorScheme.onSurfaceVariant, modifier = Modifier.testTag("account-not-configured"))
                        Text("Continue without an account: nothing changes, and $PRODUCT_NAME won't ask again.", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                    }
                    AuthState.SIGNING_IN -> {
                        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                            CircularProgressIndicator(Modifier.size(20.dp), strokeWidth = 2.dp)
                            Text("Waiting for Google sign-in…")
                        }
                        Text("You can keep dictating while this finishes.", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                        OutlinedButton(onClick = { attemptId?.let(session::cancelSignIn) }, enabled = attemptId != null, modifier = Modifier.testTag("account-cancel")) { Text("Cancel sign-in") }
                    }
                    AuthState.SIGNED_IN, AuthState.REAUTH_REQUIRED -> {
                        Text("Signed in with Google", style = MaterialTheme.typography.titleSmall)
                        snapshot.display?.let { display -> (display.email ?: display.name)?.let { Text(it, modifier = Modifier.testTag("account-email")) } }
                        if (snapshot.state == AuthState.REAUTH_REQUIRED) {
                            Text("Google needs you to sign in again to keep this account connected. Dictation is unaffected.", color = MaterialTheme.colorScheme.error)
                            Button(onClick = { notice = null; attemptId = session.reauthenticateGoogle(context) }, enabled = attemptId == null) { Text("Sign in again") }
                        } else {
                            OutlinedButton(onClick = {
                                checking = true; notice = null
                                scope.launch { val error = session.checkSession(); checking = false; notice = if (error == null) "Sign-in checked with Google." else message(error) }
                            }, enabled = !checking, modifier = Modifier.testTag("account-check")) { Text(if (checking) "Checking…" else "Check sign-in") }
                        }
                        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                            OutlinedButton(onClick = { notice = null; attemptId = session.switchAccount(context) }, enabled = !busy && attemptId == null, modifier = Modifier.testTag("account-switch")) { Text("Switch account") }
                            TextButton(onClick = { dialog = Dialog.SIGN_OUT }, enabled = !busy, modifier = Modifier.testTag("account-sign-out")) { Text("Sign out") }
                        }
                    }
                }
                notice?.let { Text(it, style = MaterialTheme.typography.bodyMedium, modifier = Modifier.testTag("account-notice")) }
            }
            if (snapshot.state == AuthState.SIGNED_IN || snapshot.state == AuthState.REAUTH_REQUIRED) {
                BackupAndSync(snapshot.account)
                TranscriptConsent()
            }
            if (account != null && snapshot.state != AuthState.SIGNING_IN) com.metrix.writer.ui.settings.SettingsSection("Account and cloud data", "Deleting and signing out everywhere ask you to confirm with Google first.") {
                if (!controls.configured) Text("Deleting cloud data and signing out on all devices aren't set up in this build.", color = MaterialTheme.colorScheme.onSurfaceVariant, modifier = Modifier.testTag("account-controls-not-configured"))
                job?.let { current ->
                    Text(jobText(current), modifier = Modifier.testTag("account-job"))
                    if (!current.completed) Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        if (current.receipt == null || current.state == "failed_retryable")
                            OutlinedButton(onClick = { follow(account, current, retry = true) }, enabled = !busy && controls.configured, modifier = Modifier.testTag("account-job-retry")) { Text("Retry now") }
                        else OutlinedButton(onClick = { follow(account, current, retry = false) }, enabled = !busy && controls.configured, modifier = Modifier.testTag("account-job-check")) { Text("Check again") }
                    }
                }
                if (busy) LinearProgressIndicator(Modifier.fillMaxWidth())
                val enabled = !busy && controls.configured && pending == null
                OutlinedButton(onClick = { dialog = Dialog.EVERYWHERE }, enabled = enabled, modifier = Modifier.testTag("account-sign-out-everywhere")) { Text("Sign out on all devices") }
                OutlinedButton(onClick = { dialog = Dialog.CLOUD }, enabled = enabled, modifier = Modifier.testTag("account-delete-cloud")) { Text("Delete cloud data") }
                TextButton(onClick = { dialog = Dialog.ACCOUNT }, enabled = enabled, modifier = Modifier.testTag("account-delete")) {
                    Text("Delete $PRODUCT_NAME account", color = MaterialTheme.colorScheme.error)
                }
            }
        }
        val held = remember(account, dialog) { account?.let { session.localCopy(it) } ?: LocalCopySummary.NONE }
        val waiting = remember(account, dialog) { if (dialog == Dialog.SIGN_OUT) account?.let { WriterAccounts.sync(context).pendingFor(it) } ?: 0 else 0 }
        when (dialog) {
            null -> Unit
            Dialog.SIGN_OUT -> LocalCopyDialog("Sign out?", if (held.empty) "Your recordings, transcripts, dictionary, settings and saved keys stay on this phone. Next time, Google asks which account to use." +
                    if (waiting > 0) " $waiting synced changes haven't reached your account yet. They're kept for this account and sent only after you sign in to it again." else ""
                else "Your own recordings, transcripts, dictionary, settings and saved keys stay on this phone. This account also has copies downloaded to this phone:", held, "Sign out",
                onDismiss = { dialog = null }) { choice ->
                dialog = null; attemptId = null
                scope.launch {
                    val ok = withContext(Dispatchers.IO) { session.signOut(choice) }
                    notice = if (ok) "Signed out. Everything on this phone is kept." + if (choice == LocalCopy.REMOVE) " The account's downloaded copies were removed." else ""
                        else "Signed out, but the account's copies on this phone couldn't all be kept or removed. They're still here."
                }
            }
            Dialog.EVERYWHERE -> LocalCopyDialog("Sign out on all devices?", "Signs this Google account out of $PRODUCT_NAME on every phone and computer, including this one. Other devices stop at their next sign-in check, within about an hour. Nothing is deleted.",
                held, "Sign out everywhere", onDismiss = { dialog = null }) { choice -> dialog = null; perform(AccountAction.SignOutEverywhere(choice)) }
            Dialog.CLOUD -> CloudDialog(onDismiss = { dialog = null }) { categories, everywhere -> dialog = null; perform(AccountAction.DeleteCloudData(categories, everywhere)) }
            Dialog.ACCOUNT -> LocalCopyDialog("Delete your $PRODUCT_NAME account?",
                "This deletes your $PRODUCT_NAME account and everything it stored in the cloud, then signs you out. Your Google account is not deleted. " +
                    "Recordings, transcripts, your dictionary and settings on this phone stay" + (if (held.empty) "." else " unless you remove the downloaded copies below.") +
                    " Files you exported earlier aren't affected. Other devices stop syncing when they reconnect.",
                held, "Delete account", destructive = true, onDismiss = { dialog = null }) { choice -> dialog = null; perform(AccountAction.DeleteAccount(choice)) }
        }
    }

    /** A confirmation that offers the local-copy choice only when participants hold copies for this account. */
    @Composable private fun LocalCopyDialog(title: String, text: String, held: LocalCopySummary, confirm: String, destructive: Boolean = false, onDismiss: () -> Unit, onConfirm: (LocalCopy) -> Unit) {
        var choice by rememberSaveable { mutableStateOf(LocalCopy.LEAVE) }
        ProtectedAlertDialog(onDismissRequest = onDismiss, title = { Text(title) }, text = {
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(text)
                if (!held.empty) listOf(
                    LocalCopy.LEAVE to "Leave them for when I sign in again",
                    LocalCopy.KEEP to "Keep a copy on this phone without the account",
                    LocalCopy.REMOVE to "Remove them from this phone",
                ).forEach { (option, label) ->
                    Row(Modifier.fillMaxWidth().selectable(choice == option, role = Role.RadioButton) { choice = option }.testTag("local-copy-${option.wire}"), verticalAlignment = Alignment.CenterVertically) {
                        RadioButton(choice == option, onClick = null); Spacer(Modifier.width(8.dp)); Text(label)
                    }
                }
            }
        }, confirmButton = {
            TextButton(onClick = { onConfirm(choice) }, modifier = Modifier.testTag("account-dialog-confirm")) {
                Text(confirm, color = if (destructive) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.primary)
            }
        }, dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } })
    }

    @Composable private fun CloudDialog(onDismiss: () -> Unit, onConfirm: (List<String>, Boolean) -> Unit) {
        var selected by rememberSaveable { mutableStateOf(listOf<String>()) }
        var everywhere by rememberSaveable { mutableStateOf(false) }
        ProtectedAlertDialog(onDismissRequest = onDismiss, title = { Text("Delete cloud data?") }, text = {
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text("Deletes what this account stored in the cloud and turns off its sync. Recordings and text that exist only on a device are never deleted.")
                listOf("settings" to "Settings", "dictionary" to "Dictionary", "transcripts" to "Transcript history").forEach { (category, label) ->
                    Row(Modifier.fillMaxWidth().toggleable(category in selected, role = Role.Checkbox) { on -> selected = if (on) selected + category else selected - category }
                        .testTag("cloud-$category"), verticalAlignment = Alignment.CenterVertically) {
                        Checkbox(category in selected, onCheckedChange = null); Spacer(Modifier.width(8.dp)); Text(label)
                    }
                }
                listOf(false to "Delete from the cloud only: this phone and your other devices keep their copies, and nothing is uploaded again unless you turn sync back on.",
                    true to "Delete everywhere: your other signed-in devices also remove their synced copies when they reconnect. Files you exported aren't affected.").forEach { (option, label) ->
                    Row(Modifier.fillMaxWidth().selectable(everywhere == option, role = Role.RadioButton) { everywhere = option }.testTag("cloud-everywhere-$option"), verticalAlignment = Alignment.CenterVertically) {
                        RadioButton(everywhere == option, onClick = null); Spacer(Modifier.width(8.dp)); Text(label)
                    }
                }
            }
        }, confirmButton = {
            TextButton(onClick = { onConfirm(selected, everywhere) }, enabled = selected.isNotEmpty(), modifier = Modifier.testTag("account-dialog-confirm")) {
                Text("Delete", color = MaterialTheme.colorScheme.error)
            }
        }, dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } })
    }

    private fun jobText(job: CleanupJob): String {
        val what = if (job.accountScope) "Deleting your $PRODUCT_NAME account" else "Deleting cloud data"
        return when (job.state) {
            null -> "A deletion request may not have reached the service yet. Retry to send the same request again."
            "pending", "running" -> "$what… Uploads are already stopped."
            "failed_retryable" -> "$what isn't finished yet. The service keeps retrying; you can also retry now."
            else -> if (job.accountScope) "Your $PRODUCT_NAME account and its cloud data are deleted. Your Google account is not affected."
                else "Cloud data deleted." + if (job.everywhere) " Your other devices remove their synced copies when they reconnect." else " The copies on your devices are kept."
        }
    }

    private fun controlMessage(error: ControlError, accountDeletion: Boolean) = when (error) {
        ControlError.NOT_CONFIGURED -> "This isn't set up in this build."
        ControlError.OFFLINE -> "You're offline. Nothing changed; try again when you're connected."
        ControlError.UNAUTHENTICATED -> if (accountDeletion) "The deletion was requested, but its result couldn't be confirmed on this phone." else "Sign in again with the same Google account, then try again."
        ControlError.REAUTH_REQUIRED -> "Sign in again with the same Google account, then try again."
        ControlError.STALE_EPOCH, ControlError.STALE_CONSENT -> "Your account changed on another device. Nothing was deleted; review and try again."
        ControlError.ACCOUNT_UNAVAILABLE -> "This account is already being deleted."
        ControlError.RATE_LIMITED -> "Too many requests. Wait a minute and try again."
        ControlError.NOT_FOUND -> "That request is no longer known to the service."
        ControlError.NOT_OWNER, ControlError.INVALID_REQUEST, ControlError.IDEMPOTENCY_CONFLICT, ControlError.INVALID_RESPONSE -> "The service refused this request. Nothing was deleted from this phone."
        ControlError.UNAVAILABLE -> "The service is unavailable right now. Try again later."
    }

    /** Settings → Account → Backup & sync. Nothing is uploaded or restored until the user confirms a preview. */
    @Composable private fun BackupAndSync(account: AccountKey?) {
        val context = LocalContext.current
        val sync = remember { WriterAccounts.sync(context) }
        val status by sync.status.collectAsState()
        val scope = rememberCoroutineScope()
        var settings by rememberSaveable { mutableStateOf(true) }
        var dictionary by rememberSaveable { mutableStateOf(true) }
        var preview by remember { mutableStateOf<MergePreview?>(null) }
        var choices by remember { mutableStateOf(mapOf<String, String>()) }
        var busy by remember { mutableStateOf(false) }
        var result by remember { mutableStateOf<String?>(null) }
        LaunchedEffect(account) { sync.kick() }
        fun act(block: suspend () -> String?) {
            if (busy) return
            busy = true; result = null
            scope.launch {
                result = try { block() } catch (error: Exception) {
                    when (error) {
                        is CloudFailure -> problem(when (error.kind) {
                            CloudFailure.Kind.OFFLINE -> "offline"; CloudFailure.Kind.NOT_SET_UP -> "not_set_up"; CloudFailure.Kind.QUOTA -> "quota"
                            CloudFailure.Kind.DENIED -> "denied"; CloudFailure.Kind.UNAUTHENTICATED -> "reauth"; else -> "unavailable"
                        })
                        is AuthFailure -> message(error.kind)
                        is StaleAccountException -> "The signed-in account changed. Nothing was sent."
                        is IllegalArgumentException, is IllegalStateException -> error.message ?: "That didn't work. Nothing was changed."
                        else -> "That didn't work. Nothing was changed."
                    }
                }
                busy = false
            }
        }
        com.metrix.writer.ui.settings.SettingsSection("Backup & sync", "Keep your settings and dictionary the same on your phone and computers. Recordings, transcripts and API keys are never synced.") {
            val enabled = status.enabled
            if (enabled.isEmpty() && preview == null) {
                Text(if (status.lastAckMs > 0) "Sync is off on this phone. Anything uploaded earlier stays in your account until it's deleted." else "Backup & sync is off. Nothing on this phone has been uploaded.")
                Row(verticalAlignment = Alignment.CenterVertically) { Checkbox(settings, { settings = it }, enabled = !busy); Text("Settings (recommended)") }
                Row(verticalAlignment = Alignment.CenterVertically) { Checkbox(dictionary, { dictionary = it }, enabled = !busy); Text("Dictionary (recommended)") }
                Text("History sync isn't available yet.", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                Button(onClick = { act {
                    preview = sync.preview(setOfNotNull("settings".takeIf { settings }, "dictionary".takeIf { dictionary })); choices = emptyMap(); null
                } }, enabled = !busy && (settings || dictionary), modifier = Modifier.testTag("sync-compare")) { Text("Compare with your account") }
                Text("This only reads your account. Nothing is uploaded until you confirm.", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
            preview?.let { current -> MergeReview(current, choices, { choices = choices + it }, busy,
                restore = { act { val receipt = sync.restoreOnce(current, choices); preview = null
                    if (receipt == null) "Nothing to restore." else "Restored from your account. Sync stays off, and nothing was uploaded. You can undo this in Export & import." } },
                enable = { act { sync.enable(current, choices); preview = null; "Sync is on. This phone's changes upload in the background." } },
                cancel = { preview = null }) }
            if (enabled.isNotEmpty()) SyncState(status, busy, sync, ::act)
            result?.let { Text(it, modifier = Modifier.testTag("sync-result")) }
            if (busy) LinearProgressIndicator(Modifier.fillMaxWidth())
        }
    }

    @Composable private fun MergeReview(preview: MergePreview, choices: Map<String, String>, choose: (Pair<String, String>) -> Unit, busy: Boolean,
                                        restore: () -> Unit, enable: () -> Unit, cancel: () -> Unit) {
        Text("This phone and your account", style = MaterialTheme.typography.titleSmall)
        for (category in listOf("settings", "dictionary").filter { it in preview.categories }) {
            val name = if (category == "settings") "Settings" else "Dictionary"
            Text("$name: ${preview.count(category, "same")} the same · ${preview.count(category, "download", "preserve")} only in your account · " +
                "${preview.count(category, "upload")} only on this phone · ${preview.count(category, "conflict", "deleted")} to choose", style = MaterialTheme.typography.bodyMedium)
        }
        if (!preview.cloudHasAccount) Text("Your account has no synced settings yet.", style = MaterialTheme.typography.bodySmall)
        if (preview.choices.count { it.action == "conflict" } > 1) Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            TextButton(onClick = { preview.choices.filter { it.action == "conflict" }.forEach { choose(it.id to "account") } }, enabled = !busy) { Text("Use the account's for all") }
            TextButton(onClick = { preview.choices.filter { it.action == "conflict" }.forEach { choose(it.id to "phone") } }, enabled = !busy) { Text("Keep this phone's for all") }
        }
        preview.items.filter { it.action != "same" }.forEach { item ->
            when (item.action) {
                "conflict" -> PreviewRow(item.title, item.detail, "Different on each. Choose which to keep.") { Column {
                    FilterChip(choices[item.id] == "phone", { choose(item.id to "phone") }, label = { Text("This phone") })
                    FilterChip(choices[item.id] == "account", { choose(item.id to "account") }, label = { Text("Account") })
                } }
                "deleted" -> PreviewRow(item.title, item.detail, null) { Column {
                    FilterChip(choices[item.id] == "keep", { choose(item.id to "keep") }, label = { Text("Keep") })
                    FilterChip(choices[item.id] == "remove", { choose(item.id to "remove") }, label = { Text("Remove") })
                } }
                "download" -> PreviewRow(item.title, item.detail, "From your account")
                "preserve" -> PreviewRow(item.title, item.detail, "Kept, not applied: ${item.reason}")
                "upload" -> PreviewRow(item.title, item.detail, "Uploads only if you turn on sync")
            }
        }
        val ready = preview.choices.all { it.id in choices }
        Text("Restoring never downloads a model, adds a key, or starts a recording or cloud request. You can undo it in Export & import.", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        if (preview.restorable) OutlinedButton(onClick = restore, enabled = !busy && ready, modifier = Modifier.testTag("sync-restore-once")) { Text("Restore once") }
        Button(onClick = enable, enabled = !busy && ready, modifier = Modifier.testTag("sync-enable")) { Text(if (preview.restorable) "Restore and keep syncing" else "Turn on sync") }
        TextButton(onClick = cancel, enabled = !busy) { Text("Cancel") }
    }

    @Composable private fun SyncState(status: SyncStatus, busy: Boolean, sync: AccountSync, act: (suspend () -> String?) -> Unit) {
        val names = listOf("settings", "dictionary").filter { it in status.enabled }.joinToString(" and ") { if (it == "settings") "Settings" else "Dictionary" }
        Text("Syncing: $names", style = MaterialTheme.typography.titleSmall, modifier = Modifier.testTag("sync-on"))
        Text(when {
            status.paused -> "Paused on this phone. Changes are saved here and wait."
            status.running -> "Syncing…"
            status.pending > 0 -> "${status.pending} change${if (status.pending == 1) "" else "s"} waiting to upload"
            status.lastAckMs > 0 -> "Up to date · last upload ${DateUtils.getRelativeTimeSpanString(status.lastAckMs)}"
            else -> "Up to date"
        }, modifier = Modifier.testTag("sync-state"))
        status.problem?.let { Text(problem(it), color = MaterialTheme.colorScheme.error, modifier = Modifier.testTag("sync-problem")) }
        if (status.failed > 0) Text("${status.failed} change${if (status.failed == 1) "" else "s"} couldn't sync and stay only on this phone. Editing them again retries.", style = MaterialTheme.typography.bodySmall)
        if (status.detached > 0) Text("${status.detached} earlier change${if (status.detached == 1) " was" else "s were"} not sent because sync was turned off or reset.", style = MaterialTheme.typography.bodySmall)
        if (status.quarantined > 0) Text("${status.quarantined} item${if (status.quarantined == 1) "" else "s"} in your account came from a newer version and weren't applied. Update $PRODUCT_NAME to use them.", style = MaterialTheme.typography.bodySmall)
        status.conflicts.forEach { conflict ->
            PreviewRow(conflict.title, "This phone: ${conflict.phone} · Account: ${conflict.cloud}",
                if (conflict.kind == "deleted") "Deleted from your account on another device." else "Changed on both.") { Column {
                FilterChip(false, { act { sync.resolve(conflict.recordId, "phone"); null } }, label = { Text(if (conflict.kind == "deleted") "Keep" else "This phone") }, enabled = !busy)
                FilterChip(false, { act { sync.resolve(conflict.recordId, "account"); null } }, label = { Text(if (conflict.kind == "deleted") "Remove" else "Account") }, enabled = !busy)
            } }
        }
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            OutlinedButton(onClick = { sync.syncNow() }, enabled = !busy && !status.paused, modifier = Modifier.testTag("sync-now")) { Text("Sync now") }
            TextButton(onClick = { act { sync.setPaused(!status.paused); null } }, enabled = !busy, modifier = Modifier.testTag("sync-pause")) { Text(if (status.paused) "Resume" else "Pause on this phone") }
        }
        status.enabled.sorted().forEach { category ->
            TextButton(onClick = { act { sync.disable(category); "Sync is off for ${if (category == "settings") "settings" else "the dictionary"}. What's in your account stays until it's deleted." } }, enabled = !busy) {
                Text("Turn off ${if (category == "settings") "settings" else "dictionary"} sync")
            }
        }
    }

    private fun problem(kind: String) = when (kind) {
        "offline" -> "You're offline. Changes are saved on this phone and sync when you're back online."
        "quota" -> "Your account reached today's storage limit. Sync resumes later; nothing is lost."
        "not_set_up" -> "Cloud storage isn't set up for this build yet. Nothing was uploaded."
        "denied" -> "Your account refused this change. Try Sync now; if it keeps happening, turn sync off and on again."
        "reauth" -> "Sign in again to keep syncing. Changes are kept on this phone."
        "turned_off" -> "Sync was turned off for your account on another device or by a cloud deletion. Changes that were waiting weren't sent."
        "account_unavailable" -> "This account is unavailable or being deleted. Sync stopped; everything on this phone is kept."
        else -> "Your account didn't answer. $PRODUCT_NAME keeps retrying; changes are kept on this phone."
    }

    private fun message(error: AuthError) = when (error) {
        AuthError.CANCELLED -> "Sign-in cancelled. You can keep using $PRODUCT_NAME without an account."
        AuthError.TIMED_OUT -> "Google didn't answer in time. Try again."
        AuthError.OFFLINE -> "You're offline. Connect to the internet and try again."
        AuthError.NOT_CONFIGURED -> "Sign-in isn't set up in this build."
        AuthError.PROVIDER_REJECTED -> "Google didn't complete this sign-in. Check that a Google account is on this phone and try again."
        AuthError.INVALID_RESPONSE -> "Google returned an unexpected answer. Try again."
        AuthError.REAUTH_REQUIRED -> "Sign in again to keep this account connected."
        AuthError.UNAVAILABLE -> "Sign-in is unavailable right now. Try again later."
    }
}
