@file:Suppress("DEPRECATION")
package com.metrix.writer

import android.content.Context
import android.test.AndroidTestCase
import com.metrix.writer.account.*
import kotlinx.coroutines.*
import java.util.concurrent.CopyOnWriteArrayList

/** The account lifecycle of the platform authentication adapter v1, against a scripted provider. */
class AccountSessionTest : AndroidTestCase() {
    private val alice = IdentityUser("uid-alice", "alice@example.com", "Alice")
    private val bob = IdentityUser("uid-bob", "bob@example.com", "Bob")
    private lateinit var scope: CoroutineScope
    private val events = CopyOnWriteArrayList<AuthEvent>()

    override fun setUp() { super.setUp(); scope = CoroutineScope(SupervisorJob() + Dispatchers.Default); events.clear() }
    override fun tearDown() { scope.cancel(); super.tearDown() }

    private fun session(identity: FakeIdentity, timeout: Long = 60_000) = AccountSession(identity, scope, timeout).also { it.subscribe { event -> events += event } }
    private fun waitFor(message: String, condition: () -> Boolean) {
        val deadline = System.currentTimeMillis() + 5_000
        while (!condition()) { assertTrue(message, System.currentTimeMillis() < deadline); Thread.sleep(10) }
    }
    private fun finished(id: String?) = events.filterIsInstance<AuthEvent.SignInFinished>().filter { it.attemptId == id }
    private fun types() = events.map { it.type }

    fun testGuestSnapshotNeedsNoProviderWorkAndSubscribersGetItFirst() {
        val identity = FakeIdentity()
        val session = session(identity)
        waitFor("snapshot delivered") { events.isNotEmpty() }
        assertEquals(AuthState.GUEST, session.snapshot().state)
        assertNull(session.snapshot().account)
        val first = events.first() as AuthEvent.Snapshot
        assertEquals(1, first.version)
        assertEquals(AuthState.GUEST, first.snapshot.state)
        assertEquals(listOf("restore"), identity.calls)
    }

    fun testUnconfiguredBuildReportsNotConfiguredWithoutProviderCalls() {
        val identity = FakeIdentity(projectId = null)
        val session = session(identity)
        assertFalse(session.configured)
        val id = session.signInGoogle(context)
        waitFor("finished") { finished(id).isNotEmpty() }
        assertEquals(AuthError.NOT_CONFIGURED, finished(id).single().error)
        assertEquals(AuthState.GUEST, session.snapshot().state)
        assertTrue(identity.calls.isEmpty())
    }

    fun testFirstLoginOrdersEventsAndLeavesSyncToConsent() {
        val identity = FakeIdentity()
        val session = session(identity)
        val id = session.signInGoogle(context)!!
        assertEquals(AuthState.SIGNING_IN, session.snapshot().state)
        assertNull("one attempt at a time", session.signInGoogle(context))
        identity.credential.complete("google-id-token")
        identity.exchange.complete(alice)
        waitFor("signed in") { session.snapshot().state == AuthState.SIGNED_IN && events.size >= 5 }
        assertEquals(AccountKey("metrix-writer", "uid-alice"), session.snapshot().account)
        assertEquals("alice@example.com", session.snapshot().display?.email)
        assertEquals(listOf("auth_snapshot", "sign_in_started", "auth_snapshot", "sign_in_finished", "auth_snapshot"), types())
        assertEquals("success", finished(id).single().result)
        assertEquals(events.map { it.sequence }.sorted(), events.map { it.sequence })
        assertEquals(events.size, events.map { it.sequence }.toSet().size)
        assertFalse("signing in does not reveal identifiers in logs", session.snapshot().account.toString().contains("uid-alice"))
    }

    fun testReturningSessionIsRestoredAfterRestartWithoutSignIn() {
        val identity = FakeIdentity(restored = alice)
        val session = session(identity)
        assertEquals(AuthState.SIGNED_IN, session.snapshot().state)
        assertEquals("uid-alice", session.snapshot().account?.uid)
        assertEquals(listOf("restore"), identity.calls)
    }

    fun testCancelDuringAccountChooserReturnsToGuestAndAdvancesGeneration() {
        val identity = FakeIdentity()
        val session = session(identity)
        val id = session.signInGoogle(context)!!
        waitFor("chooser open") { "credential" in identity.calls }
        assertTrue(session.cancelSignIn(id))
        assertFalse(session.cancelSignIn(id))
        assertEquals(AuthState.GUEST, session.snapshot().state)
        assertEquals(1L, session.snapshot().generation)
        waitFor("cancel reported") { finished(id).isNotEmpty() }
        assertEquals(AuthError.CANCELLED, finished(id).single().error)
        identity.credential.complete("late-token")
        Thread.sleep(200)
        assertFalse("a cancelled attempt never exchanges", "signIn" in identity.calls)
        assertEquals(1, finished(id).size)
    }

    fun testChooserDismissedReportsCancelled() {
        val identity = FakeIdentity()
        val session = session(identity)
        val id = session.signInGoogle(context)
        identity.credential.completeExceptionally(AuthFailure(AuthError.CANCELLED))
        waitFor("finished") { finished(id).isNotEmpty() }
        assertEquals("cancelled", finished(id).single().result)
        assertEquals(AuthState.GUEST, session.snapshot().state)
    }

    fun testLateExchangeAfterCancelIsSignedOutAgain() {
        val identity = FakeIdentity()
        val session = session(identity)
        val id = session.signInGoogle(context)!!
        identity.credential.complete("google-id-token")
        waitFor("exchanging") { "signIn" in identity.calls }
        assertTrue(session.cancelSignIn(id))
        assertNull("a new attempt waits for the cancelled exchange to settle", session.signInGoogle(context))
        identity.exchange.complete(alice)
        waitFor("late session removed") { identity.signedIn == null && "signOutLocal" in identity.calls }
        assertEquals(AuthState.GUEST, session.snapshot().state)
        assertTrue(events.none { it is AuthEvent.Snapshot && it.snapshot.state == AuthState.SIGNED_IN })
        waitFor("next attempt allowed") { session.signInGoogle(context) != null }
    }

    fun testCredentialTimeoutAndFailuresUseFixedKinds() {
        val identity = FakeIdentity()
        val session = session(identity, timeout = 150)
        val timed = session.signInGoogle(context)
        waitFor("timed out") { finished(timed).isNotEmpty() }
        assertEquals(AuthError.TIMED_OUT, finished(timed).single().error)
        assertEquals(AuthState.GUEST, session.snapshot().state)

        identity.credential = CompletableDeferred("google-id-token")
        identity.exchange = CompletableDeferred<IdentityUser>().apply { completeExceptionally(AuthFailure(AuthError.OFFLINE)) }
        val offline = session.signInGoogle(context)
        waitFor("offline") { finished(offline).isNotEmpty() }
        assertEquals(AuthError.OFFLINE, finished(offline).single().error)

        identity.exchange = CompletableDeferred<IdentityUser>().apply { completeExceptionally(IllegalStateException("upstream body https://example.invalid/secret")) }
        val unknown = session.signInGoogle(context)
        waitFor("unknown") { finished(unknown).isNotEmpty() }
        assertEquals(AuthError.UNAVAILABLE, finished(unknown).single().error)
        assertTrue(events.none { it.toString().contains("example.invalid") })
    }

    fun testSignOutInvalidatesFirstThenClearsSessionAndCredentialState() {
        val identity = FakeIdentity(restored = alice)
        val session = session(identity)
        val before = session.snapshot()
        session.signOut()
        assertEquals(AuthState.GUEST, session.snapshot().state)
        assertEquals(before.generation + 1, session.snapshot().generation)
        waitFor("credential state cleared") { "clearCredentialState" in identity.calls }
        assertTrue(identity.calls.indexOf("signOutLocal") < identity.calls.indexOf("clearCredentialState"))
        waitFor("events") { events.any { it is AuthEvent.SessionInvalidated } }
        val invalidated = events.filterIsInstance<AuthEvent.SessionInvalidated>().single()
        assertEquals(before.account, invalidated.previous)
        assertEquals(InvalidationReason.SIGNED_OUT, invalidated.reason)
        assertTrue(types().indexOf("session_invalidated") < types().lastIndexOf("auth_snapshot"))
    }

    fun testTokensAreBoundToAccountAndGeneration() {
        val identity = FakeIdentity(restored = alice)
        val session = session(identity)
        val snapshot = session.snapshot()
        runBlocking {
            assertEquals("firebase-id-token", session.getFirebaseIdToken(snapshot.account!!, snapshot.generation, true))
            waitFor("refreshed") { events.any { it is AuthEvent.SessionRefreshed } }
            assertEquals(snapshot.account, events.filterIsInstance<AuthEvent.SessionRefreshed>().single().account)
            try { session.getFirebaseIdToken(AccountKey("metrix-writer", "uid-bob"), snapshot.generation, false); fail() } catch (_: StaleAccountException) {}
            session.signOut()
            try { session.getFirebaseIdToken(snapshot.account, snapshot.generation, false); fail() } catch (_: StaleAccountException) {}
        }
        assertFalse("idToken:false" in identity.calls)
    }

    fun testSignOutDuringRefreshDropsTheToken() {
        val identity = FakeIdentity(restored = alice)
        val session = session(identity)
        val snapshot = session.snapshot()
        identity.refresh = { session.signOut(); "token-for-old-session" }
        runBlocking { try { session.getFirebaseIdToken(snapshot.account!!, snapshot.generation, true); fail() } catch (_: StaleAccountException) {} }
        assertTrue(events.none { it is AuthEvent.SessionRefreshed })
    }

    fun testRevokedSessionAsksForReauthenticationAndOnlyTheSameAccountRestoresIt() {
        val identity = FakeIdentity(restored = alice)
        val session = session(identity)
        identity.refresh = { throw AuthFailure(AuthError.REAUTH_REQUIRED) }
        runBlocking { assertEquals(AuthError.REAUTH_REQUIRED, session.checkSession()) }
        assertEquals(AuthState.REAUTH_REQUIRED, session.snapshot().state)
        assertEquals("uid-alice", session.snapshot().account?.uid)

        identity.credential.complete("google-id-token")
        identity.exchange.complete(bob)
        val wrong = session.reauthenticateGoogle(context)
        waitFor("rejected") { finished(wrong).isNotEmpty() }
        assertEquals(AuthError.PROVIDER_REJECTED, finished(wrong).single().error)
        assertEquals(AuthState.REAUTH_REQUIRED, session.snapshot().state)
        assertEquals("uid-alice", session.snapshot().account?.uid)

        identity.exchange = CompletableDeferred(alice)
        val right = session.reauthenticateGoogle(context)
        waitFor("restored") { finished(right).isNotEmpty() }
        assertNull(finished(right).single().error)
        assertEquals(AuthState.SIGNED_IN, session.snapshot().state)
        assertTrue("reauthentication never replaces the session", "signIn" !in identity.calls)
    }

    fun testUnsubscribedListenerStopsReceivingEvents() {
        val identity = FakeIdentity(restored = alice)
        val session = AccountSession(identity, scope)
        val seen = CopyOnWriteArrayList<AuthEvent>()
        val unsubscribe = session.subscribe { seen += it }
        waitFor("snapshot") { seen.isNotEmpty() }
        unsubscribe()
        session.signOut()
        Thread.sleep(200)
        assertEquals(1, seen.size)
    }

    // MW-OUATH-8: switching, local-copy choices and lifecycle hooks.

    fun testSwitchSuspendsAThenClearsItBeforeBsChooserOpens() {
        val identity = FakeIdentity(restored = alice)
        val session = session(identity)
        val log = CopyOnWriteArrayList<String>()
        session.register(FakeParticipant(log) { ":" + session.snapshot().state.wire })
        val before = session.snapshot()
        val id = session.switchAccount(context)!!
        assertEquals(listOf("suspend:uid-alice"), log)
        assertEquals(AuthState.SIGNING_IN, session.snapshot().state)
        assertNull(session.snapshot().account)
        assertTrue(session.snapshot().generation > before.generation)
        waitFor("chooser open") { "credential" in identity.calls }
        assertTrue(identity.calls.indexOf("signOutLocal") < identity.calls.indexOf("clearCredentialState"))
        assertTrue("Credential Manager forgets A before the chooser", identity.calls.indexOf("clearCredentialState") < identity.calls.indexOf("credential"))
        identity.credential.complete("google-id-token")
        identity.exchange.complete(bob)
        waitFor("signed in as B") { session.snapshot().account?.uid == "uid-bob" }
        waitFor("events") { finished(id).isNotEmpty() }
        val invalidated = events.filterIsInstance<AuthEvent.SessionInvalidated>().single()
        assertEquals("uid-alice", invalidated.previous.uid)
        assertTrue(types().indexOf("session_invalidated") < types().indexOf("sign_in_started"))
        runBlocking { try { session.getFirebaseIdToken(before.account!!, before.generation, false); fail() } catch (_: StaleAccountException) {} }
    }

    fun testAsInFlightTokenIsDroppedAfterSwitchingToB() {
        val identity = FakeIdentity(restored = alice)
        val session = session(identity)
        val a = session.snapshot()
        val release = CompletableDeferred<Unit>()
        identity.refresh = { runBlocking { release.await() }; "token-for-alice" }
        val inFlight = scope.async { runCatching { session.getFirebaseIdToken(a.account!!, a.generation, true) } }
        waitFor("refresh started") { "idToken:true" in identity.calls }
        session.switchAccount(context)
        identity.refresh = { "token-for-bob" }
        identity.credential.complete("google-id-token")
        identity.exchange.complete(bob)
        waitFor("signed in as B") { session.snapshot().account?.uid == "uid-bob" }
        release.complete(Unit)
        val result = runBlocking { inFlight.await() }
        assertTrue(result.exceptionOrNull() is StaleAccountException)
        assertTrue(events.none { it is AuthEvent.SessionRefreshed && it.account.uid == "uid-alice" })
        assertEquals("uid-bob", session.snapshot().account?.uid)
    }

    fun testSwitchNeedsASignedInAccountAndNoPendingSignIn() {
        val guest = session(FakeIdentity())
        assertNull(guest.switchAccount(context))
        val identity = FakeIdentity(restored = alice)
        val session = session(identity)
        assertNotNull(session.reauthenticateGoogle(context))
        assertNull("a pending sign-in settles first", session.switchAccount(context))
        assertEquals("uid-alice", session.snapshot().account?.uid)
    }

    fun testLocalCopyHooksRunAfterTheSessionIsGone() {
        for ((choice, hook) in listOf(LocalCopy.KEEP to "keep", LocalCopy.REMOVE to "remove", LocalCopy.LEAVE to null)) {
            events.clear()
            val identity = FakeIdentity(restored = alice)
            val session = session(identity)
            val log = CopyOnWriteArrayList<String>()
            session.register(FakeParticipant(log, LocalCopySummary(3, 1)) { ":" + session.snapshot().state.wire + ":" + ("signOutLocal" in identity.calls) })
            assertEquals(LocalCopySummary(3, 1), session.localCopy(session.snapshot().account!!))
            assertTrue(session.signOut(choice))
            assertEquals(listOfNotNull("suspend:uid-alice", hook?.let { "$it:uid-alice:guest:true" }), log)
        }
    }

    fun testFailingHooksNeverBlockSignOut() {
        val identity = FakeIdentity(restored = alice)
        val session = session(identity)
        val log = CopyOnWriteArrayList<String>()
        session.register(FakeParticipant(log, LocalCopySummary(1, 0), failIo = true))
        session.register(object : AccountParticipant by FakeParticipant(log) {
            override fun suspend(account: AccountKey) = error("listener already gone")
            override fun localCopy(account: AccountKey): LocalCopySummary = error("unreadable")
        })
        assertEquals(LocalCopySummary(1, 0), session.localCopy(session.snapshot().account!!))
        assertFalse(session.signOut(LocalCopy.REMOVE))
        assertEquals(AuthState.GUEST, session.snapshot().state)
        assertTrue("signOutLocal" in identity.calls)
        assertEquals("the second participant still runs", listOf("suspend:uid-alice", "remove:uid-alice", "remove:uid-alice"), log)
    }

    fun testUnregisteredParticipantIsNotCalled() {
        val session = session(FakeIdentity(restored = alice))
        val log = CopyOnWriteArrayList<String>()
        session.register(FakeParticipant(log))()
        session.signOut(LocalCopy.REMOVE)
        assertTrue(log.isEmpty())
    }

    fun testRevokedSessionSuspendsParticipantsBeforeReportingIt() {
        val identity = FakeIdentity(restored = alice)
        val session = session(identity)
        val log = CopyOnWriteArrayList<String>()
        session.register(FakeParticipant(log))
        identity.refresh = { throw AuthFailure(AuthError.REAUTH_REQUIRED) }
        runBlocking { session.checkSession() }
        assertEquals(listOf("suspend:uid-alice"), log)
        assertEquals(AuthState.REAUTH_REQUIRED, session.snapshot().state)
    }

    fun testSensitiveTokensNeedARecentGoogleSignInForTheSameAccount() {
        val identity = FakeIdentity(restored = alice)
        val session = session(identity)
        val a = session.snapshot()
        val now = 1_000_000L
        identity.authTime = { now - 241 }
        runBlocking {
            try { session.recentIdToken(a.account!!, a.generation) { now }; fail() } catch (failure: AuthFailure) { assertEquals(AuthError.REAUTH_REQUIRED, failure.kind) }
            identity.authTime = { now - 240 }
            assertEquals("firebase-id-token", session.recentIdToken(a.account!!, a.generation) { now })
            try { session.recentIdToken(AccountKey("metrix-writer", "uid-bob"), a.generation) { now }; fail() } catch (_: StaleAccountException) {}
        }
    }
}
