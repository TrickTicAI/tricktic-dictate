package com.metrix.writer

import android.util.AtomicFile
import org.json.JSONObject
import java.io.File
import java.util.UUID

/** Separate from audio/attempt sidecars. Editor content and target snapshots are never persisted. */
class DeliveryStore(private val directory: File) {
    init { check(directory.isDirectory || directory.mkdirs()) }
    private fun file(token: String) = AtomicFile(File(directory, "${UUID.fromString(token)}.json"))
    @Synchronized fun load(token: String) = JSONObject(String(file(token).readFully(), Charsets.UTF_8))
    @Synchronized fun save(item: JSONObject) {
        val file = file(item.getString("token")); val stream = file.startWrite()
        try { stream.write(item.toString().toByteArray(Charsets.UTF_8)); file.finishWrite(stream) }
        catch (e: Exception) { file.failWrite(stream); throw e }
    }
    @Synchronized fun create(session: String, token: String, fixture: Boolean) {
        UUID.fromString(session)
        save(JSONObject().put("token", token).put("session_id", session).put("fixture", fixture)
            .put("state", "awaiting_result").put("created_at_ms", System.currentTimeMillis()))
    }
    @Synchronized fun all(): List<JSONObject> = directory.listFiles().orEmpty()
        .filter { it.name.endsWith(".json") || it.name.endsWith(".json.bak") }
        .map { it.name.removeSuffix(".bak").removeSuffix(".json") }.distinct()
        .mapNotNull { try { load(it) } catch (_: Exception) { null } }
        .sortedByDescending { it.optLong("created_at_ms") }
}
