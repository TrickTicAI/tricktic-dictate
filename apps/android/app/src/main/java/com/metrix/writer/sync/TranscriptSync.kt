package com.metrix.writer.sync

import android.content.Context
import android.net.ConnectivityManager
import android.net.Network
import android.util.AtomicFile
import com.metrix.writer.account.*
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.util.UUID
import java.util.concurrent.atomic.AtomicLong

/** Immutable text versions only. Account workspaces never become native recording/retry stores. */
class TranscriptSync(
    private val context: Context,
    private val session: AccountSession,
    private val cloudFor: (String) -> CloudStore,
    private val scope: CoroutineScope = CoroutineScope(SupervisorJob() + Dispatchers.IO),
    // Owner-approved Firebase trust model; each account still needs explicit transcript enrollment.
    val uploadsAllowed: Boolean = true,
    private val pageSize: Int = 100,
    private val autoContinue: Boolean = true,
) : AccountParticipant {
    data class Status(val enabled: Boolean = false, val pending: Int = 0, val problem: String? = null,
                      val records: List<JSONObject> = emptyList(), val moreWork: Boolean = false, val next: String? = null, val skipped: Int = 0, val account: AccountKey? = null)
    private val journal = TranscriptStore(context)
    private val workspace = AccountWorkspace(context.filesDir)
    private val mutex = Mutex()
    private val fence = AtomicLong()
    private val current = MutableStateFlow(Status())
    val status = current.asStateFlow()
    private var started = false
    private var retry: Job? = null
    private var failures = 0
    private fun file(account: AccountKey) = AtomicFile(File(workspace.directory(account), "transcripts.json"))
    private fun fresh() = JSONObject().put("enabled", false).put("records", JSONObject()).put("base", JSONObject())
        .put("outbox", JSONObject()).put("excluded", JSONObject()).put("hidden", JSONObject()).put("failed", JSONObject()).put("skipped", 0).put("sent", JSONObject())
    private val databases = mutableMapOf<AccountKey, TranscriptDatabase>()
    private fun database(account: AccountKey) = databases.getOrPut(account) { TranscriptDatabase(file(account), setOf("records", "base", "outbox", "excluded", "hidden", "failed", "sent")) }
    private fun load(account: AccountKey) = database(account).load(::fresh)
    private fun save(account: AccountKey, state: JSONObject) = database(account).save(state)
    private fun active(): Pair<AccountKey, Long> = session.snapshot().let {
        (it.account?.takeIf { _ -> it.state == AuthState.SIGNED_IN } ?: throw StaleAccountException()) to it.generation
    }
    private fun check(account: AccountKey, generation: Long, localFence: Long) {
        if (!session.isCurrent(account, generation) || fence.get() != localFence) throw StaleAccountException()
    }
    private suspend fun <T> authorized(account: AccountKey, generation: Long, localFence: Long, action: suspend (String) -> T): T {
        check(account, generation, localFence)
        val token = session.getFirebaseIdToken(account, generation, false)
        val result = try { action(token) } catch (error: CloudFailure) {
            if (error.kind != CloudFailure.Kind.UNAUTHENTICATED) throw error
            check(account, generation, localFence)
            action(session.getFirebaseIdToken(account, generation, true))
        }
        check(account, generation, localFence)
        return result
    }

    fun start() {
        if (started) return
        started = true
        session.register(this)
        TranscriptStore.owner = { session.snapshot().account }
        session.subscribe { event ->
            if (event is AuthEvent.SessionInvalidated) { retry?.cancel(); current.value = Status() }
            if (event is AuthEvent.Snapshot) kick()
        }
        PortableChanges.listen { kick() }
        context.getSystemService(ConnectivityManager::class.java)?.registerDefaultNetworkCallback(object : ConnectivityManager.NetworkCallback() {
            override fun onAvailable(network: Network) { kick() }
        })
        scope.launch { while (true) { delay(15 * 60_000L); kick() } }
        kick()
    }
    fun kick(): Job = scope.launch { runOnce() }

    private var visibleLimit = 100
    suspend fun loadMore() = mutex.withLock {
        val (account, _) = active(); visibleLimit += 100; publish(account, load(account))
    }

    private fun continueLater() { if (autoContinue) kick() }
    private fun publish(account: AccountKey, state: JSONObject, problem: String? = null) {
        if (session.snapshot().account != account) return
        // Publish a bounded prefix. Explicit Load more increases the visible window.
        val page = database(account).page("records", null, visibleLimit + 1)
        current.value = Status(enabled = state.optBoolean("enabled"), pending = state.getJSONObject("outbox").length(), problem = problem,
            records = page.take(visibleLimit), moreWork = state.optString("poll_cursor").isNotEmpty() || state.optBoolean("local_more") || state.getJSONObject("outbox").length() > 0, next = page.getOrNull(visibleLimit)?.getString("id"), skipped = state.optInt("skipped"), account = account)
    }

    /** Explicit per-device enrollment; future-only excludes existing attempt IDs, even later cleanup. */
    suspend fun enable(mode: String) = mutex.withLock {
        check(uploadsAllowed) { "Transcript uploads are disabled in this build." }
        require(mode in setOf("future_only", "include_existing"))
        val (account, generation) = active(); val localFence = fence.get()
        journal.scan()
        val cloud = cloudFor(account.projectId)
        val state = synchronized(TranscriptStore.LOCK) {
            val next = load(account)
            if (!next.has("enrollment") || next.optString("mode") != mode || next.optBoolean("enabled")) {
                next.put("enrollment", UUID.randomUUID().toString()).put("mode", mode).put("enabled", false)
                    .put("excluded", JSONObject()).put("outbox", JSONObject()).put("local_cursor", 0L).put("delete_cursor", 0L)
                if (mode == "future_only") {
                    journal.records().forEach { (id, row) -> next.getJSONObject("excluded").put(id, true)
                        if (row.getJSONObject("data").optString("kind") == "original") next.getJSONObject("excluded").put(row.getJSONObject("data").getString("attempt_id"), true)
                    }
                    // Exclude even unsuccessful/in-flight attempts belonging to already recorded audio.
                    File(context.filesDir, "recordings").listFiles().orEmpty().filter { it.name.endsWith(".json") }.forEach { f ->
                        next.getJSONObject("excluded").put(f.name.removeSuffix(".json"), true)
                    }
                }
                save(account, next) // Enrollment identity survives a lost consent acknowledgement.
            }
            next
        }
        authorized(account, generation, localFence) { token -> cloud.transaction(token) { read ->
            val path = Planner.account(account.uid)
            val document = read(path)
            if (document == null) Planner.initialAccount(account.uid).let { it.getJSONObject("consent") to listOf(Planner.Write(path, it)) }
            else {
                if (document.optString("account_state") != "active") throw CloudFailure(CloudFailure.Kind.DENIED)
                document.getJSONObject("consent") to emptyList()
            }
        } }
        val chosen = authorized(account, generation, localFence) { token -> cloud.transaction(token) { read ->
            val doc = read(Planner.account(account.uid)) ?: throw CloudFailure(CloudFailure.Kind.UNAVAILABLE)
            if (doc.optString("account_state") != "active") throw CloudFailure(CloudFailure.Kind.DENIED)
            val before = doc.getJSONObject("consent")
            // Join the shared enrollment when another device already enabled this category.
            val enabled = before.getJSONObject("transcripts").getBoolean("enabled")
            val selected = if (enabled) before else Planner.nextConsent(before, "transcripts", true,
                JSONObject().put("mode", mode).put("enrollment_id", state.getString("enrollment")))
            selected to if (enabled) emptyList() else listOf(Planner.Write(Planner.account(account.uid), JSONObject().put("consent", selected), listOf("consent")))
        } }
        synchronized(TranscriptStore.LOCK) {
            check(account, generation, localFence)
            state.put("consent", chosen).put("enabled", true).put("enrollment", chosen.getJSONObject("transcripts").getJSONObject("history").getString("enrollment_id"))
            state.remove("revoke")
            save(account, state); publish(account, state)
        }
    }

    /** Off immediately on this phone, including offline. Account-wide consent is revoked on reconnect. */
    suspend fun disable() = mutex.withLock {
        val (account, _) = active()
        synchronized(TranscriptStore.LOCK) {
            val state = load(account).put("enabled", false).put("revoke", true).put("outbox", JSONObject())
            save(account, state); publish(account, state)
        }
    }

    suspend fun runOnce(): Unit = mutex.withLock {
        val (account, generation) = try { active() } catch (_: StaleAccountException) { current.value = Status(); return@withLock }
        val localFence = fence.get()
        val state = synchronized(TranscriptStore.LOCK) { load(account) }
        if (!state.optBoolean("enabled") && !state.optBoolean("revoke")) { publish(account, state); return@withLock }
        if (!uploadsAllowed) { publish(account, state, "privacy_hold"); return@withLock }
        val cloud = cloudFor(account.projectId)
        fun commit() = synchronized(TranscriptStore.LOCK) { check(account, generation, localFence); save(account, state) }
        try {
            journal.scan()
            collect(state, account)
            commit() // Durable desired records and operations, including while offline.
            val doc = authorized(account, generation, localFence) { cloud.get(it, Planner.account(account.uid)) }
            val consent = doc?.getJSONObject("consent")
            val toggle = consent?.getJSONObject("transcripts")
            val old = state.optJSONObject("consent")
            val reset = doc?.optJSONObject("resets")?.optJSONObject("transcripts")?.optLong("generation") ?: 0L
            if (reset > state.optLong("reset")) {
                state.put("reset", reset)
                removeDownloads(state)
            }
            if (state.optBoolean("revoke")) {
                if (toggle?.optBoolean("enabled") == true) authorized(account, generation, localFence) { token -> cloud.transaction(token) { read ->
                    val before = read(Planner.account(account.uid))!!.getJSONObject("consent")
                    Unit to listOf(Planner.Write(Planner.account(account.uid), JSONObject().put("consent", Planner.nextConsent(before, "transcripts", false)), listOf("consent")))
                } }
                state.remove("revoke"); commit(); publish(account, state); return@withLock
            }
            if (doc?.optString("account_state") != "active" || toggle?.optBoolean("enabled") != true ||
                old?.optLong("account_epoch") != consent.optLong("account_epoch") ||
                old.getJSONObject("transcripts").optLong("generation") != toggle.optLong("generation")) {
                state.put("enabled", false).put("outbox", JSONObject()); commit(); publish(account, state, "turned_off"); return@withLock
            }
            val queue = state.getJSONObject("outbox")
            // Originals before versions; insertion order keeps selected parents before later children.
            val ordered = database(account).keys("outbox", 50).map { queue.getJSONObject(it) }.sortedBy { it.getJSONObject("content").optJSONObject("data")?.optString("kind") != "original" }
            for (op in ordered.take(50)) {
                val queuedId = op.getString("record_id")
                if (state.getJSONObject("hidden").has(queuedId) && op.getJSONObject("content").optString("state") == "live" &&
                    !state.getJSONObject("sent").has(op.getString("operation_id"))) { queue.remove(queuedId); commit(); continue }
                state.getJSONObject("sent").put(op.getString("operation_id"), true); commit()
                val result = authorized(account, generation, localFence) { token -> cloud.transaction(token) { read ->
                    Planner.plan(account.uid, op, true, read).let { it to it.writes }
                } }
                val id = op.getString("record_id")
                when (result.status) {
                    "acknowledged" -> {
                        state.getJSONObject("base").put(id, result.record)
                        if (op.getJSONObject("content").optString("state") == "deleted") hideFamily(state, id, account)
                        queue.remove(id)
                    }
                    "deleted" -> { hideFamily(state, id, account); queue.remove(id) }
                    "conflict" -> {
                        val remote = result.current!!
                        if (StrictJson.equal(op.getJSONObject("content"), remote.getJSONObject("content"))) {
                            state.getJSONObject("base").put(id, remote); queue.remove(id)
                        } else if (op.getJSONObject("content").optString("state") == "deleted") {
                            // Rebase deletion, never an immutable text overwrite.
                            op.put("base_revision", remote.getLong("revision")).put("operation_id", UUID.randomUUID().toString())
                        } else {
                            state.getJSONObject("failed").put(id, "conflict"); queue.remove(id)
                        }
                    }
                    "stale_generation", "stale_epoch", "consent_required", "account_unavailable", "history_not_enrolled" -> {
                        state.put("enabled", false).put("outbox", JSONObject()); commit(); publish(account, state, "turned_off"); return@withLock
                    }
                    "quota_exceeded" -> throw CloudFailure(CloudFailure.Kind.QUOTA)
                    else -> { state.getJSONObject("failed").put(id, result.status); queue.remove(id) }
                }
                commit()
            }
            collect(state, account); commit()
            if (state.getJSONObject("outbox").length() > 0) continueLater()
            // Commit a page and its immutable UUID cursor together. A completed sweep
            // wraps to page one so earlier concurrent changes and tombstones are revisited.
            val after = state.optString("poll_cursor").takeIf { it.isNotEmpty() }
            run {
                val page = authorized(account, generation, localFence) { cloud.records(it, account.uid, "transcripts", after, pageSize) }
                val latest = authorized(account, generation, localFence) { cloud.get(it, Planner.account(account.uid)) }
                if (!StrictJson.equal(latest?.optJSONObject("consent")?.optJSONObject("transcripts"), toggle) ||
                    latest?.optLong("account_epoch") != consent.getLong("account_epoch")) throw StaleAccountException()
                for (remote in page) {
                    try { Contract.checkRecord(Planner.remoteRecord(remote)) } catch (_: Exception) { state.put("skipped", state.optInt("skipped") + 1); continue }
                    val id = remote.getString("id")
                    if (remote.getString("category") != "transcripts") continue
                    val base = state.getJSONObject("base").optJSONObject(id)
                    if (base != null && base.getLong("revision") >= remote.getLong("revision")) continue
                    state.getJSONObject("base").put(id, Planner.remoteRecord(remote))
                    if (remote.getJSONObject("content").getString("state") == "deleted") hideFamily(state, id, account)
                    else if (!state.getJSONObject("hidden").has(id) && !state.getJSONObject("hidden").has(remote.getJSONObject("content").getJSONObject("data").optString("original_record_id", id)) && !queue.has(id) && !state.getJSONObject("failed").has(id)) {
                        state.getJSONObject("records").put(id, Planner.remoteRecord(remote))
                    }
                }
                commit()
                val next = page.lastOrNull()?.getString("id")?.takeIf { page.size == pageSize }
                if (next != null && (!Contract.isUuid(next) || (after != null && next <= after))) throw CloudFailure(CloudFailure.Kind.INVALID)
                state.put("poll_cursor", next ?: ""); commit()
                if (next != null) continueLater()
            }
            authorized(account, generation, localFence) { token -> cloud.transaction(token) { read ->
                val path = Planner.device(account.uid, PortableStore.deviceId(context))
                val previous = read(path)?.takeIf { it.optLong("account_epoch") == consent.getLong("account_epoch") }
                    ?.optJSONObject("reconciled_generations")
                val seen = JSONObject()
                for (category in listOf("settings", "dictionary", "transcripts")) {
                    val value = if (category == "transcripts") toggle.getLong("generation") else previous?.optLong(category, 1L) ?: 1L
                    seen.put(category, value.coerceAtMost(consent.getJSONObject(category).getLong("generation")))
                }
                Unit to listOf(Planner.Write(path, JSONObject().put("schema_version", 1L).put("uid", account.uid)
                    .put("account_epoch", consent.getLong("account_epoch")).put("device_id", PortableStore.deviceId(context)).put("reconciled_generations", seen)))
            } }
            failures = 0
            publish(account, state, if (state.getJSONObject("failed").length() > 0) "retained" else null)
        } catch (error: Exception) {
            if (error is CancellationException) throw error
            if (error is StaleAccountException) return@withLock
            val problem = when (error) {
                is CloudFailure -> error.kind.name.lowercase()
                is AuthFailure -> error.kind.name.lowercase()
                else -> "unavailable"
            }
            publish(account, state, problem)
            if (problem in setOf("offline", "unavailable", "quota")) {
                retry?.cancel(); val wait = (5_000L shl failures++.coerceAtMost(6)).coerceAtMost(300_000L)
                retry = scope.launch { delay(wait); runOnce() }
            }
        }
    }

    private fun collect(state: JSONObject, account: AccountKey) {
        if (!state.optBoolean("enabled")) return
        val rows = state.getJSONObject("records"); val excluded = state.getJSONObject("excluded")
        val changes = journal.changes(state.optLong("local_cursor"))
        for ((sequence, entry) in changes) {
            val (id, value) = entry; val row = value as JSONObject
            state.put("local_cursor", sequence)
            val data = row.getJSONObject("data")
            val original = if (data.getString("kind") == "original") id else data.getString("original_record_id")
            if (excluded.has(id) || excluded.has(original) || excluded.has(row.optString("session")) || excluded.has(data.optString("attempt_id")) || (state.getJSONObject("hidden").has(id) || state.getJSONObject("hidden").has(original))) continue
            if (row.optString("owner").let { it.isNotEmpty() && it != TranscriptStore.ownerKey(account) }) continue
            if (!rows.has(id)) rows.put(id, JSONObject().put("schema_version", 1L).put("id", id).put("category", "transcripts")
                .put("origin_device_id", row.getString("origin")).put("revision", 1L).put("content", JSONObject().put("state", "live").put("data", data)))
        }
        val deletions = journal.deletions(state.optLong("delete_cursor"))
        state.put("local_more", changes.size == 100 || deletions.size == 100)
        if (state.optBoolean("local_more")) continueLater()
        deletions.forEach { (sequence, entry) ->
            state.put("delete_cursor", sequence); val id = entry.first; if (rows.has(id)) hideFamily(state, id, account)
        }
        val base = state.getJSONObject("base"); val queue = state.getJSONObject("outbox")
        val ids = if (!state.optBoolean("queue_initialized")) {
            state.put("queue_initialized", true)
            rows.keys().asSequence().toSet() + state.getJSONObject("hidden").keys().asSequence().toSet()
        } else database(account).pendingKeys("records") + database(account).pendingKeys("hidden")
        for (id in ids) {
            if (queue.has(id) || state.getJSONObject("failed").has(id)) continue
            val known = base.optJSONObject(id)
            val local = rows.optJSONObject(id)
            val hidden = state.getJSONObject("hidden").has(id)
            if (hidden && known == null) continue
            val content = if (hidden) JSONObject().put("state", "deleted") else local?.getJSONObject("content") ?: continue
            if (known != null && StrictJson.equal(content, known.getJSONObject("content"))) continue
            val consent = state.getJSONObject("consent")
            queue.put(id, JSONObject().put("schema_version", 1L).put("uid", account.uid).put("account_epoch", consent.getLong("account_epoch"))
                .put("category", "transcripts").put("consent_generation", consent.getJSONObject("transcripts").getLong("generation"))
                .put("history_enrollment_id", state.getString("enrollment")).put("operation_id", UUID.randomUUID().toString())
                .put("device_id", PortableStore.deviceId(context)).put("record_id", id)
                .put("origin_device_id", (known ?: local!!).getString("origin_device_id")).put("base_revision", known?.getLong("revision") ?: 0L).put("content", content))
        }
    }

    private fun hideFamily(state: JSONObject, id: String, account: AccountKey) {
        val rows = state.getJSONObject("records")
        val pending = database(account).pendingKeys("records").filter { key -> key == id || rows.optJSONObject(key)?.optJSONObject("content")?.optJSONObject("data")?.optString("original_record_id") == id }
        val related = database(account).family("records", id) + pending + id
        related.forEach { key -> state.getJSONObject("hidden").put(key, true); rows.remove(key) }
    }

    /** Append a version parented to the selected source. Concurrent edits both survive. */
    suspend fun edit(id: String, text: String) = mutex.withLock {
        val (account, _) = active()
        synchronized(TranscriptStore.LOCK) {
            val state = load(account)
            val source = state.getJSONObject("records").getJSONObject(id).getJSONObject("content").getJSONObject("data")
            val original = if (source.getString("kind") == "original") id else source.getString("original_record_id")
            val newId = UUID.randomUUID().toString()
            val data = TranscriptStore.derived("edited", original, id, text)
            require(Contract.isTranscript(newId, data))
            state.getJSONObject("records").put(newId, JSONObject().put("schema_version", 1L).put("id", newId).put("category", "transcripts")
                .put("origin_device_id", PortableStore.deviceId(context)).put("revision", 1L).put("content", JSONObject().put("state", "live").put("data", data)))
            collect(state, account); save(account, state); publish(account, state)
        }
    }
    suspend fun delete(id: String) = mutex.withLock {
        val (account, _) = active()
        synchronized(TranscriptStore.LOCK) {
            val state = load(account); hideFamily(state, id, account); collect(state, account); save(account, state); publish(account, state)
        }
    }

    override fun suspend(account: AccountKey) { visibleLimit = 100; fence.incrementAndGet(); retry?.cancel(); current.value = Status() }
    override fun localCopy(account: AccountKey): LocalCopySummary = synchronized(TranscriptStore.LOCK) {
        val state = load(account); LocalCopySummary(state.getJSONObject("records").length(), state.getJSONObject("outbox").length())
    }
    override fun keepLocalCopy(account: AccountKey) = synchronized(TranscriptStore.LOCK) {
        // Detached, text-only copy. It is never automatically enrolled into another account.
        val state = load(account)
        val kept = AtomicFile(File(context.filesDir, "kept-transcripts.json"))
        val all = TranscriptStore.read(kept) ?: JSONObject()
        state.getJSONObject("records").objects().forEach { (id, row) -> all.put(id, row) }
        TranscriptStore.write(kept, all)
    }
    private fun removeDownloads(state: JSONObject) {
        // Pending local edits and local original recordings are retained, but never sent after a fence.
        val pending = state.getJSONObject("outbox").objects().keys
        val local = journal.records().keys
        val rows = state.getJSONObject("records")
        rows.keys().asSequence().toList().filter { it !in pending && it !in local }.forEach(rows::remove)
    }
    override fun removeLocalCopy(account: AccountKey) = synchronized(TranscriptStore.LOCK) {
        val state = load(account); removeDownloads(state); save(account, state)
    }
    override fun applyFence(account: AccountKey, fence: CloudFence) {
        if (fence.category != "transcripts") return
        suspend(account)
        synchronized(TranscriptStore.LOCK) {
            val state = load(account)
            if (fence.everywhere) removeDownloads(state)
            state.put("enabled", false).put("outbox", JSONObject())
            save(account, state); publish(account, state, "turned_off")
        }
    }
}
