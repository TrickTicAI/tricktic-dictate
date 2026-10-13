package com.metrix.writer.ui

import android.content.Context
import androidx.compose.runtime.Composable

/** The Play build has no sign-in until MW-RELEASE-12 makes it store-compliant: nothing links to it. */
internal object Account {
    const val available = false
    val searchEntries: List<SettingEntry> = emptyList()
    @Suppress("UNUSED_PARAMETER") fun onVisible(context: Context) = Unit
    @Suppress("UNUSED_PARAMETER") @Composable fun SettingsEntry(navigate: (String) -> Unit) = Unit
    @Suppress("UNUSED_PARAMETER") @Composable fun HistoryEntry(navigate: (String) -> Unit) = Unit
    @Composable fun TranscriptPage() = Unit
    @Composable fun Page() = Unit
}
