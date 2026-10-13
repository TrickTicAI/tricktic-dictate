package com.metrix.writer

import android.content.Context
import android.os.SystemClock
import org.json.JSONObject
import java.io.File
import java.io.RandomAccessFile
import java.nio.ByteBuffer
import java.nio.ByteOrder

internal fun interface SilenceBatch { fun accept(flags: ByteArray, count: Int): Boolean }

internal class SilenceNative {
    external fun scan(audio: String, ceiling: Double, consumer: SilenceBatch): Boolean
    companion object { init { System.loadLibrary("writer-phone") } }
}

/** Model input only. Neither SessionStore.audio nor playback/export ever use this copy. */
internal object SilencePreparation {
    private const val FRAME = 640L // 20 ms of 16 kHz mono PCM16
    private const val MIN_FRAMES = 250 // Keep pauses shorter than five seconds verbatim.
    private const val PAD = 16_000L // Half a second at both edges; one retained pause.
    // Measured S25 room tone; see docs/features/silence-trimming.md (MW-104 R-4).
    private const val QUALIFIED_RMS = 128.0

    class Prepared(val file: File, val metrics: JSONObject, private val temporary: Boolean) : AutoCloseable {
        override fun close() { if (temporary) file.delete() }
    }
    fun prepare(context: Context, source: File, cancelled: () -> Boolean = { false }): Prepared {
        val started = SystemClock.elapsedRealtime()
        val before = source.length()
        val metrics = JSONObject().put("silence_input_bytes", before).put("silence_output_bytes", before)
            .put("silence_input_ms", (before - 44).coerceAtLeast(0) / 32)
            .put("silence_output_ms", (before - 44).coerceAtLeast(0) / 32).put("silence_removed_ms", 0)
        var temporary: File? = null
        fun finish(file: File, state: String): Prepared {
            metrics.put("silence_prepare_ms", SystemClock.elapsedRealtime() - started).put("silence_preparation", state)
            return Prepared(file, metrics, file != source)
        }
        try {
            check(!cancelled()) { "Transcription cancelled; saved audio retained" }
            RandomAccessFile(source, "r").use { input ->
                val header = ByteArray(44); input.readFully(header)
                val h = ByteBuffer.wrap(header).order(ByteOrder.LITTLE_ENDIAN)
                check(before > 44 && (before - 44) % 2 == 0L &&
                    String(header, 0, 4, Charsets.US_ASCII) == "RIFF" &&
                    h.getInt(4).toLong().and(0xffffffffL) == before - 8 &&
                    String(header, 8, 8, Charsets.US_ASCII) == "WAVEfmt " &&
                    h.getInt(16) == 16 && h.getShort(20).toInt() == 1 && h.getShort(22).toInt() == 1 &&
                    h.getInt(24) == 16000 && h.getInt(28) == 32000 && h.getShort(32).toInt() == 2 && h.getShort(34).toInt() == 16 &&
                    String(header, 36, 4, Charsets.US_ASCII) == "data" && h.getInt(40).toLong().and(0xffffffffL) == before - 44)
                var output: RandomAccessFile? = null
                val buffer = ByteArray(64 * 1024)
                var offset = 0L
                var frame = 0L
                var start = -1L
                var removedBytes = 0L
                fun copyUntil(end: Long) {
                    input.seek(44 + offset)
                    var remaining = end - offset
                    while (remaining > 0) {
                        check(!cancelled()) { "Transcription cancelled; saved audio retained" }
                        val count = minOf(remaining, buffer.size.toLong()).toInt()
                        input.readFully(buffer, 0, count); output!!.write(buffer, 0, count); remaining -= count
                    }
                    offset = end
                }
                fun closeRun() {
                    if (start >= 0 && frame - start >= MIN_FRAMES) {
                        if (output == null) {
                            val root = File(context.cacheDir, "inference-audio").apply { check(isDirectory || mkdirs()) }
                            val copy = File.createTempFile("silence-", ".wav", root).also { temporary = it }
                            output = RandomAccessFile(copy, "rw")
                            SessionStore.header(output!!, 0)
                        }
                        val first = start * FRAME + PAD
                        val end = frame * FRAME - PAD
                        copyUntil(first)
                        removedBytes += end - first
                        offset = end
                    }
                    start = -1
                }
                try {
                    val success = SilenceNative().scan(source.path, QUALIFIED_RMS, SilenceBatch { flags, count ->
                        check(!cancelled()) { "Transcription cancelled; saved audio retained" }
                        repeat(count) { i ->
                            if (flags[i].toInt() == 1) { if (start < 0) start = frame } else closeRun()
                            frame++
                        }
                        true
                    })
                    check(success && frame == (before - 44) / FRAME)
                    closeRun()
                    if (output == null) return finish(source, "unchanged")
                    copyUntil(before - 44) // Includes any partial final 20 ms frame.
                    val bytes = before - 44 - removedBytes
                    output!!.seek(0); SessionStore.header(output!!, bytes)
                    check(source.length() == before && output!!.length() == bytes + 44)
                    metrics.put("silence_output_bytes", bytes + 44).put("silence_output_ms", bytes / 32)
                        .put("silence_removed_ms", removedBytes / 32)
                    return finish(temporary!!, "trimmed")
                } finally { output?.close() }
            }
        } catch (error: Exception) {
            temporary?.delete()
            if (cancelled()) throw error
            return finish(source, "unavailable") // An uncertain/unsupported input is always kept.
        } catch (error: OutOfMemoryError) {
            temporary?.delete()
            throw java.io.IOException("Audio preparation exceeded available memory; retry saved audio", error)
        } catch (_: LinkageError) {
            temporary?.delete()
            return finish(source, "unavailable")
        }
    }
    fun addMetrics(timing: JSONObject, metrics: JSONObject) {
        metrics.keys().forEach { key -> timing.put(key, metrics.get(key)) }
    }
}
