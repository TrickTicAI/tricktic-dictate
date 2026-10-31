package com.metrix.writer

import android.util.AtomicFile
import org.json.JSONObject
import java.io.File
import java.util.UUID

/**
 * Delivery outcomes shared by the ledger, session owners and the frontend. The legacy `state`
 * values (awaiting_result, recovery, unconfirmed, delivered) stay for existing records/readers;
 * `outcome` and the content-free `reason_code` are additive and derived for older records.
 */
enum class DeliveryOutcome(val code: String) {
    AWAITING("awaiting_result"),
    INSERTED_VERIFIED("inserted_verified"),
    /** No usable target when the result was ready, or the automatic lease ended: copy explicitly. */
    SAVED_NOT_INSERTED("saved_not_inserted"),
    /** An edit was issued but exact readback did not confirm it. Never replayed automatically. */
    UNCONFIRMED("unconfirmed"),
    /** The focused editor is excluded or cannot accept/verify this insertion. */
    UNSUPPORTED("unsupported");
    companion object {
        fun of(item: JSONObject): DeliveryOutcome = entries.firstOrNull { it.code == item.optString("outcome") } ?: when (item.optString("state")) {
            "delivered" -> INSERTED_VERIFIED; "unconfirmed" -> UNCONFIRMED; "recovery" -> SAVED_NOT_INSERTED; else -> AWAITING
        }
    }
}

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

    /**
     * Explicit user recovery (copy) of a saved result. Records only that it happened; it never
     * issues an edit, so an unconfirmed insertion cannot be replayed through this path.
     * Returns null when nothing was saved for this token.
     */
    @Synchronized fun recover(token: String): String? {
        val item = load(token)
        if (!item.has("text")) return null
        item.put("recovered_at_ms", System.currentTimeMillis()); save(item)
        return item.getString("text")
    }

    companion object {
        /** Legacy `state` for each outcome; unsupported is a copyable recovery for existing readers. */
        fun JSONObject.markOutcome(outcome: DeliveryOutcome, reasonCode: String, reason: String): JSONObject =
            put("state", when (outcome) {
                DeliveryOutcome.INSERTED_VERIFIED -> "delivered"; DeliveryOutcome.UNCONFIRMED -> "unconfirmed"
                DeliveryOutcome.AWAITING -> "awaiting_result"; else -> "recovery"
            }).put("outcome", outcome.code).put("reason_code", reasonCode).put("reason", reason)
    }
}
