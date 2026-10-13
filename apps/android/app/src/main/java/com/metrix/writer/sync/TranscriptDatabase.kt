package com.metrix.writer.sync

import android.database.sqlite.SQLiteDatabase
import android.util.AtomicFile
import org.json.JSONArray
import org.json.JSONObject
import java.io.File

/** Private, text-only projection. The native recording sidecars remain authoritative. */
internal class TranscriptDatabase(private val source: AtomicFile, private val fields: Set<String>, private val bare: Boolean = false) {
    private val path = File(source.baseFile.path.removeSuffix(".json") + ".sqlite3")
    private var cached: JSONObject? = null
    private var cachedSequence = -1L
    private val dirty = linkedSetOf<Pair<String, String>>()
    private var metadataDirty = false
    var writtenBytes: Long = 0
        private set

    private fun open(): SQLiteDatabase {
        path.parentFile!!.mkdirs()
        val db = SQLiteDatabase.openOrCreateDatabase(path, null)
        try {
            db.execSQL("PRAGMA synchronous=FULL")
            db.rawQuery("PRAGMA secure_delete=ON", null).use { it.moveToFirst() }
            db.execSQL("CREATE TABLE IF NOT EXISTS meta (id INTEGER PRIMARY KEY CHECK(id=1), value TEXT NOT NULL, sequence INTEGER NOT NULL)")
            db.execSQL("CREATE TABLE IF NOT EXISTS entries (field TEXT NOT NULL, key TEXT NOT NULL, value TEXT NOT NULL, live INTEGER NOT NULL, sequence INTEGER NOT NULL, original TEXT, session TEXT, PRIMARY KEY(field,key))")
            db.execSQL("CREATE INDEX IF NOT EXISTS history_page ON entries(field,live,key)")
            db.execSQL("CREATE INDEX IF NOT EXISTS history_family ON entries(field,original)")
            db.execSQL("CREATE INDEX IF NOT EXISTS history_session ON entries(field,session)")
            db.execSQL("CREATE INDEX IF NOT EXISTS history_changes ON entries(field,sequence)")
            val ready = db.rawQuery("SELECT id FROM meta", null).use { it.moveToFirst() }
            if (!ready) {
                val old = TranscriptStore.read(source) ?: JSONObject()
                val state = if (bare) JSONObject().put("records", old) else old
                db.beginTransaction()
                try {
                    var seq = 0L
                    for (field in fields) {
                        val entries = state.optJSONObject(field) ?: JSONObject()
                        for (key in entries.keys()) { seq++; put(db, field, key, entries.get(key), seq) }
                        state.remove(field)
                    }
                    db.execSQL("INSERT INTO meta VALUES (1,?,?)", arrayOf<Any>(state.toString(), seq))
                    db.setTransactionSuccessful()
                } finally { db.endTransaction() }
            }
            // An interrupted import keeps the old file until the transaction commits.
            // An interrupted unlink is retried; no stale plaintext copy survives later deletion.
            source.delete()
            check(!source.baseFile.exists() && !File(source.baseFile.path + ".bak").exists())
            return db
        } catch (error: Exception) { db.close(); throw error }
    }
    private fun put(db: SQLiteDatabase, field: String, key: String, value: Any, sequence: Long) {
        val live = (value as? JSONObject)?.optJSONObject("content")?.optString("state") == "live"
        val data = (value as? JSONObject)?.optJSONObject("content")?.optJSONObject("data") ?: (value as? JSONObject)?.optJSONObject("data")
        val original = data?.optString("original_record_id").orEmpty()
        val text = value.toString()
        db.execSQL("INSERT INTO entries(field,key,value,live,sequence,original,session) VALUES(?,?,?,?,?,?,?) ON CONFLICT(field,key) DO UPDATE SET value=excluded.value,live=excluded.live,sequence=excluded.sequence,original=excluded.original,session=excluded.session", arrayOf<Any>(field, key, text, if (live) 1 else 0, sequence, original, (value as? JSONObject)?.optString("session").orEmpty()))
        writtenBytes += text.toByteArray(Charsets.UTF_8).size
    }
    fun load(fresh: () -> JSONObject): JSONObject {
        if (cached != null && path.exists()) {
            val sequence = open().use { db -> db.rawQuery("SELECT sequence FROM meta WHERE id=1", null).use { check(it.moveToFirst()); it.getLong(0) } }
            if (sequence == cachedSequence) return cached!!
        }
        cached = null; dirty.clear(); metadataDirty = false
        val value = open().use { db ->
            val metadata = db.rawQuery("SELECT value,sequence FROM meta WHERE id=1", null).use { check(it.moveToFirst()); cachedSequence = it.getLong(1); JSONObject(it.getString(0)) }
            val defaults = fresh()
            for (key in defaults.keys()) if (!metadata.has(key)) metadata.put(key, defaults.get(key))
            metadata
        }
        val tracked = TrackingJson(value) { key -> if (key in fields) metadataDirty = true else metadataDirty = true }
        for (field in fields) {
            val map = LazyTranscriptMap(
                read = { key -> open().use { db -> db.rawQuery("SELECT value FROM entries WHERE field=? AND key=?", arrayOf(field, key)).use { rows -> if (rows.moveToFirst()) parse(rows.getString(0)) else null } } },
                list = { open().use { db -> db.rawQuery("SELECT key FROM entries WHERE field=? ORDER BY sequence,key", arrayOf(field)).use { rows -> buildList { while (rows.moveToNext()) add(rows.getString(0)) } } } },
                count = { open().use { db -> db.rawQuery("SELECT count(*) FROM entries WHERE field=?", arrayOf(field)).use { rows -> check(rows.moveToFirst()); rows.getInt(0) } } },
                changed = { key -> dirty.add(field to key) })
            map.trackChildren { key -> { dirty.add(field to key) } }
            tracked.initial(field, map)
        }
        tracked.onPut = { key, item ->
            if (key in fields) {
                val before = tracked.optJSONObject(key)
                val replacement = item as? JSONObject ?: JSONObject()
                before?.keys()?.forEach { dirty.add(key to it) }
                replacement.keys().forEach { dirty.add(key to it) }
                TrackingJson(replacement) { child -> dirty.add(key to child) }.also { map -> map.trackChildren { child -> { dirty.add(key to child) } } }
            } else { metadataDirty = true; item }
        }
        cached = tracked; dirty.clear(); metadataDirty = false
        return tracked
    }
    fun keys(field: String, limit: Int): List<String> = open().use { db ->
        db.rawQuery("SELECT key FROM entries WHERE field=? ORDER BY sequence,key LIMIT ?", arrayOf(field, limit.toString())).use { rows -> buildList { while (rows.moveToNext()) add(rows.getString(0)) } }
    }
    fun pendingKeys(field: String): Set<String> = dirty.filter { it.first == field }.map { it.second }.toSet()
    fun family(field: String, original: String): Set<String> = open().use { db ->
        db.rawQuery("SELECT key FROM entries WHERE field=? AND (key=? OR original=?)", arrayOf(field, original, original)).use { rows ->
            buildSet { while (rows.moveToNext()) add(rows.getString(0)) }
        }
    }
    fun session(field: String, id: String): Set<String> = open().use { db ->
        db.rawQuery("SELECT key FROM entries WHERE field=? AND session=?", arrayOf(field, id)).use { rows ->
            buildSet { while (rows.moveToNext()) add(rows.getString(0)) }
        }
    }
    fun save(state: JSONObject) {
        check(state === cached) { "A transcript transaction must save its own loaded state" }
        if (dirty.isEmpty() && !metadataDirty) return
        try {
            open().use { db ->
                db.beginTransaction()
                try {
                    var seq = db.rawQuery("SELECT sequence FROM meta WHERE id=1", null).use { check(it.moveToFirst()); it.getLong(0) }
                    seq++
                    for ((field, key) in dirty) {
                        val map = state.getJSONObject(field)
                        seq++
                        if (map.has(key)) put(db, field, key, map.get(key), seq)
                        else db.execSQL("DELETE FROM entries WHERE field=? AND key=?", arrayOf(field, key))
                    }
                    val metadata = JSONObject()
                    for (key in state.keys()) if (key !in fields) metadata.put(key, state.get(key))
                    val text = metadata.toString()
                    db.execSQL("UPDATE meta SET value=?, sequence=? WHERE id=1", arrayOf<Any>(text, seq))
                    writtenBytes += text.toByteArray(Charsets.UTF_8).size
                    db.setTransactionSuccessful()
                    cachedSequence = seq
                } finally { db.endTransaction() }
            }
            for (field in fields) (state.optJSONObject(field) as? LazyTranscriptMap)?.committed()
            dirty.clear(); metadataDirty = false
        } catch (error: Exception) { cached = null; dirty.clear(); metadataDirty = false; throw error }
    }
    fun page(field: String, after: String? = null, limit: Int = 100): List<JSONObject> = open().use { db ->
        db.rawQuery("SELECT value FROM entries WHERE field=? AND live=1 AND key>? ORDER BY key LIMIT ?", arrayOf(field, after.orEmpty(), limit.coerceAtLeast(1).toString())).use { rows ->
            buildList { while (rows.moveToNext()) add(JSONObject(rows.getString(0))) }
        }
    }
    fun changes(field: String, after: Long, limit: Int = 100): List<Pair<Long, Pair<String, Any>>> = open().use { db ->
        db.rawQuery("SELECT sequence,key,value FROM entries WHERE field=? AND sequence>? ORDER BY sequence LIMIT ?", arrayOf(field, after.toString(), limit.toString())).use { rows ->
            buildList { while (rows.moveToNext()) add(rows.getLong(0) to (rows.getString(1) to parse(rows.getString(2)))) }
        }
    }
    companion object {
        private fun parse(text: String): Any = org.json.JSONTokener(text).nextValue()
    }
}

/** Writes through the existing JSONObject API while tracking only changed map entries. */
private open class TrackingJson(source: JSONObject, private val changed: (String) -> Unit) : JSONObject() {
    var onPut: ((String, Any?) -> Any?)? = null
    private var childChange: ((String) -> () -> Unit)? = null
    init { for (key in source.keys()) super.put(key, source.get(key)) }
    fun initial(key: String, value: Any?) { super.put(key, value) }
    fun trackChildren(change: (String) -> () -> Unit) {
        childChange = change
        if (this !is LazyTranscriptMap) for (key in keys().asSequence().toList()) initial(key, wrap(opt(key), change(key)))
    }
    protected fun wrap(value: Any?, callback: () -> Unit): Any? = when (value) {
        is JSONObject -> TrackingJson(value) { callback() }.also { it.trackChildren { { callback() } } }
        else -> value
    }
    override fun put(name: String, value: Any?): JSONObject {
        val prepared = onPut?.invoke(name, value) ?: childChange?.let { wrap(value, it(name)) } ?: value
        val previous = opt(name)
        if (previous === prepared || (previous !is JSONObject && previous !is JSONArray && prepared !is JSONObject && prepared !is JSONArray && previous?.toString() == prepared?.toString())) return this
        super.put(name, prepared); changed(name); return this
    }
    override fun put(name: String, value: Boolean): JSONObject = put(name, value as Any)
    override fun put(name: String, value: Int): JSONObject = put(name, value as Any)
    override fun put(name: String, value: Long): JSONObject = put(name, value as Any)
    override fun put(name: String, value: Double): JSONObject = put(name, value as Any)
    override fun remove(name: String?): Any? { if (name != null && has(name)) changed(name); return super.remove(name) }
}

/** Lookup record text only when accessed, including after process restart. */
private class LazyTranscriptMap(private val read: (String) -> Any?, private val list: () -> List<String>, private val count: () -> Int, private val changed: (String) -> Unit) : TrackingJson(JSONObject(), changed) {
    private val loaded = mutableSetOf<String>()
    private val removed = mutableSetOf<String>()
    private val originals = mutableMapOf<String, Boolean>()
    private fun fetch(name: String?) {
        if (name == null || name in loaded || name in removed) return
        val value = read(name)
        originals[name] = value != null; loaded.add(name)
        if (value != null) initial(name, wrap(value) { changed(name) })
    }
    override fun opt(name: String?): Any? { fetch(name); return super.opt(name) }
    override fun get(name: String): Any { fetch(name); return super.get(name) }
    override fun has(name: String?): Boolean { fetch(name); return super.has(name) }
    override fun put(name: String, value: Any?): JSONObject { fetch(name); removed.remove(name); return super.put(name, value) }
    override fun remove(name: String?): Any? { fetch(name); val value = super.remove(name); if (name != null) removed.add(name); return value }
    fun committed() { for (key in originals.keys.toList()) originals[key] = super.has(key) }
    override fun keys(): MutableIterator<String> = (list().filter { it !in removed } + super.keys().asSequence().toList()).distinct().toMutableList().iterator()
    override fun length(): Int = count() + originals.entries.sumOf { (key, existed) -> (if (super.has(key)) 1 else 0) - (if (existed) 1 else 0) }
}
