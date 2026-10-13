@file:Suppress("DEPRECATION")
package com.metrix.writer

import android.test.AndroidTestCase
import org.json.JSONObject
import java.io.ByteArrayInputStream
import java.io.ByteArrayOutputStream
import java.io.File
import java.io.OutputStream
import java.io.RandomAccessFile
import java.net.HttpURLConnection
import java.net.URL
import java.security.MessageDigest
import java.util.UUID

/** Production transports with bounded mock sinks; synthetic bytes, no ASR/provider credentials. */
class CloudStreamingTest : AndroidTestCase() {
    private lateinit var root: File
    override fun setUp() { super.setUp(); root = File(context.cacheDir, "stream-${UUID.randomUUID()}").apply { mkdirs() } }
    override fun tearDown() { root.deleteRecursively(); super.tearDown() }
    private val result = """{"candidates":[{"finishReason":"STOP","content":{"parts":[{"text":"שלום fixture"}]}}]}"""
    private val asset = JSONObject().put("name", "files/fixture").put("uri", DirectCloud.ORIGIN + "/v1beta/files/fixture").put("state", "ACTIVE")
    private fun options() = CompanionPreferences(context).options("gemini", DirectCloud.DEFAULT_MODEL, "mixed")
    private fun audio(size: Long): File = File(root, UUID.randomUUID().toString()).apply {
        RandomAccessFile(this, "rw").use { it.setLength(size); it.write("RIFF\u0000\u0000\u0000\u0000WAVE".toByteArray()) }
    }
    private class Connection(url: URL, private val body: String, private val status: Int = 200,
        val capture: Boolean = false, val onWrite: () -> Unit = {}) : HttpURLConnection(url) {
        var length = -1L; var sent = 0L; var maxWrite = 0; var disconnected = false
        val bytes = ByteArrayOutputStream(); val hash = MessageDigest.getInstance("SHA-256")
        var peakHeap = 0L
        override fun setFixedLengthStreamingMode(length: Long) { this.length = length; super.setFixedLengthStreamingMode(length) }
        override fun setFixedLengthStreamingMode(length: Int) { this.length = length.toLong(); super.setFixedLengthStreamingMode(length) }
        override fun connect() {}
        override fun disconnect() { disconnected = true }
        override fun usingProxy() = false
        override fun getResponseCode() = status
        override fun getInputStream() = ByteArrayInputStream(body.toByteArray(Charsets.UTF_8))
        override fun getHeaderField(name: String) = if (name.equals("X-Goog-Upload-URL", true)) DirectCloud.ORIGIN + "/upload/session" else if (name.equals("X-Goog-Upload-Status",true)) if (getRequestProperty("X-Goog-Upload-Command")=="finalize") "final" else "active" else null
        override fun getOutputStream(): OutputStream = object : OutputStream() {
            override fun write(value: Int) { write(byteArrayOf(value.toByte()), 0, 1) }
            override fun write(buffer: ByteArray, offset: Int, count: Int) {
                maxWrite = maxOf(maxWrite, count); sent += count; hash.update(buffer, offset, count)
                if (capture) bytes.write(buffer, offset, count)
                val runtime = Runtime.getRuntime(); peakHeap = maxOf(peakHeap, runtime.totalMemory() - runtime.freeMemory())
                onWrite()
            }
        }
    }
    fun testInlinePaddingUtf8ThresholdAndStreamedBody() {
        val opts = options(); opts.getJSONObject("vocabulary").getJSONArray("hints").put("שלום \"quoted\" \\ spelling")
        for (size in listOf(49151L, 49152L, 49153L, 147458L)) {
            val audio = audio(size); val calls = mutableListOf<Connection>()
            val client = GeminiClient("fixture-key") { url -> Connection(url, result, capture = true).also { calls.add(it) } }
            assertFalse(client.usesFilesFor(audio, opts)); assertEquals("שלום fixture", client.transcribeAudio(audio, opts))
            assertEquals(1, calls.size); val call = calls.single()
            assertTrue(call.url.path.endsWith(":generateContent")); assertTrue(call.disconnected)
            assertEquals(call.length, call.sent); assertTrue(call.maxWrite <= 65536)
            val body = JSONObject(call.bytes.toString("UTF-8")); val part = body.getJSONArray("contents").getJSONObject(0).getJSONArray("parts").getJSONObject(1)
            assertTrue(audio.readBytes().contentEquals(android.util.Base64.decode(part.getJSONObject("inlineData").getString("data"), android.util.Base64.DEFAULT)))
            assertTrue(body.getJSONObject("systemInstruction").toString().contains("preserve both"))
            assertFalse(call.bytes.toString("UTF-8").contains("fixture-key"))
            val overhead = call.sent - ((size + 2) / 3) * 4
            val edge = ((GeminiClient.INLINE_LIMIT - overhead) / 4) * 3
            RandomAccessFile(audio, "rw").use { it.setLength(edge) }; assertFalse(client.usesFilesFor(audio, opts))
            RandomAccessFile(audio, "rw").use { it.setLength(edge + 1) }; assertTrue(client.usesFilesFor(audio, opts))
        }
        assertEquals(4L, GeminiClient.inlineSize(1,0,0)); assertEquals(4L, GeminiClient.inlineSize(2,0,0)); assertEquals(4L, GeminiClient.inlineSize(3,0,0))
    }
    fun testFilesStreamBodyAndFreshActiveAvoidsGetButReuseChecksRemote() {
        val audio = audio(16_000_000); val calls = mutableListOf<Connection>()
        val client = GeminiClient("fixture") { url ->
            val body = when { url.path == "/upload/v1beta/files" -> "{}"; url.path == "/upload/session" -> JSONObject().put("file",asset).toString(); url.path == "/v1beta/files/fixture" -> asset.toString(); else -> result }
            Connection(url, body, capture = true).also { calls.add(it) }
        }
        assertTrue(client.usesFilesFor(audio, options()))
        val file = client.upload(audio); assertEquals("שלום fixture", client.transcribe(file, options()))
        assertEquals(5, calls.size); val upload = calls[1]
        assertEquals(8*1024*1024L, upload.sent); assertEquals(upload.sent, upload.length); assertEquals("0",upload.getRequestProperty("X-Goog-Upload-Offset"))
        assertTrue(upload.maxWrite <= 65536)
        val expected = MessageDigest.getInstance("SHA-256"); audio.inputStream().use { input -> val buffer = ByteArray(65536); while(true) { val n = input.read(buffer); if(n<0) break; expected.update(buffer,0,n) } }
        val actual = MessageDigest.getInstance("SHA-256"); actual.update(calls[1].bytes.toByteArray()); actual.update(calls[2].bytes.toByteArray()); assertTrue(expected.digest().contentEquals(actual.digest())); assertEquals(audio.length(),calls[1].sent+calls[2].sent); assertEquals("8388608",calls[2].getRequestProperty("X-Goog-Upload-Offset")); assertEquals("finalize",calls[3].getRequestProperty("X-Goog-Upload-Command"))
        client.transcribe(file,options()); assertEquals(7,calls.size); assertEquals("/v1beta/files/fixture",calls[5].url.path)
    }
    fun testCancellationAndProviderErrorsRetainAudio() {
        for (status in listOf(401,403,413,429,500,307)) {
            val audio = audio(100000); val hash = PhoneModels.hash(audio)
            val client = GeminiClient("fixture") { url -> Connection(url,"private provider body",status) }
            try { client.transcribeAudio(audio,options()); fail("Provider error accepted") } catch (error: CloudFailure) { assertEquals(status,error.status); assertFalse(error.message!!.contains("private provider")) }
            assertEquals(hash,PhoneModels.hash(audio))
        }
        val audio = audio(200000); val hash = PhoneModels.hash(audio); var sent: Connection? = null
        lateinit var client: GeminiClient
        client = GeminiClient("fixture") { url -> Connection(url,result,onWrite = { if (sent!!.sent > 10000) client.cancel() }).also { sent=it } }
        try { client.transcribeAudio(audio,options()); fail("Cancelled request succeeded") } catch (_:java.io.InterruptedIOException) {}
        assertTrue(sent!!.disconnected); assertTrue(sent!!.sent < ((audio.length()+2)/3)*4); assertEquals(hash,PhoneModels.hash(audio))
    }
    fun testEngineInlineDurabilityAndMatchingFilesReuse() {
        val store = SessionStore(File(root,"sessions"))
        fun item(): JSONObject {
            val item = store.create("mixed"); store.audio(item).appendBytes(ByteArray(32000) { (it%117).toByte() }); store.finishCapture(item,null)
            store.prepareDirect(item,options(),JSONObject().put("provider","gemini").put("credential_id",UUID.randomUUID().toString()).put("transport_version",1),"raw")
            return item
        }
        val calls = mutableListOf<Connection>()
        fun api() = GeminiClient("fixture") { url -> Connection(url,if (url.path=="/v1beta/files/fixture") asset.toString() else result).also { synchronized(calls) { calls.add(it) } } }
        val first = item(); val before = PhoneModels.hash(store.audio(first))
        DirectCloudEngine(context,store) { api() }.execute(first,DirectCloud.ORIGIN)
        val saved = store.load(first.getString("id")); assertFalse(saved.has("pending")); assertFalse(saved.has("cloud_upload"))
        assertEquals(before,PhoneModels.hash(store.audio(saved))); assertEquals(1,calls.size)
        assertEquals("שלום fixture",CompanionProcessing.raw(saved.getJSONArray("attempts").getJSONObject(0)))
        val cached = item(); cached.put("cloud_upload",JSONObject(asset.toString()).put("preparation_sha256",PhoneModels.hash(store.audio(cached))).put("upload_binding","gemini/${cached.getJSONObject("pending_cloud").getString("credential_id")}/${DirectCloud.ORIGIN}/${PhoneModels.hash(store.audio(cached))}/${cached.optString("transcript_owner")}/${cached.getString("id")}")); store.save(cached)
        DirectCloudEngine(context,store) { api() }.execute(cached,DirectCloud.ORIGIN)
        val snapshot = synchronized(calls) { calls.toList() }
        assertTrue(snapshot.any { it.url.path=="/v1beta/files/fixture" }); assertFalse(snapshot.any { it.url.path=="/upload/v1beta/files" })
    }
    fun testSyntheticBeforeAfterMemoryAndRequestCount() {
        for (size in listOf(32_000L,14_000_000L)) for (baseline in listOf(true,false)) {
            val audio = audio(size); val calls = mutableListOf<Connection>()
            val client = GeminiClient("fixture") { url ->
                val body= when { url.path=="/upload/v1beta/files" -> "{}"; url.path=="/upload/session" -> JSONObject().put("file",asset).toString(); url.path=="/v1beta/files/fixture" -> asset.toString(); else -> result }
                Connection(url,body).also { calls.add(it) }
            }
            System.gc(); val runtime=Runtime.getRuntime(); val base=runtime.totalMemory()-runtime.freeMemory()
            if(baseline) {
                // Baseline always uploaded then GET+generate+DELETE. Fresh ACTIVE now skips GET,
                // so explicitly issue the original validation request through a second client.
                val file=client.upload(audio)
                val reuse=GeminiClient("fixture") { url -> Connection(url,if(url.path=="/v1beta/files/fixture") asset.toString() else result).also { calls.add(it) } }
                reuse.transcribe(file,options()); reuse.delete(file)
            } else { assertFalse(client.usesFilesFor(audio,options())); client.transcribeAudio(audio,options()) }
            val peak=calls.maxOf { it.peakHeap }.minus(base).coerceAtLeast(0)
            android.util.Log.i("MW109","transport_heap bytes=$size variant=${if(baseline) "baseline_4eb3eae" else "inline"} sampled_additional_heap_bytes=$peak requests=${calls.size} max_write=${calls.maxOf { it.maxWrite }}")
            assertEquals(if(baseline) (if(size>8*1024*1024L) 7 else 6) else 1,calls.size); assertTrue(calls.maxOf { it.maxWrite }<=65536)
        }
    }
}
