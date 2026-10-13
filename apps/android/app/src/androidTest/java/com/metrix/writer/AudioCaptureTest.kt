@file:Suppress("DEPRECATION")
package com.metrix.writer

import android.test.AndroidTestCase
import org.json.JSONObject
import java.io.File
import java.util.UUID
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

/** Deterministic native-source faults, real WAV fsync and AtomicFile metadata. */
class AudioCaptureTest : AndroidTestCase() {
    private lateinit var store: SessionStore
    override fun setUp() { super.setUp(); store = SessionStore(File(context.cacheDir, "capture-${UUID.randomUUID()}")) }
    override fun tearDown() { store.directory.deleteRecursively(); super.tearDown() }
    private class Source : AudioCapture.Source {
        val started = CountDownLatch(1)
        val first = CountDownLatch(1)
        @Volatile var fault = ""
        @Volatile var reads = 0
        @Volatile var closed = false
        override fun start() { started.countDown() }
        override fun read(bytes: ByteArray, length: Int): Int {
            if (fault == "read") return -6
            Thread.sleep(10)
            val count = minOf(length, 320)
            for (i in 0 until count) bytes[i] = (i % 127).toByte()
            reads++; first.countDown(); return count
        }
        override fun capacity() = 640
        override fun silenced() = fault == "silenced"
        override fun route() = if (fault == "route") 2 else 1
        override fun close() { closed = true }
    }
    private fun runFault(fault: String) {
        val source = Source(); val capture = AudioCapture(store) { source }
        val item = store.create("mixed"); val done = CountDownLatch(1)
        var error: String? = null
        capture.start(item) { _, e -> error = e; done.countDown() }
        assertTrue(source.first.await(3, TimeUnit.SECONDS))
        Thread.sleep(80)
        val prefix = store.audio(item).readBytes().drop(44).toByteArray()
        source.fault = fault
        assertTrue(done.await(3, TimeUnit.SECONDS)); assertNotNull(error); assertTrue(source.closed)
        val saved = store.load(item.getString("id"))
        assertEquals("interrupted", saved.getString("capture_state"))
        assertEquals("interrupted", saved.getJSONObject("capture_completion").getString("state"))
        assertTrue(prefix.contentEquals(store.audio(item).readBytes().drop(44).take(prefix.size).toByteArray()))
    }
    fun testContentionRetainsSavedPrefix() = runFault("silenced")
    fun testRouteChangeRetainsSavedPrefix() = runFault("route")
    fun testDeadRecorderRetainsSavedPrefix() = runFault("read")
    fun testStopDrainsOnceAndCannotRecoverActiveWriter() {
        val source = Source(); val capture = AudioCapture(store) { source }; val item = store.create("he")
        val done = CountDownLatch(1); var callbacks = 0
        capture.start(item) { _, e -> assertNull(e); callbacks++; done.countDown() }
        assertTrue(source.first.await(3, TimeUnit.SECONDS))
        assertEquals("capturing", store.restore().single().getString("capture_state"))
        try { capture.start(item) { _, _ -> }; fail("Conflicting owner") } catch (_: IllegalStateException) { }
        val reads = source.reads
        capture.stop(); capture.stop()
        assertTrue(done.await(3, TimeUnit.SECONDS)); assertEquals(1, callbacks)
        assertTrue("Drain is bounded by ring capacity", source.reads in reads + 2..reads + 3)
        val saved = store.load(item.getString("id"))
        assertEquals("complete", saved.getString("capture_state"))
        assertEquals(44L + source.reads * 320, saved.getLong("byte_length"))
        assertEquals(item.getString("id"), saved.getJSONObject("capture_completion").getString("event_id"))
    }
    fun testDiscardIntentWinsCrashAndCompletionRace() {
        val item = store.create("en")
        item.put("capture_state", "discarded"); store.save(item)
        val stale = JSONObject(item.toString()).put("capture_state", "capturing")
        store.finishCapture(stale, null)
        assertFalse(store.audio(item).exists())
        assertFalse(store.load(item.getString("id")).has("capture_completion"))
        assertTrue(store.restore().isEmpty())
    }
    fun testMetadataWriteFailureLeavesFramesForRecovery() {
        val item = store.create("en")
        val source = Source(); val capture = AudioCapture(store) { source }; val done = CountDownLatch(1)
        var error: String? = null
        capture.start(item) { _, e -> error = e; done.countDown() }
        assertTrue(source.first.await(3, TimeUnit.SECONDS))
        // AtomicFile's staging path is a directory: reproducible storage failure, no fake success.
        val obstruction = File(store.directory, item.getString("id") + ".json.new")
        assertTrue(obstruction.mkdir()); File(obstruction, "keep").writeText("fixture")
        capture.stop(); assertTrue(done.await(3, TimeUnit.SECONDS)); assertNotNull(error)
        val pcm = store.audio(item).readBytes().drop(44).toByteArray()
        assertTrue(pcm.isNotEmpty()); assertTrue(obstruction.deleteRecursively())
        // Callback happens just before the engine releases its ownership flag.
        while (capture.isActive) Thread.sleep(5)
        val recovered = store.restore().single()
        assertEquals("interrupted", recovered.getString("capture_state"))
        assertTrue(pcm.contentEquals(store.audio(item).readBytes().drop(44).toByteArray()))
    }
    fun testReadinessWaitsForDurableFirstFramesAndStopKeepsThoseFrames() {
        val release = CountDownLatch(1); val ready = CountDownLatch(1); val done = CountDownLatch(1)
        val source = object : AudioCapture.Source {
            override fun start() {}
            override fun read(bytes: ByteArray, length: Int): Int {
                release.await(3, TimeUnit.SECONDS)
                val count = minOf(320, length); for (i in 0 until count) bytes[i] = 42
                return count
            }
            override fun capacity() = 320
            override fun silenced() = false
            override fun route() = 1
            override fun close() {}
        }
        val capture = AudioCapture(store) { source }; val item = store.create("en", reserveCapture = true)
        assertEquals("capturing", store.restore().single().getString("capture_state"))
        capture.start(item, { _, _ -> done.countDown() }, {
            assertTrue(store.audio(item).length() > 44)
            ready.countDown()
        })
        assertFalse(ready.await(80, TimeUnit.MILLISECONDS)); assertFalse(capture.isRecording)
        release.countDown(); assertTrue(ready.await(3, TimeUnit.SECONDS)); capture.stop()
        assertTrue(done.await(3, TimeUnit.SECONDS))
        val saved = store.load(item.getString("id")); val timing = saved.getJSONObject("startup_timing")
        assertTrue(timing.getLong("first_audio_saved_elapsed_ms") >= timing.getLong("first_audio_read_elapsed_ms"))
        assertTrue(store.audio(saved).readBytes().drop(44).take(320).all { it == 42.toByte() })
    }

}
