package com.metrix.writer

import android.app.Instrumentation
import android.content.Context
import com.metrix.writer.account.FreshToken
import com.metrix.writer.account.GoogleIdentity
import com.metrix.writer.account.IdentityUser
import com.metrix.writer.sync.CloudFailure
import com.metrix.writer.sync.CloudStore
import com.metrix.writer.sync.Planner
import com.metrix.writer.sync.PortableAdapter
import com.metrix.writer.sync.PortableChanges
import com.metrix.writer.sync.StrictJson
import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.util.concurrent.atomic.AtomicInteger

/** The shared MW-OUATH-1 vectors, packaged into the test APK unchanged by generateSyncFixtures. */
object SyncFixtures {
    fun bytes(instrumentation: Instrumentation, name: String): ByteArray =
        instrumentation.context.assets.open("account-sync/$name").use { it.readBytes() }
    fun json(instrumentation: Instrumentation, name: String): Any = StrictJson.parse(bytes(instrumentation, name))
}

/**
 * Puts every store that portable sync reads or writes into a known empty state and restores the
 * originals afterwards, so tests never leave their data in the app under test.
 */
class PortableSandbox(private val context: Context) {
    private val adapter = PortableAdapter(context)
    private val saved = adapter.snapshotNative()
    private val directories = listOf("portable-sync", "account-sync", "accounts", "recordings", "transcript-journal.json", "transcript-journal.sqlite3", "kept-transcripts.json").map { File(context.filesDir, it) }

    init {
        PortableChanges.settle()
        directories.forEach { directory -> if (directory.exists()) check(directory.renameTo(File(directory.path + ".sandbox"))) }
        clear()
    }

    /** Every portable preference absent: defaults, an empty dictionary, no sync state. */
    fun clear() {
        PortableChanges.settle()
        val empty = JSONObject(saved.toString())
        empty.keys().forEach { file -> val values = empty.getJSONObject(file); values.keys().asSequence().toList().forEach { values.put(it, JSONObject.NULL) } }
        adapter.restoreNative(empty)
        directories.forEach { it.deleteRecursively() }
    }

    fun restore() {
        // A change handled after this point must not write test state into the restored directories.
        PortableChanges.settle()
        adapter.restoreNative(saved)
        PortableChanges.settle()
        directories.forEach { directory ->
            directory.deleteRecursively()
            File(directory.path + ".sandbox").takeIf { it.exists() }?.let { check(it.renameTo(directory)) }
        }
    }
}

/** A dictionary document in the app's stored format. */
fun dictionary(vararg entries: Pair<String, JSONObject>): JSONObject {
    val document = JSONObject().put("hints", JSONArray()).put("corrections", JSONArray()).put("snippets", JSONArray())
    entries.forEach { (kind, entry) -> document.getJSONArray(kind).put(entry.put("enabled", entry.optBoolean("enabled", true))) }
    return document
}

/** A signed-in Firebase user whose ID token names its UID, which [FakeCloud] checks like the rules would. */
class SyncIdentity(var user: IdentityUser?, private val token: (String) -> String = { "token-$it" }) : GoogleIdentity {
    @Volatile var next: IdentityUser? = null
    override val projectId: String? = "demo-metrix-writer"
    override fun restore() = user
    override suspend fun googleIdToken(context: Context) = "google-id-token"
    override suspend fun signIn(googleIdToken: String): IdentityUser = next!!.also { user = it }
    override suspend fun reauthenticate(googleIdToken: String, uid: String) = user!!
    override suspend fun idToken(uid: String, forceRefresh: Boolean) = token(uid)
    override suspend fun freshIdToken(uid: String) = FreshToken(token(uid), System.currentTimeMillis() / 1000)
    override fun signOutLocal() { user = null }
    override suspend fun clearCredentialState() {}
}

/**
 * In-memory Firestore for the engine: transactions with optimistic read checks, per-UID
 * ownership, masked updates and injected failures. Firestore's rules themselves are exercised by
 * FirestoreEmulatorTest against the real emulator.
 */
class FakeCloud : CloudStore {
    val documents = LinkedHashMap<String, JSONObject>()
    @Volatile var offline = false
    /** One-shot: the next commit whose writes match is applied, then reported as a network failure. */
    @Volatile var loseAck: ((List<Planner.Write>) -> Boolean)? = null
    val commits = AtomicInteger()
    val reads = AtomicInteger()

    private fun owner(token: String, path: String) {
        if (!path.startsWith("users/${token.removePrefix("token-")}/") && path != "users/${token.removePrefix("token-")}") throw CloudFailure(CloudFailure.Kind.DENIED)
    }

    override suspend fun <T> transaction(token: String, block: suspend (read: suspend (String) -> JSONObject?) -> Pair<T, List<Planner.Write>>): T {
        if (offline) throw CloudFailure(CloudFailure.Kind.OFFLINE)
        val seen = LinkedHashMap<String, JSONObject?>()
        val (result, writes) = block { path -> owner(token, path); reads.incrementAndGet(); synchronized(this) { documents[path]?.let(StrictJson::copy) }.also { seen[path] = it } }
        if (writes.isEmpty()) return result
        synchronized(this) {
            if (seen.any { (path, value) -> !StrictJson.equal(documents[path], value) }) throw CloudFailure(CloudFailure.Kind.UNAVAILABLE)
            for (write in writes) {
                owner(token, write.path)
                documents[write.path] = write.mask?.let { mask -> (documents[write.path]?.let(StrictJson::copy) ?: JSONObject()).also { document -> mask.forEach { document.put(it, write.data.get(it)) } } }
                    ?: StrictJson.copy(write.data)
            }
            commits.incrementAndGet()
        }
        if (loseAck?.invoke(writes) == true) { loseAck = null; throw CloudFailure(CloudFailure.Kind.OFFLINE) }
        return result
    }

    override suspend fun get(token: String, path: String): JSONObject? {
        if (offline) throw CloudFailure(CloudFailure.Kind.OFFLINE)
        owner(token, path); reads.incrementAndGet()
        return synchronized(this) { documents[path]?.let(StrictJson::copy) }
    }

    override suspend fun records(token: String, uid: String, category: String, after: String?, limit: Int): List<JSONObject> {
        if (offline) throw CloudFailure(CloudFailure.Kind.OFFLINE)
        owner(token, Planner.account(uid))
        return synchronized(this) {
            documents.filterKeys { it.startsWith("users/$uid/records/") }.values.filter { it.getString("category") == category }
                .sortedBy { it.getString("id") }.filter { after == null || it.getString("id") > after }.take(limit).map(StrictJson::copy)
        }.also { reads.addAndGet(it.size.coerceAtLeast(1)) }
    }

    fun records(uid: String): Map<String, JSONObject> = synchronized(this) {
        documents.filterKeys { it.startsWith("users/$uid/records/") }.values.associate { it.getString("id") to Planner.remoteRecord(it) }
    }

    /** Another device's change, through the same planner and commit path. */
    suspend fun remote(uid: String, op: JSONObject): Planner.Plan = transaction("token-$uid") { read -> Planner.plan(uid, op, read = read).let { it to it.writes } }

    /** The account document's consent, as another device or the trusted cleanup would change it. */
    fun consent(uid: String): JSONObject = synchronized(this) { documents.getValue(Planner.account(uid)).getJSONObject("consent") }
    fun setConsent(uid: String, consent: JSONObject) = synchronized(this) { documents.getValue(Planner.account(uid)).put("consent", consent) }
}
