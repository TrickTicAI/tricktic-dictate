@file:Suppress("DEPRECATION")
package com.metrix.writer

import android.test.InstrumentationTestCase
import android.util.AtomicFile
import com.metrix.writer.sync.TranscriptDatabase
import com.metrix.writer.sync.TranscriptStore
import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.util.UUID

/** Only deterministic synthetic text and private temporary directories. */
class HistoryScaleTest : InstrumentationTestCase() {
    private val context get() = instrumentation.targetContext
    private fun id(i: Int) = UUID(0, i.toLong()).toString()
    private fun row(i: Int) = JSONObject().put("schema_version", 1).put("id", id(i)).put("category", "transcripts")
        .put("origin_device_id", id(100001)).put("revision", 1).put("content", JSONObject().put("state", "live")
            .put("data", JSONObject().put("kind", "original").put("attempt_id", id(i)).put("recorded_at_ms", 10)
                .put("input_language", "mixed").put("output_language", JSONObject().put("mode", "preserve"))
                .put("selection", JSONObject().put("provider", "gemini").put("model", "synthetic-stt"))
                .put("text", "English עברית 🌍 ".repeat(32))))

    fun testFixed1k10kIndexedTranscriptPagesAndIncrementalWrites() {
        for (n in listOf(1000, 10000)) {
            val directory = File(context.cacheDir, "mw112-text-${UUID.randomUUID()}").also { check(it.mkdirs()) }
            try {
                val file = AtomicFile(File(directory, "history.json"))
                val records = JSONObject(); (1..n).forEach { records.put(id(it), row(it)) }
                val old = JSONObject().put("enabled", true).put("records", records).put("outbox", JSONObject())
                val bytes = old.toString().toByteArray(Charsets.UTF_8)
                TranscriptStore.write(file, old)
                val before = System.nanoTime(); val baseline = TranscriptStore.read(file)!!
                val baselineRows = baseline.getJSONObject("records").keys().asSequence().sorted().take(100).map { baseline.getJSONObject("records").getJSONObject(it) }.toList()
                val beforeUs = (System.nanoTime() - before) / 1000
                val db = TranscriptDatabase(file, setOf("records", "outbox"))
                assertEquals(100, db.page("records").size) // One-time import excluded from timing.
                assertFalse(file.baseFile.exists())
                val reopened = TranscriptDatabase(file, setOf("records", "outbox"))
                val after = System.nanoTime(); val page = reopened.page("records"); val afterUs = (System.nanoTime() - after) / 1000
                assertEquals(baselineRows.map { it.toString() }, page.map { it.toString() })
                var cursor: String? = null; val seen = mutableSetOf<String>()
                while (true) {
                    val part = reopened.page("records", cursor)
                    part.forEach { assertTrue(seen.add(it.getString("id"))) }
                    if (part.size < 100) break
                    cursor = part.last().getString("id")
                }
                assertEquals(n, seen.size)
                val state = reopened.load { JSONObject().put("enabled", false) }
                val inserted = row(n+1)
                inserted.getJSONObject("content").put("data", TranscriptStore.derived("edited", id(1), id(1), "offline edit עברית", 20))
                val beforeBytes = reopened.writtenBytes
                state.getJSONObject("records").put(id(n+1), inserted)
                state.getJSONObject("outbox").put(id(n+1), JSONObject().put("operation_id", id(n+2)))
                reopened.save(state)
                val written = reopened.writtenBytes-beforeBytes
                assertTrue("one edit writes bounded text bytes", written < 2048)
                val restart = TranscriptDatabase(file, setOf("records", "outbox"))
                assertEquals(1, restart.load { JSONObject() }.getJSONObject("outbox").length())
                val version = restart.page("records", id(n), 100).single().getJSONObject("content").getJSONObject("data")
                assertEquals(id(1), version.getString("parent_record_id"))
                // A second account has a separate path and receives no copies.
                val other = TranscriptDatabase(AtomicFile(File(directory, "other.json")), setOf("records", "outbox"))
                assertTrue(other.page("records").isEmpty())
                android.util.Log.i("MW112", "records=$n before_list_us=$beforeUs after_cold_list_us=$afterUs before_list_rows=$n after_list_rows=100 before_rewrite_bytes=${bytes.size} after_serialized_bytes=$written")
            } finally { directory.deleteRecursively() }
        }
    }
    fun testNativeIndexedPagesUseImmutableOrderAndRepairSidecarCommitGap() {
        val directory = File(context.cacheDir, "mw112-native-${UUID.randomUUID()}").also { check(it.mkdirs()) }
        try {
            for (i in 1..1000) {
                val item = JSONObject().put("id", id(i)).put("created_at_ms", 10).put("capture_state", "complete").put("attempts", JSONArray())
                TranscriptStore.write(AtomicFile(File(directory, "${id(i)}.json")), item)
            }
            val store = SessionStore(directory)
            val first = store.page(); assertEquals(50, first.records.size); val cursor = first.next!!
            val updated = store.load(id(940)).put("note", "offline edit ÄCCENTED עברית")
            store.save(updated)
            assertEquals(id(940), store.page(query = "äccented").records.single().getString("id"))
            store.save(JSONObject().put("id", id(2001)).put("created_at_ms", 20).put("capture_state", "complete").put("attempts", JSONArray()))
            val next = store.page(cursor)
            assertEquals(50, next.records.size)
            assertTrue(next.records.none { it.getString("id") in first.records.map { r -> r.getString("id") } })
            assertTrue(next.records.none { it.getString("id") == id(2001) })
            assertEquals(id(2001), store.page().records.first().getString("id"))
            // Simulate sidecar success with an uncommitted projection.
            android.database.sqlite.SQLiteDatabase.openDatabase(File(directory, "history-index.sqlite3").path, null, 0).use { db ->
                db.execSQL("INSERT OR IGNORE INTO repairs VALUES (?)", arrayOf(id(1000)))
            }
            val changed = store.load(id(1000)).put("note", "recovered projection")
            TranscriptStore.write(AtomicFile(File(directory, "${id(1000)}.json")), changed)
            val reopened = SessionStore(directory)
            assertEquals("recovered projection", reopened.page().records.first { it.getString("id") == id(1000) }.getString("note"))
            assertTrue("source metadata is preserved", File(directory, "${id(1000)}.json").exists())
        } finally { directory.deleteRecursively() }
    }
    fun testInterruptedMigrationAndUnknownHistoricalParentArePreserved() {
        val directory = File(context.cacheDir, "mw112-recovery-${UUID.randomUUID()}").also { check(it.mkdirs()) }
        try {
            val file = AtomicFile(File(directory, "history.json"))
            val orphan = row(1)
            orphan.getJSONObject("content").put("data", JSONObject().put("kind", "edited").put("original_record_id", id(2)).put("text", "historical unknown parent"))
            TranscriptStore.write(file, JSONObject().put("records", JSONObject().put(id(1), orphan)))
            android.database.sqlite.SQLiteDatabase.openOrCreateDatabase(File(directory, "history.sqlite3"), null).use { db ->
                db.execSQL("CREATE TABLE meta (id INTEGER PRIMARY KEY CHECK(id=1), value TEXT NOT NULL, sequence INTEGER NOT NULL)")
                db.beginTransaction(); try { db.execSQL("INSERT INTO meta VALUES (1,'{}',0)") } finally { db.endTransaction() }
            }
            assertTrue(file.baseFile.exists())
            val db = TranscriptDatabase(file, setOf("records"))
            val recovered = db.page("records").single().getJSONObject("content").getJSONObject("data")
            assertEquals("historical unknown parent", recovered.getString("text"))
            assertFalse("migration must not invent ancestry", recovered.has("parent_record_id"))
            assertFalse(file.baseFile.exists())
        } finally { directory.deleteRecursively() }
    }
}
