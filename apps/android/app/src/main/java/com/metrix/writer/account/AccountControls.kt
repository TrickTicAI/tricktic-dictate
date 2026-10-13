package com.metrix.writer.account

import android.util.AtomicFile
import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.util.UUID
import java.util.concurrent.ConcurrentHashMap

/*
 * Session revocation and cloud/account deletion (MW-OUATH-8) through the trusted cleanup service
 * (infra/firebase/trusted-cleanup.md, "Service HTTP API v1"). Every operation needs a Google sign-in
 * at most 240 seconds old (the service allows 300) for the same UID, is bound to the account key and generation captured
 * when the user confirmed, and is dropped if either changes before it is sent or applied.
 * Tokens stay in memory; only the request and its receipt are stored.
 */

/** The only failure kinds the account page words. Never an upstream message. */
enum class ControlError(val wire: String) {
    NOT_CONFIGURED("not_configured"), OFFLINE("offline"), INVALID_RESPONSE("invalid_response"),
    UNAUTHENTICATED("unauthenticated"), REAUTH_REQUIRED("reauth_required"), NOT_OWNER("not_owner"), NOT_FOUND("not_found"),
    INVALID_REQUEST("invalid_request"), ACCOUNT_UNAVAILABLE("account_unavailable"), STALE_EPOCH("stale_epoch"),
    STALE_CONSENT("stale_consent"), IDEMPOTENCY_CONFLICT("idempotency_conflict"), RATE_LIMITED("rate_limited"),
    UNAVAILABLE("unavailable");
    companion object { fun of(wire: String?) = entries.find { it.wire == wire } }
}

class ServiceFailure(val kind: ControlError) : Exception(kind.wire)

/** The trusted cleanup service and the account document read. The dev build uses HTTPS; tests use a fake. */
interface AccountServiceApi {
    /** False when this build has no service URL: every control reports [ControlError.NOT_CONFIGURED]. */
    val configured: Boolean
    /** `users/{uid}` read under the Security Rules, or null when it does not exist. */
    suspend fun readAccount(account: AccountKey, idToken: String): RemoteAccount?
    /** `POST /v1/cleanup`; returns the receipt. Failures are [ServiceFailure]. */
    suspend fun startCleanup(idToken: String, request: JSONObject): JSONObject
    /** `GET /v1/cleanup/{operationId}`; returns the receipt. */
    suspend fun cleanupStatus(idToken: String, operationId: String): JSONObject
    /** `POST /v1/sessions/revoke`. */
    suspend fun revokeSessions(idToken: String, uid: String)
}

/** One started cleanup: its exact request, the latest receipt and the local-copy choice. Never a token. */
data class CleanupJob(val operationId: String, val request: JSONObject, val receipt: JSONObject?, val localCopy: LocalCopy) {
    val state: String? get() = receipt?.optString("state")
    val accountScope get() = request.getJSONObject("scope").getString("kind") == "account"
    val everywhere get() = request.getString("effect") == "everywhere"
    val categories: List<String> get() = if (accountScope) RemoteAccount.CATEGORIES
        else request.getJSONObject("scope").getJSONArray("categories").let { list -> List(list.length()) { list.getString(it) } }
    val completed get() = state == "completed"

    fun toJson(): JSONObject = JSONObject().put("operation_id", operationId).put("request", request)
        .put("receipt", receipt ?: JSONObject.NULL).put("local_copy", localCopy.wire)
    companion object {
        fun fromJson(json: JSONObject) = CleanupJob(json.getString("operation_id"), json.getJSONObject("request"),
            json.optJSONObject("receipt"), LocalCopy.entries.first { it.wire == json.getString("local_copy") })
    }
}

/** Started cleanups, per account workspace, so a lost response or a restart retries the same operation ID. */
class CleanupTracker(private val workspace: AccountWorkspace) {
    private val lock = Any()
    private fun file(account: AccountKey) = AtomicFile(File(workspace.directory(account), "cleanup-jobs.json"))

    fun jobs(account: AccountKey): List<CleanupJob> = synchronized(lock) {
        val file = file(account)
        if (!file.baseFile.exists()) return emptyList()
        runCatching { JSONArray(String(file.readFully(), Charsets.UTF_8)).let { list -> List(list.length()) { CleanupJob.fromJson(list.getJSONObject(it)) } } }
            .getOrDefault(emptyList())
    }

    fun save(account: AccountKey, job: CleanupJob) = write(account) { jobs -> (jobs.filter { it.operationId != job.operationId } + job).takeLast(10) }
    fun remove(account: AccountKey, operationId: String) = write(account) { jobs -> jobs.filter { it.operationId != operationId } }

    private fun write(account: AccountKey, change: (List<CleanupJob>) -> List<CleanupJob>) = synchronized(lock) {
        val file = file(account)
        file.baseFile.parentFile?.mkdirs()
        val bytes = JSONArray(change(jobs(account)).map { it.toJson() }).toString().toByteArray(Charsets.UTF_8)
        val stream = file.startWrite()
        try { stream.write(bytes); file.finishWrite(stream) } catch (error: Exception) { file.failWrite(stream); throw error }
    }
}

sealed interface AccountAction {
    data class DeleteCloudData(val categories: List<String>, val everywhere: Boolean) : AccountAction
    data class DeleteAccount(val localCopy: LocalCopy) : AccountAction
    data class SignOutEverywhere(val localCopy: LocalCopy) : AccountAction
}

sealed interface ControlResult {
    /** The service answered with a receipt. [localCopyApplied] is false when a local-copy hook failed after account deletion. */
    data class Job(val job: CleanupJob, val localCopyApplied: Boolean = true) : ControlResult
    data class SignedOutEverywhere(val localCopyApplied: Boolean) : ControlResult
    /** Sign in again with the same Google account, then repeat the action. */
    data object NeedsReauth : ControlResult
    /** The account or its generation changed; nothing was applied. */
    data object Dropped : ControlResult
    data class Failed(val error: ControlError) : ControlResult
}

class AccountControls(
    private val session: AccountSession,
    private val service: AccountServiceApi,
    private val tracker: CleanupTracker,
    private val nowSeconds: () -> Long = { System.currentTimeMillis() / 1000 },
) {
    /** The token that requested an account deletion, for the final read after the Firebase user is gone. Memory only. */
    private val deletionTokens = ConcurrentHashMap<String, String>()

    val configured get() = service.configured

    fun jobs(account: AccountKey) = tracker.jobs(account)

    suspend fun run(action: AccountAction): ControlResult {
        if (!service.configured) return ControlResult.Failed(ControlError.NOT_CONFIGURED)
        val snapshot = session.snapshot()
        val account = snapshot.account ?: return ControlResult.Dropped
        if (snapshot.state == AuthState.REAUTH_REQUIRED) return ControlResult.NeedsReauth
        val generation = snapshot.generation
        val token = when (val fresh = recentToken(account, generation)) { is Token.Ok -> fresh.value; is Token.No -> return fresh.result }
        return try {
            when (action) {
                is AccountAction.SignOutEverywhere -> {
                    if (!session.isCurrent(account, generation)) return ControlResult.Dropped
                    service.revokeSessions(token, account.uid)
                    if (!session.isCurrent(account, generation)) return ControlResult.Dropped
                    ControlResult.SignedOutEverywhere(session.signOut(action.localCopy))
                }
                is AccountAction.DeleteCloudData -> cleanup(account, generation, token,
                    JSONObject().put("kind", "categories").put("categories", JSONArray(RemoteAccount.CATEGORIES.filter { it in action.categories })),
                    if (action.everywhere) "everywhere" else "cloud_only", LocalCopy.LEAVE)
                is AccountAction.DeleteAccount -> cleanup(account, generation, token, JSONObject().put("kind", "account"), "cloud_only", action.localCopy)
            }
        } catch (failure: ServiceFailure) {
            if (failure.kind == ControlError.REAUTH_REQUIRED) ControlResult.NeedsReauth else ControlResult.Failed(failure.kind)
        }
    }

    /** Sends a stored request again, unchanged. Needs a recent sign-in like the first send. */
    suspend fun retry(account: AccountKey, operationId: String): ControlResult {
        if (!service.configured) return ControlResult.Failed(ControlError.NOT_CONFIGURED)
        val job = tracker.jobs(account).find { it.operationId == operationId } ?: return ControlResult.Failed(ControlError.NOT_FOUND)
        val snapshot = session.snapshot()
        if (snapshot.account != account) return ControlResult.Dropped
        if (snapshot.state == AuthState.REAUTH_REQUIRED) return ControlResult.NeedsReauth
        val token = when (val fresh = recentToken(account, snapshot.generation)) { is Token.Ok -> fresh.value; is Token.No -> return fresh.result }
        return send(account, snapshot.generation, token, job)
    }

    /** Reads the job's current receipt. An account deletion uses the token that requested it once the user is gone. */
    suspend fun check(account: AccountKey, operationId: String): ControlResult {
        if (!service.configured) return ControlResult.Failed(ControlError.NOT_CONFIGURED)
        val job = tracker.jobs(account).find { it.operationId == operationId } ?: return ControlResult.Failed(ControlError.NOT_FOUND)
        if (job.receipt == null) return retry(account, operationId)
        val snapshot = session.snapshot()
        if (snapshot.account != account) return ControlResult.Dropped
        val token = deletionTokens[operationId].takeIf { job.accountScope } ?: try {
            session.getFirebaseIdToken(account, snapshot.generation, false)
        } catch (failure: AuthFailure) { return ControlResult.Failed(if (failure.kind == AuthError.REAUTH_REQUIRED) ControlError.UNAUTHENTICATED else authError(failure.kind)) }
          catch (_: StaleAccountException) { return ControlResult.Dropped }
        val receipt = try { service.cleanupStatus(token, operationId) } catch (failure: ServiceFailure) { return ControlResult.Failed(failure.kind) }
        if (!sameAccount(account, snapshot.generation)) return ControlResult.Dropped
        return apply(account, job, receipt)
    }

    private suspend fun cleanup(account: AccountKey, generation: Long, token: String, scope: JSONObject, effect: String, localCopy: LocalCopy): ControlResult {
        val remote = service.readAccount(account, token) ?: RemoteAccount.INITIAL
        if (remote.state != "active") return ControlResult.Failed(ControlError.ACCOUNT_UNAVAILABLE)
        val operationId = UUID.randomUUID().toString()
        val request = JSONObject().put("schema_version", 1).put("uid", account.uid).put("account_epoch", remote.epoch)
            .put("operation_id", operationId).put("expected_consent_revision", remote.consentRevision).put("scope", scope).put("effect", effect)
        val job = CleanupJob(operationId, request, null, localCopy)
        // Stored before sending: a lost response is retried with this same operation ID, never a second job.
        tracker.save(account, job)
        return send(account, generation, token, job)
    }

    private suspend fun send(account: AccountKey, generation: Long, token: String, job: CleanupJob): ControlResult {
        if (!session.isCurrent(account, generation)) return ControlResult.Dropped
        if (job.accountScope) deletionTokens[job.operationId] = token
        val receipt = try { service.startCleanup(token, job.request) } catch (failure: ServiceFailure) {
            // These refusals prove no job exists for this operation; anything else may have reached the service.
            if (failure.kind in NO_JOB) { tracker.remove(account, job.operationId); deletionTokens.remove(job.operationId) }
            return if (failure.kind == ControlError.REAUTH_REQUIRED) ControlResult.NeedsReauth else ControlResult.Failed(failure.kind)
        }
        if (!session.isCurrent(account, generation)) return ControlResult.Dropped
        return apply(account, job, receipt)
    }

    private fun apply(account: AccountKey, job: CleanupJob, receipt: JSONObject): ControlResult {
        val state = receipt.optString("state")
        val consent = receipt.optJSONObject("fenced_consent")
        if (receipt.optString("operation_id") != job.operationId || receipt.optString("uid") != account.uid ||
            state !in STATES || consent == null || job.categories.any { consent.optJSONObject(it) == null }) {
            return ControlResult.Failed(ControlError.INVALID_RESPONSE)
        }
        val updated = job.copy(receipt = receipt)
        tracker.save(account, updated)
        // The service fenced before acknowledging; detach the categories once, on the first receipt.
        if (job.receipt == null) session.applyFences(account, job.categories.map {
            CloudFence(it, consent.getJSONObject(it).getLong("generation"), job.everywhere)
        })
        if (updated.completed && job.accountScope) {
            deletionTokens.remove(job.operationId)
            return ControlResult.Job(updated, session.signOut(job.localCopy))
        }
        return ControlResult.Job(updated)
    }

    private sealed interface Token { class Ok(val value: String) : Token; class No(val result: ControlResult) : Token }

    private suspend fun recentToken(account: AccountKey, generation: Long): Token = try {
        Token.Ok(session.recentIdToken(account, generation, nowSeconds = nowSeconds))
    } catch (failure: AuthFailure) {
        Token.No(if (failure.kind == AuthError.REAUTH_REQUIRED) ControlResult.NeedsReauth else ControlResult.Failed(authError(failure.kind)))
    } catch (_: StaleAccountException) { Token.No(ControlResult.Dropped) }

    private fun sameAccount(account: AccountKey, generation: Long) = session.snapshot().let {
        it.account == account && it.generation == generation && it.state in setOf(AuthState.SIGNED_IN, AuthState.REAUTH_REQUIRED)
    }

    private fun authError(kind: AuthError) = when (kind) {
        AuthError.OFFLINE -> ControlError.OFFLINE
        AuthError.NOT_CONFIGURED -> ControlError.NOT_CONFIGURED
        AuthError.INVALID_RESPONSE -> ControlError.INVALID_RESPONSE
        AuthError.REAUTH_REQUIRED -> ControlError.REAUTH_REQUIRED
        else -> ControlError.UNAVAILABLE
    }

    private companion object {
        val STATES = setOf("pending", "running", "failed_retryable", "completed")
        val NO_JOB = setOf(ControlError.NOT_OWNER, ControlError.INVALID_REQUEST, ControlError.ACCOUNT_UNAVAILABLE,
            ControlError.STALE_EPOCH, ControlError.STALE_CONSENT)
    }
}
