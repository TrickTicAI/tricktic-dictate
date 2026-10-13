@file:Suppress("DEPRECATION")
package com.metrix.writer

import android.test.AndroidTestCase
import org.json.JSONObject
import java.io.ByteArrayInputStream
import java.io.File
import java.io.IOException
import java.io.OutputStream
import java.net.HttpURLConnection
import java.net.URL
import java.util.UUID

/** Real production transport + encrypted checkpoint, injected receiver disconnects; no paid ASR. */
class UploadRecoveryTest : AndroidTestCase() {
    private lateinit var root: File
    override fun setUp() { super.setUp(); root=File(context.cacheDir,"resume-${UUID.randomUUID()}").apply { mkdirs() } }
    override fun tearDown() { root.deleteRecursively(); super.tearDown() }
    private class Receiver {
        val bytes=java.io.ByteArrayOutputStream(); var starts=0; var finalizations=0; var queries=0
        var stage=""; var failed=false; var name="files/fixture"; var final=false; var expire=false
        val offsets=mutableListOf<Long>()
        fun asset() = JSONObject().put("name",name).put("uri",DirectCloud.ORIGIN+"/v1beta/"+name).put("state","ACTIVE")
        fun connect(url: URL): HttpURLConnection = object : HttpURLConnection(url) {
            val sent=java.io.ByteArrayOutputStream()
            var evaluated=false
            override fun connect() {}
            override fun disconnect() {}
            override fun usingProxy()=false
            override fun getOutputStream(): OutputStream = sent
            private fun evaluate() {
                if(evaluated) return
                evaluated=true
                val command=getRequestProperty("X-Goog-Upload-Command") ?: "get"
                when(command) {
                    "start" -> { starts++; name=JSONObject(sent.toString("UTF-8")).getJSONObject("file").getString("name"); bytes.reset(); final=false }
                    "query" -> queries++
                    "upload" -> { val offset=getRequestProperty("X-Goog-Upload-Offset").toLong(); offsets.add(offset); check(offset==bytes.size().toLong()); bytes.write(sent.toByteArray()) }
                    "finalize" -> { check(!final); final=true; finalizations++ }
                }
                if(!failed && command==stage) { failed=true; throw IOException("private upload capability must never escape") }
            }
            override fun getResponseCode(): Int { evaluate(); return if(expire && getRequestProperty("X-Goog-Upload-Command")=="query") 410 else 200 }
            override fun getHeaderField(key: String): String? {
                evaluate()
                return when(key.lowercase()) {
                    "x-goog-upload-url" -> DirectCloud.ORIGIN+"/upload/session?upload_id=secret-token"
                    "x-goog-upload-status" -> if(final) "final" else "active"
                    "x-goog-upload-size-received" -> bytes.size().toString()
                    else -> null
                }
            }
            override fun getInputStream() = ByteArrayInputStream(when(getRequestProperty("X-Goog-Upload-Command")) {
                "finalize" -> JSONObject().put("file",asset()).toString()
                null -> asset().toString()
                else -> "{}"
            }.toByteArray())
        }
    }
    private fun audio() = File(root,"audio.wav").apply { writeBytes(ByteArray(10*1024*1024+17) { (it%251).toByte() }) }
    fun testDisconnectStartMiddleFinalizeAndRestartKeepHashAndOneFinalization() {
        for(stage in listOf("start","upload","finalize")) {
            val audio=audio(); val hash=PhoneModels.hash(audio); val id=UUID.randomUUID().toString(); val receiver=Receiver().apply { this.stage=stage }
            val progress=mutableListOf<Long>()
            fun recovery() = UploadCheckpointStore(context,id,"account/provider/destination/source") { accepted,total,_ -> assertEquals(audio.length(),total); progress.add(accepted) }
            fun client() = GeminiClient("fixture") { url -> receiver.connect(url) }.apply { this.recovery=recovery() }
            try { client().upload(audio); fail("Disconnect was ignored") } catch(error:IOException) { assertFalse(GeminiClient.explain(error).contains("secret-token")) }
            // A new client and checkpoint-store instance model app restart; only encrypted disk state survives.
            val file=client().upload(audio)
            assertEquals(receiver.name,file.getString("name")); assertEquals(1,receiver.finalizations)
            assertEquals(hash,PhoneModels.hash(audio)); assertEquals(hash,PhoneModels.hash(File(root,"received.wav").apply { writeBytes(receiver.bytes.toByteArray()) }))
            if(stage!="start") assertEquals(1,receiver.starts)
            if(stage=="upload") { assertEquals(listOf(0L,8*1024*1024L),receiver.offsets); assertTrue(receiver.queries>0) }
            assertEquals(audio.length(),progress.last())
            val encrypted=File(context.noBackupFilesDir,"upload-$id.checkpoint").readBytes().toString(Charsets.ISO_8859_1)
            assertFalse(encrypted.contains("secret-token")); assertFalse(encrypted.contains(DirectCloud.ORIGIN))
            assertNull(UploadCheckpointStore(context,id,"different-account") { _,_,_ -> }.load())
            recovery().save(null)
        }
    }
    fun testExpirySourceChangeAndCredentialChangeRestartFromZero() {
        for(change in listOf("expiry","source","key","account","destination")) {
            val audio=audio(); val id=UUID.randomUUID().toString(); val receiver=Receiver().apply { stage="upload" }
            var scope="original-account"
            fun recovery()=UploadCheckpointStore(context,id,scope) { _,_,_ -> }
            fun client(key: String)=GeminiClient(key) { url -> receiver.connect(url) }.apply { this.recovery=recovery() }
            try { client("first-key").upload(audio); fail() } catch(_:IOException) {}
            when(change) {
                "expiry" -> receiver.expire=true
                "source" -> java.io.RandomAccessFile(audio,"rw").use { it.seek(70); it.writeByte(99) }
                "account" -> scope="other-account"
                "destination" -> scope="other-destination"
            }
            client(if(change=="key") "second-key" else "first-key").upload(audio)
            assertEquals(2,receiver.starts); assertEquals(1,receiver.finalizations)
            assertEquals(PhoneModels.hash(audio),PhoneModels.hash(File(root,"accepted.wav").apply { writeBytes(receiver.bytes.toByteArray()) }))
            recovery().save(null)
        }
    }
    fun testExplicitHistoryDeletionErasesEncryptedCheckpoint() {
        val store=SessionStore(File(root,"sessions"),context)
        val item=store.create("mixed"); store.audio(item).appendBytes(ByteArray(3200)); store.finishCapture(item,null)
        val id=item.getString("id"); val recovery=UploadCheckpointStore(context,id,"scope") { _,_,_ -> }
        recovery.save(JSONObject().put("location",DirectCloud.ORIGIN+"/upload/session?private-token"))
        assertNotNull(recovery.load()); store.deleteSaved(id)
        assertNull(recovery.load()); assertFalse(File(context.noBackupFilesDir,"upload-$id.checkpoint").exists())
    }
    fun testCancellationRetainsCheckpointAndOriginal() {
        val audio=audio(); val hash=PhoneModels.hash(audio); val id=UUID.randomUUID().toString(); val receiver=Receiver()
        val recovery=UploadCheckpointStore(context,id,"scope") { _,_,_ -> }
        lateinit var client:GeminiClient
        client=GeminiClient("fixture") { url -> receiver.connect(url) }.apply { this.recovery=object:UploadRecovery {
            override fun load()=recovery.load()
            override fun save(value:JSONObject?) { recovery.save(value); client.cancel() }
            override fun progress(confirmed:Long,total:Long,resumed:Boolean) {}
        } }
        try { client.upload(audio); fail() } catch(_:java.io.InterruptedIOException) {}
        assertEquals(hash,PhoneModels.hash(audio)); assertNotNull(recovery.load()); assertEquals(0,receiver.bytes.size()); recovery.save(null)
    }
}
