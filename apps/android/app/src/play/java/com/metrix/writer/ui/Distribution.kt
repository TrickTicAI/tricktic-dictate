package com.metrix.writer.ui

import android.content.Context
import android.content.Intent
import androidx.compose.runtime.Composable

/** The Play app is updated by Google Play only: no updater, install permission or sideload guidance. */
internal object Distribution {
    val searchEntries: List<SettingEntry> = emptyList()
    @Suppress("UNUSED_PARAMETER") fun onVisible(context: Context) = Unit
    @Suppress("UNUSED_PARAMETER") @Composable fun InstallationHelp(open: (Intent) -> Unit) = Unit
    @Composable fun InstallationNote() = Unit
    @Suppress("UNUSED_PARAMETER") @Composable fun UpdatesSection(vm: WriterViewModel, state: WriterState) = Unit
}
