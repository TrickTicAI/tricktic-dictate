@file:Suppress("DEPRECATION")
package com.metrix.writer

import android.os.Debug
import android.test.AndroidTestCase
import org.json.JSONArray
import org.json.JSONObject
import java.io.*
import java.net.HttpURLConnection
import java.net.URL
import java.util.UUID

internal fun interface PreparationCheckpoint { fun accept(nativePeak: Long, mappedBytes: Long) }
internal class PreparationProbe {
    external fun run(audio: String, mode: Int, checkpoint: PreparationCheckpoint): Long
    companion object { init { System.loadLibrary("writer-phone") } }
}

/** Opt-in debug native probe; generated non-speech only, never loads an ASR model. */
class LongAudioPreparationTest : AndroidTestCase() {
    private lateinit var directory: File
    override fun setUp() { super.setUp(); directory = File(context.cacheDir, "long-audio-${UUID.randomUUID()}").apply { mkdirs() } }
    override fun tearDown() { directory.deleteRecursively(); super.tearDown() }
    private fun wav(name: String, samples: Long, quiet: Boolean = false): File {
        val file = File(directory, name)
        RandomAccessFile(file, "rw").use { out ->
            SessionStore.header(out, samples * 2)
            val buffer = ByteArray(64 * 1024)
            if (!quiet) for (i in 0 until buffer.size / 2) {
                val sample = i * 2 - 32768
                buffer[i * 2] = sample.toByte(); buffer[i * 2 + 1] = (sample shr 8).toByte()
            }
            var left = samples * 2
            while (left > 0) { val count = minOf(left, buffer.size.toLong()).toInt(); out.write(buffer, 0, count); left -= count }
        }
        return file
    }
    private fun snapshot(): JSONObject {
        val runtime = Runtime.getRuntime(); val memory = Debug.MemoryInfo(); Debug.getMemoryInfo(memory)
        val status = File("/proc/self/status").readLines().first { it.startsWith("VmRSS:") }.trim().split(Regex("\\s+"))[1].toLong()
        return JSONObject().put("java_used_bytes", runtime.totalMemory() - runtime.freeMemory())
            .put("java_heap_bytes", runtime.totalMemory()).put("java_max_bytes", runtime.maxMemory())
            .put("native_heap_bytes", Debug.getNativeHeapAllocatedSize()).put("rss_kib", status)
            .put("native_pss_kib", memory.nativePss).put("java_pss_kib", memory.dalvikPss).put("total_pss_kib", memory.totalPss)
    }
    private class HeapSampler : AutoCloseable {
        @Volatile private var active = true
        @Volatile var javaPeak = 0L; private set
        @Volatile var nativePeak = 0L; private set
        private val worker = Thread {
            val runtime = Runtime.getRuntime()
            while (active) {
                javaPeak = maxOf(javaPeak, runtime.totalMemory() - runtime.freeMemory())
                nativePeak = maxOf(nativePeak, Debug.getNativeHeapAllocatedSize())
                Thread.sleep(2)
            }
        }.apply { start() }
        override fun close() { active = false; worker.join() }
        fun addTo(receipt: JSONObject) { receipt.put("java_peak_sampled_bytes", javaPeak).put("native_peak_sampled_bytes", nativePeak).put("sampling_interval_ms", 2) }
    }
    fun testMemoryScalingAndExactNativeSamples() {
        if (!BuildConfig.PREPARATION_PROBE_ENABLED) return
        val receipts = JSONArray()
        for (minutes in listOf(1, 30, 60)) {
            val samples = minutes * 60 * 16000L
            val file = wav("$minutes.wav", samples)
            val hash = PhoneModels.hash(file)
            val before = snapshot()
            var legacy: JSONObject? = null; var current: JSONObject? = null
            // Classifier byte-array/ranges are former silence preparation; compare to streamed path.
            val legacySampler = HeapSampler()
            legacySampler.use {
                val flags = SilenceNative().quietFrames(file.path, 128.0)!!
                assertTrue(SilencePreparation.removed(flags).isEmpty())
                assertEquals(samples, PreparationProbe().run(file.path, 1, PreparationCheckpoint { peak, mapped ->
                    legacy = snapshot().put("native_peak_bytes", peak).put("mapped_bytes", mapped)
                }))
            }
            legacySampler.addTo(legacy!!)
            val currentSampler = HeapSampler()
            currentSampler.use { SilencePreparation.prepare(context, file).use { prepared ->
                assertEquals("unchanged", prepared.metrics.getString("silence_preparation"))
                assertEquals(file, prepared.file)
                assertEquals(samples, PreparationProbe().run(prepared.file.path, 0, PreparationCheckpoint { peak, mapped ->
                    current = snapshot().put("native_peak_bytes", peak).put("mapped_bytes", mapped)
                }))
            } }
            currentSampler.addTo(current!!)
            // Separate sample-by-sample traversal from the preparation memory measurement.
            assertEquals(samples, PreparationProbe().run(file.path, 2, PreparationCheckpoint { _, _ -> }))
            assertEquals(hash, PhoneModels.hash(file))
            assertTrue("Native baseline includes full float vector", legacy!!.getLong("native_peak_bytes") - current!!.getLong("native_peak_bytes") > samples * 3)
            receipts.put(JSONObject().put("minutes", minutes).put("samples", samples).put("input_bytes", file.length())
                .put("before", before).put("legacy_decode_peak", legacy).put("file_backed_prepare_peak", current))
            file.delete()
        }
        val receipt = File(context.filesDir, "mw114-memory.json")
        receipt.writeText(JSONObject().put("device", android.os.Build.FINGERPRINT).put("abi", android.os.Build.SUPPORTED_ABIS[0])
            .put("scope", "Preparation only; no model/inference; checkpoints capture live native allocations, not post-free estimates")
            .put("runs", receipts).toString(2))
    }
    fun testNativeCancellationAndAllocationFailureLeaveSavedAudioRetryable() {
        if (!BuildConfig.PREPARATION_PROBE_ENABLED) return
        val file = wav("retry.wav", 90 * 16000L + 17)
        val hash = PhoneModels.hash(file)
        for (mode in listOf(3, 4)) {
            try { PreparationProbe().run(file.path, mode, PreparationCheckpoint { _, _ -> }); fail("Injected failure accepted") }
            catch (_: IOException) { }
            assertEquals(hash, PhoneModels.hash(file))
            assertEquals(listOf(file.name), directory.list()!!.toList())
            assertEquals(90 * 16000L + 17, PreparationProbe().run(file.path, 2, PreparationCheckpoint { _, _ -> }))
        }
    }
    fun testNativeStorageFailureAndMalformedInputRetainSource() {
        if (!BuildConfig.PREPARATION_PROBE_ENABLED) return
        val root = File(directory, "read-only").apply { mkdirs() }
        val source = wav("storage.wav", 16000L)
        val file = File(root, "saved.wav"); check(source.renameTo(file))
        val hash = PhoneModels.hash(file)
        check(root.setWritable(false, false))
        try {
            try { PreparationProbe().run(file.path, 0, PreparationCheckpoint { _, _ -> }); fail("Unwritable backing directory accepted") }
            catch (_: IOException) { }
            assertEquals(hash, PhoneModels.hash(file))
        } finally { check(root.setWritable(true, true)) }
        assertEquals(16000L, PreparationProbe().run(file.path, 2, PreparationCheckpoint { _, _ -> }))
        val invalid = File(directory, "invalid.wav").apply { writeText("invalid saved input") }
        try { PreparationProbe().run(invalid.path, 0, PreparationCheckpoint { _, _ -> }); fail("Invalid PCM accepted") }
        catch (_: IOException) { }
        assertEquals("invalid saved input", invalid.readText())
        assertFalse(File("/proc/self/fd").listFiles()!!.any { fd ->
            runCatching { android.system.Os.readlink(fd.path).contains("writer-pcm") }.getOrDefault(false)
        })
    }
    fun testBatchedSilenceMatchesLegacyRangesAcrossBoundaryAndPartialFrame() {
        val file = wav("mixed.wav", 180 * 16000L + 17, true)
        // High energy, non-speech boundary markers: one uncertain span divides a long pause.
        RandomAccessFile(file, "rw").use { out ->
            for (frame in listOf(0L, 4095L, 8999L)) {
                out.seek(44 + frame * 640)
                repeat(320) { out.write(255); out.write(127) }
            }
            out.seek(file.length() - 34); repeat(17) { out.write(0); out.write(128) }
        }
        val hash = PhoneModels.hash(file)
        val flags = SilenceNative().quietFrames(file.path, 128.0)!!
        val ranges = SilencePreparation.removed(flags)
        val expected = File(directory, "expected.wav")
        RandomAccessFile(file, "r").use { input -> RandomAccessFile(expected, "rw").use { output ->
            val bytes = file.length() - 44 - ranges.sumOf { it.last - it.first + 1 }
            SessionStore.header(output, bytes)
            val buffer = ByteArray(65536); var offset = 0L
            for (range in ranges + listOf((file.length() - 44)..(file.length() - 45))) {
                input.seek(44 + offset); var left = range.first - offset
                while (left > 0) { val n = minOf(left, buffer.size.toLong()).toInt(); input.readFully(buffer, 0, n); output.write(buffer, 0, n); left -= n }
                offset = range.last + 1
            }
        } }
        SilencePreparation.prepare(context, file).use { result ->
            assertEquals("trimmed", result.metrics.getString("silence_preparation"))
            assertEquals(expected.length(), result.file.length()); assertEquals(PhoneModels.hash(expected), PhoneModels.hash(result.file))
            assertEquals(hash, PhoneModels.hash(file))
        }
    }
    fun testCancellationDuringClassificationAndCopyRetainsSourceAndRetries() {
        val file = wav("silence.wav", 180 * 16000L + 17, true); val hash = PhoneModels.hash(file)
        for (after in listOf(2, 5)) {
            var checks = 0
            try { SilencePreparation.prepare(context, file) { ++checks >= after }.close(); fail("Cancellation ignored") }
            catch (_: IllegalStateException) { }
            assertEquals(hash, PhoneModels.hash(file))
            SilencePreparation.prepare(context, file).use { result -> assertEquals("trimmed", result.metrics.getString("silence_preparation")) }
        }
    }
    private class StreamingResponse(url: URL, private val body: String, private val uploadUrl: String? = null,
        private val consume: (ByteArray, Int, Int) -> Unit = { _, _, _ -> }) : HttpURLConnection(url) {
        var largestWrite = 0; var written = 0L
        override fun connect() {}
        override fun disconnect() {}
        override fun usingProxy() = false
        override fun getResponseCode() = 200
        override fun getInputStream() = ByteArrayInputStream(body.toByteArray())
        override fun getHeaderField(name: String?) = if (name == "X-Goog-Upload-URL") uploadUrl else null
        override fun getOutputStream() = object : OutputStream() {
            override fun write(b: Int) { write(byteArrayOf(b.toByte()), 0, 1) }
            override fun write(b: ByteArray, off: Int, len: Int) {
                largestWrite = maxOf(largestWrite, len); written += len; consume(b, off, len)
            }
        }
    }
    fun testLongGeminiUploadStreamsByteIdenticalAudioAndCancellationRetries() {
        val file = wav("upload.wav", 60 * 60 * 16000L); val hash = PhoneModels.hash(file)
        val digest = java.security.MessageDigest.getInstance("SHA-256")
        var send: StreamingResponse? = null
        val client = GeminiClient("fixture-only-key") { url ->
            if (url.query == null) StreamingResponse(url, "{}", DirectCloud.ORIGIN + "/upload/v1beta/files?upload_id=fixture")
            else StreamingResponse(url, """{"file":{"name":"files/fixture","uri":"${DirectCloud.ORIGIN}/v1beta/files/fixture"}}""", consume = { b, off, len -> digest.update(b, off, len) }).also { send = it }
        }
        client.upload(file)
        assertEquals(file.length(), send!!.written); assertTrue(send!!.largestWrite <= 65536)
        assertEquals(hash, digest.digest().joinToString("") { "%02x".format(it) })
        var interrupted: GeminiClient? = null
        interrupted = GeminiClient("fixture-only-key") { url ->
            if (url.query == null) StreamingResponse(url, "{}", DirectCloud.ORIGIN + "/upload/v1beta/files?upload_id=fixture")
            else StreamingResponse(url, "{}", consume = { _, _, _ -> interrupted!!.cancel() })
        }
        try { interrupted.upload(file); fail("Upload cancellation ignored") } catch (_: InterruptedIOException) { }
        assertEquals(hash, PhoneModels.hash(file)); assertEquals("files/fixture", client.upload(file).getString("name"))
    }
    fun testCompatibleMultipartStreamsByteIdenticalAudio() {
        val file = wav("multipart.wav", 10 * 60 * 16000L); val hash = PhoneModels.hash(file)
        val digest = java.security.MessageDigest.getInstance("SHA-256")
        var header = true; var left = file.length(); var sent: StreamingResponse? = null
        val client = CompatibleCloudClient("openai", "fixture-only-key") { url ->
            StreamingResponse(url, "{\"text\":\"protocol fixture\"}", consume = { bytes, offset, size ->
                if (header) {
                    assertTrue(String(bytes, offset, size).contains("filename=\"dictation.wav\"")); header = false
                } else if (left > 0) {
                    val count = minOf(left, size.toLong()).toInt(); digest.update(bytes, offset, count); left -= count
                }
            }).also { sent = it }
        }
        assertEquals("protocol fixture", client.transcribeAudio(file,
            CompanionPreferences(context).options("openai", "gpt-4o-mini-transcribe", "mixed")))
        assertEquals(0L, left); assertEquals(hash, digest.digest().joinToString("") { "%02x".format(it) })
        assertTrue(sent!!.largestWrite <= 65536); assertTrue(sent!!.written > file.length())
        assertEquals(hash, PhoneModels.hash(file))
    }
    fun testCloudAllocationFailureRetainsPendingAndPriorText() {
        val store = SessionStore(File(directory, "recordings"))
        val item = store.create("en"); store.audio(item).appendBytes(ByteArray(640) { 17 }); store.finishCapture(item, null)
        item.getJSONArray("attempts").put(JSONObject().put("id", "prior").put("raw_text", "earlier transcript"))
        val provider = "groq"; val spec = CloudProviders.get(provider)
        store.prepareDirect(item, CompanionPreferences(context).options(provider, spec.speech.first().id, "en"), JSONObject().put("provider", provider).put("transport_version", 1).put("credential_id", UUID.randomUUID()), "raw")
        val before = store.load(item.getString("id")); val hash = PhoneModels.hash(store.audio(before))
        val client = CompatibleCloudClient(provider, "fixture-only-key") { throw OutOfMemoryError("injected transport allocation") }
        DirectCloudEngine(context, store) { client }.execute(before, spec.origin)
        val saved = store.load(item.getString("id"))
        assertEquals(before.getJSONObject("pending").toString(), saved.getJSONObject("pending").toString())
        assertEquals(before.getJSONArray("attempts").toString(), saved.getJSONArray("attempts").toString())
        assertEquals(hash, PhoneModels.hash(store.audio(saved)))
    }
}
