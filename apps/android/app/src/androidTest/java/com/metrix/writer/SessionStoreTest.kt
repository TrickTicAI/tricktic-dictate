package com.metrix.writer

import android.test.AndroidTestCase
import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.io.RandomAccessFile
import java.util.UUID

@Suppress("DEPRECATION")
class SessionStoreTest : AndroidTestCase() {
    private lateinit var store: SessionStore
    override fun setUp() { super.setUp(); store = SessionStore(File(context.cacheDir, "test-${UUID.randomUUID()}")) }
    override fun tearDown() { store.directory.deleteRecursively(); super.tearDown() }
    fun testInterruptedCapturePreservesPcmAndRepairsHeader() {
        val item = store.create("mixed")
        val pcm = ByteArray(3200) { (it % 127).toByte() }
        RandomAccessFile(store.audio(item), "rw").use { it.seek(44); it.write(pcm); it.write(99) }
        val recovered = store.restore().single()
        assertEquals("interrupted", recovered.getString("capture_state"))
        assertEquals(3244L, recovered.getLong("byte_length"))
        assertTrue(pcm.contentEquals(store.audio(item).readBytes().drop(44).toByteArray()))
        assertEquals(100L, recovered.getLong("duration_ms"))
        assertTrue(store.audio(item).readBytes().contentEquals(store.audio(store.restore().single()).readBytes()))
    }
    fun testRecoveredDictationIsMarkedForTranscriptionButHistoryIsNot() {
        val bubble = store.create("en", configure = { it.put("capture_origin", "bubble") })
        val companion = store.create("en", configure = { it.put("capture_origin", "companion") })
        val historical = store.create("en", configure = { it.put("capture_origin", "app") })
        store.finishCapture(historical, "Screen locked or switched off. Saved audio retained.")
        for (item in listOf(bubble, companion)) RandomAccessFile(store.audio(item), "rw").use { it.seek(44); it.write(ByteArray(3200)) }
        val restored = store.restore().associateBy { it.getString("id") }
        assertTrue(restored.getValue(bubble.getString("id")).optBoolean("auto_submit"))
        assertEquals(100L, restored.getValue(bubble.getString("id")).getLong("duration_ms"))
        assertFalse(restored.getValue(companion.getString("id")).has("auto_submit"))
        // Recordings interrupted before this change are never resubmitted on upgrade.
        assertEquals("interrupted", restored.getValue(historical.getString("id")).getString("capture_state"))
        assertFalse(restored.getValue(historical.getString("id")).has("auto_submit"))
        // The mark is set once, at recovery; clearing it is final.
        val cleared = store.load(bubble.getString("id")).also { it.remove("auto_submit"); store.save(it) }
        assertFalse(store.restore().single { it.getString("id") == cleared.getString("id") }.has("auto_submit"))
    }
    fun testCompleteWavAttemptsAndUncertainRequestSurviveReload() {
        val item = store.create("he").put("capture_state", "complete")
        val attempt = JSONObject().put("id", "old-job").put("status", JSONObject().put("state", "succeeded")
            .put("result", JSONObject().put("transcript", JSONObject().put("raw_text", "fixture עברית"))))
        item.put("attempts", JSONArray().put(attempt)); store.save(item)
        val originalAudio = store.audio(item).readBytes()
        store.prepare(item, "https://example.invalid", "local", "ivrit-turbo", "he")
        val pending = item.getJSONObject("pending").toString()
        val reopened = SessionStore(store.directory)
        val restored = reopened.restore().single()
        reopened.prepare(restored, "https://example.invalid", "openai", "other", "en")
        assertEquals(pending, reopened.load(item.getString("id")).getJSONObject("pending").toString())
        assertEquals("old-job", restored.getJSONObject("pending").getString("retry_of_job_id"))
        assertEquals(attempt.toString(), restored.getJSONArray("attempts").getJSONObject(0).toString())
        assertTrue(originalAudio.contentEquals(reopened.audio(restored).readBytes()))
        try { reopened.prepare(restored, "https://different.invalid", "local", "model", "he"); fail("Changed pending server") }
        catch (_: IllegalStateException) { }
    }
    fun testAtomicBackupRestoredWithoutBaseFile() {
        val item = store.create("en").put("capture_state", "complete"); store.save(item)
        val base = File(store.directory, item.getString("id") + ".json")
        assertTrue(base.renameTo(File(base.path + ".bak")))
        assertEquals(item.toString(), store.restore().single().toString())
        assertTrue(base.isFile)
    }
    fun testDamagedMetadataDoesNotHideHealthySessions() {
        val item = store.create("en").put("capture_state", "complete"); store.save(item)
        File(store.directory, UUID.randomUUID().toString() + ".json").writeText("broken")
        assertEquals(item.toString(), store.restore().single().toString())
        assertEquals(1, store.restoreFailures)
    }
    fun testUnsafeEndpointDoesNotCreatePendingRequest() {
        val item = store.create("en").put("capture_state", "complete"); store.save(item)
        try { store.prepare(item, "http://example.invalid", "local", "model", "en"); fail("Cleartext allowed") }
        catch (_: IllegalArgumentException) { }
        assertFalse(store.load(item.getString("id")).has("pending"))
    }
    fun testRejectPathTraversal() {
        try { store.load("../../elsewhere"); fail("Invalid session ID accepted") }
        catch (_: IllegalArgumentException) { }
    }
}
