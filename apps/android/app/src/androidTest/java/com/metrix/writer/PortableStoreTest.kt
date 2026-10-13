@file:Suppress("DEPRECATION")
package com.metrix.writer

import android.test.InstrumentationTestCase
import com.metrix.writer.sync.Contract
import com.metrix.writer.sync.ContractException
import com.metrix.writer.sync.PortableAdapter
import com.metrix.writer.sync.PortableStore
import com.metrix.writer.sync.StrictJson
import com.metrix.writer.ui.AppearancePreferences
import org.json.JSONArray
import org.json.JSONObject

/** MW-OUATH-7 export/import on real app storage, checked against the shared MW-OUATH-1 fixtures. */
class PortableStoreTest : InstrumentationTestCase() {
    private val context get() = instrumentation.targetContext
    private lateinit var sandbox: PortableSandbox
    private lateinit var store: PortableStore

    override fun setUp() { super.setUp(); sandbox = PortableSandbox(context); store = PortableStore(context) }
    override fun tearDown() { PortableStore.beforeCommit = null; sandbox.restore(); super.tearDown() }

    private fun backup() = JSONObject(String(SyncFixtures.bytes(instrumentation, "wire.json"), Charsets.UTF_8)).getJSONObject("catalog").getJSONObject("backup")
    private fun bytes(value: Any) = value.toString().toByteArray(Charsets.UTF_8)
    private fun reason(block: () -> Unit): String = try { block(); fail("accepted"); "" } catch (error: ContractException) { error.reason }
    private fun hebrewDictionary() = dictionary(
        "hints" to JSONObject().put("text", "מטריקס Writer 🌍"),
        "corrections" to JSONObject().put("from", "rtl \u200Fמילה\u200E mark").put("to", "e\u0301 combined"),
        "snippets" to JSONObject().put("phrase", "חתימה").put("text", "שלום\nAlex · mixed עברית and English").put("enabled", false))

    fun testSharedFixturesAreValidContractDocuments() {
        val wire = SyncFixtures.json(instrumentation, "wire.json") as JSONObject
        val parsed = Contract.parseBackup(bytes(wire.getJSONObject("catalog").getJSONObject("backup")))
        assertEquals(9, parsed.getJSONArray("settings").length())
        val records = wire.getJSONArray("records")
        var checked = 0
        for (i in 0 until records.length()) {
            val record = records.getJSONObject(i)
            if (record.getString("category") == "transcripts") continue // MW-OUATH-10
            Contract.checkRecord(record); checked++
        }
        assertTrue(checked >= 9)
        val polish = (0 until records.length()).map { records.getJSONObject(it) }
            .single { it.optString("id") == "00000000-0000-4000-8000-000000000009" }
        assertEquals("polish_instructions", polish.getJSONObject("content").getJSONObject("data").getString("key"))
        assertTrue(SyncFixtures.json(instrumentation, "conflicts.json") is JSONArray)
    }

    fun testStrictParsingRefusesWhatOrgJsonWouldAccept() {
        assertEquals("duplicate_key", reason { StrictJson.parse(bytes("""{"a":1,"a":2}""")) })
        assertEquals("byte_order_mark", reason { StrictJson.parse(byteArrayOf(0xEF.toByte(), 0xBB.toByte(), 0xBF.toByte()) + bytes("{}")) })
        assertEquals("malformed_utf8", reason { StrictJson.parse(byteArrayOf('"'.code.toByte(), 0xC3.toByte(), '"'.code.toByte())) })
        assertEquals("unpaired_surrogate", reason { StrictJson.parse(bytes("\"\\ud800\"")) })
        assertEquals("nul", reason { StrictJson.parse(bytes("\"a\\u0000b\"")) })
        assertEquals("non_integer", reason { StrictJson.parse(bytes("1.5")) })
        assertEquals("unsafe_integer", reason { StrictJson.parse(bytes("9007199254740992")) })
        assertEquals("שלום 🌍", StrictJson.parse(bytes("\"\\u05e9\\u05dc\\u05d5\\u05dd \\ud83c\\udf0d\"")))
    }

    fun testBackupValidationIsClosedAndBounded() {
        fun mutate(change: (JSONObject) -> Unit) = backup().also(change)
        assertEquals("unsupported_version", reason { Contract.parseBackup(bytes(mutate { it.put("format_version", 2) })) })
        assertEquals("unsupported_version", reason { Contract.parseBackup(bytes(mutate { it.getJSONArray("dictionary").getJSONObject(0).put("schema_version", 2) })) })
        assertEquals("invalid", reason { Contract.parseBackup(bytes(mutate { it.put("api_key", "secret") })) })
        assertEquals("invalid", reason { Contract.parseBackup(bytes(mutate { it.getJSONArray("dictionary").getJSONObject(0).getJSONObject("data").put("audio_uri", "x") })) })
        assertEquals("invalid", reason { Contract.parseBackup(bytes(mutate { it.getJSONArray("dictionary").getJSONObject(0).getJSONObject("data").put("text", "\u00A0\u3000") })) })
        assertEquals("invalid", reason { Contract.parseBackup(bytes(mutate { it.getJSONArray("dictionary").getJSONObject(0).getJSONObject("data").put("text", "א".repeat(4097)) })) })
        assertEquals("invalid", reason { Contract.parseBackup(bytes(mutate { it.getJSONArray("settings").getJSONObject(0).put("id", Contract.SETTING_IDS.getValue("theme")) })) })
        assertEquals("invalid", reason { Contract.parseBackup(bytes(mutate { it.getJSONArray("dictionary").getJSONObject(0).put("id", Contract.SETTING_IDS.getValue("theme")) })) })
        assertEquals("invalid", reason { Contract.parseBackup(bytes(mutate { it.getJSONArray("settings").getJSONObject(3).getJSONObject("data").getJSONObject("value").put("model", "a/../b") })) })
        assertEquals("duplicate_id", reason { Contract.parseBackup(bytes(mutate { it.getJSONArray("dictionary").put(JSONObject(it.getJSONArray("dictionary").getJSONObject(0).toString())) })) })
        assertEquals("too_large", reason { Contract.parseBackup(ByteArray(Contract.MAX_FILE_BYTES + 1) { ' '.code.toByte() }) })
        // 4096 two-byte letters are exactly 8192 bytes: the limit is inclusive.
        Contract.parseBackup(bytes(mutate { it.getJSONArray("dictionary").getJSONObject(0).getJSONObject("data").put("text", "א".repeat(4096)) }))
    }

    fun testPolishInstructionsValidationIsClosedAndBounded() {
        fun polish(change: (JSONObject) -> Unit) = backup().also { root ->
            change(root.getJSONArray("settings").getJSONObject(8).getJSONObject("data").getJSONObject("value"))
        }
        assertEquals("invalid", reason { Contract.parseBackup(bytes(polish { it.put("instructions", "   ") })) })
        assertEquals("invalid", reason { Contract.parseBackup(bytes(polish { it.put("instructions", 0xA0.toChar().toString()) })) })
        assertEquals("nul", reason { Contract.parseBackup(bytes(polish { it.put("instructions", "a" + 0.toChar() + "b") })) })
        assertEquals("invalid", reason { Contract.parseBackup(bytes(polish { it.put("instructions", "x".repeat(16385)) })) })
        assertEquals("invalid", reason { Contract.parseBackup(bytes(polish { it.put("instructions", 42) })) })
        assertEquals("invalid", reason { Contract.parseBackup(bytes(polish { it.getJSONObject("formatting").put("rewrite", "flowery") })) })
        assertEquals("invalid", reason { Contract.parseBackup(bytes(polish { it.getJSONObject("formatting").put("lists", "yes") })) })
        assertEquals("invalid", reason { Contract.parseBackup(bytes(polish { it.getJSONObject("formatting").remove("layout") })) })
        assertEquals("invalid", reason { Contract.parseBackup(bytes(polish { it.put("prompt_versions", JSONArray()) })) })
        assertEquals("invalid", reason { Contract.parseBackup(bytes(backup().also {
            it.getJSONArray("settings").getJSONObject(8).put("id", Contract.SETTING_IDS.getValue("theme"))
        })) })
        assertEquals("too_large", reason { Contract.parseBackup(bytes(backup().also {
            it.getJSONArray("settings").put(JSONObject(it.getJSONArray("settings").getJSONObject(8).toString()))
        })) })
        // The limit is inclusive, and the desktop default passes.
        Contract.parseBackup(bytes(polish { it.put("instructions", "א".repeat(8192)) }))
        Contract.parseBackup(bytes(polish { it.put("instructions", PolishPrompts.SUPER) }))
        assertTrue(Contract.isPolishInstructions(backup().getJSONArray("settings").getJSONObject(8).getJSONObject("data").get("value")))
    }

    fun testLegacyDictionaryGetsStableIdsOnceAndRequestsNeverSeeThem() {
        val prefs = context.getSharedPreferences("companion", android.content.Context.MODE_PRIVATE)
        check(prefs.edit().putString("dictionary", """{"hints":[{"text":"Metrix","enabled":true}],"corrections":[{"from":"a","to":"b","enabled":true}],"snippets":[]}""").commit())
        val first = CompanionPreferences(context).document()
        val id = first.getJSONArray("hints").getJSONObject(0).getString("id")
        assertTrue(Contract.isUuid(id))
        assertEquals(store.deviceId, first.getJSONArray("hints").getJSONObject(0).getString("origin"))
        assertEquals(id, CompanionPreferences(context).document().getJSONArray("hints").getJSONObject(0).getString("id"))
        val vocabulary = CompanionPreferences(context).vocabulary()
        assertEquals("Metrix", vocabulary.getJSONArray("hints").getString(0))
        assertEquals("""{"from":"a","to":"b"}""", vocabulary.getJSONArray("corrections").getJSONObject(0).toString())
    }

    fun testExportImportRoundTripsHebrewMixedAndBidiExactly() {
        CompanionPreferences(context).saveDocument(hebrewDictionary())
        DictationChoice(context).saveLanguage("mixed")
        AppearancePreferences(context).theme = "dark"
        val before = store.projection()
        val exported = store.exportBackup()
        Contract.parseBackup(bytes(exported))
        assertFalse(exported.toString().contains("credential"))
        sandbox.clear()
        val plan = store.previewFile(bytes(exported), setOf("settings", "dictionary"), "merge")
        assertTrue(plan.conflicts.isEmpty())
        assertEquals("applied", store.apply(plan.importId, emptyMap()).getString("state"))
        val after = store.projection()
        for ((id, record) in before) {
            assertTrue(id, StrictJson.equal(record.data, after.getValue(id).data))
            if (record.category == "dictionary") assertEquals(record.origin, after.getValue(id).origin)
        }
        assertEquals("mixed", DictationChoice(context).language())
        assertEquals("dark", AppearancePreferences(context).theme)
    }

    fun testPolishInstructionsExportImportAndPreview() {
        val prefs = CompanionPreferences(context)
        prefs.savePolish("super", "Edited fixture instructions.", PolishFormatting("concise", "single_line", false, true, false, true))
        val exported = store.exportBackup()
        Contract.parseBackup(bytes(exported))
        val row = exported.getJSONArray("settings").let { array -> (0 until array.length()).map { array.getJSONObject(it) } }
            .single { it.getString("id") == Contract.SETTING_IDS.getValue("polish_instructions") }
        assertEquals("Edited fixture instructions.", row.getJSONObject("data").getJSONObject("value").getString("instructions"))
        assertEquals("concise", row.getJSONObject("data").getJSONObject("value").getJSONObject("formatting").getString("rewrite"))
        sandbox.clear()
        assertTrue(CompanionPreferences(context).isPolishDefault())
        val plan = store.previewFile(bytes(exported), setOf("settings", "dictionary"), "merge")
        val item = plan.items.single { it.id == Contract.SETTING_IDS.getValue("polish_instructions") }
        assertEquals("apply", item.action)
        assertEquals("Polishing instructions", item.title)
        assertEquals("Default instructions · Polished rewrite → Edited instructions · Concise rewrite", item.detail)
        assertEquals("applied", store.apply(plan.importId, emptyMap()).getString("state"))
        assertEquals("Edited fixture instructions.", CompanionPreferences(context).polish().instructions)
        assertEquals(PolishFormatting("concise", "single_line", false, true, false, true), CompanionPreferences(context).polish().formatting)
        assertEquals("Default instructions · Polished rewrite", PortableStore.describe("polish_instructions",
            JSONObject().put("instructions", PolishPrompts.SUPER).put("formatting", PolishFormatting().toJson())))
    }

    fun testRejectedImportLeavesEveryStoreUntouched() {
        CompanionPreferences(context).saveDocument(hebrewDictionary())
        val native = PortableAdapter(context).snapshotNative().toString()
        val revision = store.observe()
        val broken = backup().put("format_version", 2)
        try { store.previewFile(bytes(broken), setOf("settings", "dictionary"), "merge"); fail() } catch (_: ContractException) {}
        try { store.previewFile(bytes("{\"format\":\"metrix-writer-portable\",\"format\":1}"), setOf("settings"), "merge"); fail() } catch (_: ContractException) {}
        assertEquals(native, PortableAdapter(context).snapshotNative().toString())
        assertEquals(revision, store.observe())
    }

    fun testStalePreviewAndUnresolvedConflictAreNotApplied() {
        val entry = JSONObject().put("text", "phone")
        CompanionPreferences(context).saveDocument(dictionary("hints" to entry))
        val id = entry.getString("id")
        val file = store.exportBackup()
        file.getJSONArray("dictionary").getJSONObject(0).getJSONObject("data").put("text", "file")
        val plan = store.previewFile(bytes(file), setOf("dictionary"), "merge")
        assertEquals(listOf(id), plan.conflicts.map { it.id })
        assertEquals("conflict", store.apply(plan.importId, emptyMap()).getString("reason"))
        CompanionPreferences(context).save("off", "dictionary") // an edit after the preview
        assertEquals("stale_preview", store.apply(plan.importId, mapOf(id to false)).getString("reason"))
        val again = store.previewFile(bytes(file), setOf("dictionary"), "merge")
        val receipt = store.apply(again.importId, mapOf(id to false))
        assertEquals("applied", receipt.getString("state"))
        assertEquals("file", store.projection().getValue(id).data.getString("text"))
        assertEquals("the same import never applies twice", receipt.toString(), store.apply(again.importId, mapOf(id to true)).toString())
    }

    fun testReplaceRemovesOnlyShownDictionaryEntriesAndRollbackRestoresThem() {
        val keep = JSONObject().put("text", "keep"); val drop = JSONObject().put("text", "drop")
        CompanionPreferences(context).saveDocument(dictionary("hints" to keep, "hints" to drop))
        val file = store.exportBackup()
        val dictionary = file.getJSONArray("dictionary")
        (0 until dictionary.length()).firstOrNull { dictionary.getJSONObject(it).getString("id") == drop.getString("id") }?.let(dictionary::remove)
        val plan = store.previewFile(bytes(file), setOf("dictionary"), "replace_selected")
        assertEquals(listOf(drop.getString("id")), plan.items.filter { it.action == "remove" }.map { it.id })
        val receipt = store.apply(plan.importId, emptyMap())
        assertFalse(drop.getString("id") in store.projection())
        val undo = store.undoAvailable()!!
        assertEquals(receipt.getString("rollback_snapshot_id"), undo.first)
        // An edit after the import makes the rollback stale instead of overwriting it.
        CompanionPreferences(context).save("off", "cleaned")
        assertEquals("stale_preview", store.rollback(plan.importId, undo.first, receipt.getLong("local_revision")).getString("reason"))
        val rolled = store.rollback(plan.importId, undo.first, store.observe())
        assertEquals("applied", rolled.getString("state"))
        assertTrue(drop.getString("id") in store.projection())
        assertEquals("rollback restores portable state as it was", "raw", CompanionPreferences(context).delivery())
        assertEquals(rolled.toString(), store.rollback(plan.importId, undo.first, 0).toString())
    }

    fun testCrashBeforeCommitRollsBackTheWholeImport() {
        CompanionPreferences(context).saveDocument(dictionary("hints" to JSONObject().put("text", "original")))
        val native = PortableAdapter(context).snapshotNative().toString()
        val file = backup()
        val plan = store.previewFile(bytes(file), setOf("settings", "dictionary"), "merge")
        PortableStore.beforeCommit = { throw Error("simulated process death") }
        try { store.apply(plan.importId, emptyMap()); fail() } catch (_: Error) {}
        PortableStore.beforeCommit = null
        PortableStore(context).recover()
        assertEquals(native, PortableAdapter(context).snapshotNative().toString())
        assertNull(store.undoAvailable())
    }

    fun testUnsupportedValuesArePreservedNotAppliedAndRoundTrip() {
        val file = backup() // super mode, Hebrew UI, a desktop-only local model and a gemini polishing model without a key
        val before = DictationChoice(context).engineLabel()
        val plan = store.previewFile(bytes(file), setOf("settings"), "merge")
        val preserved = plan.items.filter { it.action == "preserve" }.map { it.title }.toSet()
        assertTrue(preserved.toString(), preserved.containsAll(setOf("Interface language", "Dictation mode", "Dictation engine", "Polishing")))
        assertEquals("applied", store.apply(plan.importId, emptyMap()).getString("state"))
        assertEquals("the phone engine never switches to an unavailable model", before, DictationChoice(context).engineLabel())
        assertEquals("off", CompanionPreferences(context).cleanup())
        assertEquals("cleaned", CompanionPreferences(context).delivery())
        assertTrue(store.preserved().keys.containsAll(listOf("ui_language", "dictation_mode", "transcription_model", "cleanup")))
        val exported = store.exportBackup().getJSONArray("settings")
        val original = file.getJSONArray("settings")
        for (i in 0 until original.length()) {
            val key = original.getJSONObject(i).getJSONObject("data").getString("key")
            if (key in setOf("ui_language", "dictation_mode", "transcription_model", "cleanup")) {
                val out = (0 until exported.length()).map { exported.getJSONObject(it) }.single { it.getJSONObject("data").getString("key") == key }
                assertTrue(key, StrictJson.equal(original.getJSONObject(i).getJSONObject("data"), out.getJSONObject("data")))
            }
        }
        // A local change to that setting replaces the preserved value.
        CompanionPreferences(context).save("gemini/gemini-3.8-flash", "raw")
        assertFalse("cleanup" in store.preserved())
        assertTrue("dictation_mode" in store.preserved())
    }

    fun testImportNeverSelectsACloudProviderWithoutAKey() {
        if (DirectCloud(context, "openai").configured()) return // Only meaningful where no OpenAI key is saved.
        val file = backup()
        file.getJSONArray("settings").getJSONObject(3).getJSONObject("data").put("value", JSONObject().put("provider", "openai").put("model", "gpt-4o-transcribe"))
        val provider = DirectCloud.selectedProvider(context)
        val plan = store.previewFile(bytes(file), setOf("settings"), "merge")
        assertEquals("preserve", plan.items.single { it.title == "Dictation engine" }.action)
        store.apply(plan.importId, emptyMap())
        assertEquals(provider, DirectCloud.selectedProvider(context))
        assertFalse(DictationChoice(context).isPhone())
    }

    fun testCapacityProblemsAreReportedWithoutChangingAnything() {
        CompanionPreferences(context).saveDocument(dictionary(*Array(150) { "hints" to JSONObject().put("text", "local $it") }))
        val file = backup()
        val rows = JSONArray()
        repeat(60) { rows.put(JSONObject().put("schema_version", 1).put("id", java.util.UUID.randomUUID().toString()).put("origin_device_id", store.deviceId)
            .put("category", "dictionary").put("data", JSONObject().put("kind", "hint").put("enabled", true).put("text", "file $it"))) }
        file.put("dictionary", rows)
        val native = PortableAdapter(context).snapshotNative().toString()
        try { store.previewFile(bytes(file), setOf("dictionary"), "merge"); fail() } catch (error: IllegalStateException) { assertTrue(error.message!!.contains("200")) }
        assertEquals(native, PortableAdapter(context).snapshotNative().toString())
    }
}
