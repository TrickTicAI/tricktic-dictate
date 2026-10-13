package com.metrix.writer.sync

import org.json.JSONArray
import org.json.JSONObject

/**
 * Kotlin form of packages/contracts/account-sync.firestore.mjs (MW-OUATH-5): the Firestore
 * layout and the transactional commit planner. The Android adapter runs [plan] inside a
 * Firestore transaction; firestore.rules enforces every write independently. It is checked
 * against the shared conflicts.json vectors by SyncPlannerTest.
 */
object Planner {
    const val MAX_FAMILY_MEMBERS = 200
    const val MAX_LIST_LIMIT = 500
    private val UID = Regex("^[^/]{1,128}$")
    private val UUID = Regex("^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$")

    /** Firebase UIDs are opaque; refuse anything that could change the path shape. */
    fun checkedUid(uid: String?): String {
        require(uid != null && UID.matches(uid) && uid != "." && uid != ".." && !(uid.startsWith("__") && uid.endsWith("__"))) { "invalid account UID" }
        return uid
    }
    private fun checkedId(id: String): String { require(UUID.matches(id)) { "invalid record or operation ID" }; return id }

    fun account(uid: String) = "users/${checkedUid(uid)}"
    fun record(uid: String, id: String) = "users/${checkedUid(uid)}/records/${checkedId(id)}"
    fun receipt(uid: String, id: String) = "users/${checkedUid(uid)}/receipts/${checkedId(id)}"
    fun family(uid: String, id: String) = "users/${checkedUid(uid)}/families/${checkedId(id)}"
    fun attempt(uid: String, id: String) = "users/${checkedUid(uid)}/attempts/${checkedId(id)}"
    fun device(uid: String, id: String) = "users/${checkedUid(uid)}/devices/${checkedId(id)}"

    /** The only account document a client may create: everything disabled. */
    fun initialAccount(uid: String): JSONObject {
        fun off() = JSONObject().put("enabled", false).put("generation", 1L)
        return JSONObject().put("schema_version", 1L).put("account_epoch", 1L).put("account_state", "active").put("consent", JSONObject()
            .put("schema_version", 1L).put("uid", uid).put("account_epoch", 1L).put("revision", 1L).put("settings", off()).put("dictionary", off())
            .put("transcripts", off().put("history", JSONObject().put("mode", "off"))))
    }

    /** The next ConsentState for an explicit choice; a changed toggle advances its generation by one. */
    fun nextConsent(consent: JSONObject, category: String, enabled: Boolean, history: JSONObject? = null): JSONObject {
        val next = StrictJson.copy(consent)
        val before = consent.getJSONObject(category)
        val after = JSONObject().put("enabled", enabled).put("generation", before.getLong("generation"))
        if (category == "transcripts") after.put("history", if (enabled) StrictJson.copy(history!!) else JSONObject().put("mode", "off"))
        val changed = before.getBoolean("enabled") != enabled || (category == "transcripts" && !StrictJson.equal(before.opt("history"), after.opt("history")))
        if (changed) after.put("generation", before.getLong("generation") + 1)
        next.put(category, after)
        next.put("revision", consent.getLong("revision") + 1)
        return next
    }

    /** Storage document for an accepted operation at [revision]. */
    fun recordDocument(op: JSONObject, revision: Long): JSONObject = JSONObject().put("schema_version", 1L).put("id", op.getString("record_id"))
        .put("category", op.getString("category")).put("origin_device_id", op.getString("origin_device_id")).put("revision", revision)
        .put("content", StrictJson.copy(op.getJSONObject("content"))).put("operation_id", op.getString("operation_id"))
        .put("consent_generation", op.getLong("consent_generation"))

    /** The RemoteRecord fields of a stored record document. */
    fun remoteRecord(document: JSONObject): JSONObject = JSONObject().apply {
        for (key in listOf("schema_version", "id", "category", "origin_device_id", "revision", "content")) put(key, document.get(key))
    }

    /** packages/contracts/account-sync.reference.mjs validPortableData: fixed setting IDs, dictionary text, model IDs. */
    fun validPortableData(id: String, category: String, data: JSONObject): Boolean {
        if (category == "settings" && Contract.SETTING_IDS[data.optString("key")] != id) return false
        if (category != "settings" && id in Contract.SETTING_KEY_BY_ID) return false
        if (category == "dictionary") for (key in data.keys()) {
            val value = data.get(key)
            if (value is String && key != "kind" && !Contract.isDictionaryText(value)) return false
        }
        val models = listOfNotNull(data.optJSONObject("selection"),
            if (data.optString("key") == "transcription_model") data.optJSONObject("value") else null,
            if (data.optString("key") == "cleanup") data.optJSONObject("value")?.optJSONObject("selection") else null)
        return models.all { model -> model.getString("model").let { !it.contains("..") && it.split('/').none(String::isEmpty) } }
    }

    /** A full-document write, or only the [mask] fields when set. */
    private fun JSONObject.text(key: String): String? = opt(key) as? String

    data class Write(val path: String, val data: JSONObject, val mask: List<String>? = null)
    data class Plan(val status: String, val revision: Long? = null, val record: JSONObject? = null, val current: JSONObject? = null, val writes: List<Write> = emptyList())

    /**
     * Decides one outbox operation. [read] returns the transaction's snapshot of a document,
     * or null. [attemptEnrolled] is the device's own history-enrollment decision (transcripts only).
     * Only an acknowledged plan with writes is committed. Precondition: [op] passed schema validation.
     */
    suspend fun plan(authUid: String?, op: JSONObject, attemptEnrolled: Boolean = true, read: suspend (String) -> JSONObject?): Plan {
        fun reject(status: String) = Plan(status)
        if (authUid == null || authUid != op.text("uid")) return reject("wrong_account")
        val uid = checkedUid(op.getString("uid"))
        val category = op.getString("category")
        val recordId = op.getString("record_id")
        val account = read(account(uid)) ?: return reject("consent_required")
        if (account.optString("account_state") != "active") return reject("account_unavailable")
        if (account.getLong("account_epoch") != op.getLong("account_epoch")) return reject("stale_epoch")
        val consent = account.getJSONObject("consent").getJSONObject(category)
        if (consent.getLong("generation") != op.getLong("consent_generation")) return reject("stale_generation")
        if (!consent.getBoolean("enabled")) return reject("consent_required")
        if (category == "transcripts" && (!attemptEnrolled ||
                consent.getJSONObject("history").text("enrollment_id") != op.text("history_enrollment_id"))) return reject("history_not_enrolled")

        read(receipt(uid, op.getString("operation_id")))?.let { receipt ->
            if (!StrictJson.equal(receipt.get("operation"), op)) return reject("idempotency_conflict")
            val revision = receipt.getLong("revision")
            return Plan("acknowledged", revision, remoteRecord(recordDocument(op, revision)))
        }

        if ((category == "settings") != (recordId in Contract.SETTING_KEY_BY_ID)) return reject("invalid_record")
        val current = read(record(uid, recordId))
        if (current != null && (current.getString("category") != category || current.getString("origin_device_id") != op.getString("origin_device_id"))) {
            return reject("invalid_record")
        }
        val currentDeleted = current?.getJSONObject("content")?.getString("state") == "deleted"
        // A fixed setting ID may be recreated only in a newer confirmed generation.
        if (currentDeleted && !(category == "settings" && current.getLong("consent_generation") < op.getLong("consent_generation"))) return reject("deleted")

        val writes = mutableListOf<Write>()
        val content = op.getJSONObject("content")
        val data = if (content.getString("state") == "live") content.getJSONObject("data") else null
        if (data != null) {
            if (!validPortableData(recordId, category, data)) return reject("invalid_record")
            if (category == "dictionary" && current?.getJSONObject("content")?.getString("state") == "live" &&
                current.getJSONObject("content").getJSONObject("data").getString("kind") != data.getString("kind")) return reject("invalid_record")
        }
        if (category == "transcripts" && data?.getString("kind") == "original" && current == null) {
            if (read(attempt(uid, data.getString("attempt_id"))) != null) return reject("invalid_record")
            writes += Write(attempt(uid, data.getString("attempt_id")), JSONObject().put("original_record_id", recordId))
            writes += Write(family(uid, recordId), JSONObject().put("member_ids", JSONArray()).put("deleted", false))
        } else if (category == "transcripts" && data != null && data.getString("kind") != "original" && current == null) {
            val originalId = data.getString("original_record_id")
            val family = read(family(uid, originalId))
            if (family?.getBoolean("deleted") == true) return reject("deleted")
            val parentId = data.getString("parent_record_id")
            val parent = read(record(uid, parentId))
            val members = family?.getJSONArray("member_ids")?.let { array -> (0 until array.length()).map { array.getString(it) } }.orEmpty()
            if (family == null || parent == null || parent.getString("category") != "transcripts" || parent.getJSONObject("content").getString("state") != "live" ||
                (parentId != originalId && parentId !in members)) return reject("invalid_record")
            if (members.size >= MAX_FAMILY_MEMBERS) return reject("quota_exceeded")
            writes += Write(family(uid, originalId), JSONObject().put("member_ids", JSONArray(members + recordId)).put("deleted", false))
        }

        val base = op.getLong("base_revision")
        if (current == null && base != 0L) return reject("invalid_base")
        if (current != null && current.getLong("revision") != base) {
            return Plan("conflict", current.getLong("revision"), current = remoteRecord(current))
        }
        if (current != null && category == "transcripts" && data != null) return reject("immutable")

        val revision = (current?.getLong("revision") ?: 0L) + 1
        writes.add(0, Write(record(uid, recordId), recordDocument(op, revision)))
        writes.add(1, Write(receipt(uid, op.getString("operation_id")), JSONObject().put("operation", StrictJson.copy(op)).put("revision", revision)))
        // Deleting an original fences its family and tombstones every live version in the same commit.
        if (category == "transcripts" && data == null && current?.getJSONObject("content")?.optJSONObject("data")?.optString("kind") == "original") {
            val family = read(family(uid, recordId))
            if (family != null && !family.getBoolean("deleted")) {
                writes += Write(family(uid, recordId), StrictJson.copy(family).put("deleted", true))
                val members = family.getJSONArray("member_ids")
                for (i in 0 until members.length()) {
                    val member = read(record(uid, members.getString(i)))
                    if (member?.getJSONObject("content")?.getString("state") != "live") continue
                    writes += Write(record(uid, members.getString(i)), StrictJson.copy(member).put("revision", member.getLong("revision") + 1)
                        .put("content", JSONObject().put("state", "deleted")))
                }
            }
        }
        return Plan("acknowledged", revision, remoteRecord(recordDocument(op, revision)), writes = writes)
    }
}
