@file:Suppress("DEPRECATION")
package com.metrix.writer

import android.test.AndroidTestCase
import com.metrix.writer.account.*
import kotlinx.coroutines.*
import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.util.UUID
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.atomic.AtomicInteger

/** Session revocation and cloud/account deletion (MW-OUATH-8) against a scripted cleanup service and real storage. */
class AccountControlsTest : AndroidTestCase() {
    private class FakeService(override var configured: Boolean = true) : AccountServiceApi {
        val calls = CopyOnWriteArrayList<String>()
        val sent = CopyOnWriteArrayList<Pair<String, JSONObject>>()
        var remote: RemoteAccount? = RemoteAccount(4, "active", 9, emptyMap())
        var generations = mapOf("settings" to 3L, "dictionary" to 5L, "transcripts" to 2L)
        /** Answers each POST with a receipt state, or throws. */
        var onStart: (JSONObject) -> String = { "pending" }
        var onStatus: (String) -> String = { "completed" }
        val requests = mutableMapOf<String, JSONObject>()

        fun receipt(request: JSONObject, state: String): JSONObject {
            val consent = JSONObject().put("revision", request.getLong("expected_consent_revision") + 1)
            generations.forEach { (category, generation) -> consent.put(category, JSONObject().put("enabled", false).put("generation", generation + 1)) }
            return JSONObject(request.toString()).apply { remove("expected_consent_revision") }
                .put("job_id", "job-" + request.getString("operation_id")).put("state", state)
                .put("fenced_account_epoch", request.getLong("account_epoch")).put("fenced_consent", consent)
        }
        override suspend fun readAccount(account: AccountKey, idToken: String): RemoteAccount? { calls += "read"; return remote }
        override suspend fun startCleanup(idToken: String, request: JSONObject): JSONObject {
            calls += "start"; sent += idToken to JSONObject(request.toString())
            requests[request.getString("operation_id")] = request
            return receipt(request, onStart(request))
        }
        override suspend fun cleanupStatus(idToken: String, operationId: String): JSONObject {
            calls += "status:$idToken"; return receipt(requests.getValue(operationId), onStatus(operationId))
        }
        override suspend fun revokeSessions(idToken: String, uid: String) { calls += "revoke:$uid" }
    }

    private val alice = IdentityUser("uid-alice", "alice@example.com", "Alice")
    private val bob = IdentityUser("uid-bob", "bob@example.com", "Bob")
    private lateinit var scope: CoroutineScope
    private lateinit var root: File
    private lateinit var identity: FakeIdentity
    private lateinit var session: AccountSession
    private lateinit var service: FakeService
    private lateinit var controls: AccountControls
    private val log = CopyOnWriteArrayList<String>()
    private val tokens = AtomicInteger()

    override fun setUp() {
        super.setUp()
        scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
        root = File(context.cacheDir, "account-controls-${UUID.randomUUID()}")
        identity = FakeIdentity(restored = alice).apply { refresh = { "token-${tokens.incrementAndGet()}" } }
        session = AccountSession(identity, scope)
        session.register(FakeParticipant(log, LocalCopySummary(2, 0)))
        service = FakeService()
        controls = AccountControls(session, service, CleanupTracker(AccountWorkspace(root)))
        log.clear()
    }
    override fun tearDown() { scope.cancel(); root.deleteRecursively(); super.tearDown() }

    private val aliceKey get() = AccountKey("metrix-writer", "uid-alice")
    private fun run(action: AccountAction) = runBlocking { controls.run(action) }
    private fun fences() = log.filter { it.startsWith("fence:") }

    fun testUnconfiguredBuildRefusesWithoutAnyWork() {
        service.configured = false
        for (action in listOf(AccountAction.SignOutEverywhere(LocalCopy.LEAVE), AccountAction.DeleteCloudData(listOf("settings"), false), AccountAction.DeleteAccount(LocalCopy.LEAVE))) {
            assertEquals(ControlResult.Failed(ControlError.NOT_CONFIGURED), run(action))
        }
        assertTrue(service.calls.isEmpty())
        assertFalse("freshIdToken" in identity.calls)
        assertEquals(AuthState.SIGNED_IN, session.snapshot().state)
    }

    fun testSensitiveActionsWaitForARecentSameAccountSignIn() {
        identity.authTime = { System.currentTimeMillis() / 1000 - 3_600 }
        assertEquals(ControlResult.NeedsReauth, run(AccountAction.DeleteAccount(LocalCopy.LEAVE)))
        assertEquals(ControlResult.NeedsReauth, run(AccountAction.SignOutEverywhere(LocalCopy.LEAVE)))
        assertTrue(service.calls.isEmpty())
        assertTrue(controls.jobs(aliceKey).isEmpty())
        identity.authTime = { System.currentTimeMillis() / 1000 - 10 }
        assertTrue(run(AccountAction.DeleteCloudData(listOf("dictionary"), false)) is ControlResult.Job)
    }

    fun testCloudDeletionRequestIsBuiltFromTheAccountDocumentAndFencesOnce() {
        service.onStart = { "failed_retryable" }
        val first = run(AccountAction.DeleteCloudData(listOf("transcripts", "settings"), true)) as ControlResult.Job
        val request = service.sent.single().second
        assertEquals("uid-alice", request.getString("uid"))
        assertEquals(4, request.getLong("account_epoch"))
        assertEquals(9, request.getLong("expected_consent_revision"))
        assertEquals("everywhere", request.getString("effect"))
        assertEquals(JSONArray(listOf("settings", "transcripts")).toString(), request.getJSONObject("scope").getJSONArray("categories").toString())
        assertFalse("no token in the request", request.toString().contains("token-"))
        assertEquals("failed_retryable", first.job.state)
        assertEquals(listOf("fence:uid-alice:settings:4:true", "fence:uid-alice:transcripts:3:true"), fences())
        assertEquals(AuthState.SIGNED_IN, session.snapshot().state)

        val done = runBlocking { controls.check(aliceKey, first.job.operationId) } as ControlResult.Job
        assertTrue(done.job.completed)
        assertEquals("fences are applied once, on the first receipt", 2, fences().size)
        assertTrue(controls.jobs(aliceKey).single().completed)
        assertFalse("stored jobs hold no token", File(AccountWorkspace(root).directory(aliceKey), "cleanup-jobs.json").readText().contains("token-"))
    }

    fun testAbsentAccountDocumentIsTheInitialAccount() {
        service.remote = null
        run(AccountAction.DeleteAccount(LocalCopy.LEAVE))
        val request = service.sent.single().second
        assertEquals(1, request.getLong("account_epoch"))
        assertEquals(1, request.getLong("expected_consent_revision"))
        assertEquals("account", request.getJSONObject("scope").getString("kind"))
        assertEquals("cloud_only", request.getString("effect"))
    }

    fun testLostResponseIsRetriedWithTheSameOperation() {
        var lost = true
        service.onStart = { if (lost) { lost = false; throw ServiceFailure(ControlError.OFFLINE) } else "pending" }
        assertEquals(ControlResult.Failed(ControlError.OFFLINE), run(AccountAction.DeleteCloudData(listOf("settings"), false)))
        val stored = controls.jobs(aliceKey).single()
        assertNull(stored.receipt)
        assertTrue(fences().isEmpty())

        // A restart: new controls over the same workspace, then an explicit retry.
        controls = AccountControls(session, service, CleanupTracker(AccountWorkspace(root)))
        val retried = runBlocking { controls.retry(aliceKey, stored.operationId) } as ControlResult.Job
        assertEquals(2, service.sent.size)
        assertEquals("the identical request is sent again", service.sent[0].second.toString(), service.sent[1].second.toString())
        assertEquals(stored.operationId, retried.job.operationId)
        assertEquals(listOf("fence:uid-alice:settings:4:false"), fences())
        assertEquals(1, controls.jobs(aliceKey).size)
    }

    fun testRefusalsThatProveNoJobForgetTheRequest() {
        service.onStart = { throw ServiceFailure(ControlError.STALE_CONSENT) }
        assertEquals(ControlResult.Failed(ControlError.STALE_CONSENT), run(AccountAction.DeleteCloudData(listOf("settings"), false)))
        assertTrue(controls.jobs(aliceKey).isEmpty())
        service.onStart = { throw ServiceFailure(ControlError.REAUTH_REQUIRED) }
        assertEquals(ControlResult.NeedsReauth, run(AccountAction.DeleteCloudData(listOf("settings"), false)))
        assertEquals("a refusal that may be retried keeps the same operation", 1, controls.jobs(aliceKey).size)
    }

    fun testResponseAfterSwitchingAccountsIsNotApplied() {
        service.onStart = {
            session.signOut()
            session.signInGoogle(context)
            identity.credential.complete("google-id-token")
            identity.exchange.complete(bob)
            runBlocking { withTimeout(5_000) { while (session.snapshot().account?.uid != "uid-bob") delay(10) } }
            "pending"
        }
        assertEquals(ControlResult.Dropped, run(AccountAction.DeleteCloudData(listOf("settings"), false)))
        assertTrue("no fence applied under B or A", fences().isEmpty())
        assertNull("A's receipt was never adopted", controls.jobs(aliceKey).single().receipt)
        assertTrue(controls.jobs(AccountKey("metrix-writer", "uid-bob")).isEmpty())
        assertEquals("uid-bob", session.snapshot().account?.uid)
    }

    fun testChangedGenerationBeforeSendingSendsNothing() {
        service.remote = RemoteAccount(4, "active", 9, emptyMap())
        val guarded = object : AccountServiceApi by service {
            override suspend fun readAccount(account: AccountKey, idToken: String): RemoteAccount? { session.signOut(); return service.remote }
        }
        controls = AccountControls(session, guarded, CleanupTracker(AccountWorkspace(root)))
        assertEquals(ControlResult.Dropped, run(AccountAction.DeleteAccount(LocalCopy.LEAVE)))
        assertTrue(service.sent.isEmpty())
    }

    fun testAccountDeletionFinishesWithTheRequestingTokenAndSignsOut() {
        service.onStart = { "running" }
        val started = run(AccountAction.DeleteAccount(LocalCopy.REMOVE)) as ControlResult.Job
        val requestToken = service.sent.single().first
        assertEquals(AuthState.SIGNED_IN, session.snapshot().state)
        assertEquals(RemoteAccount.CATEGORIES.map { "fence:uid-alice:$it:" }, fences().map { it.substringBeforeLast(':').substringBeforeLast(':') + ":" })
        // The Firebase user is deleted last: refreshing now fails, so the final read uses the in-memory token.
        identity.refresh = { throw AuthFailure(AuthError.REAUTH_REQUIRED) }
        val done = runBlocking { controls.check(aliceKey, started.job.operationId) } as ControlResult.Job
        assertTrue(done.job.completed)
        assertTrue(done.localCopyApplied)
        assertTrue("status:$requestToken" in service.calls)
        assertEquals(AuthState.GUEST, session.snapshot().state)
        assertTrue("remove:uid-alice" in log)
    }

    fun testUnconfirmedAccountDeletionAfterRestartIsNotReportedCompleted() {
        service.onStart = { "pending" }
        val started = run(AccountAction.DeleteAccount(LocalCopy.LEAVE)) as ControlResult.Job
        identity.refresh = { throw AuthFailure(AuthError.REAUTH_REQUIRED) }
        controls = AccountControls(session, service, CleanupTracker(AccountWorkspace(root)))
        assertEquals(ControlResult.Failed(ControlError.UNAUTHENTICATED), runBlocking { controls.check(aliceKey, started.job.operationId) })
        assertEquals("pending", controls.jobs(aliceKey).single().state)
    }

    fun testReceiptForAnotherOperationIsRejected() {
        val wrong = object : AccountServiceApi by service {
            override suspend fun startCleanup(idToken: String, request: JSONObject) =
                service.receipt(JSONObject(request.toString()).put("operation_id", UUID.randomUUID().toString()), "completed")
        }
        controls = AccountControls(session, wrong, CleanupTracker(AccountWorkspace(root)))
        assertEquals(ControlResult.Failed(ControlError.INVALID_RESPONSE), run(AccountAction.DeleteCloudData(listOf("settings"), false)))
        assertTrue(fences().isEmpty())
        assertNull(controls.jobs(aliceKey).single().receipt)
    }

    fun testSignOutEverywhereRevokesThenSignsOutWithTheLocalCopyChoice() {
        val result = run(AccountAction.SignOutEverywhere(LocalCopy.KEEP))
        assertEquals(ControlResult.SignedOutEverywhere(true), result)
        assertEquals(listOf("revoke:uid-alice"), service.calls)
        assertEquals(AuthState.GUEST, session.snapshot().state)
        assertEquals(listOf("suspend:uid-alice", "keep:uid-alice"), log)
    }

    fun testWorkspacesAreSeparatePerAccountAndHideTheUid() {
        val workspace = AccountWorkspace(root)
        val a = workspace.directory(aliceKey)
        val b = workspace.directory(AccountKey("metrix-writer", "uid-bob"))
        assertFalse(a == b)
        assertFalse(a.path.contains("uid-alice"))
        assertEquals(File(root, "accounts/metrix-writer"), a.parentFile)
        assertEquals(32, a.name.length)
        try { workspace.directory(AccountKey("../escape", "uid-alice")); fail() } catch (_: IllegalArgumentException) {}
    }

    fun testFirestoreAccountDocumentAndResetsAreParsed() {
        fun integer(value: Long) = JSONObject().put("integerValue", value.toString())
        fun map(fields: JSONObject) = JSONObject().put("mapValue", JSONObject().put("fields", fields))
        val reset = map(JSONObject().put("generation", integer(7)).put("job_id", JSONObject().put("stringValue", "job")))
        val document = JSONObject().put("fields", JSONObject()
            .put("account_epoch", integer(3)).put("account_state", JSONObject().put("stringValue", "active"))
            .put("consent", map(JSONObject().put("revision", integer(12))))
            .put("resets", map(JSONObject().put("dictionary", reset))))
        val remote = RemoteAccount.fromFirestore(document)
        assertEquals(RemoteAccount(3, "active", 12, mapOf("dictionary" to 7L)), remote)
        assertEquals(listOf(CloudFence("dictionary", 7, true)), remote.pendingResets(mapOf("dictionary" to 6L)))
        assertTrue(remote.pendingResets(mapOf("dictionary" to 7L)).isEmpty())
    }
}
