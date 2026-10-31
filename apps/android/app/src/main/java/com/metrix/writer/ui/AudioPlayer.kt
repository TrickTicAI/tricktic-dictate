package com.metrix.writer.ui

import android.media.MediaPlayer
import android.media.PlaybackParams
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.*
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.LifecycleResumeEffect
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.withContext
import java.io.File

/**
 * A recording's shape for the compact player: [peaks] (0..1, one per bar) and [sections], the
 * start times after pauses longer than two seconds. Computed in one bounded streaming pass over the
 * saved 16 kHz mono PCM WAV, so a 30-minute file never loads into memory.
 */
data class AudioShape(val durationMs: Long, val peaks: List<Float>, val sections: List<Long>) {
    companion object {
        private const val WINDOW_MS = 100
        private const val PAUSE_MS = 2000L
        fun read(file: File, bars: Int = 96): AudioShape? = runCatching {
            val bytes = file.length() - 44
            if (bytes <= 0) return null
            val samplesPerWindow = 16 * WINDOW_MS
            val levels = FloatArray(((bytes / 2) / samplesPerWindow + 1).toInt().coerceAtMost(200_000))
            file.inputStream().buffered(64 * 1024).use { input ->
                input.skip(44)
                val buffer = ByteArray(samplesPerWindow * 2)
                var window = 0
                while (window < levels.size) {
                    var read = 0
                    while (read < buffer.size) { val n = input.read(buffer, read, buffer.size - read); if (n < 0) break; read += n }
                    if (read < 2) break
                    var sum = 0.0
                    for (i in 0 until read / 2) { val v = ((buffer[i * 2 + 1].toInt() shl 8) or (buffer[i * 2].toInt() and 0xFF)).toShort() / 32768.0; sum += v * v }
                    levels[window++] = Math.sqrt(sum / (read / 2)).toFloat()
                    if (read < buffer.size) break
                }
                val used = levels.copyOf(window)
                val loud = used.sortedArray().let { if (it.isEmpty()) 0f else it[(it.size * .9).toInt().coerceAtMost(it.size - 1)] }.coerceAtLeast(1e-4f)
                val quiet = loud * .12f
                val sections = mutableListOf<Long>()
                var run = 0
                used.forEachIndexed { i, level ->
                    if (level < quiet) run++ else { if (run * WINDOW_MS >= PAUSE_MS && i > run) sections += i.toLong() * WINDOW_MS; run = 0 }
                }
                val per = (used.size / bars.toFloat()).coerceAtLeast(1f)
                val peaks = (0 until minOf(bars, used.size)).map { b ->
                    val from = (b * per).toInt(); val to = ((b + 1) * per).toInt().coerceIn(from + 1, used.size)
                    ((used.copyOfRange(from, to).maxOrNull() ?: 0f) / loud).coerceIn(0f, 1f)
                }
                AudioShape(bytes / 32, peaks, sections)
            }
        }.getOrNull()
    }
}

/**
 * Compact player: Play, a waveform scrubber, times and speed; for recordings over a minute also
 * ±10 s and pause sections. It always reads left to right (D-6). Playback stops when the screen pauses.
 */
@Composable internal fun AudioPlayer(sessionId: String, file: () -> File, durationMs: Long, enabled: Boolean, failed: () -> Unit) {
    var player by remember { mutableStateOf<MediaPlayer?>(null) }
    var playing by remember { mutableStateOf(false) }
    var position by rememberSaveable(sessionId) { mutableLongStateOf(0L) }
    var speed by rememberSaveable(sessionId) { mutableFloatStateOf(1f) }
    var shape by remember(sessionId) { mutableStateOf<AudioShape?>(null) }
    LaunchedEffect(sessionId, enabled) { if (enabled) shape = withContext(Dispatchers.IO) { AudioShape.read(file()) } }
    val total = (shape?.durationMs ?: durationMs).coerceAtLeast(1)
    fun release() { player?.release(); player = null; playing = false }
    DisposableEffect(sessionId) { onDispose { release() } }
    LifecycleResumeEffect(sessionId) { onPauseOrDispose { release() } }
    LaunchedEffect(playing) { while (playing) { player?.let { runCatching { position = it.currentPosition.toLong() } }; delay(200) } }
    fun seek(ms: Long) {
        position = ms.coerceIn(0, total)
        player?.let { runCatching { it.seekTo(position, MediaPlayer.SEEK_CLOSEST) } }
    }
    fun play() {
        val existing = player
        if (existing != null) { runCatching { existing.start(); playing = true }; return }
        val audio = MediaPlayer()
        try {
            audio.setDataSource(file().path)
            audio.setOnPreparedListener {
                if (player !== it) return@setOnPreparedListener
                runCatching { it.playbackParams = PlaybackParams().setSpeed(speed) }
                it.seekTo(position, MediaPlayer.SEEK_CLOSEST); it.start(); playing = true
            }
            audio.setOnCompletionListener { position = 0; release() }
            audio.setOnErrorListener { _, _, _ -> release(); failed(); true }
            player = audio; audio.prepareAsync()
        } catch (_: Exception) { audio.release(); player = null; failed() }
    }
    LeftToRight {
        Surface(shape = MaterialTheme.shapes.medium, color = MaterialTheme.colorScheme.surfaceContainerLow, modifier = Modifier.fillMaxWidth().testTag("audio-player")) {
            Column(Modifier.padding(horizontal = 8.dp, vertical = 8.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    FilledTonalIconButton(onClick = { if (playing) { player?.pause(); playing = false } else play() }, enabled = enabled,
                        modifier = Modifier.size(48.dp).testTag("audio-play").semantics { contentDescription = if (playing) "Pause recording" else "Play recording" }) {
                        WriterIcon(if (playing) "pause" else "play", tint = MaterialTheme.colorScheme.onSecondaryContainer)
                    }
                    Scrubber(shape, position, total, enabled, ::seek, Modifier.weight(1f).padding(horizontal = 8.dp))
                }
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Figures("${formatClock(position)} / ${formatClock(total)}", Modifier.padding(start = 8.dp).weight(1f), MaterialTheme.typography.labelMedium, MaterialTheme.colorScheme.onSurfaceVariant)
                    if (total > 60_000) {
                        IconButton(onClick = { seek(position - 10_000) }, enabled = enabled, modifier = Modifier.semantics { contentDescription = "Back 10 seconds" }) { WriterIcon("back10") }
                        IconButton(onClick = { seek(position + 10_000) }, enabled = enabled, modifier = Modifier.semantics { contentDescription = "Forward 10 seconds" }) { WriterIcon("forward10") }
                    }
                    TextButton(onClick = {
                        speed = when (speed) { 1f -> 1.25f; 1.25f -> 1.5f; 1.5f -> 2f; else -> 1f }
                        player?.let { p -> runCatching { val was = p.isPlaying; p.playbackParams = PlaybackParams().setSpeed(speed); if (!was) p.pause() } }
                    }, enabled = enabled, modifier = Modifier.heightIn(min = 48.dp).semantics { contentDescription = "Playback speed ${speed}×, change speed" }) {
                        Figures("${if (speed % 1f == 0f) speed.toInt().toString() else speed.toString()}×")
                    }
                }
                val sections = shape?.sections.orEmpty()
                if (total > 60_000 && sections.isNotEmpty()) {
                    Row(Modifier.horizontalScroll(rememberScrollState()).semantics { contentDescription = "Sections at pauses" }, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                        (listOf(0L) + sections).forEach { start ->
                            AssistChip(onClick = { seek(start) }, label = { Figures(formatClock(start), style = MaterialTheme.typography.labelMedium) }, enabled = enabled,
                                modifier = Modifier.heightIn(min = 48.dp).semantics { contentDescription = "Go to ${formatClock(start)}" })
                        }
                    }
                }
            }
        }
    }
}

@Composable private fun Scrubber(shape: AudioShape?, position: Long, total: Long, enabled: Boolean, seek: (Long) -> Unit, modifier: Modifier) {
    val played = MaterialTheme.colorScheme.primary
    val rest = MaterialTheme.colorScheme.outline
    Box(modifier.height(48.dp), contentAlignment = Alignment.Center) {
        val peaks = shape?.peaks
        if (peaks != null && peaks.isNotEmpty()) Canvas(Modifier.fillMaxWidth().height(32.dp)) {
            val step = size.width / peaks.size
            val split = (position.toFloat() / total) * peaks.size
            peaks.forEachIndexed { i, value ->
                val h = 2.dp.toPx() + value * (size.height - 2.dp.toPx()); val x = step * (i + .5f)
                drawLine(if (i < split) played else rest, Offset(x, (size.height - h) / 2), Offset(x, (size.height + h) / 2), (step * .6f).coerceAtLeast(1f), StrokeCap.Round)
            }
        }
        // The slider carries input and semantics (adjustable by TalkBack); the waveform is decoration.
        Slider(value = position.toFloat(), onValueChange = { seek(it.toLong()) }, valueRange = 0f..total.toFloat(), enabled = enabled,
            colors = if (peaks != null && peaks.isNotEmpty()) SliderDefaults.colors(activeTrackColor = androidx.compose.ui.graphics.Color.Transparent, inactiveTrackColor = androidx.compose.ui.graphics.Color.Transparent) else SliderDefaults.colors(),
            modifier = Modifier.fillMaxWidth().semantics { contentDescription = "Playback position" })
    }
}
