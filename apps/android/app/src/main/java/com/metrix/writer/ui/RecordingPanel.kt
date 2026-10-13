package com.metrix.writer.ui

import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.Alignment
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.semantics.*
import androidx.compose.ui.unit.dp
import androidx.compose.ui.platform.LocalLayoutDirection
import androidx.compose.ui.unit.LayoutDirection
import kotlinx.coroutines.delay
import com.metrix.writer.PRODUCT_NAME

@Composable fun IdleControl(size: Int, onClick: () -> Unit) {
    Box(Modifier.size(size.coerceAtLeast(48).dp), contentAlignment = Alignment.Center) {
        Surface(onClick = onClick, color = MaterialTheme.colorScheme.primary, shape = RoundedCornerShape(12.dp),
            shadowElevation = 3.dp, modifier = Modifier.size(size.dp).semantics { contentDescription = "$PRODUCT_NAME microphone" }) {
            Box(contentAlignment = Alignment.Center) { WriterIcon("wave", Modifier.size((size * .55f).dp), MaterialTheme.colorScheme.onPrimary) }
        }
    }
}
@Composable fun RecordingPanel(state: String, level: () -> Float, finish: () -> Unit, cancel: () -> Unit, bubble: Boolean = false, preview: Boolean = false) {
    val recording = state == "recording"
    val processing = state == "processing"
    val sessionId = com.metrix.writer.CaptureService.activeSessionId
    LaunchedEffect(recording, sessionId, preview) {
        if (recording && !preview) {
            withFrameNanos { }
            com.metrix.writer.CaptureService.visibleRecording(sessionId, bubble)
        }
    }
    // Keep the agreed physical order (X / waveform / finish) in every UI language.
    CompositionLocalProvider(LocalLayoutDirection provides LayoutDirection.Ltr) {
    Surface(shape = RoundedCornerShape(18.dp), color = MaterialTheme.colorScheme.surfaceContainerHigh, shadowElevation = 3.dp) {
        Row(Modifier.heightIn(min = 56.dp).padding(horizontal = 4.dp), verticalAlignment = Alignment.CenterVertically) {
            IconButton(onClick = cancel, enabled = recording || processing, modifier = Modifier.semantics { contentDescription = if(processing) "Stop processing and keep audio" else "Cancel recording" }) { WriterIcon("close") }
            Box(Modifier.width(if (recording) 72.dp else 40.dp).height(48.dp), contentAlignment = Alignment.Center) {
                if (recording) LiveWaveform(level)
                else CircularProgressIndicator(Modifier.size(20.dp).semantics {
                    contentDescription = when (state) { "starting" -> "Starting microphone"; "saving" -> "Finishing recording"; else -> "Preparing text" }
                }, strokeWidth = 2.dp)
            }
            IconButton(onClick = finish, enabled = recording, modifier = Modifier.semantics { contentDescription = "$PRODUCT_NAME stop recording" }) { WriterIcon("check", tint = if(recording) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.outline) }
        }
    }
}
}
@Composable fun LiveWaveform(level: () -> Float) {
    var samples by remember { mutableStateOf(List(13) { 0f }) }
    LaunchedEffect(Unit) { while (true) { samples = samples.drop(1) + (level() * 5f).coerceIn(0f,1f); delay(55) } }
    val amplitudes = samples.map { animateFloatAsState(it, tween(90), label = "microphone").value }
    val color = MaterialTheme.colorScheme.primary
    Canvas(Modifier.fillMaxWidth().height(32.dp).semantics { contentDescription = "Live microphone waveform" }) {
        val step = size.width / amplitudes.size
        amplitudes.forEachIndexed { i, value -> val h = 3.dp.toPx() + value * (size.height - 3.dp.toPx()); val x = step * (i + .5f); drawLine(color, Offset(x,(size.height-h)/2),Offset(x,(size.height+h)/2),3.dp.toPx(),StrokeCap.Round) }
    }
}
