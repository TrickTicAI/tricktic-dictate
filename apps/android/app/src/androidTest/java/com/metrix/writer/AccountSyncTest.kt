@file:Suppress("DEPRECATION")
package com.metrix.writer

import android.test.InstrumentationTestCase
import com.metrix.writer.account.AccountKey
import com.metrix.writer.account.AccountSession
import com.metrix.writer.account.AuthState
import com.metrix.writer.account.IdentityUser
import com.metrix.writer.sync.AccountSync
import com.metrix.writer.sync.Planner
import com.metrix.writer.sync.PortableStore
import com.metrix.writer.sync.StrictJson
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.runBlocking
import org.json.JSONObject
import java.util.UUID

/**
 * The Android settings/dictionary sync engine (MW-OUATH-7) over an in-memory Firestore: consent
 * before upload, durable outbox across process death and network loss, lost acknowledgements,
 * conflicts, remote deletion, account isolation and generation fences.
 */
class AccountSyncTest : InstrumentationTestCase() {
    private val context get() = instrumentation.targetContext
    private val alice = IdentityUser("synthetic-user-a", null, null)
    private val bob = IdentityUser("synthetic-user-b", null, null)
    private val otherDevice = "22222222-2222-4222-8222-000000000001"
    private lateinit var sandbox: PortableSandbox
    private lateinit var scope: CoroutineScope
    private lateinit var cloud: FakeCloud
    private lateinit var identity: SyncIdentity
    private lateinit var session: AccountSession

    override fun setUp() {
        super.setUp()
        sandbox = PortableSandbox(context)
        scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
        cloud = FakeCloud()
        identity = SyncIdentity(alice)
        session = AccountSession(identity, scope)
    }
    override fun tearDown() { scope.cancel(); sandbox.restore(); super.tearDown() }

    /** A new engine is what the app builds after process death: it only has the files. */
    private fun engine(within: CoroutineScope = scope) = AccountSync(context, session, { cloud }, within, pullIntervalMs = 60 * 60_000L)
    private fun AccountSync.pass() = runBlocking { syncNow().join() }
    private fun hint(text: String) = JSONObject().put("text", text)
    private fun entries() = PortableStore(context).projection().values.filter { it.category == "dictionary" }.associate { it.id to it.data.optString("text", it.data.optString("to")) }
    private fun dictionaryRecords(uid: String = alice.uid) = cloud.records(uid).filterValues { it.getString("category") == "dictionary" }
    private fun liveText(record: JSONObject) = record.getJSONObject("content").optJSONObject("data")?.optString("text")

    private fun enable(sync: AccountSync, choices: Map<String, String> = emptyMap(), categories: Set<String> = setOf("settings", "dictionary")) = runBlocking {
        val preview = sync.preview(categories)
        sync.enable(preview, choices + preview.choices.filter { it.id !in choices }.associate { it.id to if (it.action == "conflict") "phone" else "keep" })
        sync.syncNow().join()
    }

    private fun remoteOp(recordId: String, base: Long, content: JSONObject, category: String = "dictionary", origin: String = otherDevice): JSONObject {
        val consent = cloud.consent(alice.uid)
        return JSONObject().put("schema_version", 1).put("uid", alice.uid).put("account_epoch", consent.getLong("account_epoch"))
            .put("consent_generation", consent.getJSONObject(category).getLong("generation")).put("category", category)
            .put("operation_id", UUID.randomUUID().toString()).put("device_id", otherDevice).put("record_id", recordId)
            .put("origin_device_id", origin).put("base_revision", base).put("content", content)
    }
    private fun live(data: JSONObject) = JSONObject().put("state", "live").put("data", data)
    private fun hintData(text: String) = JSONObject().put("kind", "hint").put("enabled", true).put("text", text)

    fun testSigningInAndPreviewingUploadNothing() {
        CompanionPreferences(context).saveDocument(dictionary("hints" to hint("פרטי private")))
        val sync = engine().also { it.start() }
        sync.pass()
        val preview = runBlocking { sync.preview(setOf("settings", "dictionary")) }
        assertEquals(1, preview.count("dictionary", "upload"))
        assertFalse(preview.cloudHasAccount)
        assertEquals("no document was written", 0, cloud.commits.get())
        assertTrue(cloud.documents.isEmpty())
    }

    fun testEnableUploadsOnlyChosenCategoriesAndRecordsConsent() {
        CompanionPreferences(context).saveDocument(dictionary("hints" to hint("Metrix"), "snippets" to JSONObject().put("phrase", "sig").put("text", "שלום 🌍")))
        DictationChoice(context).saveLanguage("he")
        val sync = engine()
        enable(sync, categories = setOf("dictionary"))
        val consent = cloud.consent(alice.uid)
        assertTrue(consent.getJSONObject("dictionary").getBoolean("enabled"))
        assertEquals(2L, consent.getJSONObject("dictionary").getLong("generation"))
        assertFalse("settings were not chosen", consent.getJSONObject("settings").getBoolean("enabled"))
        assertEquals(2, dictionaryRecords().size)
        assertTrue("no settings record without consent", cloud.records(alice.uid).values.none { it.getString("category") == "settings" })
        assertTrue(dictionaryRecords().values.any { it.getJSONObject("content").getJSONObject("data").optString("text") == "שלום 🌍" })
        assertEquals(0, sync.status.value.pending)
    }

    fun testRestoreOnceOnAFreshInstallUploadsNothingAndCanBeUndone() {
        // Device A turns sync on with Hebrew speech and three entries.
        CompanionPreferences(context).saveDocument(dictionary("hints" to hint("one"), "hints" to hint("שתיים"), "corrections" to JSONObject().put("from", "a").put("to", "b")))
        DictationChoice(context).saveLanguage("he")
        enable(engine())
        val uploaded = cloud.records(alice.uid)
        // A fresh install: nothing local, no sync state.
        sandbox.clear()
        DictationChoice(context).saveLanguage("en")
        val commits = cloud.commits.get()
        val sync = engine()
        val preview = runBlocking { sync.preview(setOf("settings", "dictionary")) }
        assertTrue(preview.restorable)
        assertEquals(3, preview.count("dictionary", "download"))
        // Fresh-install defaults that differ from the account (English here) are shown as choices.
        assertTrue(preview.choices.any { it.title == "Speech language" })
        val receipt = runBlocking { sync.restoreOnce(preview, preview.choices.associate { it.id to "account" }) }!!
        assertEquals("applied", receipt.getString("state"))
        assertEquals("he", DictationChoice(context).language())
        assertEquals(setOf("one", "שתיים", "b"), entries().values.toSet())
        assertEquals("restore once never uploads", commits, cloud.commits.get())
        assertTrue(sync.status.value.enabled.isEmpty())
        assertEquals(uploaded.keys.filter { uploaded.getValue(it).getString("category") == "dictionary" }.toSet(), entries().keys)
        // The restore keeps a rollback snapshot.
        val (snapshot, importId, revision) = PortableStore(context).undoAvailable()!!
        assertEquals("applied", PortableStore(context).rollback(importId, snapshot, revision).getString("state"))
        assertTrue(entries().isEmpty())
        assertEquals("en", DictationChoice(context).language())
    }

    fun testOfflineEditSurvivesProcessDeathAndSendsTheSameOperation() {
        CompanionPreferences(context).saveDocument(dictionary("hints" to hint("before")))
        enable(engine())
        cloud.offline = true
        val document = CompanionPreferences(context).document()
        document.getJSONArray("hints").getJSONObject(0).put("text", "edited offline")
        CompanionPreferences(context).saveDocument(document)
        val dying = CoroutineScope(SupervisorJob() + Dispatchers.IO)
        val first = engine(dying)
        first.pass()
        assertEquals("offline", first.status.value.problem)
        assertEquals(1, first.status.value.pending)
        assertEquals("the edit stays on the phone", "edited offline", entries().values.single())
        dying.cancel()
        // Process death: a new engine and session read only what is on disk.
        cloud.offline = false
        session = AccountSession(identity, scope)
        val second = engine()
        second.pass()
        assertEquals(0, second.status.value.pending)
        assertEquals("edited offline", liveText(dictionaryRecords().values.single()))
        assertEquals(2L, dictionaryRecords().values.single().getLong("revision"))
    }

    fun testLostAcknowledgementIsRetriedWithoutDuplicating() {
        CompanionPreferences(context).saveDocument(dictionary("hints" to hint("once")))
        val sync = engine()
        // The first record commit succeeds but its answer is lost.
        cloud.loseAck = { writes -> writes.any { it.path.contains("/receipts/") } }
        enable(sync)
        sync.pass()
        assertNull("an acknowledgement was lost", cloud.loseAck)
        assertEquals(0, sync.status.value.pending)
        assertEquals("one receipt per record: the retry reused it", cloud.records(alice.uid).size, cloud.documents.keys.count { it.contains("/receipts/") })
        assertEquals(1L, dictionaryRecords().values.single().getLong("revision"))
    }

    fun testConcurrentEditsBecomeAChoiceAndBothResolutionsWork() {
        val entry = hint("original"); val other = hint("second")
        CompanionPreferences(context).saveDocument(dictionary("hints" to entry, "hints" to other))
        val sync = engine()
        enable(sync)
        val id = entry.getString("id")
        // Another device edits the entry; this phone edits it too before hearing about it.
        assertEquals("acknowledged", runBlocking { cloud.remote(alice.uid, remoteOp(id, 1, live(hintData("computer")), origin = store().deviceId)) }.status)
        editHint(id, "phone")
        sync.pass()
        val conflict = sync.status.value.conflicts.single()
        assertEquals(id, conflict.recordId)
        assertEquals("phone", conflict.phone); assertEquals("computer", conflict.cloud)
        assertEquals("both versions are kept until the user chooses", "phone", entries()[id])
        runBlocking { sync.resolve(id, "phone") }
        sync.pass()
        assertEquals("phone", liveText(dictionaryRecords().getValue(id)))
        assertEquals(3L, dictionaryRecords().getValue(id).getLong("revision"))
        // And the other way round, on another entry.
        val otherId = other.getString("id")
        runBlocking { cloud.remote(alice.uid, remoteOp(otherId, 1, live(hintData("computer 2")), origin = store().deviceId)) }
        editHint(otherId, "phone 2")
        sync.pass()
        runBlocking { sync.resolve(otherId, "account") }
        sync.pass()
        assertEquals("computer 2", entries()[otherId])
        assertEquals("computer 2", liveText(dictionaryRecords().getValue(otherId)))
        assertTrue(sync.status.value.conflicts.isEmpty())
    }

    fun testRemoteChangesAndDeletionsApplyWhenThisPhoneDidNotEditThem() {
        val keep = hint("keep"); val gone = hint("gone")
        CompanionPreferences(context).saveDocument(dictionary("hints" to keep, "hints" to gone))
        val sync = engine()
        enable(sync)
        runBlocking {
            cloud.remote(alice.uid, remoteOp(keep.getString("id"), 1, live(hintData("keep, edited")), origin = store().deviceId))
            cloud.remote(alice.uid, remoteOp(gone.getString("id"), 1, JSONObject().put("state", "deleted"), origin = store().deviceId))
            cloud.remote(alice.uid, remoteOp(UUID.randomUUID().toString(), 0, live(hintData("from the computer"))))
        }
        sync.pass()
        assertEquals(setOf("keep, edited", "from the computer"), entries().values.toSet())
        assertEquals("pulling queued nothing back", 0, sync.status.value.pending)
    }

    fun testSignOutAndAccountSwitchNeverSendAccountAWorkToB() {
        CompanionPreferences(context).saveDocument(dictionary("hints" to hint("alice")))
        val sync = engine().also { it.start() }
        enable(sync)
        cloud.offline = true
        editHint(entries().keys.single(), "alice offline edit")
        sync.pass()
        assertEquals(1, sync.pendingFor(AccountKey("demo-metrix-writer", alice.uid)))
        session.signOut()
        identity.next = bob
        session.signInGoogle(context)
        val deadline = System.currentTimeMillis() + 5_000
        while (session.snapshot().state != AuthState.SIGNED_IN) { assertTrue(System.currentTimeMillis() < deadline); Thread.sleep(10) }
        assertEquals(bob.uid, session.snapshot().account!!.uid)
        cloud.offline = false
        sync.pass()
        assertTrue("B has no account data and nothing was sent", cloud.documents.keys.none { it.startsWith("users/${bob.uid}") })
        assertEquals("alice", liveText(dictionaryRecords().values.single()))
        assertEquals("A's work waits for A", 1, sync.pendingFor(AccountKey("demo-metrix-writer", alice.uid)))
        assertTrue(sync.status.value.enabled.isEmpty())
    }

    fun testCloudDeletionGenerationFencesQueuedWorkWithoutResending() {
        CompanionPreferences(context).saveDocument(dictionary("hints" to hint("fenced")))
        val sync = engine()
        enable(sync)
        cloud.offline = true
        editHint(entries().keys.single(), "late offline edit")
        sync.pass()
        assertEquals(1, sync.status.value.pending)
        // The trusted cleanup turns the dictionary off and advances its generation.
        val consent = cloud.consent(alice.uid)
        consent.put("dictionary", JSONObject().put("enabled", false).put("generation", consent.getJSONObject("dictionary").getLong("generation") + 1))
        consent.put("revision", consent.getLong("revision") + 1)
        cloud.setConsent(alice.uid, consent)
        cloud.offline = false
        val receipts = cloud.documents.keys.count { it.contains("/receipts/") }
        sync.pass()
        assertEquals("fenced work is never sent", receipts, cloud.documents.keys.count { it.contains("/receipts/") })
        assertEquals("fenced", liveText(dictionaryRecords().values.single()))
        assertEquals(0, sync.status.value.pending)
        assertEquals(1, sync.status.value.detached)
        assertEquals("turned_off", sync.status.value.problem)
        assertFalse("dictionary" in sync.status.value.enabled)
        assertEquals("the local edit is kept", "late offline edit", entries().values.single())
    }

    fun testUnusableRemoteSettingIsPreservedAndNeverOverwrittenBack() {
        if (DirectCloud(context, "openai").configured()) return
        val sync = engine()
        enable(sync, categories = setOf("settings"))
        val id = com.metrix.writer.sync.Contract.SETTING_IDS.getValue("transcription_model")
        val current = cloud.records(alice.uid).getValue(id)
        val engineBefore = DictationChoice(context).engineLabel()
        val value = JSONObject().put("provider", "openai").put("model", "gpt-4o-transcribe")
        runBlocking { cloud.remote(alice.uid, remoteOp(id, current.getLong("revision"), live(JSONObject().put("kind", "setting").put("key", "transcription_model").put("value", value)),
            category = "settings", origin = current.getString("origin_device_id"))) }
        sync.pass()
        assertEquals("no provider switch without a key", engineBefore, DictationChoice(context).engineLabel())
        assertTrue(PortableStore(context).preserved().getValue("transcription_model").contains("key"))
        sync.pass()
        assertEquals("the phone did not upload its own engine over it", "openai", cloud.records(alice.uid).getValue(id).getJSONObject("content").getJSONObject("data").getJSONObject("value").getString("provider"))
    }

    fun testPauseStopsTransfersOnThisPhoneOnly() {
        CompanionPreferences(context).saveDocument(dictionary("hints" to hint("paused")))
        val sync = engine()
        enable(sync)
        runBlocking { sync.setPaused(true) }
        editHint(entries().keys.single(), "while paused")
        val commits = cloud.commits.get()
        sync.pass()
        assertEquals(commits, cloud.commits.get())
        assertTrue("consent is unchanged", cloud.consent(alice.uid).getJSONObject("dictionary").getBoolean("enabled"))
        runBlocking { sync.setPaused(false) }
        sync.pass()
        assertEquals("while paused", liveText(dictionaryRecords().values.single()))
    }

    fun testTurningOffCancelsQueuedSendsAndKeepsCloudData() {
        CompanionPreferences(context).saveDocument(dictionary("hints" to hint("kept in cloud")))
        val sync = engine()
        enable(sync)
        cloud.offline = true
        editHint(entries().keys.single(), "never sent")
        sync.pass()
        cloud.offline = false
        runBlocking { sync.disable("dictionary") }
        sync.pass()
        assertFalse(cloud.consent(alice.uid).getJSONObject("dictionary").getBoolean("enabled"))
        assertEquals("kept in cloud", liveText(dictionaryRecords().values.single()))
        assertEquals(0, sync.status.value.pending)
    }

    private fun store() = PortableStore(context)
    private fun editHint(id: String, text: String) {
        val document = CompanionPreferences(context).document()
        val hints = document.getJSONArray("hints")
        (0 until hints.length()).map { hints.getJSONObject(it) }.single { it.getString("id") == id }.put("text", text)
        CompanionPreferences(context).saveDocument(document)
    }
}
