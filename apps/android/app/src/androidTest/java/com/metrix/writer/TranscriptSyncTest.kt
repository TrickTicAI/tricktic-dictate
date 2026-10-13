@file:Suppress("DEPRECATION")
package com.metrix.writer

import android.test.InstrumentationTestCase
import com.metrix.writer.account.*
import com.metrix.writer.sync.*
import kotlinx.coroutines.*
import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.util.UUID

class TranscriptSyncTest : InstrumentationTestCase() {
    private val context get() = instrumentation.targetContext
    private lateinit var sandbox: PortableSandbox
    private lateinit var scope: CoroutineScope
    private lateinit var cloud: FakeCloud
    private lateinit var identity: SyncIdentity
    private lateinit var session: AccountSession
    private val alice = IdentityUser("synthetic-text-a", null, null)
    private val bob = IdentityUser("synthetic-text-b", null, null)
    override fun setUp() {
        super.setUp(); sandbox = PortableSandbox(context)
        scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
        cloud = FakeCloud(); identity = SyncIdentity(alice); session = AccountSession(identity, scope)
        TranscriptStore.owner = { session.snapshot().account }
    }
    override fun tearDown() { runBlocking { scope.coroutineContext[Job]?.cancelAndJoin() }; TranscriptStore.owner = null; sandbox.restore(); super.tearDown() }
    private fun engine(allowed: Boolean = true, pageSize: Int = 2) = TranscriptSync(context, session, { cloud }, scope, allowed, pageSize, autoContinue = false)
    private fun store() = SessionStore(File(context.filesDir, "recordings"), context)
    private fun recording(text: String = "English עברית 🌍", language: String = "mixed"): JSONObject {
        val store = store(); val item = store.create(language); store.finishCapture(item, null)
        val options = JSONObject().put("selection", JSONObject().put("provider", "gemini").put("model", "fixture-model"))
            .put("input_language", language).put("output_language", JSONObject().put("mode", "preserve"))
        item.getJSONArray("attempts").put(JSONObject().put("id", UUID.randomUUID().toString())
            .put("request", JSONObject().put("options", options).put("secret", "NEVER-PORTABLE"))
            .put("status", JSONObject().put("state", "succeeded").put("result", JSONObject().put("transcript", JSONObject().put("raw_text", text)))))
        item.put("pending_cloud", JSONObject().put("credential_id", "NEVER-PORTABLE")); store.save(item)
        return item
    }
    private fun TranscriptSync.enableAndRun(mode: String = "include_existing") = runBlocking { enable(mode); runOnce(); while (status.value.moreWork && status.value.problem == null) runOnce() }
    private fun TranscriptSync.pass() = runBlocking { runOnce(); while (status.value.moreWork && status.value.problem == null) runOnce() }
    private fun liveRows() = cloud.records(alice.uid).values.filter { it.getJSONObject("content").getString("state") == "live" }
    private fun original() = liveRows().first { it.getJSONObject("content").getJSONObject("data").getString("kind") == "original" }

    fun testFixed1k10kRemoteSyncReadsOnePageAndResumesCursorAfterRestart() {
        for (count in listOf(1000, 10000)) {
            sandbox.clear(); cloud.documents.clear()
            recording("synthetic template")
            val sync = engine(pageSize = 100); sync.enableAndRun()
            val template = StrictJson.copy(cloud.documents.values.first { it.optString("category") == "transcripts" })
            cloud.documents.keys.filter { it.startsWith("users/${alice.uid}/records/") }.forEach { cloud.documents.remove(it) }
            for (i in 1..count) {
                val id = UUID(0, i.toLong()).toString()
                val row = StrictJson.copy(template).put("id", id)
                row.getJSONObject("content").getJSONObject("data").put("attempt_id", id)
                cloud.documents["users/${alice.uid}/records/$id"] = row
            }
            val before = cloud.reads.get()
            runBlocking { sync.runOnce() }
            assertTrue("one sync pass reads one bounded cloud page", cloud.reads.get() - before < 120)
            assertEquals(100, sync.status.value.records.size)
            assertTrue(sync.status.value.moreWork)
            val restarted = engine(pageSize = 100)
            val restartReads = cloud.reads.get()
            runBlocking { restarted.runOnce() }
            assertTrue("restart resumes one page", cloud.reads.get() - restartReads < 120)
            val state = TranscriptDatabase(android.util.AtomicFile(File(AccountWorkspace(context.filesDir).directory(session.snapshot().account!!), "transcripts.json")), setOf("records", "base", "outbox", "excluded", "hidden", "failed", "sent")).load { JSONObject() }
            assertEquals(UUID(0, 200).toString(), state.getString("poll_cursor"))
            assertTrue(restarted.status.value.moreWork)
        }
    }

    fun testSignInDoesNotUploadAndBuildLevelDisableFailsClosed() {
        recording()
        val productionDefaults = TranscriptSync(context, session, { cloud }, scope)
        assertTrue(productionDefaults.uploadsAllowed)
        productionDefaults.pass()
        assertTrue("sign-in still grants no transcript consent", cloud.documents.isEmpty())
        val sync = engine(false); sync.pass()
        assertTrue(cloud.documents.isEmpty())
        try { sync.enableAndRun(); fail("privacy gate must refuse") } catch (_: IllegalStateException) {}
        assertTrue(cloud.documents.isEmpty())
    }
    fun testFutureOnlyExcludesExistingRecordingAndItsLaterAttemptsAndVersions() {
        val old = recording("old"); val sync = engine(); sync.enableAndRun("future_only")
        assertTrue(liveRows().isEmpty())
        old.getJSONArray("attempts").getJSONObject(0).put("cleaned_text", "old cleanup")
        store().save(old); sync.pass(); assertTrue(liveRows().isEmpty())
        recording("future"); sync.pass(); assertEquals(1, liveRows().size)
        assertEquals("future", original().getJSONObject("content").getJSONObject("data").getString("text"))
    }
    fun testEnglishHebrewMixedVersionsRoundTripAndNoExecutionFields() {
        listOf("en" to "English", "he" to "שלום נִקּוּד", "mixed" to "עברית English ‏🌍").forEach { (lang, text) ->
            val item = recording(text, lang)
            item.getJSONArray("attempts").getJSONObject(0).put("dictionary_text", "$text corrected").put("cleaned_text", "$text cleaned")
            store().save(item)
        }
        val sync = engine(); sync.enableAndRun()
        assertEquals(9, liveRows().size)
        File(context.cacheDir, "android-transcript-wire.json").writeText(JSONArray(liveRows()).toString())
        val exported = liveRows().map { it.getJSONObject("content").getJSONObject("data") }
        assertTrue(exported.any { it.getString("text") == "עברית English ‏🌍" })
        assertFalse(cloud.documents.toString().contains("NEVER-PORTABLE"))
        assertFalse(cloud.documents.toString().contains("audio/wav"))
        sandbox.clear() // Another installation restores only text, no native recording metadata.
        val fresh = engine(); fresh.enableAndRun("future_only")
        assertEquals(9, fresh.status.value.records.size)
        assertEquals(exported.map { StrictJson.canonical(it) }.toSet(), fresh.status.value.records.map { StrictJson.canonical(it.getJSONObject("content").getJSONObject("data")) }.toSet())
        assertFalse(File(context.filesDir, "recordings").exists())
    }
    fun testOfflineOutboxAndLostAcknowledgementSurviveProcessRecreation() {
        val sync = engine(); sync.enableAndRun()
        recording(); cloud.offline = true; sync.pass()
        assertEquals(1, sync.status.value.pending)
        cloud.offline = false; cloud.loseAck = { writes -> writes.any { it.path.contains("/records/") } }
        val recreated = engine(); recreated.pass()
        assertEquals(1, liveRows().size)
        val before = original().toString()
        val again = engine(); again.pass()
        assertEquals(before, original().toString())
        assertEquals(0, again.status.value.pending)
    }
    fun testSeparateEditsRetainOriginalAndSiblings() {
        recording("original"); val sync = engine(); sync.enableAndRun()
        val id = original().getString("id")
        runBlocking { sync.edit(id, "edit one"); sync.edit(id, "עריכה שתיים") }; sync.pass()
        assertEquals(3, liveRows().size)
        assertEquals("original", original().getJSONObject("content").getJSONObject("data").getString("text"))
        assertEquals(2, liveRows().count { it.getJSONObject("content").getJSONObject("data").optString("parent_record_id") == id })
    }
    fun testEditingSelectedDerivedVersionKeepsItsParentAndOriginalAfterRestart() {
        val item = recording("raw")
        item.getJSONArray("attempts").getJSONObject(0).put("cleaned_text", "polished")
        store().save(item); val sync = engine(); sync.enableAndRun()
        val raw = original().getString("id")
        val selected = liveRows().first { it.getJSONObject("content").getJSONObject("data").getString("kind") == "cleaned" }.getString("id")
        cloud.offline = true
        runBlocking { sync.edit(selected, "offline edit") }; sync.pass()
        cloud.offline = false; val restarted = engine(); restarted.pass()
        val data = liveRows().first { it.getJSONObject("content").getJSONObject("data").optString("text") == "offline edit" }.getJSONObject("content").getJSONObject("data")
        assertEquals(raw, data.getString("original_record_id")); assertEquals(selected, data.getString("parent_record_id"))
        assertEquals("raw", original().getJSONObject("content").getJSONObject("data").getString("text"))
    }

    fun testLocalRecordingDeletionTombstonesFamilyAndRetainsNoPortableText() {
        val item = recording(); item.getJSONArray("attempts").getJSONObject(0).put("cleaned_text", "cleaned"); store().save(item)
        val sync = engine(); sync.enableAndRun(); assertEquals(2, liveRows().size)
        store().deleteSaved(item.getString("id")); sync.pass(); sync.pass()
        assertTrue(liveRows().isEmpty()); assertTrue(sync.status.value.records.isEmpty())
        val fresh = engine(); fresh.pass(); assertTrue(fresh.status.value.records.isEmpty())
    }
    fun testRemoteDeletionCannotResurrectLocalAudioOrOldText() {
        val item = recording(); val sync = engine(); sync.enableAndRun(); val original = original()
        val consent = cloud.consent(alice.uid)
        val op = JSONObject().put("schema_version", 1).put("uid", alice.uid).put("account_epoch", 1)
            .put("category", "transcripts").put("consent_generation", consent.getJSONObject("transcripts").getLong("generation"))
            .put("history_enrollment_id", consent.getJSONObject("transcripts").getJSONObject("history").getString("enrollment_id"))
            .put("operation_id", UUID.randomUUID().toString()).put("device_id", UUID.randomUUID().toString())
            .put("record_id", original.getString("id")).put("origin_device_id", original.getString("origin_device_id"))
            .put("base_revision", 1).put("content", JSONObject().put("state", "deleted"))
        runBlocking { assertEquals("acknowledged", cloud.remote(alice.uid, op).status) }
        sync.pass(); sync.pass()
        assertTrue(sync.status.value.records.isEmpty()); assertTrue(liveRows().isEmpty())
        assertTrue("native audio stays", store().audio(item).exists())
    }
    fun testConsentGenerationChangeFencesOfflineQueue() {
        val sync = engine(); sync.enableAndRun(); recording(); cloud.offline = true; sync.pass()
        cloud.offline = false
        cloud.setConsent(alice.uid, Planner.nextConsent(cloud.consent(alice.uid), "transcripts", false))
        val recreated = engine(); recreated.pass()
        assertFalse(recreated.status.value.enabled); assertEquals(0, recreated.status.value.pending); assertTrue(liveRows().isEmpty())
    }
    fun testAccountSwitchDoesNotUploadOrExposeOtherAccountsCopies() {
        recording("A's private text"); val a = engine(); a.enableAndRun()
        runBlocking { session.signOut(LocalCopy.LEAVE) }
        identity.next = bob; session.signInGoogle(context)
        runBlocking { withTimeout(5000) { while (session.snapshot().account?.uid != bob.uid) delay(10) } }
        val b = engine(); b.enableAndRun("include_existing")
        assertTrue(cloud.records(bob.uid).isEmpty()); assertTrue(b.status.value.records.isEmpty())
        recording("B's text"); b.pass()
        assertEquals(1, cloud.records(bob.uid).size)
        assertEquals(1, liveRows().size)
    }
    fun testOfflineOptOutPersistsAcrossRestart() {
        val sync = engine(); sync.enableAndRun(); recording(); cloud.offline = true; sync.pass()
        runBlocking { sync.disable() }; assertEquals(0, sync.status.value.pending)
        cloud.offline = false; val recreated = engine(); recreated.pass()
        assertFalse(cloud.consent(alice.uid).getJSONObject("transcripts").getBoolean("enabled"))
        assertTrue(liveRows().isEmpty())
    }
    fun testAccountChangeDuringDownloadDiscardsResponse() {
        recording(); engine().enableAndRun()
        sandbox.clear()
        val wrapped = object : CloudStore by cloud {
            override suspend fun records(token: String, uid: String, category: String, after: String?, limit: Int): List<JSONObject> {
                val page = cloud.records(token, uid, category, after, limit)
                session.signOut(LocalCopy.LEAVE)
                return page
            }
        }
        val sync = TranscriptSync(context, session, { wrapped }, scope, true, 2)
        runBlocking { sync.enable("future_only"); sync.runOnce() }
        val account = AccountKey("demo-metrix-writer", alice.uid)
        assertEquals(0, sync.localCopy(account).items)
    }
    fun testDownloadedCopiesCanBeKeptOrRemovedWithoutCloudDeletion() {
        recording(); engine().enableAndRun(); sandbox.clear()
        val sync = engine(); sync.enableAndRun("future_only")
        val account = AccountKey("demo-metrix-writer", alice.uid)
        assertEquals(1, sync.localCopy(account).items)
        sync.keepLocalCopy(account)
        assertTrue(File(context.filesDir, "kept-transcripts.json").isFile)
        sync.removeLocalCopy(account)
        assertEquals(0, sync.localCopy(account).items)
        assertEquals(1, liveRows().size)
    }
    fun testOriginalTombstoneHidesDescendantArrivingOnLaterPageAfterRestart() {
        val sync = engine(pageSize = 1); sync.enableAndRun("future_only")
        val records = (SyncFixtures.json(instrumentation, "wire.json") as JSONObject).getJSONArray("records")
        val shared = (0 until records.length()).map { records.getJSONObject(it) }.filter { it.optString("category") == "transcripts" && it.getJSONObject("content").getString("state") == "live" }
        val original = StrictJson.copy(shared.first { it.getJSONObject("content").getJSONObject("data").getString("kind") == "original" })
        val child = shared.first { it.getJSONObject("content").getJSONObject("data").optString("original_record_id") == original.getString("id") }
        original.put("revision", 2).put("content", JSONObject().put("state", "deleted"))
        cloud.documents[Planner.record(alice.uid, original.getString("id"))] = original
        cloud.documents[Planner.record(alice.uid, child.getString("id"))] = child
        runBlocking { sync.runOnce() } // Tombstone is first; leave the cursor durable.
        val restarted = engine(pageSize = 1); restarted.pass()
        assertTrue("later orphan must not resurrect an original's text family", restarted.status.value.records.isEmpty())
    }

    fun testDesktopFixtureImportsExactlyAndConcurrentRemoteEditSurvives() {
        val sync = engine(); sync.enableAndRun("future_only")
        val records = (SyncFixtures.json(instrumentation, "wire.json") as JSONObject).getJSONArray("records")
        val shared = (0 until records.length()).map { records.getJSONObject(it) }.filter { it.optString("category") == "transcripts" && it.getJSONObject("content").getString("state") == "live" }
        // These are the exact shared desktop wire documents, including a chain of derived versions.
        shared.forEach { row -> cloud.documents[Planner.record(alice.uid, row.getString("id"))] = StrictJson.copy(row) }
        sync.pass()
        assertEquals(shared.map { StrictJson.canonical(it) }.toSet(), sync.status.value.records.map { StrictJson.canonical(it) }.toSet())
        val original = shared.first { it.getJSONObject("content").getJSONObject("data").getString("kind") == "original" }
        val id = original.getString("id")
        runBlocking { sync.edit(id, "Android offline edit") }
        val remoteId = UUID.randomUUID().toString()
        cloud.documents[Planner.record(alice.uid, remoteId)] = JSONObject().put("schema_version", 1).put("id", remoteId).put("category", "transcripts")
            .put("origin_device_id", original.getString("origin_device_id")).put("revision", 1)
            .put("content", JSONObject().put("state", "live").put("data", TranscriptStore.derived("edited", id, id, "Desktop offline edit")))
        // Fixture family indexes are needed for the local edit's real planner commit.
        cloud.documents[Planner.family(alice.uid, id)] = JSONObject().put("deleted", false).put("member_ids", JSONArray())
        sync.pass()
        val texts = sync.status.value.records.map { it.getJSONObject("content").getJSONObject("data").getString("text") }
        assertTrue(texts.contains("Android offline edit")); assertTrue(texts.contains("Desktop offline edit"))
    }

    fun testSharedWireFixturesAndUnknownFieldsAreValidated() {
        val root = SyncFixtures.json(instrumentation, "wire.json") as JSONObject
        fun visit(value: Any?) {
            when (value) {
                is JSONObject -> {
                    if (value.optString("category") == "transcripts" && value.has("revision") && value.has("content")) Contract.checkRecord(value)
                    value.keys().forEach { visit(value.opt(it)) }
                }
                is JSONArray -> for (i in 0 until value.length()) visit(value.opt(i))
            }
        }
        visit(root)
        recording(); val sync = engine(); sync.enableAndRun()
        val data = original().getJSONObject("content").getJSONObject("data").put("audio_path", "secret")
        assertFalse(Contract.isTranscript(UUID.randomUUID().toString(), data))
    }
}
