package com.metrix.writer.account

import android.content.Context
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.TimeoutCancellationException
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeout
import java.util.UUID
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.Executors

/*
 * Optional Writer account: the Android side of the platform authentication adapter v1
 * (infra/firebase/auth-adapter-contract.md). Dictation never waits on or reads this state;
 * a guest never touches the identity provider. Signing in alone uploads nothing: sync
 * consent is checked separately by the later sync adapters (MW-OUATH-7/10).
 */

enum class AuthState(val wire: String) { GUEST("guest"), SIGNING_IN("signing_in"), SIGNED_IN("signed_in"), REAUTH_REQUIRED("reauth_required") }

/** The only error kinds that cross into UI and events. Never an upstream message. */
enum class AuthError(val wire: String) {
    NOT_CONFIGURED("not_configured"), CANCELLED("cancelled"), TIMED_OUT("timed_out"), OFFLINE("offline"),
    PROVIDER_REJECTED("provider_rejected"), INVALID_RESPONSE("invalid_response"), REAUTH_REQUIRED("reauth_required"), UNAVAILABLE("unavailable"),
}

/** Firebase project and UID: the account key. Never the Google subject or email. */
data class AccountKey(val projectId: String, val uid: String) {
    override fun toString() = "AccountKey(projectId=$projectId, uid=…)"
}

/** Presentation only; never an identity. */
data class AccountDisplay(val email: String?, val name: String?) {
    override fun toString() = "AccountDisplay(…)"
}

data class AuthSnapshot(val state: AuthState, val account: AccountKey?, val display: AccountDisplay?, val generation: Long)

enum class InvalidationReason(val wire: String) { SIGNED_OUT("signed_out"), SIGN_IN_CANCELLED("sign_in_cancelled"), SESSION_REJECTED("session_rejected") }

/** Ordered account lifecycle events, version 1. [sequence] is process-local and monotonic. */
sealed interface AuthEvent {
    val version: Int get() = 1
    val type: String
    val sequence: Long
    val generation: Long

    data class Snapshot(override val sequence: Long, val snapshot: AuthSnapshot) : AuthEvent {
        override val type get() = "auth_snapshot"
        override val generation get() = snapshot.generation
    }
    data class SignInStarted(override val sequence: Long, override val generation: Long, val attemptId: String) : AuthEvent {
        override val type get() = "sign_in_started"
    }
    /** [error] is null on success; a cancelled attempt reports [AuthError.CANCELLED]. */
    data class SignInFinished(override val sequence: Long, override val generation: Long, val attemptId: String, val error: AuthError?) : AuthEvent {
        override val type get() = "sign_in_finished"
        val result get() = when (error) { null -> "success"; AuthError.CANCELLED -> "cancelled"; else -> "error" }
    }
    data class SessionRefreshed(override val sequence: Long, override val generation: Long, val account: AccountKey) : AuthEvent {
        override val type get() = "session_refreshed"
    }
    data class SessionInvalidated(override val sequence: Long, override val generation: Long, val previous: AccountKey, val reason: InvalidationReason) : AuthEvent {
        override val type get() = "session_invalidated"
    }
}

/** A sanitized failure. Its message is the fixed error kind only. */
class AuthFailure(val kind: AuthError) : Exception(kind.wire)

/** The captured account or generation changed; drop the work that asked. */
class StaleAccountException : Exception("stale account generation")

data class IdentityUser(val uid: String, val email: String?, val name: String?)

/** A just-refreshed Firebase ID token and its `auth_time` (seconds): when Google last signed this user in. */
class FreshToken(val token: String, val authTimeSeconds: Long) {
    override fun toString() = "FreshToken(…)"
}

/** The provider behind [AccountSession]; Firebase in the dev build, a fake in tests. Failures are [AuthFailure]. */
interface GoogleIdentity {
    /** Null when this build has no sign-in configuration. */
    val projectId: String?
    /** The locally persisted, validated session. Must not use the network or initialize anything for a guest. */
    fun restore(): IdentityUser?
    /** Explicit user action: Google's account chooser, returning a short-lived Google ID token. */
    suspend fun googleIdToken(context: Context): String
    suspend fun signIn(googleIdToken: String): IdentityUser
    /** Must prove the same [uid]; another account never replaces it. */
    suspend fun reauthenticate(googleIdToken: String, uid: String): IdentityUser
    suspend fun idToken(uid: String, forceRefresh: Boolean): String
    /** Forces a refresh and reports the token's `auth_time`, for operations that need a recent Google sign-in. */
    suspend fun freshIdToken(uid: String): FreshToken
    /** Clears the protected local session. Synchronous and local. */
    fun signOutLocal()
    /** Clears Credential Manager's remembered choice so the next sign-in asks again. */
    suspend fun clearCredentialState()
}

class AccountSession(
    private val identity: GoogleIdentity,
    private val scope: CoroutineScope,
    private val credentialTimeoutMs: Long = 60_000,
) {
    private class Attempt(val id: String, val generation: Long, val reauth: AccountKey?, val clearFirst: Boolean) { var job: Job? = null; @Volatile var exchanging = false }

    private val lock = Any()
    private val delivery = Executors.newSingleThreadExecutor { Thread(it, "writer-account-events").apply { isDaemon = true } }
    private val listeners = CopyOnWriteArrayList<(AuthEvent) -> Unit>()
    private val participants = CopyOnWriteArrayList<AccountParticipant>()
    private var sequence = 0L
    private var attempt: Attempt? = null
    private var draining: Job? = null
    private val current: MutableStateFlow<AuthSnapshot>
    val snapshots: StateFlow<AuthSnapshot>

    init {
        val restored = identity.projectId?.let { project -> identity.restore()?.let { project to it } }
        current = MutableStateFlow(restored?.let { (project, user) -> signedIn(project, user, 0) } ?: AuthSnapshot(AuthState.GUEST, null, null, 0))
        snapshots = current.asStateFlow()
    }

    val configured get() = identity.projectId != null

    fun snapshot(): AuthSnapshot = current.value

    /** Adds [listener]; it receives the current snapshot first, then every later event in order. */
    fun subscribe(listener: (AuthEvent) -> Unit): () -> Unit = synchronized(lock) {
        listeners += listener
        val event = AuthEvent.Snapshot(++sequence, current.value)
        delivery.execute { if (listener in listeners) listener(event) }
        return { listeners -= listener }
    }

    /** Registers a sync adapter's lifecycle hooks (MW-OUATH-8). Returns the unregister function. */
    fun register(participant: AccountParticipant): () -> Unit {
        participants += participant
        return { participants -= participant }
    }

    /** What the participants hold for [account] in its workspace. A failing participant counts as nothing. */
    fun localCopy(account: AccountKey): LocalCopySummary =
        participants.fold(LocalCopySummary.NONE) { total, participant -> total + (runCatching { participant.localCopy(account) }.getOrNull() ?: LocalCopySummary.NONE) }

    /** Starts Google sign-in from a guest. Returns the attempt ID, or null while another sign-in is still settling. */
    fun signInGoogle(context: Context): String? = start(context, null)

    /**
     * Signs account A out and starts B's sign-in in one transition: A's work is suspended and its
     * session cleared before the chooser opens, and Credential Manager forgets A's choice first.
     * Returns the new attempt ID, or null when no account is signed in or a sign-in is still settling.
     */
    fun switchAccount(context: Context): String? = synchronized(lock) {
        if (current.value.account == null || attempt != null || draining?.isCompleted == false) return null
        invalidate(InvalidationReason.SIGNED_OUT)
        identity.signOutLocal()
        start(context, null, clearFirst = true)
    }

    /** Proves the signed-in account again; choosing a different Google account is rejected. */
    fun reauthenticateGoogle(context: Context): String? = start(context, current.value.account ?: return null)

    private fun start(context: Context, reauth: AccountKey?, clearFirst: Boolean = false): String? = synchronized(lock) {
        val state = current.value.state
        if (attempt != null || draining?.isCompleted == false) return null
        if (reauth == null && state != AuthState.GUEST) return null
        if (reauth != null && (state !in setOf(AuthState.SIGNED_IN, AuthState.REAUTH_REQUIRED) || current.value.account != reauth)) return null
        val id = UUID.randomUUID().toString()
        val generation = current.value.generation
        emit(AuthEvent.SignInStarted(++sequence, generation, id))
        if (identity.projectId == null) {
            emit(AuthEvent.SignInFinished(++sequence, generation, id, AuthError.NOT_CONFIGURED))
            return id
        }
        val pending = Attempt(id, generation, reauth, clearFirst)
        attempt = pending
        if (reauth == null) publish(AuthSnapshot(AuthState.SIGNING_IN, null, null, generation))
        pending.job = scope.launch { run(pending, context) }
        id
    }

    private suspend fun run(pending: Attempt, context: Context) {
        try {
            if (pending.clearFirst) try { identity.clearCredentialState() } catch (error: Exception) { if (error is CancellationException) throw error }
            val token = try { withTimeout(credentialTimeoutMs) { identity.googleIdToken(context) } }
                catch (_: TimeoutCancellationException) { throw AuthFailure(AuthError.TIMED_OUT) }
            // The exchange cannot be abandoned half way: once Firebase answers, a cancelled
            // attempt's session is removed again below instead of being left behind.
            pending.exchanging = true
            val user = withContext(NonCancellable) {
                val reauth = pending.reauth
                if (reauth == null) identity.signIn(token) else identity.reauthenticate(token, reauth.uid)
            }
            synchronized(lock) {
                if (attempt !== pending || current.value.generation != pending.generation) {
                    if (pending.reauth == null) identity.signOutLocal()
                    return
                }
                attempt = null
                val project = identity.projectId!!
                val reauth = pending.reauth
                if (reauth != null && (user.uid != reauth.uid || project != reauth.projectId)) {
                    emit(AuthEvent.SignInFinished(++sequence, pending.generation, pending.id, AuthError.PROVIDER_REJECTED))
                    return
                }
                emit(AuthEvent.SignInFinished(++sequence, pending.generation, pending.id, null))
                publish(signedIn(project, user, pending.generation))
            }
        } catch (error: Throwable) {
            if (error is CancellationException) {
                // cancelSignIn/signOut already reported this attempt; a late session is removed.
                if (pending.exchanging && pending.reauth == null) identity.signOutLocal()
                throw error
            }
            val kind = (error as? AuthFailure)?.kind ?: AuthError.UNAVAILABLE
            synchronized(lock) {
                if (attempt !== pending) return
                attempt = null
                emit(AuthEvent.SignInFinished(++sequence, pending.generation, pending.id, kind))
                if (pending.reauth == null) publish(AuthSnapshot(AuthState.GUEST, null, null, pending.generation))
            }
        }
    }

    /** Invalidates only [attemptId]; its late result cannot establish a session. */
    fun cancelSignIn(attemptId: String): Boolean = synchronized(lock) {
        val pending = attempt?.takeIf { it.id == attemptId } ?: return false
        attempt = null
        draining = pending.job
        pending.job?.cancel()
        emit(AuthEvent.SignInFinished(++sequence, pending.generation, pending.id, AuthError.CANCELLED))
        if (pending.reauth == null) publish(AuthSnapshot(AuthState.GUEST, null, null, pending.generation + 1))
        true
    }

    /**
     * Stops account work first, then clears the local session and Credential Manager state. Local dictations are
     * untouched. [localCopy] then keeps or removes the account's downloaded copies; those hooks do I/O on the caller's
     * thread, after the session is already gone. Returns false when a hook failed; the sign-out itself always happens.
     */
    fun signOut(localCopy: LocalCopy = LocalCopy.LEAVE): Boolean {
        val previous = synchronized(lock) {
            val previous = invalidate(InvalidationReason.SIGNED_OUT)
            identity.signOutLocal()
            previous
        }
        scope.launch { try { identity.clearCredentialState() } catch (error: Exception) { if (error is CancellationException) throw error } }
        return previous == null || applyLocalCopy(previous, localCopy)
    }

    /** Runs the chosen local-copy hooks for an account that is no longer signed in. */
    fun applyLocalCopy(account: AccountKey, localCopy: LocalCopy): Boolean = eachParticipant { participant ->
        when (localCopy) {
            LocalCopy.LEAVE -> Unit
            LocalCopy.KEEP -> participant.keepLocalCopy(account)
            LocalCopy.REMOVE -> participant.removeLocalCopy(account)
        }
    }

    /** A cleanup fence took effect: suspend [account]'s work, then let each participant detach the categories. */
    fun applyFences(account: AccountKey, fences: List<CloudFence>): Boolean {
        suspendParticipants(account)
        return fences.map { fence -> eachParticipant { it.applyFence(account, fence) } }.all { it }
    }

    // Every participant gets the hook even when another one fails.
    private fun eachParticipant(hook: (AccountParticipant) -> Unit): Boolean =
        participants.map { participant -> runCatching { hook(participant) }.isSuccess }.all { it }

    // Called with [lock] held. Cancels any attempt, suspends the account's participants, then publishes guest.
    private fun invalidate(reason: InvalidationReason): AccountKey? {
        attempt?.let { pending ->
            attempt = null
            draining = pending.job
            pending.job?.cancel()
            emit(AuthEvent.SignInFinished(++sequence, pending.generation, pending.id, AuthError.CANCELLED))
        }
        val previous = current.value
        val next = previous.generation + 1
        previous.account?.let { account ->
            suspendParticipants(account)
            emit(AuthEvent.SessionInvalidated(++sequence, next, account, reason))
        }
        publish(AuthSnapshot(AuthState.GUEST, null, null, next))
        return previous.account
    }

    private fun suspendParticipants(account: AccountKey) = participants.forEach { runCatching { it.suspend(account) } }

    /** Whether work captured for [account] at [generation] may still send or apply a response. */
    fun isCurrent(account: AccountKey, generation: Long): Boolean = synchronized(lock) {
        current.value.let { it.state == AuthState.SIGNED_IN && it.account == account && it.generation == generation }
    }

    /**
     * A Firebase ID token whose Google sign-in is at most [maxAgeSeconds] old, for sensitive operations.
     * The service accepts 300 s; the default leaves room for the request and clock skew.
     * Throws [AuthFailure] with [AuthError.REAUTH_REQUIRED] when it is older: reauthenticate the same account first.
     */
    suspend fun recentIdToken(account: AccountKey, generation: Long, maxAgeSeconds: Long = RECENT_SIGN_IN_SECONDS, nowSeconds: () -> Long = { System.currentTimeMillis() / 1000 }): String {
        synchronized(lock) { requireCurrent(account, generation) }
        val fresh = identity.freshIdToken(account.uid)
        synchronized(lock) { requireCurrent(account, generation) }
        if (nowSeconds() - fresh.authTimeSeconds > maxAgeSeconds) throw AuthFailure(AuthError.REAUTH_REQUIRED)
        return fresh.token
    }

    /**
     * A Firebase ID token for exactly [account] at [generation], for native sync adapters only.
     * Throws [StaleAccountException] when either changed, or [AuthFailure].
     */
    suspend fun getFirebaseIdToken(account: AccountKey, generation: Long, forceRefresh: Boolean): String {
        synchronized(lock) { requireCurrent(account, generation) }
        val token = try { identity.idToken(account.uid, forceRefresh) } catch (failure: AuthFailure) {
            if (failure.kind == AuthError.REAUTH_REQUIRED) synchronized(lock) {
                if (current.value.state == AuthState.SIGNED_IN && current.value.account == account && current.value.generation == generation) {
                    suspendParticipants(account)
                    publish(current.value.copy(state = AuthState.REAUTH_REQUIRED))
                }
            }
            throw failure
        }
        synchronized(lock) {
            requireCurrent(account, generation)
            if (forceRefresh) emit(AuthEvent.SessionRefreshed(++sequence, generation, account))
        }
        return token
    }

    /** Refreshes the current session once, for the account page's check. Returns the error kind, or null. */
    suspend fun checkSession(): AuthError? {
        val snapshot = current.value
        val account = snapshot.account ?: return AuthError.REAUTH_REQUIRED
        if (snapshot.state == AuthState.REAUTH_REQUIRED) return AuthError.REAUTH_REQUIRED
        return try { getFirebaseIdToken(account, snapshot.generation, true); null }
            catch (failure: AuthFailure) { failure.kind }
            catch (_: StaleAccountException) { null }
    }

    private fun requireCurrent(account: AccountKey, generation: Long) {
        val snapshot = current.value
        if (snapshot.state != AuthState.SIGNED_IN || snapshot.account != account || snapshot.generation != generation) throw StaleAccountException()
    }

    private fun signedIn(project: String, user: IdentityUser, generation: Long) =
        AuthSnapshot(AuthState.SIGNED_IN, AccountKey(project, user.uid), AccountDisplay(user.email, user.name), generation)

    private fun publish(snapshot: AuthSnapshot) {
        current.value = snapshot
        emit(AuthEvent.Snapshot(++sequence, snapshot))
    }

    // Called with [lock] held, so the single delivery thread sees events in sequence order.
    private fun emit(event: AuthEvent) {
        val targets = listeners.toList()
        delivery.execute { targets.forEach { listener -> if (listener in listeners) listener(event) } }
    }
}

/** How recent a Google sign-in must be before a sensitive request; the cleanup service allows 300 s. */
const val RECENT_SIGN_IN_SECONDS = 240L
