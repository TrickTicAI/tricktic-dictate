package com.metrix.writer.sync

import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import org.json.JSONArray
import org.json.JSONObject
import java.io.IOException
import java.net.HttpURLConnection
import java.net.URL

/** A sanitized cloud failure. Only the kind reaches the UI; never a response body or token. */
class CloudFailure(val kind: Kind) : Exception(kind.name) {
    enum class Kind { OFFLINE, UNAVAILABLE, QUOTA, DENIED, UNAUTHENTICATED, NOT_SET_UP, INVALID }
}

/** The documents the sync engine reads and writes. Firestore REST in the app, an in-memory fake in tests. */
interface CloudStore {
    /**
     * Reads with [read], then commits the returned writes only if nothing read has changed since
     * (none writes nothing). [block] may run more than once, so it must only plan from what it reads.
     */
    suspend fun <T> transaction(token: String, block: suspend (read: suspend (String) -> JSONObject?) -> Pair<T, List<Planner.Write>>): T
    suspend fun get(token: String, path: String): JSONObject?
    /** One page of `users/{uid}/records` in [category], ordered by ID after [after]. Never a listener. */
    suspend fun records(token: String, uid: String, category: String, after: String?, limit: Int): List<JSONObject>
}

/**
 * Cloud Firestore over REST with the user's Firebase ID token, so every request is checked by
 * infra/firebase/firestore.rules. No Firestore SDK, offline cache or listener: Writer's own
 * stores stay authoritative and the outbox decides what is sent.
 */
class FirestoreRest(projectId: String, origin: String = "https://firestore.googleapis.com") : CloudStore {
    private val root = "projects/${Planner.checkedUid(projectId)}/databases/(default)/documents"
    private val api = "$origin/v1/$root"

    /** The commit's preconditions no longer hold: something read has changed. */
    private class Stale : Exception()

    private suspend fun call(token: String, method: String, url: String, body: JSONObject?, stale: Boolean = false): Any = withContext(Dispatchers.IO) {
        val connection = try { URL(url).openConnection() as HttpURLConnection } catch (_: IOException) { throw CloudFailure(CloudFailure.Kind.OFFLINE) }
        try {
            connection.requestMethod = method
            connection.connectTimeout = 15_000
            connection.readTimeout = 30_000
            connection.setRequestProperty("Authorization", "Bearer $token")
            connection.setRequestProperty("Content-Type", "application/json; charset=utf-8")
            if (body != null) { connection.doOutput = true; connection.outputStream.use { it.write(body.toString().toByteArray(Charsets.UTF_8)) } }
            val status = connection.responseCode
            val text = (if (status in 200..299) connection.inputStream else connection.errorStream)?.use { it.readBytes().toString(Charsets.UTF_8) }.orEmpty()
            if (status !in 200..299) {
                val code = runCatching { JSONObject(text).getJSONObject("error").optString("status") }.getOrDefault("")
                if (stale && code == "FAILED_PRECONDITION") throw Stale()
                throw CloudFailure(when {
                    status == 401 || code == "UNAUTHENTICATED" -> CloudFailure.Kind.UNAUTHENTICATED
                    status == 403 || code == "PERMISSION_DENIED" -> CloudFailure.Kind.DENIED
                    status == 429 || code == "RESOURCE_EXHAUSTED" -> CloudFailure.Kind.QUOTA
                    status == 404 || code == "NOT_FOUND" -> CloudFailure.Kind.NOT_SET_UP
                    status == 409 || status >= 500 || code in setOf("ABORTED", "UNAVAILABLE", "DEADLINE_EXCEEDED", "INTERNAL") -> CloudFailure.Kind.UNAVAILABLE
                    else -> CloudFailure.Kind.INVALID
                })
            }
            if (text.isBlank()) JSONObject() else if (text.trimStart().startsWith("[")) JSONArray(text) else JSONObject(text)
        } catch (error: IOException) { throw CloudFailure(CloudFailure.Kind.OFFLINE) } finally { connection.disconnect() }
    }

    // Production Firestore refuses a read-write beginTransaction for rules-authenticated (end-user)
    // tokens with 403; only the emulator allows it. So, like the Firebase client SDKs, read without
    // a transaction and commit each write with a precondition on the version read, plus a `verify`
    // for every document read but not written. A concurrent change fails the commit with
    // FAILED_PRECONDITION and the block runs again from fresh reads. The rules still gate
    // consent, epoch and ownership against the committed state.
    override suspend fun <T> transaction(token: String, block: suspend (read: suspend (String) -> JSONObject?) -> Pair<T, List<Planner.Write>>): T {
        repeat(ATTEMPTS) {
            val seen = LinkedHashMap<String, String?>() // path -> updateTime read, or null when missing
            val (result, writes) = block { path -> read(token, path).also { (_, update) -> seen.putIfAbsent(path, update) }.first }
            if (writes.isEmpty()) return result
            fun precondition(path: String) = if (!seen.containsKey(path)) null
                else seen[path]?.let { JSONObject().put("updateTime", it) } ?: JSONObject().put("exists", false)
            val body = JSONArray()
            writes.forEach { write ->
                body.put(JSONObject().put("update", JSONObject().put("name", "$root/${write.path}").put("fields", fields(write.data)))
                    .apply { write.mask?.let { put("updateMask", JSONObject().put("fieldPaths", JSONArray(it))) } }
                    .apply { precondition(write.path)?.let { put("currentDocument", it) } })
            }
            seen.keys.filter { path -> writes.none { it.path == path } }.forEach { path ->
                body.put(JSONObject().put("verify", "$root/$path").put("currentDocument", precondition(path)))
            }
            try {
                call(token, "POST", "$api:commit", JSONObject().put("writes", body), stale = true)
                return result
            } catch (_: Stale) { /* read again */ }
        }
        throw CloudFailure(CloudFailure.Kind.UNAVAILABLE)
    }

    /** One document and its updateTime (null when missing). batchGet, so a missing document is not an error. */
    private suspend fun read(token: String, path: String): Pair<JSONObject?, String?> {
        val body = JSONObject().put("documents", JSONArray().put("$root/$path"))
        val entry = (call(token, "POST", "$api:batchGet", body) as JSONArray).getJSONObject(0)
        val found = entry.optJSONObject("found") ?: return null to null
        return decodeFields(found.optJSONObject("fields")) to found.getString("updateTime")
    }

    override suspend fun get(token: String, path: String) = read(token, path).first

    override suspend fun records(token: String, uid: String, category: String, after: String?, limit: Int): List<JSONObject> {
        require(limit in 1..Planner.MAX_LIST_LIMIT)
        val parent = Planner.account(uid)
        val query = JSONObject().put("from", JSONArray().put(JSONObject().put("collectionId", "records")))
            .put("where", JSONObject().put("fieldFilter", JSONObject().put("field", JSONObject().put("fieldPath", "category")).put("op", "EQUAL").put("value", JSONObject().put("stringValue", category))))
            .put("orderBy", JSONArray().put(JSONObject().put("field", JSONObject().put("fieldPath", "__name__")).put("direction", "ASCENDING")))
            .put("limit", limit)
        after?.let { query.put("startAt", JSONObject().put("before", false).put("values", JSONArray().put(JSONObject().put("referenceValue", "$root/${Planner.record(uid, it)}")))) }
        val rows = call(token, "POST", "$api/$parent:runQuery", JSONObject().put("structuredQuery", query)) as JSONArray
        return (0 until rows.length()).mapNotNull { rows.getJSONObject(it).optJSONObject("document")?.let { document -> decodeFields(document.optJSONObject("fields")) } }
    }

    companion object {
        private const val ATTEMPTS = 5

        fun encode(value: Any?): JSONObject = when (value) {
            null, JSONObject.NULL -> JSONObject().put("nullValue", JSONObject.NULL)
            is Boolean -> JSONObject().put("booleanValue", value)
            is Int, is Long -> JSONObject().put("integerValue", value.toString())
            is String -> JSONObject().put("stringValue", value)
            is JSONArray -> JSONObject().put("arrayValue", if (value.length() == 0) JSONObject() else JSONObject().put("values", JSONArray((0 until value.length()).map { encode(value.get(it)) })))
            is JSONObject -> JSONObject().put("mapValue", if (value.length() == 0) JSONObject() else JSONObject().put("fields", fields(value)))
            else -> throw IllegalArgumentException("unsupported value")
        }
        fun fields(value: JSONObject) = JSONObject().apply { value.keys().forEach { key -> put(key, encode(value.get(key))) } }

        fun decode(value: JSONObject): Any {
            val type = value.keys().next()
            return when (type) {
                "nullValue" -> JSONObject.NULL
                "booleanValue" -> value.getBoolean(type)
                "integerValue" -> value.getString(type).toLong()
                "stringValue" -> value.getString(type)
                "arrayValue" -> JSONArray(value.getJSONObject(type).optJSONArray("values")?.let { values -> (0 until values.length()).map { decode(values.getJSONObject(it)) } }.orEmpty())
                "mapValue" -> decodeFields(value.getJSONObject(type).optJSONObject("fields"))
                else -> throw CloudFailure(CloudFailure.Kind.INVALID) // Contract documents never hold other types.
            }
        }
        fun decodeFields(fields: JSONObject?): JSONObject = JSONObject().apply { fields?.keys()?.forEach { key -> put(key, decode(fields.getJSONObject(key))) } }
    }
}
