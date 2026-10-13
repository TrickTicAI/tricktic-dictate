@file:Suppress("DEPRECATION")
package com.metrix.writer

import android.test.InstrumentationTestCase
import android.test.InstrumentationTestRunner
import android.util.Base64
import com.metrix.writer.account.AccountSession
import com.metrix.writer.account.IdentityUser
import com.metrix.writer.sync.AccountSync
import com.metrix.writer.sync.CloudFailure
import com.metrix.writer.sync.FirestoreRest
import com.metrix.writer.sync.Planner
import com.metrix.writer.sync.PortableStore
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.runBlocking
import org.json.JSONObject
import java.net.HttpURLConnection
import java.net.URL
import java.util.UUID

/**
 * The Android adapter against the real Firestore emulator and infra/firebase/firestore.rules,
 * over the same REST calls the app makes. Run through apps/android/scripts/account-sync-emulator.py,
 * which starts a private emulator with a demo- project and passes `-e firestoreEmulator host:port`.
 * Without that argument every test returns immediately; it never contacts a real project.
 */
class FirestoreEmulatorTest : InstrumentationTestCase() {
    private val context get() = instrumentation.targetContext
    private val project = "demo-metrix-writer"
    private val alice = IdentityUser("synthetic-user-a", null, null)
    private val bob = IdentityUser("synthetic-user-b", null, null)
    private var host: String? = null
    private lateinit var sandbox: PortableSandbox
    private lateinit var scope: CoroutineScope

    override fun setUp() {
        super.setUp()
        host = (instrumentation as? InstrumentationTestRunner)?.arguments?.getString("firestoreEmulator")
        if (host == null) return
        sandbox = PortableSandbox(context)
        scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
        val connection = URL("http://$host/emulator/v1/projects/$project/databases/(default)/documents").openConnection() as HttpURLConnection
        connection.requestMethod = "DELETE"; check(connection.responseCode == 200); connection.disconnect()
    }
    override fun tearDown() { if (host != null) { scope.cancel(); sandbox.restore() }; super.tearDown() }

    /** An unsigned emulator ID token for a Google-provider Firebase user. */
    private fun token(uid: String): String {
        fun part(value: JSONObject) = Base64.encodeToString(value.toString().toByteArray(), Base64.URL_SAFE or Base64.NO_PADDING or Base64.NO_WRAP)
        val now = System.currentTimeMillis() / 1000
        return part(JSONObject().put("alg", "none").put("typ", "JWT")) + "." + part(JSONObject().put("sub", uid).put("user_id", uid).put("iat", now)
            .put("exp", now + 3600).put("auth_time", now).put("aud", project).put("iss", "https://securetoken.google.com/$project")
            .put("firebase", JSONObject().put("sign_in_provider", "google.com").put("identities", JSONObject()))) + "."
    }
    private val rest get() = FirestoreRest(project, "http://$host")
    private fun engine(user: IdentityUser): Pair<AccountSync, AccountSession> {
        val session = AccountSession(SyncIdentity(user, ::token), scope)
        return AccountSync(context, session, { rest }, scope, pullIntervalMs = 60 * 60_000L) to session
    }
    private fun owner(path: String) = runBlocking { rest.get("owner", path) }
    private fun records(uid: String) = runBlocking { listOf("settings", "dictionary").flatMap { rest.records("owner", uid, it, null, 500) } }.associateBy { it.getString("id") }
    private fun store() = PortableStore(context)

    fun testTranscriptRoundTripAndFamilyDeletionUnderRealRules() {
        if (host == null) return
        val session = AccountSession(SyncIdentity(alice, ::token), scope)
        val sync = com.metrix.writer.sync.TranscriptSync(context, session, { rest }, scope, true, 2)
        val store = SessionStore(java.io.File(context.filesDir, "recordings"), context)
        val item = store.create("mixed"); store.finishCapture(item, null)
        val options = JSONObject().put("selection", JSONObject().put("provider", "gemini").put("model", "synthetic-stt"))
            .put("input_language", "mixed").put("output_language", JSONObject().put("mode", "preserve"))
        item.getJSONArray("attempts").put(JSONObject().put("id", UUID.randomUUID().toString()).put("request", JSONObject().put("options", options))
            .put("status", JSONObject().put("state", "succeeded").put("result", JSONObject().put("transcript", JSONObject().put("raw_text", "שלום English 🌍"))))
            .put("dictionary_text", "corrected עברית").put("cleaned_text", "cleaned English"))
        store.save(item)
        runBlocking { sync.enable("include_existing"); sync.runOnce() }
        assertEquals(null, sync.status.value.problem)
        assertEquals(3, sync.status.value.records.size)
        val original = sync.status.value.records.first { it.getJSONObject("content").getJSONObject("data").getString("kind") == "original" }
        runBlocking { sync.edit(original.getString("id"), "user edit עברית"); sync.runOnce() }
        assertEquals(null, sync.status.value.problem)
        assertEquals(4, sync.status.value.records.size)
        val before = sync.status.value.records.map { com.metrix.writer.sync.StrictJson.canonical(it) }.toSet()
        sandbox.clear()
        val fresh = com.metrix.writer.sync.TranscriptSync(context, session, { rest }, scope, true, 2)
        runBlocking { fresh.enable("future_only"); fresh.runOnce() }
        assertEquals(null, fresh.status.value.problem)
        assertEquals(before, fresh.status.value.records.map { com.metrix.writer.sync.StrictJson.canonical(it) }.toSet())
        assertFalse(java.io.File(context.filesDir, "recordings").exists())
        runBlocking { fresh.delete(original.getString("id")); fresh.runOnce(); fresh.runOnce() }
        assertEquals(null, fresh.status.value.problem)
        assertTrue(fresh.status.value.records.isEmpty())
        val documents = runBlocking { rest.records("owner", alice.uid, "transcripts", null, 500) }
        assertEquals(4, documents.size)
        assertTrue(documents.all { it.getJSONObject("content").getString("state") == "deleted" })
    }

    fun testEnableUploadPullConflictAndResolveUnderTheRealRules() {
        if (host == null) return
        val entry = JSONObject().put("text", "עברית and English 🌍")
        CompanionPreferences(context).saveDocument(dictionary("hints" to entry, "snippets" to JSONObject().put("phrase", "sig").put("text", "שלום\n‏bidi")))
        val (sync, _) = engine(alice)
        runBlocking {
            val preview = sync.preview(setOf("settings", "dictionary"))
            assertEquals("previewing wrote nothing", null, rest.get("owner", Planner.account(alice.uid)))
            sync.enable(preview, emptyMap())
            sync.syncNow().join()
        }
        assertEquals(null, sync.status.value.problem)
        assertEquals(0, sync.status.value.pending)
        val account = owner(Planner.account(alice.uid))!!
        assertTrue(account.getJSONObject("consent").getJSONObject("dictionary").getBoolean("enabled"))
        assertEquals(2L, account.getJSONObject("consent").getLong("revision"))
        val stored = records(alice.uid)
        val id = entry.getString("id")
        assertEquals("עברית and English 🌍", stored.getValue(id).getJSONObject("content").getJSONObject("data").getString("text"))
        assertTrue("settings uploaded too", stored.values.any { it.getString("category") == "settings" })
        assertNotNull("device acknowledgement accepted by the rules", owner(Planner.device(alice.uid, store().deviceId)))

        // Another device (the same account's desktop) edits the entry through the rules.
        fun op(base: Long, text: String) = JSONObject().put("schema_version", 1).put("uid", alice.uid).put("account_epoch", 1)
            .put("consent_generation", account.getJSONObject("consent").getJSONObject("dictionary").getLong("generation")).put("category", "dictionary")
            .put("operation_id", UUID.randomUUID().toString()).put("device_id", "22222222-2222-4222-8222-000000000001").put("record_id", id)
            .put("origin_device_id", store().deviceId).put("base_revision", base)
            .put("content", JSONObject().put("state", "live").put("data", JSONObject().put("kind", "hint").put("enabled", true).put("text", text)))
        val remote = op(1, "from the computer")
        val plan = runBlocking { rest.transaction(token(alice.uid)) { read -> Planner.plan(alice.uid, remote, read = read).let { it to it.writes } } }
        assertEquals("acknowledged", plan.status)
        // A lost acknowledgement retried against the emulator returns the same revision and writes nothing.
        val retry = runBlocking { rest.transaction(token(alice.uid)) { read -> Planner.plan(alice.uid, remote, read = read).let { it to it.writes } } }
        assertEquals(2L, retry.revision); assertTrue(retry.writes.isEmpty())
        runBlocking { sync.syncNow().join() }
        assertEquals("from the computer", store().projection().getValue(id).data.getString("text"))

        // Both change it: the phone's write conflicts, keeps both, and resolves through a new operation.
        runBlocking { rest.transaction(token(alice.uid)) { read -> Planner.plan(alice.uid, op(2, "computer again"), read = read).let { it to it.writes } } }
        val document = CompanionPreferences(context).document()
        (0 until document.getJSONArray("hints").length()).map { document.getJSONArray("hints").getJSONObject(it) }.single { it.getString("id") == id }.put("text", "phone edit")
        CompanionPreferences(context).saveDocument(document)
        runBlocking { sync.syncNow().join() }
        assertEquals(id, sync.status.value.conflicts.single().recordId)
        runBlocking { sync.resolve(id, "phone"); sync.syncNow().join() }
        assertEquals("phone edit", records(alice.uid).getValue(id).getJSONObject("content").getJSONObject("data").getString("text"))
        assertEquals(4L, records(alice.uid).getValue(id).getLong("revision"))

        // Turning the dictionary off is a consent CAS the rules accept; cloud data stays.
        runBlocking { sync.disable("dictionary") }
        assertFalse(owner(Planner.account(alice.uid))!!.getJSONObject("consent").getJSONObject("dictionary").getBoolean("enabled"))
        assertTrue(records(alice.uid).containsKey(id))
    }

    fun testAnotherAccountCanNeitherReadNorReceiveTheData() {
        if (host == null) return
        CompanionPreferences(context).saveDocument(dictionary("hints" to JSONObject().put("text", "alice only")))
        val (aliceSync, _) = engine(alice)
        runBlocking { aliceSync.enable(aliceSync.preview(setOf("dictionary")), emptyMap()); aliceSync.syncNow().join() }
        assertEquals(1, records(alice.uid).size)
        for (path in listOf(Planner.account(alice.uid), Planner.record(alice.uid, records(alice.uid).keys.single()))) {
            try { runBlocking { rest.get(token(bob.uid), path) }; fail(path) } catch (failure: CloudFailure) { assertEquals(CloudFailure.Kind.DENIED, failure.kind) }
        }
        try { runBlocking { rest.records(token(bob.uid), alice.uid, "dictionary", null, 500) }; fail() } catch (failure: CloudFailure) { assertEquals(CloudFailure.Kind.DENIED, failure.kind) }
        val (bobSync, _) = engine(bob)
        val preview = runBlocking { bobSync.preview(setOf("dictionary")) }
        assertFalse(preview.cloudHasAccount)
        assertEquals("B sees only this phone's entries, as uploads it could choose", 1, preview.count("dictionary", "upload"))
        assertTrue(records(bob.uid).isEmpty())
    }

    fun testATrustedCloudDeletionFencesQueuedWorkAndTheRulesRefuseIt() {
        if (host == null) return
        CompanionPreferences(context).saveDocument(dictionary("hints" to JSONObject().put("text", "before")))
        val (sync, _) = engine(alice)
        runBlocking { sync.enable(sync.preview(setOf("dictionary")), emptyMap()); sync.syncNow().join() }
        val id = records(alice.uid).keys.single()
        // The trusted cleanup (owner access, bypassing rules) turns the dictionary off and advances its generation.
        val account = owner(Planner.account(alice.uid))!!
        val consent = account.getJSONObject("consent")
        consent.put("dictionary", JSONObject().put("enabled", false).put("generation", consent.getJSONObject("dictionary").getLong("generation") + 1))
            .put("revision", consent.getLong("revision") + 1)
        runBlocking { rest.transaction("owner") { _ -> Unit to listOf(Planner.Write(Planner.account(alice.uid), account)) } }
        val document = CompanionPreferences(context).document()
        document.getJSONArray("hints").getJSONObject(0).put("text", "stale offline edit")
        CompanionPreferences(context).saveDocument(document)
        runBlocking { sync.syncNow().join() }
        assertEquals("turned_off", sync.status.value.problem)
        assertEquals("before", records(alice.uid).getValue(id).getJSONObject("content").getJSONObject("data").getString("text"))
        // A write that skips the planner with the old generation is refused by the rules themselves.
        val record = records(alice.uid).getValue(id)
        val stale = JSONObject().put("schema_version", 1).put("uid", alice.uid).put("account_epoch", 1).put("consent_generation", 2).put("category", "dictionary")
            .put("operation_id", UUID.randomUUID().toString()).put("device_id", store().deviceId).put("record_id", id).put("origin_device_id", record.getString("origin_device_id"))
            .put("base_revision", record.getLong("revision")).put("content", JSONObject().put("state", "live").put("data", JSONObject().put("kind", "hint").put("enabled", true).put("text", "forced")))
        try {
            runBlocking { rest.transaction(token(alice.uid)) { _ -> Unit to listOf(Planner.Write(Planner.record(alice.uid, id), Planner.recordDocument(stale, record.getLong("revision") + 1)),
                Planner.Write(Planner.receipt(alice.uid, stale.getString("operation_id")), JSONObject().put("operation", stale).put("revision", record.getLong("revision") + 1))) } }
            fail("stale generation accepted")
        } catch (failure: CloudFailure) { assertEquals(CloudFailure.Kind.DENIED, failure.kind) }
    }
}
