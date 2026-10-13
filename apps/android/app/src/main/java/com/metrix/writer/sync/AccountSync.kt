package com.metrix.writer.sync

import android.content.Context
import android.net.ConnectivityManager
import android.net.Network
import android.util.AtomicFile
import com.metrix.writer.account.AccountKey
import com.metrix.writer.account.AccountSession
import com.metrix.writer.account.AuthError
import com.metrix.writer.account.AuthEvent
import com.metrix.writer.account.AuthFailure
import com.metrix.writer.account.AuthState
import com.metrix.writer.account.StaleAccountException
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.io.FileNotFoundException
import java.security.MessageDigest
import java.util.UUID
import java.util.concurrent.atomic.AtomicBoolean

/** A record both this phone and the account changed. [kind] is changed, or deleted when the account deleted it. */
data class SyncConflict(val recordId: String, val category: String, val kind: String, val title: String, val phone: String, val cloud: String)

/** What the account page shows. [problem] is a fixed kind, never an upstream message. */
data class SyncStatus(
    val account: AccountKey? = null,
    val enabled: Set<String> = emptySet(),
    val paused: Boolean = false,
    val running: Boolean = false,
    val pending: Int = 0,
    val conflicts: List<SyncConflict> = emptyList(),
    val failed: Int = 0,
    val detached: Int = 0,
    val quarantined: Int = 0,
    val lastAckMs: Long = 0,
    val problem: String? = null,
)

/** One row of the enable/restore preview. [action] is same, download, preserve, upload, conflict or deleted. */
data class MergeItem(val id: String, val category: String, val action: String, val title: String, val detail: String, val reason: String? = null)

/** The local/cloud comparison shown before anything is uploaded or restored. */
class MergePreview internal constructor(
    val account: AccountKey,
    val categories: Set<String>,
    val cloudHasAccount: Boolean,
    val items: List<MergeItem>,
    internal val cloud: Map<String, JSONObject>,
) {
    fun count(category: String, vararg actions: String) = items.count { it.category == category && it.action in actions }
    val choices get() = items.filter { it.action == "conflict" || it.action == "deleted" }
    val restorable get() = items.any { it.action == "download" || it.action == "preserve" || it.action == "conflict" }
}

/**
 * Settings and dictionary sync for Android (MW-OUATH-7), following the MW-OUATH-1 contract and the
 * MW-OUATH-5 Firestore protocol. Nothing is uploaded until the user previews and confirms a
 * category for the signed-in account. Local stores stay authoritative: every change is saved on
 * the phone first, then queued in a durable outbox bound to one account UID, epoch and consent
 * generation. Sync never runs transcription, provider work, model downloads or text insertion,
 * and dictation never waits on it.
 *
 * Per-account state (outbox, acknowledged revisions, conflicts) lives in its own file keyed by a
 * hash of the Firebase project and UID, so account B never reads or sends account A's work.
 */
class AccountSync(
    context: Context,
    private val session: AccountSession,
    private val cloudFor: (String) -> CloudStore,
    private val scope: CoroutineScope = CoroutineScope(SupervisorJob() + Dispatchers.IO),
    private val now: () -> Long = System::currentTimeMillis,
    private val pullIntervalMs: Long = 15 * 60_000L,
) {
    private val context = context.applicationContext
    private val store = PortableStore(this.context)
    private val directory = File(this.context.filesDir, "account-sync").apply { mkdirs() }
    private val mutex = Mutex()
    private val again = AtomicBoolean()
    private val pullWanted = AtomicBoolean(true)
    private val current = MutableStateFlow(SyncStatus())
    val status: StateFlow<SyncStatus> = current.asStateFlow()
    private var failures = 0
    private var retry: Job? = null
    private var started = false

    /** Starts listening for account events, local changes and connectivity. Idempotent. */
    fun start() {
        synchronized(this) { if (started) return; started = true }
        session.subscribe { event ->
            if (event is AuthEvent.SessionInvalidated) { retry?.cancel(); publish(null) }
            if (event is AuthEvent.Snapshot && event.snapshot.state == AuthState.SIGNED_IN) { pullWanted.set(true); kick() }
        }
        PortableChanges.listen { kick() }
        context.getSystemService(ConnectivityManager::class.java)?.registerDefaultNetworkCallback(object : ConnectivityManager.NetworkCallback() {
            override fun onAvailable(network: Network) { failures = 0; kick() }
        })
        scope.launch { while (true) { delay(pullIntervalMs); pullWanted.set(true); kick() } }
        kick()
    }

    /** Runs a sync pass soon. Passes never overlap; a request during one runs another afterwards. */
    fun kick(): Job {
        again.set(true)
        return scope.launch { mutex.withLock { while (again.getAndSet(false)) pass() } }
    }

    fun syncNow(): Job { pullWanted.set(true); failures = 0; return kick() }

    // ---- Per-account durable state ----------------------------------------

    private fun file(account: AccountKey): AtomicFile {
        val digest = MessageDigest.getInstance("SHA-256").digest("${account.projectId}\n${account.uid}".toByteArray(Charsets.UTF_8))
        return AtomicFile(File(directory, digest.joinToString("") { "%02x".format(it) } + ".json"))
    }

    private fun load(account: AccountKey): JSONObject? = try {
        JSONObject(String(file(account).readFully(), Charsets.UTF_8)).takeIf { it.getString("uid") == account.uid && it.getString("project_id") == account.projectId }
    } catch (_: FileNotFoundException) { null }

    private fun fresh(account: AccountKey) = JSONObject().put("schema", 1).put("project_id", account.projectId).put("uid", account.uid)
        .put("enabled", JSONArray()).put("consent", JSONObject.NULL).put("paused", false).put("base", JSONObject()).put("outbox", JSONArray())
        .put("conflicts", JSONArray()).put("failed", JSONObject()).put("detached", 0).put("quarantined", 0).put("last_ack_ms", 0L).put("last_pull_ms", 0L)

    private fun save(state: JSONObject, account: AccountKey) {
        val atomic = file(account)
        val out = atomic.startWrite()
        try { out.write(state.toString().toByteArray(Charsets.UTF_8)); atomic.finishWrite(out) } catch (error: Exception) { atomic.failWrite(out); throw error }
    }

    private fun JSONObject.enabled(): Set<String> = getJSONArray("enabled").let { array -> (0 until array.length()).map { array.getString(it) }.toSet() }
    private fun JSONObject.outbox(): MutableList<JSONObject> = getJSONArray("outbox").let { array -> (0 until array.length()).map { array.getJSONObject(it) }.toMutableList() }
    private fun JSONObject.setOutbox(rows: List<JSONObject>) = put("outbox", JSONArray(rows))
    private fun JSONObject.conflicts(): MutableList<JSONObject> = getJSONArray("conflicts").let { array -> (0 until array.length()).map { array.getJSONObject(it) }.toMutableList() }
    private fun JSONObject.consent(): JSONObject? = optJSONObject("consent")

    // ---- Status -----------------------------------------------------------

    private fun publish(problem: String?, running: Boolean = false) {
        val snapshot = session.snapshot()
        val account = snapshot.account
        val state = account?.let(::load)
        current.value = if (account == null || state == null) SyncStatus(account, problem = if (account == null) null else problem) else SyncStatus(
            account, state.enabled(), state.getBoolean("paused"), running, state.getJSONArray("outbox").length(),
            state.conflicts().map { conflict(it) }, state.getJSONObject("failed").length(), state.getInt("detached"), state.getInt("quarantined"),
            state.getLong("last_ack_ms"), problem ?: state.optString("problem").takeIf { it.isNotEmpty() },
        )
    }

    private fun conflict(row: JSONObject): SyncConflict {
        val local = row.getJSONObject("local")
        val cloud = row.getJSONObject("server").getJSONObject("content")
        fun describe(content: JSONObject): String = if (content.getString("state") == "deleted") "Deleted" else content.getJSONObject("data").let { data ->
            if (row.getString("category") == "settings") PortableStore.describe(data.getString("key"), data.get("value")) else PortableStore.entryTitle(data)
        }
        val live = listOf(local, cloud).firstOrNull { it.getString("state") == "live" }?.getJSONObject("data")
        val title = when {
            live == null -> "Entry"
            row.getString("category") == "settings" -> PortableStore.settingTitle(live.getString("key"))
            else -> PortableStore.kindName(live).replaceFirstChar { it.uppercase() }
        }
        return SyncConflict(row.getString("record_id"), row.getString("category"), row.getString("kind"), title, describe(local), describe(cloud))
    }

    /** Unsynced changes kept for [account]; for the sign-out and account-switch confirmations. */
    fun pendingFor(account: AccountKey): Int = load(account)?.let { it.getJSONArray("outbox").length() + it.getJSONArray("conflicts").length() } ?: 0

    // ---- Tokens -----------------------------------------------------------

    private suspend fun <T> authorized(account: AccountKey, generation: Long, block: suspend (String) -> T): T {
        val token = session.getFirebaseIdToken(account, generation, false)
        return try { block(token) } catch (failure: CloudFailure) {
            if (failure.kind != CloudFailure.Kind.UNAUTHENTICATED) throw failure
            block(session.getFirebaseIdToken(account, generation, true))
        }
    }

    private fun requireSignedIn(account: AccountKey? = null): Pair<AccountKey, Long> {
        val snapshot = session.snapshot()
        val signedIn = snapshot.account?.takeIf { snapshot.state == AuthState.SIGNED_IN } ?: throw StaleAccountException()
        if (account != null && account != signedIn) throw StaleAccountException()
        return signedIn to snapshot.generation
    }

    // ---- The sync pass ----------------------------------------------------

    private suspend fun pass() {
        val snapshot = session.snapshot()
        val account = snapshot.account
        if (account == null || snapshot.state != AuthState.SIGNED_IN) { publish(if (snapshot.state == AuthState.REAUTH_REQUIRED) "reauth" else null); return }
        val state = load(account)
        if (state == null || state.enabled().isEmpty() || state.getBoolean("paused")) { publish(null); return }
        publish(null, running = true)
        val cloud = cloudFor(account.projectId)
        try {
            // Queue first, offline or not: the outbox is durable before anything is sent.
            reconcile(state, account)
            refreshConsent(state, account, snapshot.generation, cloud)
            if (state.enabled().isNotEmpty()) {
                dispatch(state, account, snapshot.generation, cloud)
                if (pullWanted.getAndSet(false) || now() - state.getLong("last_pull_ms") >= pullIntervalMs) {
                    pull(state, account, snapshot.generation, cloud)
                    reconcile(state, account)
                    dispatch(state, account, snapshot.generation, cloud)
                }
            }
            // Transient problems end with a good pass; a remote turn-off stays until the user acts on it.
            if (state.optString("problem") !in setOf("turned_off", "account_unavailable")) state.remove("problem")
            save(state, account)
            failures = 0
            publish(null)
        } catch (error: Throwable) {
            if (error is CancellationException) throw error
            val problem = when (error) {
                is StaleAccountException -> null // The account changed; its work stays in its own file.
                is AuthFailure -> if (error.kind == AuthError.OFFLINE) "offline" else if (error.kind == AuthError.REAUTH_REQUIRED) "reauth" else "unavailable"
                is CloudFailure -> when (error.kind) {
                    CloudFailure.Kind.OFFLINE -> "offline"; CloudFailure.Kind.QUOTA -> "quota"; CloudFailure.Kind.NOT_SET_UP -> "not_set_up"
                    CloudFailure.Kind.DENIED -> "denied"; CloudFailure.Kind.UNAUTHENTICATED -> "reauth"; else -> "unavailable"
                }
                else -> "unavailable"
            }
            load(account)?.let { latest -> if (problem != null) { latest.put("problem", problem); save(latest, account) } }
            publish(problem)
            if (problem in setOf("offline", "unavailable", "quota")) scheduleRetry()
        }
    }

    // Capped exponential backoff; connectivity or a local change also retries sooner.
    private fun scheduleRetry() {
        failures++
        val wait = (5_000L shl (failures - 1).coerceAtMost(6)).coerceAtMost(5 * 60_000L)
        retry?.cancel()
        retry = scope.launch { delay(wait); kick() }
    }

    /** Re-reads the account's consent before each pass. A changed epoch or generation fences queued work; it is never resent. */
    private suspend fun refreshConsent(state: JSONObject, account: AccountKey, generation: Long, cloud: CloudStore) {
        val document = authorized(account, generation) { cloud.get(it, Planner.account(account.uid)) }
        requireSignedIn(account)
        val consent = document?.takeIf { it.optString("account_state") == "active" }?.getJSONObject("consent")
        val previous = state.consent()
        if (previous != null && consent != null && previous.getLong("account_epoch") != consent.getLong("account_epoch")) {
            // A new account epoch (account deletion/reset): nothing from the old one may be applied or sent.
            state.put("base", JSONObject()).put("conflicts", JSONArray()).put("failed", JSONObject())
        }
        state.put("consent", consent ?: JSONObject.NULL)
        val turnedOff = state.enabled().filter { category -> consent?.getJSONObject(category)?.getBoolean("enabled") != true }
        if (turnedOff.isNotEmpty()) {
            val fenced = state.outbox().count { it.getJSONObject("op").getString("category") in turnedOff }
            state.put("detached", state.getInt("detached") + fenced)
            state.put("enabled", JSONArray(state.enabled() - turnedOff.toSet()))
            forgetCategories(state, turnedOff.toSet())
            state.put("problem", if (document?.optString("account_state") == "active") "turned_off" else "account_unavailable")
        }
        fence(state)
        save(state, account)
    }

    /** Drops queued operations bound to an older epoch or generation, counting them. Never requeued. */
    private fun fence(state: JSONObject) {
        val consent = state.consent()
        val (keep, drop) = state.outbox().partition { entry ->
            val op = entry.getJSONObject("op")
            consent != null && op.getString("category") in state.enabled() && op.getLong("account_epoch") == consent.getLong("account_epoch") &&
                op.getLong("consent_generation") == consent.getJSONObject(op.getString("category")).getLong("generation")
        }
        if (drop.isNotEmpty()) state.setOutbox(keep).put("detached", state.getInt("detached") + drop.size)
    }

    private fun forgetCategories(state: JSONObject, categories: Set<String>) {
        val base = state.getJSONObject("base")
        base.keys().asSequence().toList().filter { base.getJSONObject(it).getString("category") in categories }.forEach(base::remove)
        state.setOutbox(state.outbox().filter { it.getJSONObject("op").getString("category") !in categories })
        state.put("conflicts", JSONArray(state.conflicts().filter { it.getString("category") !in categories }))
        val failed = state.getJSONObject("failed")
        failed.keys().asSequence().toList().filter { failed.getJSONObject(it).getString("category") in categories }.forEach(failed::remove)
    }

    private fun live(data: JSONObject) = JSONObject().put("state", "live").put("data", data)
    private val deleted get() = JSONObject().put("state", "deleted")

    /**
     * Turns differences between this phone and the last acknowledged account state into queued
     * operations. An operation already sent is never changed: a lost response is retried exactly.
     */
    private fun reconcile(state: JSONObject, account: AccountKey) {
        val consent = state.consent() ?: return
        val projection = store.projection()
        val base = state.getJSONObject("base")
        val outbox = state.outbox()
        val conflicts = state.conflicts().map { it.getString("record_id") }.toSet()
        val failed = state.getJSONObject("failed")
        for (category in state.enabled()) {
            val ids = projection.values.filter { it.category == category }.map { it.id } +
                base.keys().asSequence().filter { base.getJSONObject(it).getString("category") == category }
            for (id in ids.toSortedSet()) {
                if (id in conflicts) continue
                val local = projection[id]
                val known = base.optJSONObject(id)
                val desired = when {
                    local != null -> live(local.data)
                    known != null && category == "dictionary" && known.getJSONObject("content").getString("state") == "live" -> deleted
                    else -> null
                } ?: continue
                // A change the account refused is not retried unchanged; a new edit tries again.
                val refused = failed.optJSONObject(id)
                if (refused != null && StrictJson.equal(refused.get("content"), desired)) continue
                failed.remove(id)
                if (local != null && !Contract.isData(category, id, local.data)) {
                    failed.put(id, JSONObject().put("category", category).put("reason", "invalid_record").put("content", desired)); continue
                }
                val pending = outbox.find { it.getJSONObject("op").getString("record_id") == id }
                if (known != null && StrictJson.equal(known.get("content"), desired)) {
                    if (pending != null && !pending.getBoolean("sent")) outbox.remove(pending)
                    continue
                }
                if (known != null && known.getJSONObject("content").getString("state") == "deleted") {
                    if (category == "dictionary") {
                        state.getJSONArray("conflicts").put(JSONObject().put("record_id", id).put("category", category).put("kind", "deleted")
                            .put("local", desired).put("server", serverRecord(id, known)))
                        continue
                    }
                    // A fixed setting ID comes back only in a newer, confirmed generation.
                    if (known.optLong("generation") >= consent.getJSONObject(category).getLong("generation")) continue
                }
                if (pending != null) {
                    if (pending.getBoolean("sent") || StrictJson.equal(pending.getJSONObject("op").get("content"), desired)) continue
                    outbox.remove(pending)
                }
                val op = JSONObject().put("schema_version", 1L).put("uid", account.uid).put("account_epoch", consent.getLong("account_epoch"))
                    .put("consent_generation", consent.getJSONObject(category).getLong("generation")).put("category", category)
                    .put("operation_id", UUID.randomUUID().toString()).put("device_id", store.deviceId).put("record_id", id)
                    .put("origin_device_id", known?.getString("origin") ?: local!!.origin).put("base_revision", known?.getLong("revision") ?: 0L)
                    .put("content", desired)
                outbox += JSONObject().put("op", op).put("sent", false)
            }
        }
        state.setOutbox(outbox.filter { it.getJSONObject("op").getString("category") in state.enabled() })
        save(state, account)
    }

    private fun serverRecord(id: String, known: JSONObject) = JSONObject().put("schema_version", 1L).put("id", id).put("category", known.getString("category"))
        .put("origin_device_id", known.getString("origin")).put("revision", known.getLong("revision")).put("content", known.get("content"))

    private fun remember(state: JSONObject, record: JSONObject, generation: Long) {
        state.getJSONObject("base").put(record.getString("id"), JSONObject().put("category", record.getString("category"))
            .put("origin", record.getString("origin_device_id")).put("revision", record.getLong("revision"))
            .put("content", record.get("content")).put("generation", generation))
    }

    /** Sends queued operations in order, each as one Firestore transaction planned by [Planner]. */
    private suspend fun dispatch(state: JSONObject, account: AccountKey, generation: Long, cloud: CloudStore) {
        while (true) {
            fence(state)
            val entry = state.outbox().firstOrNull() ?: break
            val op = entry.getJSONObject("op")
            // Recorded before sending: an unanswered request is only ever retried unchanged.
            if (!entry.getBoolean("sent")) { entry.put("sent", true); state.setOutbox(listOf(entry) + state.outbox().drop(1)); save(state, account) }
            requireSignedIn(account)
            val plan = authorized(account, generation) { token -> cloud.transaction(token) { read -> Planner.plan(account.uid, op, read = read).let { it to it.writes } } }
            // The response applies only to the same account, epoch, generation and operation.
            requireSignedIn(account)
            val latest = state.outbox().firstOrNull()?.getJSONObject("op")
            if (latest?.getString("operation_id") != op.getString("operation_id")) continue
            val recordId = op.getString("record_id")
            val category = op.getString("category")
            state.setOutbox(state.outbox().drop(1))
            when (plan.status) {
                "acknowledged" -> { remember(state, plan.record!!, op.getLong("consent_generation")); state.put("last_ack_ms", now()) }
                "conflict" -> {
                    val server = plan.current!!
                    if (StrictJson.equal(server.get("content"), op.get("content"))) remember(state, server, op.getLong("consent_generation"))
                    else state.getJSONArray("conflicts").put(JSONObject().put("record_id", recordId).put("category", category).put("kind", "changed")
                        .put("local", op.get("content")).put("server", server))
                }
                "deleted" -> {
                    val tombstone = authorized(account, generation) { cloud.get(it, Planner.record(account.uid, recordId)) }?.let(Planner::remoteRecord)
                    if (tombstone == null || category != "dictionary") state.getJSONObject("failed").put(recordId, JSONObject().put("category", category).put("reason", "deleted").put("content", op.get("content")))
                    else if (op.getJSONObject("content").getString("state") == "deleted") remember(state, tombstone, op.getLong("consent_generation"))
                    else state.getJSONArray("conflicts").put(JSONObject().put("record_id", recordId).put("category", category).put("kind", "deleted")
                        .put("local", op.get("content")).put("server", tombstone))
                }
                "stale_generation", "consent_required", "stale_epoch", "account_unavailable" -> {
                    state.setOutbox(listOf(entry) + state.outbox()) // fenced by the refresh below, never resent
                    refreshConsent(state, account, generation, cloud)
                    if (state.outbox().firstOrNull()?.getJSONObject("op")?.getString("operation_id") == op.getString("operation_id")) {
                        state.setOutbox(state.outbox().drop(1)).put("detached", state.getInt("detached") + 1)
                    }
                }
                "invalid_base" -> state.getJSONObject("base").remove(recordId)
                "quota_exceeded" -> { state.setOutbox(listOf(entry) + state.outbox()); save(state, account); throw CloudFailure(CloudFailure.Kind.QUOTA) }
                else -> state.getJSONObject("failed").put(recordId, JSONObject().put("category", category).put("reason", plan.status).put("content", op.get("content")))
            }
            save(state, account)
        }
    }

    /** Pages through the account's records (500 at a time, no listener) and applies changes this phone hasn't edited. */
    private suspend fun pull(state: JSONObject, account: AccountKey, generation: Long, cloud: CloudStore) {
        val remote = fetch(account, generation, cloud, state.enabled())
        requireSignedIn(account)
        refreshConsent(state, account, generation, cloud)
        val projection = store.projection()
        val base = state.getJSONObject("base")
        val busy = state.outbox().map { it.getJSONObject("op").getString("record_id") }.toSet() + state.conflicts().map { it.getString("record_id") }
        val downloads = mutableListOf<LocalRecord>()
        val deletions = mutableSetOf<String>()
        val adopted = mutableListOf<Pair<JSONObject, Long>>()
        var quarantined = 0
        for ((id, document) in remote.entries) {
            if (document == null) { quarantined++; continue }
            val record = Planner.remoteRecord(document)
            val category = record.getString("category")
            if (category !in state.enabled() || id in busy) continue
            val known = base.optJSONObject(id)
            if (known != null && record.getLong("revision") <= known.getLong("revision")) continue
            val content = record.getJSONObject("content")
            val isLive = content.getString("state") == "live"
            val local = projection[id]
            val unchanged = if (known == null) local == null else {
                val knownContent = known.getJSONObject("content")
                if (local != null) knownContent.getString("state") == "live" && StrictJson.equal(knownContent.getJSONObject("data"), local.data)
                else knownContent.getString("state") == "deleted" || category == "settings"
            }
            when {
                known == null && local != null && isLive && StrictJson.equal(local.data, content.getJSONObject("data")) -> Unit
                known == null && local != null -> {
                    state.getJSONArray("conflicts").put(JSONObject().put("record_id", id).put("category", category).put("kind", if (isLive) "changed" else "deleted")
                        .put("local", live(local.data)).put("server", record))
                    continue
                }
                !unchanged -> continue // This phone's edit is queued against the old revision and will conflict.
                isLive -> downloads += LocalRecord(id, category, record.getString("origin_device_id"), content.getJSONObject("data"))
                local != null && category == "dictionary" -> deletions += id
            }
            adopted += record to document.optLong("consent_generation")
        }
        requireSignedIn(account)
        if (downloads.isNotEmpty() || deletions.isNotEmpty()) store.applyRemote(downloads, deletions)
        adopted.forEach { (record, recordGeneration) -> remember(state, record, recordGeneration) }
        state.put("quarantined", quarantined).put("last_pull_ms", now())
        save(state, account)
        acknowledgeDevice(state, account, generation, cloud)
    }

    /** Every settings/dictionary record of [categories], by ID; null for a record this version can't read. */
    private suspend fun fetch(account: AccountKey, generation: Long, cloud: CloudStore, categories: Set<String>): Map<String, JSONObject?> {
        val result = LinkedHashMap<String, JSONObject?>()
        for (category in categories.sorted()) {
            var after: String? = null
            while (true) {
                val page = authorized(account, generation) { cloud.records(it, account.uid, category, after, Planner.MAX_LIST_LIMIT) }
                for (document in page) {
                    val id = document.optString("id")
                    // A record from a newer client, or anything malformed, is kept in the cloud untouched and never applied.
                    result[id] = try { Contract.checkRecord(Planner.remoteRecord(document)); document } catch (_: Exception) { null }
                }
                if (page.size < Planner.MAX_LIST_LIMIT) break
                after = page.last().optString("id").takeIf(Contract::isUuid) ?: break
            }
        }
        return result
    }

    /** Records this device's reconciled generations (MW-OUATH-5 DeviceSyncState). Best effort; never blocks sync. */
    private suspend fun acknowledgeDevice(state: JSONObject, account: AccountKey, generation: Long, cloud: CloudStore) {
        val consent = state.consent() ?: return
        try {
            authorized(account, generation) { token -> cloud.transaction(token) { read ->
                val path = Planner.device(account.uid, store.deviceId)
                val previous = read(path)?.takeIf { it.optLong("account_epoch") == consent.getLong("account_epoch") }?.getJSONObject("reconciled_generations")
                val seen = JSONObject()
                for (category in listOf("settings", "dictionary", "transcripts")) {
                    val mine = if (category in state.enabled()) consent.getJSONObject(category).getLong("generation") else 1L
                    seen.put(category, maxOf(mine, previous?.optLong(category) ?: 1L).coerceAtMost(consent.getJSONObject(category).getLong("generation")))
                }
                val document = JSONObject().put("schema_version", 1L).put("uid", account.uid).put("account_epoch", consent.getLong("account_epoch"))
                    .put("device_id", store.deviceId).put("reconciled_generations", seen)
                Unit to listOf(Planner.Write(path, document))
            } }
        } catch (failure: CloudFailure) { if (failure.kind == CloudFailure.Kind.OFFLINE) throw failure }
    }

    // ---- Explicit user actions (Settings → Account) ------------------------

    /** Compares this phone with the account for [categories]. Reads only; uploads and changes nothing. */
    suspend fun preview(categories: Set<String>): MergePreview = mutex.withLock {
        require(categories.isNotEmpty() && categories.all { it in setOf("settings", "dictionary") })
        val (account, generation) = requireSignedIn()
        val cloud = cloudFor(account.projectId)
        val document = authorized(account, generation) { cloud.get(it, Planner.account(account.uid)) }
        val remote = if (document == null) emptyMap() else fetch(account, generation, cloud, categories)
        requireSignedIn(account)
        val local = store.projection()
        val adapter = PortableAdapter(context)
        val items = mutableListOf<MergeItem>()
        for ((id, record) in remote) {
            if (record == null) continue
            val category = record.getString("category")
            val content = record.getJSONObject("content")
            val mine = local[id]
            if (content.getString("state") == "deleted") {
                if (mine != null && category == "dictionary") items += MergeItem(id, category, "deleted", PortableStore.entryTitle(mine.data), "Deleted from your account on another device")
                continue
            }
            val data = content.getJSONObject("data")
            val title = if (category == "settings") PortableStore.settingTitle(data.getString("key")) else PortableStore.entryTitle(data)
            items += when {
                mine != null && StrictJson.equal(mine.data, data) -> MergeItem(id, category, "same", title, "Same on both")
                mine != null -> MergeItem(id, category, "conflict", title, if (category == "settings")
                    "This phone: ${PortableStore.describe(data.getString("key"), mine.data.get("value"))} · Account: ${PortableStore.describe(data.getString("key"), data.get("value"))}"
                    else "This phone: ${PortableStore.entryTitle(mine.data)} · Account: ${PortableStore.entryTitle(data)}")
                category == "settings" -> adapter.unsupported(data.getString("key"), data.get("value"))?.let {
                    MergeItem(id, category, "preserve", title, PortableStore.describe(data.getString("key"), data.get("value")), it)
                } ?: MergeItem(id, category, "download", title, PortableStore.describe(data.getString("key"), data.get("value")))
                else -> MergeItem(id, category, "download", title, "Add ${PortableStore.kindName(data)}")
            }
        }
        for (record in local.values.filter { it.category in categories && it.id !in remote }) {
            val title = if (record.category == "settings") PortableStore.settingTitle(record.data.getString("key")) else PortableStore.entryTitle(record.data)
            val detail = if (record.category == "settings") PortableStore.describe(record.data.getString("key"), record.data.get("value")) else "Upload ${PortableStore.kindName(record.data)}"
            items += MergeItem(record.id, record.category, "upload", title, detail)
        }
        MergePreview(account, categories, document != null, items.filter { it.category in categories }, remote.filterValues { it != null }.mapValues { it.value!! })
    }

    /**
     * Applies the account's side of [preview] to this phone, atomically and with an undo snapshot.
     * [choices] answers each conflict ("phone" or "account") and each deleted entry ("keep" or "remove").
     * [disposition] restore_once leaves sync off and uploads nothing.
     */
    private fun restoreLocally(preview: MergePreview, choices: Map<String, String>, disposition: String): JSONObject? {
        val incoming = preview.items.filter { it.action in setOf("download", "preserve") || (it.action == "conflict" && choices[it.id] == "account") }
            .mapNotNull { item -> preview.cloud[item.id]?.let { LocalRecord(item.id, item.category, it.getString("origin_device_id"), it.getJSONObject("content").getJSONObject("data")) } }
        val receipt = if (incoming.isEmpty()) null else {
            val plan = store.preview(incoming, UUID.randomUUID().toString(), preview.categories, "merge", disposition, "account")
            store.apply(plan.importId, plan.conflicts.associate { it.id to false }).also {
                check(it.getString("state") == "applied") { if (it.optString("reason") == "stale_preview") "Something on this phone changed while restoring. Review the preview again." else "Could not save the restored settings. Nothing was changed." }
            }
        }
        val removals = preview.items.filter { it.action == "deleted" && choices[it.id] == "remove" }.map { it.id }.toSet()
        if (removals.isNotEmpty()) store.applyRemote(emptyList(), removals)
        preview.items.filter { it.action == "deleted" && choices[it.id] == "keep" }.forEach { store.reidentify(it.id) }
        return receipt
    }

    private fun checkChoices(preview: MergePreview, choices: Map<String, String>) {
        for (item in preview.choices) require(choices[item.id] in if (item.action == "conflict") setOf("phone", "account") else setOf("keep", "remove")) { "Choose what to keep for every item first." }
    }

    /** Restore once: copies the account's settings/dictionary onto this phone. Sync stays off; nothing is uploaded. */
    suspend fun restoreOnce(preview: MergePreview, choices: Map<String, String>): JSONObject? = mutex.withLock {
        checkChoices(preview, choices)
        requireSignedIn(preview.account)
        restoreLocally(preview, choices, "restore_once")
    }

    /**
     * Restore and keep syncing: applies the account's side, then turns the categories on for this
     * account (creating its all-off account document if needed, then one consent CAS), and queues
     * this phone's remaining differences for upload.
     */
    suspend fun enable(preview: MergePreview, choices: Map<String, String>) {
        mutex.withLock {
            checkChoices(preview, choices)
            val (account, generation) = requireSignedIn(preview.account)
            val cloud = cloudFor(account.projectId)
            restoreLocally(preview, choices, "restore_and_sync")
            val path = Planner.account(account.uid)
            authorized(account, generation) { token -> cloud.transaction(token) { read ->
                Unit to if (read(path) == null) listOf(Planner.Write(path, Planner.initialAccount(account.uid))) else emptyList()
            } }
            val consent = authorized(account, generation) { token -> cloud.transaction(token) { read ->
                val document = read(path) ?: throw CloudFailure(CloudFailure.Kind.UNAVAILABLE)
                if (document.optString("account_state") != "active") throw CloudFailure(CloudFailure.Kind.DENIED)
                val before = document.getJSONObject("consent")
                var next = before
                for (category in preview.categories) if (!next.getJSONObject(category).getBoolean("enabled")) next = Planner.nextConsent(next, category, true)
                // One consent write advances the revision by exactly one, whatever it enables.
                next.put("revision", before.getLong("revision") + if (next === before) 0 else 1)
                next to if (next === before) emptyList() else listOf(Planner.Write(path, JSONObject().put("consent", next), listOf("consent")))
            } }
            requireSignedIn(account)
            val state = load(account) ?: fresh(account)
            if (state.consent()?.optLong("account_epoch") != consent.getLong("account_epoch")) state.put("base", JSONObject()).put("conflicts", JSONArray())
            state.put("consent", consent).put("enabled", JSONArray(state.enabled() + preview.categories)).put("paused", false).remove("problem")
            forgetCategories(state, preview.categories)
            for ((_, record) in preview.cloud) if (record.getString("category") in preview.categories) remember(state, Planner.remoteRecord(record), record.optLong("consent_generation"))
            save(state, account)
        }
        syncNow()
    }

    /** Turns [category] off for this account: one consent CAS, then queued sends for it are cancelled. Cloud data stays. */
    suspend fun disable(category: String) {
        mutex.withLock {
            val (account, generation) = requireSignedIn()
            val cloud = cloudFor(account.projectId)
            val path = Planner.account(account.uid)
            val consent = authorized(account, generation) { token -> cloud.transaction(token) { read ->
                val document = read(path) ?: return@transaction JSONObject.NULL to emptyList()
                val before = document.getJSONObject("consent")
                if (document.optString("account_state") != "active" || !before.getJSONObject(category).getBoolean("enabled")) before to emptyList()
                else Planner.nextConsent(before, category, false).let { it to listOf(Planner.Write(path, JSONObject().put("consent", it), listOf("consent"))) }
            } }
            val state = load(account) ?: return@withLock
            state.put("consent", consent).put("enabled", JSONArray(state.enabled() - category))
            forgetCategories(state, setOf(category))
            save(state, account)
        }
        publish(null)
    }

    /** Pause on this phone only: stops sending and pulling. Account-wide consent and cloud data are unchanged. */
    suspend fun setPaused(paused: Boolean) {
        mutex.withLock {
            val (account, _) = requireSignedIn()
            val state = load(account) ?: return@withLock
            state.put("paused", paused); save(state, account)
        }
        if (paused) publish(null) else syncNow()
    }

    /** Resolves one conflict: "phone" keeps this phone's value and sends it; "account" takes the account's; for a deleted entry, "keep" re-adds it as a new entry and "remove" removes it here. */
    suspend fun resolve(recordId: String, choice: String) {
        mutex.withLock {
            val (account, _) = requireSignedIn()
            val state = load(account) ?: return@withLock
            val conflicts = state.conflicts()
            val row = conflicts.find { it.getString("record_id") == recordId } ?: return@withLock
            val server = row.getJSONObject("server")
            val content = server.getJSONObject("content")
            when (choice) {
                "phone", "keep" -> if (content.getString("state") == "deleted" && row.getString("category") == "dictionary") store.reidentify(recordId)
                "account", "remove" -> if (content.getString("state") == "live") {
                    store.applyRemote(listOf(LocalRecord(recordId, row.getString("category"), server.getString("origin_device_id"), content.getJSONObject("data"))), emptySet())
                } else if (row.getString("category") == "dictionary") store.applyRemote(emptyList(), setOf(recordId))
                else throw IllegalArgumentException("unknown choice")
            }
            remember(state, server, state.getJSONObject("base").optJSONObject(recordId)?.optLong("generation") ?: 0L)
            state.put("conflicts", JSONArray(conflicts - row))
            save(state, account)
        }
        kick()
    }

    /**
     * Hook for MW-OUATH-8 (remove this device's copy, cloud deletion): deletes this phone's sync
     * cache for [account] — its acknowledged revisions, conflicts and unsent outbox. Local settings
     * and dictionary are untouched.
     */
    fun forgetAccount(account: AccountKey) { file(account).delete(); publish(null) }
}
