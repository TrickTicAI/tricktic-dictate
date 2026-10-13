package com.metrix.writer.ui

import android.content.Context
import android.content.Intent
import android.net.Uri
import android.provider.Settings
import androidx.compose.foundation.layout.*
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.LifecycleResumeEffect
import com.metrix.writer.*

/** The maintainer's private build: signed APK self-updates from the private feed and sideload guidance. */
internal object Distribution {
    /** Settings search entries for what only this build shows. */
    val searchEntries = listOf(
        SettingEntry("Permissions", "Android won't let me enable insertion", "Text insertion", "Allow a private installation. Restricted setting. Allow restricted settings. Open App info. Samsung permission"),
        SettingEntry("Storage & about", "$PRODUCT_NAME updates", "$PRODUCT_NAME updates", "Installed version. Last checked. Updates check automatically on launch. $PRODUCT_NAME is up to date. Check for updates. Download update. Check again. Release notes. Download size. Cancel download. Install update. $PRODUCT_NAME needs permission to install updates on this phone. Allow update installation. Finish or stop recording before updating. Retry download. Private installation. Updates arrive as signed $PRODUCT_NAME APKs directly from $PRODUCT_NAME without Google Play. apk upgrade"),
    )

    fun onVisible(context: Context) = AndroidUpdater.getInstance(context).checkOnLaunch()

    @Composable fun InstallationHelp(open: (Intent) -> Unit) {
        val context = LocalContext.current
        var restricted by rememberSaveable { mutableStateOf(false) }
        TextButton(onClick = { restricted = true }) { Text("Android won't let me enable insertion") }
        if (restricted) ProtectedAlertDialog(onDismissRequest = { restricted = false }, title = { Text("Allow a private installation") },
            text = { Text("If Samsung says Restricted setting, open $PRODUCT_NAME App info → ⋮ → Allow restricted settings. Return here and enable $PRODUCT_NAME · voice insertion. Only do this for the $PRODUCT_NAME app you installed and trust.") },
            confirmButton = { TextButton(onClick = { restricted = false; open(Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS, Uri.parse("package:${context.packageName}"))) }) { Text("Open App info") } },
            dismissButton = { TextButton(onClick = { restricted = false }) { Text("Cancel") } })
    }

    @Composable fun InstallationNote() {
        Text("Private installation", style = MaterialTheme.typography.titleSmall)
        Text("Updates arrive as signed $PRODUCT_NAME APKs directly from $PRODUCT_NAME without Google Play.", style = MaterialTheme.typography.bodyMedium)
    }

    @Composable fun UpdatesSection(vm: WriterViewModel, state: WriterState) {
        val context = LocalContext.current
        val updater = remember { AndroidUpdater.getInstance(vm.context) }
        val updateState by updater.state.collectAsState()
        com.metrix.writer.ui.settings.SettingsSection("$PRODUCT_NAME updates") {
            Text("Installed version ${BuildConfig.VERSION_NAME} (${BuildConfig.VERSION_CODE})", style = MaterialTheme.typography.titleMedium)

            val lastChecked = updateState.lastCheckedMs
            val locale = androidx.compose.ui.platform.LocalConfiguration.current.locales[0]
            val lastCheckedText = if (lastChecked != null && lastChecked > 0) {
                val formatted = java.text.SimpleDateFormat("MMM d, HH:mm", locale).format(java.util.Date(lastChecked))
                "Last checked: $formatted"
            } else {
                "Updates check automatically on launch."
            }
            Text(lastCheckedText, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)

            when (val current = updateState) {
                is UpdateState.Idle -> {
                    if (current.message != null) {
                        Text(current.message, style = MaterialTheme.typography.bodyMedium)
                    } else {
                        Text("$PRODUCT_NAME is up to date.", style = MaterialTheme.typography.bodyMedium)
                    }
                    Button(onClick = { updater.checkForUpdate(isAutomatic = false) }) {
                        Text("Check for updates")
                    }
                }
                is UpdateState.Checking -> {
                    Row(
                        modifier = Modifier.fillMaxWidth().padding(vertical = 8.dp),
                        verticalAlignment = Alignment.CenterVertically,
                        horizontalArrangement = Arrangement.spacedBy(12.dp)
                    ) {
                        CircularProgressIndicator(modifier = Modifier.size(20.dp), strokeWidth = 2.dp)
                        Text("Checking for updates…", style = MaterialTheme.typography.bodyMedium)
                    }
                }
                is UpdateState.Available -> {
                    Text("Version ${current.info.version} (${current.info.versionCode}) is available", style = MaterialTheme.typography.titleSmall, color = MaterialTheme.colorScheme.primary)
                    Text("Download size: ${(current.info.bytes + 1048575) / 1048576} MB", style = MaterialTheme.typography.bodySmall)
                    if (current.info.notes.isNotBlank()) {
                        Surface(
                            shape = MaterialTheme.shapes.small,
                            color = MaterialTheme.colorScheme.surfaceContainerLow,
                            modifier = Modifier.fillMaxWidth().padding(vertical = 4.dp)
                        ) {
                            Column(Modifier.padding(8.dp)) {
                                Text("Release notes", style = MaterialTheme.typography.labelSmall)
                                Text(current.info.notes, style = MaterialTheme.typography.bodySmall)
                            }
                        }
                    }
                    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        Button(onClick = { updater.downloadUpdate(current.info) }) {
                            Text("Download update")
                        }
                        OutlinedButton(onClick = { updater.checkForUpdate(isAutomatic = false) }) {
                            Text("Check again")
                        }
                    }
                }
                is UpdateState.Downloading -> {
                    Text("Downloading update: ${current.percent}% (${current.downloadedBytes / 1048576} / ${(current.totalBytes + 1048575) / 1048576} MB)")
                    LinearProgressIndicator(
                        progress = { current.percent / 100f },
                        modifier = Modifier.fillMaxWidth().height(8.dp)
                    )
                    OutlinedButton(onClick = { updater.cancelDownload() }) {
                        Text("Cancel download")
                    }
                }
                is UpdateState.Downloaded -> {
                    Text("Update ${current.info.version} (${current.info.versionCode}) ready to install", style = MaterialTheme.typography.titleSmall, color = MaterialTheme.colorScheme.primary)
                    if (current.info.notes.isNotBlank()) {
                        Text(current.info.notes, style = MaterialTheme.typography.bodySmall)
                    }

                    var canInstall by remember(current.info.versionCode) { mutableStateOf(updater.canRequestPackageInstalls()) }
                    LifecycleResumeEffect(current.info.versionCode) {
                        canInstall = updater.canRequestPackageInstalls()
                        onPauseOrDispose {}
                    }
                    val isBusy = CaptureService.isActive() || state.processing

                    if (!canInstall) {
                        Text("$PRODUCT_NAME needs permission to install updates on this phone.", color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodySmall)
                        Button(onClick = {
                            context.startActivity(updater.createRequestInstallPermissionIntent())
                        }) {
                            Text("Allow update installation")
                        }
                    } else if (isBusy) {
                        Text("Finish or stop recording before updating.", color = MaterialTheme.colorScheme.onSurfaceVariant, style = MaterialTheme.typography.bodySmall)
                        Button(onClick = {}, enabled = false) {
                            Text("Install update")
                        }
                    } else {
                        Button(onClick = {
                            updater.installUpdate(current.info, current.apkFile)
                        }) {
                            Text("Install update")
                        }
                    }
                }
                is UpdateState.Installing -> {
                    Row(
                        modifier = Modifier.fillMaxWidth().padding(vertical = 8.dp),
                        verticalAlignment = Alignment.CenterVertically,
                        horizontalArrangement = Arrangement.spacedBy(12.dp)
                    ) {
                        CircularProgressIndicator(modifier = Modifier.size(20.dp), strokeWidth = 2.dp)
                        Text(
                            if (current.pendingUserAction) "Confirm update in system prompt…" else "Starting update installation…",
                            style = MaterialTheme.typography.bodyMedium
                        )
                    }
                }
                is UpdateState.Error -> {
                    Text(current.message, color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodySmall)
                    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        if (current.info != null) {
                            Button(onClick = { updater.downloadUpdate(current.info) }) {
                                Text("Retry download")
                            }
                        }
                        OutlinedButton(onClick = { updater.checkForUpdate(isAutomatic = false) }) {
                            Text("Check again")
                        }
                    }
                }
            }
        }
    }
}
