@file:Suppress("DEPRECATION")
package com.metrix.writer

import android.test.AndroidTestCase
import org.json.JSONObject
import java.io.File
import java.io.ByteArrayInputStream
import java.util.UUID

class PhoneOfflineTest : AndroidTestCase() {
    fun testDictionaryPrecedenceLiteralUnicodeAndNoCascade() {
        val v = JSONObject("""{"hints":[],"snippets":[{"phrase":"email","text":"Exact 🌍\nשלום"},{"phrase":"email address","text":"long"}],"corrections":[{"from":"email","to":"wrong"},{"from":"foo","to":"bar"},{"from":"bar","to":"bad"},{"from":"שלום","to":"hi"}]}""")
        assertEquals("Exact 🌍\nשלום long bar bad xemail emailx שָלום hi", OfflineVocabulary.process("email email address foo bar xemail emailx שָלום שלום", v))
        assertEquals("a\u0301 שלום\u05b0", OfflineVocabulary.process("a\u0301 שלום\u05b0", v))
    }
    fun testSnapshotWithoutPairingAndIncompatibleLanguage() {
        val models = PhoneModels(context)
        assertEquals("phone", models.snapshot("base", "en").getString("execution_location"))
        try { models.snapshot("ivrit-turbo", "en"); fail() } catch (_: IllegalArgumentException) { }
        assertEquals("unsupported", models.snapshot("base", "mixed").getString("hints"))
    }
    fun testRetiredModelCannotBeSelectedDownloadedOrResumed() {
        val models = PhoneModels(context)
        assertFalse(models.models().any { it.getString("id") == "ivrit-turbo" })
        assertTrue(models.languages("ivrit-turbo").isEmpty())
        for (language in listOf("he", "en", "mixed")) {
            try { models.snapshot("ivrit-turbo", language); fail("Retired model admitted") }
            catch (_: IllegalArgumentException) { }
        }
        try { models.download("ivrit-turbo", { false }, { _, _ -> }); fail("Retired model download admitted") }
        catch (_: IllegalArgumentException) { }
        val store = SessionStore(File(context.cacheDir, UUID.randomUUID().toString()))
        try {
            val item = store.create("he"); store.finishCapture(item, null)
            val previous = JSONObject("""{"id":"saved","status":{"state":"succeeded","result":{"transcript":{"raw_text":"Earlier original"}}}}""")
            item.getJSONArray("attempts").put(previous)
            val audio = PhoneModels.hash(store.audio(item))
            val legacy = JSONObject().put("id", "ivrit-turbo").put("runtime_revision", models.runtime)
                .put("sha256", "c8090411113357097bfafc2b8e228ec1639fa7f5fe4ecb5d054ac0ccef8641b1")
            store.preparePhone(item, CompanionPreferences(context).options("phone", "ivrit-turbo", "he"), legacy, "raw")
            PhoneEngine(context, store).execute(item, "phone")
            val saved = store.load(item.getString("id"))
            assertFalse(saved.has("pending"))
            assertEquals("failed", saved.getJSONArray("attempts").getJSONObject(1).getJSONObject("status").getString("state"))
            assertEquals(previous.toString(), saved.getJSONArray("attempts").getJSONObject(0).toString())
            assertEquals(audio, PhoneModels.hash(store.audio(saved)))
        } finally { store.directory.deleteRecursively() }
    }
    fun testPhoneRequestSurvivesRecoveryAndIgnoresNewSelection() {
        val store = SessionStore(File(context.cacheDir, UUID.randomUUID().toString()))
        try {
            val item = store.create("en"); store.finishCapture(item, null)
            val options = CompanionPreferences(context).options("phone", "base", "en")
            store.preparePhone(item, options, PhoneModels(context).snapshot("base", "en"), "dictionary")
            val request = item.getJSONObject("pending").toString()
            options.put("input_language", "mixed")
            val recovered = SessionStore(store.directory).restore().single()
            store.preparePhone(recovered, options, PhoneModels(context).snapshot("turbo-q5_0", "mixed"), "raw")
            assertEquals(request, store.load(item.getString("id")).getJSONObject("pending").toString())
            assertEquals("base", recovered.getJSONObject("pending_engine").getString("id"))
            assertEquals("dictionary", recovered.getString("pending_delivery"))
        } finally { store.directory.deleteRecursively() }
    }
    fun testMissingModelFailureRetainsAudioAndPriorRaw() {
        val store = SessionStore(File(context.cacheDir, UUID.randomUUID().toString()))
        try {
            val item = store.create("en"); store.finishCapture(item, null)
            val snapshot = PhoneModels(context).snapshot("base", "en").put("runtime_revision", "unavailable")
            val prior = JSONObject("""{"id":"earlier","status":{"state":"succeeded","result":{"transcript":{"raw_text":"saved original"}}}}""")
            item.getJSONArray("attempts").put(prior)
            val hash = PhoneModels.hash(store.audio(item))
            store.preparePhone(item, CompanionPreferences(context).options("phone", "base", "en"), snapshot, "raw")
            PhoneEngine(context, store).execute(item, "phone")
            val result = store.load(item.getString("id"))
            assertFalse(result.has("pending")); assertEquals(hash, PhoneModels.hash(store.audio(result)))
            assertEquals(prior.toString(), result.getJSONArray("attempts").getJSONObject(0).toString())
            assertEquals("failed", result.getJSONArray("attempts").getJSONObject(1).getJSONObject("status").getString("state"))
        } finally { store.directory.deleteRecursively() }
    }
    fun testStorageReserveAndUnsafeModelIdentity() {
        try { PhoneModels.checkSpace(100, 100); fail() } catch (_: IllegalStateException) { }
        PhoneModels.checkSpace(100, 100 + 32L * 1024 * 1024)
        try { PhoneModels(context).file("../../recordings"); fail() } catch (_: IllegalArgumentException) { }
    }
    fun testNativeInvalidAudioAndEarlyCancellation() {
        val native = PhoneNative(); val handle = native.create()
        val audio = File(context.cacheDir, "native-${UUID.randomUUID()}.wav")
        try {
            audio.writeBytes(ByteArray(45))
            try { native.transcribe(handle, "/missing", audio.path, "en"); fail() } catch (_: java.io.IOException) { }
            val store = SessionStore(File(context.cacheDir, UUID.randomUUID().toString()))
            try {
                val item = store.create("en")
                java.io.RandomAccessFile(store.audio(item), "rw").use { it.seek(44); it.write(ByteArray(3200)) }
                store.finishCapture(item, null); native.cancel(handle)
                try { native.transcribe(handle, "/missing", store.audio(item).path, "en"); fail() } catch (error: java.io.IOException) { assertTrue(error.message!!.contains("cancelled")) }
            } finally { store.directory.deleteRecursively() }
        } finally { native.destroy(handle); audio.delete() }
    }
    fun testCloudRetryDoesNotReferencePhoneJob() {
        val store = SessionStore(File(context.cacheDir, UUID.randomUUID().toString()))
        try {
            val item = store.create("en"); store.finishCapture(item, null)
            item.getJSONArray("attempts").put(JSONObject().put("id", "phone-id").put("execution_location", "phone"))
            store.prepareOptions(item, "https://example.invalid", CompanionPreferences(context).options("gemini", "test", "en"), "raw")
            assertFalse(item.getJSONObject("pending").has("retry_of_job_id"))
        } finally { store.directory.deleteRecursively() }
    }
}
