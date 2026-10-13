package com.metrix.writer.ui

import android.view.WindowManager
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import com.metrix.writer.CloudProviders
import com.metrix.writer.ui.settings.*

/** Routes `Settings/<page>` to its page; the pages live in [com.metrix.writer.ui.settings]. */
@Composable fun SettingsScreen(vm: WriterViewModel, state: WriterState, route: String, navigate: (String) -> Unit, setTheme: (String) -> Unit) {
    when (val page = SettingsRoutes.page(route)) {
        SettingsRoutes.SETUP -> SetupPage(vm, state, navigate)
        SettingsRoutes.ENGINE -> SpeechEnginePage(vm, state, navigate)
        SettingsRoutes.SPEECH_MODEL -> SpeechModelPage(vm, state, navigate)
        SettingsRoutes.LANGUAGE -> SpeechLanguagePage(vm)
        SettingsRoutes.POLISHING -> PolishingPage(vm, state, navigate)
        SettingsRoutes.POLISHING_MODEL -> PolishingModelPage(vm, state, navigate)
        SettingsRoutes.POLISHING_OPTIONS -> PolishingOptionsPage(vm, state)
        SettingsRoutes.POLISHING_INSTRUCTIONS -> PolishingInstructionsPage(vm, state)
        SettingsRoutes.MODELS -> PhoneModelsPage(vm, state, navigate)
        SettingsRoutes.KEYS -> KeysPage(vm, state, navigate)
        SettingsRoutes.ACCOUNT -> Account.Page()
        SettingsRoutes.BACKUP -> BackupPage(vm)
        SettingsRoutes.FLOATING -> FloatingPage(vm, state)
        SettingsRoutes.PERMISSIONS -> PermissionsPage(vm)
        SettingsRoutes.APPEARANCE -> AppearancePage(vm, setTheme)
        SettingsRoutes.STORAGE -> StoragePage(vm, state, navigate)
        SettingsRoutes.LEGACY -> LegacyConnectionPage(vm)
        SettingsRoutes.LICENSES -> LicensesScreen()
        else -> CloudProviders.all.find { SettingsRoutes.keyPage(it) == page }?.let { ProviderKeyPage(vm, state, it) } ?: SettingsHome(vm, state, navigate)
    }
}

/** Keeps secrets out of screenshots and the recents preview while a key or credential page is shown. */
@Composable internal fun SecureScreen() {
    val activity = androidx.activity.compose.LocalActivity.current ?: return
    DisposableEffect(activity) {
        activity.window.addFlags(WindowManager.LayoutParams.FLAG_SECURE)
        onDispose { activity.window.clearFlags(WindowManager.LayoutParams.FLAG_SECURE) }
    }
}
