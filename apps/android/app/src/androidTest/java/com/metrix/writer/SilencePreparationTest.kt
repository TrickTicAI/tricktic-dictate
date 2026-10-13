@file:Suppress("DEPRECATION")
package com.metrix.writer

import android.test.AndroidTestCase
import java.io.File
import java.io.RandomAccessFile
import java.util.UUID

class SilencePreparationTest : AndroidTestCase() {
    private lateinit var directory: File
    override fun setUp() { super.setUp(); directory = File(context.cacheDir, "silence-test-${UUID.randomUUID()}").apply { mkdirs() } }
    override fun tearDown() { directory.deleteRecursively(); super.tearDown() }
    private fun wav(seconds: Int, signal: (Int) -> Short): File {
        val file = File(directory, "original.wav")
        RandomAccessFile(file, "rw").use { out ->
            SessionStore.header(out, seconds * 32000L)
            repeat(seconds * 16000) { i -> val v = signal(i).toInt(); out.write(v and 255); out.write(v shr 8 and 255) }
        }
        return file
    }
    fun testLongSilenceBecomesOnePauseAndOriginalAndPartialFrameAreRetained() {
        val file = wav(12) { i -> if (i < 32000 || i >= 160000) (if (i % 2 == 0) 2000 else -2000).toShort() else 0 }
        val original = file.readBytes()
        val prepared = SilencePreparation.prepare(context, file)
        assertEquals("trimmed", prepared.metrics.getString("silence_preparation"))
        assertEquals(5000L, prepared.metrics.getLong("silence_output_ms"))
        assertTrue(original.contentEquals(file.readBytes()))
        val copy = prepared.file.readBytes()
        assertTrue(copy.copyOfRange(44, 64044).contentEquals(original.copyOfRange(44, 64044)))
        assertTrue(copy.takeLast(64000).toByteArray().contentEquals(original.takeLast(64000).toByteArray()))
        prepared.close(); assertFalse(prepared.file.exists()); assertTrue(file.exists())
    }
    fun testNaturalPauseAndQuietSpeechRemain() {
        for (signal in listOf<(Int) -> Short>(
            { i -> if (i in 32000..63999) 0 else (if (i % 2 == 0) 2000 else -2000).toShort() },
            { i -> (kotlin.math.sin(i * 2 * Math.PI * 180 / 16000) * 12).toInt().toShort() }
        )) {
            val file = wav(8, signal); val original = file.readBytes()
            SilencePreparation.prepare(context, file).use { result ->
                assertEquals("unchanged", result.metrics.getString("silence_preparation"))
                assertEquals(file, result.file); assertTrue(original.contentEquals(file.readBytes()))
            }
        }
    }
    fun testPrivateS25MicrophoneQualification() {
        val fixture = File(context.filesDir, "mw104-proof/microphone.wav")
        if (!fixture.exists()) return
        val hash = PhoneModels.hash(fixture)
        SilencePreparation.prepare(context, fixture).use { prepared ->
            assertEquals(hash, PhoneModels.hash(fixture))
            File(fixture.parentFile, "microphone-preparation.json").writeText(prepared.metrics.toString())
            assertTrue(prepared.metrics.getLong("silence_removed_ms") > 4000)
            assertTrue(prepared.metrics.getLong("silence_removed_ms") < 6000)
        }
    }
    fun testPrivateMicrophoneQualificationAndLongSilenceReplay() {
        val fixture = File(context.filesDir, "mw104-proof/input.wav")
        if (!fixture.exists()) return
        val model = File(context.filesDir, "mw104-proof/base.bin")
        val hash = PhoneModels.hash(fixture)
        SilencePreparation.prepare(context, fixture).use { prepared ->
            assertEquals(hash, PhoneModels.hash(fixture))
            val directory = fixture.parentFile!!
            File(directory, "preparation.json").writeText(prepared.metrics.toString())
            File(directory, "quiet-frames.bin").writeBytes(SilenceNative().quietFrames(fixture.path, 128.0)!!)
            assertTrue(prepared.file.length() < 25_000_000)
            File(directory, "trimmed.wav").writeBytes(prepared.file.readBytes())
            if (model.exists()) {
                val native = PhoneNative(); val handle = native.create()
                try {
                    if (File(directory, "baseline.flag").exists()) {
                        val before = android.os.SystemClock.elapsedRealtime()
                        val baseline = native.transcribe(handle, model.path, fixture.path, "en")
                        File(directory, "base-before-result.txt").writeBytes(baseline)
                        File(directory, "base-before-timing.json").writeText(org.json.JSONObject().put("inference_ms", android.os.SystemClock.elapsedRealtime() - before).toString())
                    }
                    val started = android.os.SystemClock.elapsedRealtime()
                    val text = String(native.transcribe(handle, model.path, prepared.file.path, "en"), Charsets.UTF_8)
                    File(directory, "base-result.txt").writeText(text)
                    File(directory, "base-timing.json").writeText(org.json.JSONObject().put("inference_ms", android.os.SystemClock.elapsedRealtime() - started).toString())
                    assertFalse(text.contains("camera clicking", ignoreCase = true))
                    assertTrue(text.contains("country", ignoreCase = true))
                } finally { native.destroy(handle) }
            }
        }
    }
    fun testOffsetsKeepAnUncertainFrameInsideALongPause() {
        val flags = ByteArray(1000) { 1 }; flags[500] = 0
        val ranges = SilencePreparation.removed(flags)
        assertEquals(2, ranges.size)
        assertTrue(ranges[0].last < 500L * 640)
        assertTrue(ranges[1].first > 501L * 640)
        assertTrue(ranges.all { it.first % 2 == 0L && (it.last + 1) % 2 == 0L })
    }
    fun testInvalidWavFallsBackWithoutTouchingSource() {
        val file = File(directory, "bad.wav").apply { writeText("uncertain input") }
        SilencePreparation.prepare(context, file).use { result ->
            assertEquals("unavailable", result.metrics.getString("silence_preparation")); assertEquals(file, result.file)
            assertEquals("uncertain input", file.readText())
        }
    }
}
