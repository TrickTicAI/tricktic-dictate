@file:Suppress("DEPRECATION")
package com.metrix.writer

import android.test.AndroidTestCase
import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.io.IOException
import java.util.UUID
import java.util.concurrent.CountDownLatch
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit

class DirectCloudTest : AndroidTestCase() {
    private lateinit var store: SessionStore
    private val fake = "writer-fixture-key-never-a-real-secret-0001"
    override fun setUp() { super.setUp(); store=SessionStore(File(context.cacheDir,"direct-${UUID.randomUUID()}")) }
    override fun tearDown() { store.directory.deleteRecursively(); super.tearDown() }
    private fun cloud() = JSONObject().put("provider","gemini").put("credential_id",UUID.randomUUID().toString()).put("transport_version",1)
    private fun item(): JSONObject {
        val i=store.create("mixed");store.audio(i).appendBytes(ByteArray(32000){ (it%117).toByte() });store.finishCapture(i,null)
        store.prepareDirect(i,CompanionPreferences(context).options("gemini",DirectCloud.DEFAULT_MODEL,"mixed"),cloud(),"cleaned")
        return i
    }
    private fun file() = JSONObject().put("name","files/fixture").put("uri",DirectCloud.ORIGIN+"/v1beta/files/fixture").put("state","ACTIVE")
    fun testEncryptedCredentialRevisionsAndForgetPreservePendingIdentity() {
        assertTrue("Use an isolated emulator", android.os.Build.HARDWARE.contains("ranchu") || android.os.Build.HARDWARE.contains("goldfish"))
        val account=DirectCloud(context)
        assertFalse("Use an unconfigured fixture app", account.configured())
        val oldPrefs=context.getSharedPreferences("direct-cloud",0).all
        // Use the same actual Android Keystore and AtomicFile boundary as the release.
        account.saveKey(fake);val first=account.snapshot()
        try {
            val bytes=File(context.noBackupFilesDir,"gemini-${first.getString("credential_id")}.credential").readBytes()
            assertFalse(String(bytes).contains(fake))
            account.saveKey(fake+"2");val second=account.snapshot()
            assertFalse(first.getString("credential_id")==second.getString("credential_id"))
            assertEquals(fake,account.key(first));assertEquals(fake+"2",account.key(second))
            val i=item();val before=i.getJSONObject("pending").toString()
            val bytesBefore=store.audio(i).readBytes()
            account.replacePending(store,i.getString("id"))
            val replaced=store.load(i.getString("id"))
            assertFalse(before==replaced.getJSONObject("pending").toString())
            assertEquals(before,replaced.getJSONArray("attempts").getJSONObject(0).getJSONObject("request").toString())
            val capturedCloud=JSONObject(replaced.getJSONObject("pending_cloud").toString())
            assertTrue(capturedCloud.has("polish_policy"));capturedCloud.remove("polish_policy")
            assertEquals(second.toString(),capturedCloud.toString())
            assertTrue(bytesBefore.contentEquals(store.audio(replaced).readBytes()))
            val replacement=replaced.getJSONObject("pending").toString()
            account.forget();assertEquals(replacement,store.load(i.getString("id")).getJSONObject("pending").toString())
            try { account.key(first);fail("Forgotten key remained") } catch (_:CloudFailure) { }
        } finally {
            // This suite runs only on the isolated debug fixture app; preserve non-secret preferences.
            val edit=context.getSharedPreferences("direct-cloud",0).edit().clear()
            for ((k,v) in oldPrefs) if (v is String && k!="credential") edit.putString(k,v)
            edit.commit()
        }
    }
    fun testImmutableRequestAndCredentialSnapshot() {
        val i=item();val before=i.toString()
        val options=CompanionPreferences(context).options("gemini","different","en")
        store.prepareDirect(i,options,cloud(),"raw")
        assertEquals(before,i.toString())
        assertFalse(i.toString().contains(fake))
        val reopened=store.load(i.getString("id"))
        assertEquals("mixed",reopened.getJSONObject("pending").getJSONObject("options").getString("input_language"))
    }
    fun testUncertainRequestKeepsUploadedFileAndNeverFabricatesOriginal() {
        val i=item();val bytes=store.audio(i).readBytes();val identity=i.getJSONObject("pending").toString()
        val client=object:GeminiClient(fake) {
            override fun usesFilesFor(audio: File, options: JSONObject) = true // This fixture exercises Files retry/deletion.
            override fun upload(audio:File)=file()
            override fun transcribe(file:JSONObject,options:JSONObject):String=throw IOException("private transport details")
        }
        DirectCloudEngine(context,store){client}.execute(i,DirectCloud.ORIGIN)
        val after=store.load(i.getString("id"))
        assertEquals(identity,after.getJSONObject("pending").toString());assertTrue(after.has("cloud_upload"))
        assertEquals(0,after.getJSONArray("attempts").length());assertTrue(bytes.contentEquals(store.audio(after).readBytes()))
        assertFalse(after.toString().contains("private transport details"))
    }
    fun testResumeUsesSavedUploadAndRawSurvivesDeleteFailure() {
        val i=item();i.put("cloud_upload",file().put("preparation_sha256",PhoneModels.hash(store.audio(i))).put("upload_binding","gemini/${i.getJSONObject("pending_cloud").getString("credential_id")}/${DirectCloud.ORIGIN}/${PhoneModels.hash(store.audio(i))}/${i.optString("transcript_owner")}/${i.getString("id")}"));store.save(i)
        val deleted=CountDownLatch(1)
        val client=object:GeminiClient(fake) {
            override fun usesFilesFor(audio: File, options: JSONObject) = true // This fixture exercises Files retry/deletion.
            override fun upload(audio:File):JSONObject=error("Must not re-upload")
            override fun transcribe(file:JSONObject,options:JSONObject)="Raw שלום English"
            override fun delete(file:JSONObject) { deleted.countDown(); throw IOException("delete failed") }
        }
        DirectCloudEngine(context,store){client}.execute(i,DirectCloud.ORIGIN)
        assertTrue(deleted.await(5,TimeUnit.SECONDS))
        val after=store.load(i.getString("id"));assertFalse(after.has("pending"))
        val job=after.getJSONArray("attempts").getJSONObject(0)
        assertEquals("Raw שלום English",CompanionProcessing.raw(job));assertEquals("succeeded",job.getJSONObject("status").getString("state"))
        assertTrue(job.getString("cloud_file_cleanup").startsWith("unconfirmed"))
    }
    fun testLegacyUploadIsReplacedByTrimmedAudioAndRetryKeepsOriginal() {
        val i = item(); val original = store.audio(i)
        original.appendBytes(ByteArray(8 * 32000)); original.appendBytes(ByteArray(32000) { (it % 117).toByte() })
        java.io.RandomAccessFile(original, "rw").use { SessionStore.header(it, original.length() - 44) }
        i.put("byte_length", original.length()).put("cloud_upload", file()); store.save(i)
        val hash = PhoneModels.hash(original)
        var uploads = 0
        val client = object : GeminiClient(fake) {
            override fun usesFilesFor(audio: File, options: JSONObject) = true // Explicit Files fixture; inline has CloudStreamingTest.
            override fun upload(audio: File): JSONObject {
                uploads++; assertTrue(audio.length() < original.length()); return file()
            }
            override fun transcribe(file: JSONObject, options: JSONObject): String = throw IOException("retry")
        }
        val engine = DirectCloudEngine(context, store) { client }
        engine.execute(i, DirectCloud.ORIGIN)
        assertEquals(1, uploads); assertEquals(hash, PhoneModels.hash(original))
        val retry = store.load(i.getString("id"))
        assertTrue(retry.getJSONObject("pending_timing").getLong("silence_removed_ms") > 0)
        engine.execute(retry, DirectCloud.ORIGIN)
        assertEquals(1, uploads); assertEquals(hash, PhoneModels.hash(original))
    }
    fun testSlowFileDeletionDoesNotDelayResultOrOverwriteNewWork() {
        val i=item();val id=i.getString("id");val audio=store.audio(i).readBytes()
        val deleting=CountDownLatch(1);val release=CountDownLatch(1);val deleted=CountDownLatch(1)
        val client=object:GeminiClient(fake) {
            override fun usesFilesFor(audio: File, options: JSONObject) = true // This fixture exercises Files retry/deletion.
            override fun upload(audio:File)=file()
            override fun transcribe(file:JSONObject,options:JSONObject)="Ready before deletion"
            override fun delete(file:JSONObject) {
                deleting.countDown()
                try { check(release.await(15,TimeUnit.SECONDS)) } finally { deleted.countDown() }
            }
        }
        val worker=Executors.newSingleThreadExecutor()
        try {
            val result=worker.submit<String> { DirectCloudEngine(context,store){client}.execute(i,DirectCloud.ORIGIN) }
            assertTrue(deleting.await(5,TimeUnit.SECONDS))
            assertEquals("Cloud transcript saved on this phone.",result.get(5,TimeUnit.SECONDS))
            assertEquals(1L,release.count)
            val ready=store.load(id)
            assertFalse(ready.has("pending"))
            assertEquals("Ready before deletion",CompanionProcessing.raw(ready.getJSONArray("attempts").getJSONObject(0)))
            DirectCloud.prepareChoice(store,ready,JSONObject().put("cloud",cloud())
                .put("options",CompanionPreferences(context).options("gemini",DirectCloud.DEFAULT_MODEL,"en")).put("delivery","raw"))
            val newer=store.load(id).toString()
            release.countDown();assertTrue(deleted.await(5,TimeUnit.SECONDS))
            assertEquals(newer,store.load(id).toString())
            assertTrue(audio.contentEquals(store.audio(ready).readBytes()))
        } finally { release.countDown();worker.shutdownNow() }
    }
    fun testRedirectAndForeignUploadAddressNeverReceiveForwardedKey() {
        var calls=0
        fun connection(url:java.net.URL,code:Int,location:String?=null)=object:java.net.HttpURLConnection(url) {
            override fun connect() { }
            override fun disconnect() { }
            override fun usingProxy()=false
            override fun getResponseCode()=code
            override fun getInputStream()=java.io.ByteArrayInputStream(ByteArray(0))
            override fun getOutputStream()=java.io.ByteArrayOutputStream()
            override fun getHeaderField(name:String):String?=if(name=="X-Goog-Upload-URL") location else null
        }
        lateinit var opened:java.net.HttpURLConnection
        val redirect=GeminiClient(fake) { url -> calls++; connection(url,307).also { opened=it } }
        try { redirect.checkModel(DirectCloud.DEFAULT_MODEL);fail("Redirect accepted") } catch (_:CloudFailure) { }
        assertEquals(1,calls);assertFalse(opened.instanceFollowRedirects)
        assertEquals(fake,opened.getRequestProperty("x-goog-api-key"));assertFalse(opened.url.toString().contains(fake))
        calls=0
        val upload=GeminiClient(fake) { url -> calls++;connection(url,200,"https://foreign.invalid/upload") }
        try { upload.upload(store.audio(item()));fail("Foreign upload address accepted") } catch (_:IllegalArgumentException) { }
        assertEquals("Foreign address must be rejected before connecting",1,calls)
    }
    fun testPartialProviderOutputIsRejected() {
        val body=JSONObject().put("candidates",JSONArray().put(JSONObject().put("finishReason","MAX_TOKENS")
            .put("content",JSONObject().put("parts",JSONArray().put(JSONObject().put("text","partial"))))))
        try { GeminiClient.text(body);fail("Partial transcript accepted") } catch (_:IllegalStateException) { }
        body.getJSONArray("candidates").getJSONObject(0).put("finishReason","STOP")
        assertEquals("partial",GeminiClient.text(body))
    }
    fun testExactDictionarySpansNeverEnterCleanupAndRawRemainsSeparate() {
        val i=item();val options=i.getJSONObject("pending").getJSONObject("options")
        options.getJSONObject("vocabulary").put("snippets",JSONArray().put(JSONObject().put("phrase","signature").put("text","שלום\n🌍")))
        val client=object:GeminiClient(fake) {
            override fun usesFilesFor(audio: File, options: JSONObject) = true // This fixture exercises Files retry/deletion.
            override fun upload(audio:File)=file()
            override fun transcribe(file:JSONObject,options:JSONObject)="hello signature world"
            override fun delete(file:JSONObject) { }
            override fun cleanup(model:String,text:String,reasoning:String,systemPrompt:String):String {
                assertFalse(text.contains("שלום")); assertTrue(systemPrompt.isNotBlank())
                val spans = JSONObject(text).getJSONArray("editable_spans")
                assertEquals(2, spans.length())
                return JSONObject().put("spans", JSONArray((0 until spans.length()).map { spans.getString(it).uppercase() })).toString()
            }
        }
        store.save(i);DirectCloudEngine(context,store){client}.execute(i,DirectCloud.ORIGIN)
        val id=i.getString("id");val job=store.load(id).getJSONArray("attempts").getJSONObject(0)
        val processing=DirectCloudProcessing(context,store){client}
        processing.prepare(id,job.getString("id"),JSONObject().put("provider","gemini").put("model",DirectCloud.DEFAULT_MODEL),cloud());processing.run(id)
        val after=store.load(id).getJSONArray("attempts").getJSONObject(0)
        assertEquals("hello signature world",CompanionProcessing.raw(after))
        assertEquals("hello שלום\n🌍 world",after.getString("dictionary_text"));assertEquals("HELLO שלום\n🌍 WORLD",after.getString("cleaned_text"))
    }
    fun testFailedCleanupRetainsPriorCleanedOutputAndFrozenChoice() {
        val i=item();val client=object:GeminiClient(fake) {
            override fun usesFilesFor(audio: File, options: JSONObject) = true // This fixture exercises Files retry/deletion.
            override fun upload(audio:File)=file()
            override fun transcribe(file:JSONObject,options:JSONObject)="original"
            override fun delete(file:JSONObject) { }
            override fun cleanup(model:String,text:String,reasoning:String,systemPrompt:String):String=throw IOException("provider failure")
        }
        DirectCloudEngine(context,store){client}.execute(i,DirectCloud.ORIGIN)
        val id=i.getString("id");val loaded=store.load(id);val job=loaded.getJSONArray("attempts").getJSONObject(0)
        job.put("cleaned_text","previous cleaned");store.save(loaded)
        val processing=DirectCloudProcessing(context,store){client}
        processing.prepare(id,job.getString("id"),JSONObject().put("provider","gemini").put("model",DirectCloud.DEFAULT_MODEL),cloud())
        processing.run(id)
        val after=store.load(id);assertFalse(after.has("pending_postprocess"))
        assertTrue(after.getJSONArray("attempts").getJSONObject(0).getJSONObject("polish_outcome").getBoolean("original_used"))
        assertEquals("previous cleaned",after.getJSONArray("attempts").getJSONObject(0).getString("cleaned_text"))
        assertEquals("original",CompanionProcessing.raw(after.getJSONArray("attempts").getJSONObject(0)))
        // Explicitly replace a failed cloud cleanup with dictionary-only processing, retaining prior output.
        processing.prepare(id,job.getString("id"),null);processing.run(id)
        val replaced=store.load(id)
        assertFalse(replaced.has("pending_postprocess"))
        val updated=replaced.getJSONArray("attempts").getJSONObject(0)
        assertEquals("previous cleaned",updated.getString("cleaned_text"))
        assertEquals("original",CompanionProcessing.raw(updated))
        assertEquals(1,updated.getJSONArray("processing_attempts").length())
    }
}
