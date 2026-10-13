package com.metrix.writer.ui

import androidx.compose.material3.AlertDialog
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.window.DialogWindowProvider
import com.metrix.writer.protectTouches

/** Dialogs have their own Window; the Activity's touch guard does not cover them. */
@Composable internal fun ProtectedAlertDialog(
    onDismissRequest: () -> Unit,
    confirmButton: @Composable () -> Unit,
    dismissButton: @Composable (() -> Unit)? = null,
    title: @Composable (() -> Unit)? = null,
    text: @Composable (() -> Unit)? = null,
    icon: @Composable (() -> Unit)? = null,
) {
    AlertDialog(onDismissRequest = onDismissRequest, title = title, text = text, icon = icon,
        dismissButton = dismissButton, confirmButton = {
            val window = (LocalView.current.parent as DialogWindowProvider).window
            DisposableEffect(window) {
                val restore = window.protectTouches()
                onDispose { restore() }
            }
            confirmButton()
        })
}
