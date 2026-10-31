@file:Suppress("DEPRECATION")
package com.metrix.writer

import android.test.AndroidTestCase
import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.io.IOException
import java.io.RandomAccessFile
import java.net.ServerSocket
import java.net.Socket
import java.security.MessageDigest
import java.util.Collections
import java.util.UUID
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicInteger

/**
 * MW-ANDROID-05: live recognition while recording, chunked fallback, checkpoints and resume.
 * Synthetic PCM, fake live connections and fake range clients; no provider or credential.
 */
class StreamingRecognitionTest : AndroidTestCase() {
    private lateinit var root: File
    override fun setUp() { super.setUp(); root = File(context.cacheDir, "streaming-${UUID.randomUUID()}").apply { mkdirs() } }
    override fun tearDown() { root.deleteRecursively(); super.tearDown() }

    private val second = 32000
    private fun options(language: String = "en") = JSONObject().put("selection", JSONObject().put("provider", "gemini").put("model", DirectCloud.DEFAULT_MODEL))
        .put("input_language", language).put("output_language", JSONObject().put("mode", "preserve"))
        .put("vocabulary", JSONObject().put("hints", JSONArray()).put("corrections", JSONArray()).put("snippets", JSONArray()))

    /** Loud "speech" with quiet pauses at the given seconds (each 300 ms). */
    private fun pcm(seconds: Int, pauses: List<Double> = emptyList()): ByteArray {
        val bytes = ByteArray(seconds * second)
        val random = java.util.Random(7)
        for (i in 0 until bytes.size / 2) {
            val t = i / 16000.0
            val quiet = pauses.any { t >= it && t < it + 0.3 }
            val sample = if (quiet) 0 else random.nextInt(6000) - 3000
            bytes[2 * i] = (sample and 255).toByte(); bytes[2 * i + 1] = (sample shr 8).toByte()
        }
        return bytes
    }
    private fun wav(data: ByteArray): File = File(root, "${UUID.randomUUID()}.wav").also { file ->
        RandomAccessFile(file, "rw").use { SessionStore.header(it, data.size.toLong()); it.seek(44); it.write(data) }
    }
    /** Feeds a capture the way AudioCapture does: 100 ms blocks after they are written. */
    private fun capture(sink: AudioCapture.FrameSink, data: ByteArray, block: Int = 3200, pace: (Int) -> Unit = {}) {
        var at = 0
        while (at < data.size) { val n = minOf(block, data.size - at); sink.frames(data.copyOfRange(at, at + n), n, at.toLong()); at += n; pace(at) }
        sink.finished(data.size.toLong(), true)
    }

    /** Text identifies the audio range, so coverage, order and overlap are visible in the result. */
    private inner class Ranges(val behavior: (Long, Long, Int) -> String = { s, e, _ -> "r$s-$e" }) {
        val calls = Collections.synchronizedList(mutableListOf<Pair<Long, Long>>())
        val active = AtomicInteger(); val peak = AtomicInteger()
        fun client(): RangeClient = object : RangeClient {
            @Volatile var cancelled = false
            override fun transcribeRange(audio: File, start: Long, end: Long, options: JSONObject): String {
                assertTrue(44 + end <= audio.length())
                val n = synchronized(calls) { calls += start to end; calls.count { it == start to end } }
                peak.accumulateAndGet(active.incrementAndGet(), ::maxOf)
                try {
                    val text = behavior(start, end, n)
                    if (cancelled) throw java.io.InterruptedIOException("cancelled")
                    return text
                } finally { active.decrementAndGet() }
            }
            override fun cancel() { cancelled = true }
        }
    }

    /** A live connection whose provider answers each turn when the stream ends (or on demand). */
    private inner class FakeLive(val listener: GeminiLive.Listener, val script: FakeLive.() -> Unit = { }) : LiveConnection {
        val received = java.io.ByteArrayOutputStream(); @Volatile var ended = false; @Volatile var closed = false
        @Volatile override var turnOpen = false
        @Volatile var failAfter = Long.MAX_VALUE
        @Volatile var onEnd: FakeLive.() -> Unit = { turnOpen = false; listener.text("live:${received.size()}", true); listener.complete() }
        override fun audio(bytes: ByteArray, offset: Int, count: Int) {
            if (closed) return // a dropped connection loses audio silently; failed() reports it
            synchronized(received) { received.write(bytes, offset, count) }
            if (ChunkPlanner.rms(bytes.copyOfRange(offset, offset + count), count) > 300) turnOpen = true
            if (received.size() >= failAfter) { closed = true; Thread { listener.failed(IOException("dropped")) }.start() }
        }
        override fun end() { ended = true; Thread { Thread.sleep(50); onEnd() }.start() }
        override fun close() { closed = true }
    }

    private fun recognizer(audio: File, ranges: Ranges, limits: StreamingRecognition.Limits = StreamingRecognition.Limits(drainTimeoutMs = 3000, drainGraceMs = 100, chunkTimeoutMs = 3000),
        live: ((GeminiLive.Listener) -> LiveConnection)?, id: String = UUID.randomUUID().toString()) =
        StreamingRecognition(id, audio, options(), StreamingRecognition.Checkpoints(File(root, "recognition")), live, ranges::client, limits).start()

    private fun deadline() = android.os.SystemClock.elapsedRealtime() + 30_000

    fun testPlannerCutsAtTheQuietestFrameAndMatchesTheSavedFilePlan() {
        val data = pcm(70, listOf(24.0, 51.0))
        val planner = ChunkPlanner(0); val cuts = mutableListOf<Long>()
        var at = 0
        while (at < data.size) { val n = minOf(3000, data.size - at); cuts += planner.feed(data, at, n); at += n }
        assertEquals(2, cuts.size)
        // The quiet 300 ms run spans frames 240..242; the cut follows the first quietest frame.
        assertEquals(241L * 3200, cuts[0]); assertEquals(cuts[0] + (51.0 * 10 - 241).toLong() * 3200 + 3200, cuts[1])
        assertEquals(listOf(0L) + cuts + data.size.toLong(), ChunkPlanner.plan(wav(data), 0, data.size.toLong()))
        assertTrue(cuts.zipWithNext().all { (a, b) -> b - a in 20L * second..30L * second })
    }

    fun testDuplicationGuardRejectsImplausibleRatesAndRepeatedSpans() {
        val sentence = "we walked to the market and bought bread and cheese for the trip"
        assertNull(DuplicationGuard.reason(sentence, 6_000))
        assertEquals("repeated_span", DuplicationGuard.reason("$sentence then $sentence", 25_000))
        val many = (1..130).joinToString(" ") { "word$it" }
        assertEquals("word_rate", DuplicationGuard.reason(many, 26_000)) // 300 words/min
        assertNull("fast read speech (~190 words/min, LibriSpeech) passes", DuplicationGuard.reason((1..82).joinToString(" ") { "w$it" }, 26_000))
        assertEquals("twice this recording's typical rate", "word_rate", DuplicationGuard.reason((1..100).joinToString(" ") { "w$it" }, 26_000, typical = 120.0))
        assertNull("short ranges are not rate-checked", DuplicationGuard.reason(many, 5_000))
    }

    fun testChunkedRetriesATimeoutCountsItUncertainAndMergesAnIncompleteRange() {
        val data = pcm(70, listOf(24.0, 51.0)); val audio = wav(data)
        val bounds = ChunkPlanner.plan(audio, 0, data.size.toLong())
        val ranges = Ranges { s, e, n -> when {
            s == bounds[0] && e == bounds[1] && n == 1 -> { Thread.sleep(1500); "late" } // exceeds the 500 ms deadline once
            s == bounds[1] && e == bounds[2] -> error("Gemini did not return a complete result. Audio and earlier text are saved.")
            else -> "r$s-$e"
        } }
        val chunker = ChunkedTranscriber(audio, options(), ranges::client, timeoutMs = 500, retryDelayMs = 10)
        for (i in 0 until bounds.size - 1) chunker.submit(bounds[i], bounds[i + 1])
        val results = chunker.await(deadline())
        // Middle range failed twice, then succeeded merged with its predecessor; nothing overlaps.
        assertEquals(listOf("r${bounds[0]}-${bounds[2]}", "r${bounds[2]}-${bounds[3]}"), results.map { it.text })
        assertTrue(StreamingRecognition.covers(results.map { it.start to it.end }, data.size.toLong()))
        val receipt = chunker.receipt()
        assertEquals(1, receipt.getInt("uncertain_requests")); assertEquals(1, receipt.getInt("merged_retries"))
        assertTrue(receipt.getInt("requests") >= 5)
    }

    fun testChunkedRejectsAProviderErrorWithoutRetry() {
        val data = pcm(10); val audio = wav(data)
        val calls = AtomicInteger()
        val chunker = ChunkedTranscriber(audio, options(), { object : RangeClient {
            override fun transcribeRange(audio: File, start: Long, end: Long, options: JSONObject): String { calls.incrementAndGet(); throw CloudFailure("key", 401) }
            override fun cancel() {}
        } }, retryDelayMs = 10)
        chunker.submit(0, data.size.toLong())
        try { chunker.await(deadline()); fail() } catch (error: CloudFailure) { assertEquals(401, error.status) }
        assertEquals("a rejected key is not a transient failure", 1, calls.get())
    }

    fun testDuplicatedChunkResponseIsReRequested() {
        val data = pcm(26); val audio = wav(data)
        val good = (1..50).joinToString(" ") { "w$it" }
        val ranges = Ranges { _, _, n -> if (n == 1) "$good ${(1..10).joinToString(" ") { "w$it" }}" else good }
        val chunker = ChunkedTranscriber(audio, options(), ranges::client, retryDelayMs = 10)
        chunker.submit(0, data.size.toLong())
        assertEquals(good, chunker.await(deadline()).single().text)
        assertEquals(1, chunker.receipt().getInt("guard_rejections")); assertEquals(0, chunker.receipt().getInt("duplication_suspected"))
    }

    fun testLiveOnlyKeepsPartialTextSeparateAndJoinsFinalTurns() {
        val data = pcm(8); val audio = wav(data); val ranges = Ranges()
        val started = CountDownLatch(1); lateinit var fake: FakeLive
        val recognition = recognizer(audio, ranges, live = { listener -> FakeLive(listener).also { fake = it; started.countDown() } })
        assertTrue(started.await(5, TimeUnit.SECONDS))
        var halfway = false
        capture(recognition, data) { at ->
            if (at == 4 * second) { fake.listener.text("first turn", true); fake.listener.text("sec", false); halfway = true
                assertEquals("first turn sec", recognition.preview()) }
        }
        val outcome = recognition.await(deadline())
        assertTrue(halfway)
        assertEquals("first turn live:${data.size}", outcome.text)
        assertEquals("live", outcome.receipt.getString("method")); assertEquals("generation_complete", outcome.receipt.getString("end_signal"))
        assertTrue("no batch request on the healthy live path", ranges.calls.isEmpty())
        assertEquals(data.size, fake.received.size())
        val checkpoint = StreamingRecognition.Checkpoints(File(root, "recognition")).load(recognition.sessionId)!!
        assertEquals(1, checkpoint.getJSONArray("pieces").length())
    }

    fun testRotationAtAPauseTilesStableSegmentsWithoutOverlap() {
        val data = pcm(12, listOf(4.2, 9.0)); val audio = wav(data); val ranges = Ranges()
        val connections = Collections.synchronizedList(mutableListOf<FakeLive>())
        val limits = StreamingRecognition.Limits(rotateMinBytes = 3L * second, rotateMaxBytes = 6L * second, drainTimeoutMs = 3000, drainGraceMs = 100)
        val recognition = recognizer(audio, ranges, limits, live = { listener -> FakeLive(listener).also { connections += it } })
        capture(recognition, data)
        val outcome = recognition.await(deadline())
        assertEquals(3, connections.size)
        assertEquals(data.size, connections.sumOf { it.received.size() })
        assertEquals(3, outcome.receipt.getInt("segments")); assertEquals("live", outcome.receipt.getString("method"))
        assertEquals(connections.joinToString(" ") { "live:${it.received.size()}" }, outcome.text)
        assertTrue(ranges.calls.isEmpty())
    }

    fun testLiveFailureSwitchesThatSegmentAndLaterAudioToChunksWithoutSeamLoss() {
        val data = pcm(70, listOf(24.0, 51.0)); val audio = wav(data); val ranges = Ranges()
        // A real capture arrives at 32 KB/s; this one arrives at once, so the queue bound is raised.
        val recognition = recognizer(audio, ranges, StreamingRecognition.Limits(drainTimeoutMs = 3000, drainGraceMs = 100, chunkTimeoutMs = 3000, queueBytes = 8 shl 20),
            live = { listener -> FakeLive(listener).also { it.failAfter = 40L * second } })
        capture(recognition, data)
        val outcome = recognition.await(deadline())
        val bounds = ChunkPlanner.plan(audio, 0, data.size.toLong())
        assertEquals(bounds.zipWithNext().joinToString(" ") { (s, e) -> "r$s-$e" }, outcome.text)
        assertEquals("chunked", outcome.receipt.getString("method")); assertEquals("connection_lost", outcome.receipt.getString("live_failure"))
        assertEquals("each range requested once", ranges.calls.size, ranges.calls.toSet().size)
    }

    fun testQueueOverflowFallsBackToTheSavedFileWithoutSeamLoss() {
        val data = pcm(70, listOf(24.0, 51.0)); val audio = wav(data); val ranges = Ranges()
        val release = CountDownLatch(1)
        val recognition = recognizer(audio, ranges, StreamingRecognition.Limits(drainTimeoutMs = 3000, drainGraceMs = 100, chunkTimeoutMs = 3000, queueBytes = 64 * 1024),
            live = { listener -> object : LiveConnection {
                override val turnOpen = true
                override fun audio(bytes: ByteArray, offset: Int, count: Int) { release.await() } // a stalled network
                override fun end() {}
                override fun close() { release.countDown() }
            } })
        capture(recognition, data) { at -> if (at == 10 * second) release.countDown() }
        val outcome = recognition.await(deadline())
        assertEquals("queue_overflow", outcome.receipt.getString("live_failure"))
        assertEquals(ChunkPlanner.plan(audio, 0, data.size.toLong()).zipWithNext().joinToString(" ") { (s, e) -> "r$s-$e" }, outcome.text)
    }

    fun testFifteenMinuteChunkedRecordingIsTiledWithBoundedMemory() {
        val minutes = 15; val data = ByteArray(minutes * 60 * second)
        val random = java.util.Random(11)
        for (i in 0 until data.size / 2) { val t = i / 16000.0; val v = if (t % 23.0 < 0.3) 0 else random.nextInt(6000) - 3000
            data[2 * i] = (v and 255).toByte(); data[2 * i + 1] = (v shr 8).toByte() }
        val audio = wav(data); val ranges = Ranges()
        val runtime = Runtime.getRuntime(); System.gc(); val before = runtime.totalMemory() - runtime.freeMemory(); var peak = before
        val recognition = recognizer(audio, ranges, StreamingRecognition.Limits(chunkTimeoutMs = 3000), live = null)
        // Paced in blocks so the 2 MB production queue bound applies, as for a real capture.
        capture(recognition, data) { at ->
            if (at % (2 * second) == 0) Thread.sleep(2)
            if (at % (60 * second) == 0) { System.gc(); peak = maxOf(peak, runtime.totalMemory() - runtime.freeMemory()) } // live objects only
        }
        val outcome = recognition.await(deadline())
        val bounds = ChunkPlanner.plan(audio, 0, data.size.toLong())
        assertEquals(bounds.size - 1, ranges.calls.size)
        assertTrue(ranges.calls.size in 30..45)
        assertEquals(bounds.zipWithNext().joinToString(" ") { (s, e) -> "r$s-$e" }, outcome.text)
        assertTrue("at most 4 requests in flight", ranges.peak.get() <= 4)
        assertEquals("no worker error on the chunked-from-start path", "live_unavailable", outcome.receipt.getString("live_failure"))
        assertTrue("heap growth stays far below the ${data.size / 1048576} MB recording: ${(peak - before) / 1048576} MB", peak - before < 24L * 1048576)
    }

    fun testFailedDrainAtStopChunksTheLastSegmentIncludingItsTail() {
        val data = pcm(35, listOf(26.0)); val audio = wav(data); val ranges = Ranges()
        val recognition = recognizer(audio, ranges, StreamingRecognition.Limits(drainTimeoutMs = 400, drainGraceMs = 100, chunkTimeoutMs = 3000),
            live = { listener -> FakeLive(listener).also { it.onEnd = { /* the provider never answers the open turn */ } } })
        capture(recognition, data)
        val outcome = recognition.await(deadline())
        assertEquals("drain_timeout", outcome.receipt.getString("live_failure"))
        val bounds = ChunkPlanner.plan(audio, 0, data.size.toLong())
        assertEquals(bounds.zipWithNext().joinToString(" ") { (s, e) -> "r$s-$e" }, outcome.text)
    }

    fun testCloseAfterStreamEndWithoutAnOpenTurnIsACleanEnd() {
        // Speech, then a silent tail: the provider answers the turn while recording, then closes after the end (probe: 1008).
        val data = pcm(4) + ByteArray(2 * second); val audio = wav(data); val ranges = Ranges()
        lateinit var fake: FakeLive; val started = CountDownLatch(1)
        val recognition = recognizer(audio, ranges, live = { listener -> FakeLive(listener).also { fake = it; it.onEnd = { listener.failed(java.io.EOFException("1008")) }; started.countDown() } })
        assertTrue(started.await(5, TimeUnit.SECONDS))
        capture(recognition, data) { at ->
            if (at == 5 * second) {
                while (fake.received.size() < at) Thread.sleep(1) // the worker has sent all speech
                fake.turnOpen = false; fake.listener.text("whole turn", true); fake.listener.complete()
            }
        }
        val outcome = recognition.await(deadline())
        assertEquals("whole turn", outcome.text)
        assertEquals("live", outcome.receipt.getString("method")); assertEquals("closed_after_end", outcome.receipt.getString("end_signal"))
        assertTrue(ranges.calls.isEmpty())
    }

    fun testResumeAfterProcessDeathRequestsOnlyUncheckpointedAudio() {
        val data = pcm(70, listOf(24.0, 51.0)); val audio = wav(data)
        val id = UUID.randomUUID().toString(); val store = StreamingRecognition.Checkpoints(File(root, "recognition"))
        val bounds = ChunkPlanner.plan(audio, 0, data.size.toLong())
        // A stable live segment and one finished chunk survived the process; the middle did not.
        store.update(id, options()) { doc -> doc.getJSONArray("pieces")
            .put(JSONObject().put("start", 0).put("end", bounds[1]).put("source", "live").put("text", "kept live"))
            .put(JSONObject().put("start", bounds[2]).put("end", bounds[3]).put("source", "chunk").put("text", "kept chunk")) }
        val ranges = Ranges()
        val outcome = StreamingRecognition.resume(id, audio, options(), store, ranges::client, deadline())!!
        assertEquals("kept live r${bounds[1]}-${bounds[2]} kept chunk", outcome.text)
        assertEquals(listOf(bounds[1] to bounds[2]), ranges.calls.toList())
        assertEquals(2, outcome.receipt.getInt("reused_pieces"))
        assertNull("another batch model or language cannot reuse pieces", StreamingRecognition.resume(id, audio, options("he"), store, ranges::client, deadline()))
    }

    fun testDiscardStopsSendingAndForgetsInMemoryText() {
        val data = pcm(10); val audio = wav(data); val ranges = Ranges()
        lateinit var fake: FakeLive; val started = CountDownLatch(1)
        val recognition = recognizer(audio, ranges, live = { listener -> FakeLive(listener).also { fake = it; started.countDown() } })
        assertTrue(started.await(5, TimeUnit.SECONDS))
        StreamingRecognition.register(recognition)
        var at = 0
        while (at < 4 * second) { recognition.frames(data.copyOfRange(at, at + 3200), 3200, at.toLong()); at += 3200 }
        StreamingRecognition.discard(recognition.sessionId)
        assertNull(StreamingRecognition.get(recognition.sessionId))
        val sent = fake.received.size()
        recognition.frames(data.copyOfRange(at, at + 3200), 3200, at.toLong()); recognition.finished(at + 3200L, true)
        Thread.sleep(300)
        assertTrue(fake.closed); assertFalse(fake.ended)
        assertTrue("nothing is sent after discard", fake.received.size() <= sent)
        assertEquals("", recognition.preview())
        assertTrue(ranges.calls.isEmpty())
    }

    fun testSessionDiscardAndDeletionRemoveRecognitionCheckpoints() {
        val store = SessionStore(File(root, "recordings"), context)
        val checkpoints = StreamingRecognition.Checkpoints.of(context)
        for (deleteAfterSave in listOf(false, true)) {
            val item = store.create("en"); store.audio(item).appendBytes(ByteArray(3200));
            checkpoints.update(item.getString("id"), options()) { it.getJSONArray("pieces").put(JSONObject().put("start", 0).put("end", 3200).put("source", "live").put("text", "private")) }
            if (deleteAfterSave) { store.finishCapture(item, null); store.deleteSaved(item.getString("id")) } else store.discard(item)
            assertNull(checkpoints.load(item.getString("id")))
        }
    }

    // --- Wire protocol against an in-process WebSocket server -------------------------------------

    private class Server(val script: (Socket, String) -> Unit) : AutoCloseable {
        val socket = ServerSocket(0); val headers = mutableListOf<String>()
        val thread = Thread {
            socket.accept().use { client ->
                val input = client.getInputStream(); val head = StringBuilder()
                while (!head.endsWith("\r\n\r\n")) head.append(input.read().toChar())
                synchronized(headers) { headers += head.split("\r\n") }
                val key = head.split("\r\n").first { it.startsWith("Sec-WebSocket-Key:") }.substringAfter(":").trim()
                script(client, key)
            }
        }.apply { isDaemon = true; start() }
        fun connect(path: String = "/live", headers: Map<String, String> = emptyMap()) = LiveSocket.open("localhost", path, headers) { Socket("127.0.0.1", socket.localPort) }
        override fun close() { socket.close() }
        companion object {
            fun accept(key: String) = android.util.Base64.encodeToString(MessageDigest.getInstance("SHA-1").digest((key + "258EAFA5-E914-47DA-95CA-C5AB0DC11B85").toByteArray()), android.util.Base64.NO_WRAP)
            fun upgrade(client: Socket, key: String, accept: String = accept(key)) = client.getOutputStream().write(
                "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: $accept\r\n\r\n".toByteArray())
            fun frame(client: Socket, opcode: Int, payload: ByteArray, fin: Boolean = true) {
                val out = client.getOutputStream()
                out.write((if (fin) 0x80 else 0) or opcode)
                if (payload.size < 126) out.write(payload.size) else { out.write(126); out.write(payload.size shr 8); out.write(payload.size and 255) }
                out.write(payload); out.flush()
            }
            /** Reads one masked client frame. */
            fun read(client: Socket): Pair<Int, ByteArray> {
                val input = client.getInputStream()
                val first = input.read(); val second = input.read()
                assertTrue("client frames are masked", second and 0x80 != 0)
                var length = (second and 127).toLong()
                if (length == 126L) length = ((input.read() shl 8) or input.read()).toLong()
                else if (length == 127L) { length = 0; repeat(8) { length = (length shl 8) or input.read().toLong() } }
                val mask = ByteArray(4).also { var n = 0; while (n < 4) n += input.read(it, n, 4 - n) }
                val payload = ByteArray(length.toInt()); var n = 0
                while (n < payload.size) n += input.read(payload, n, payload.size - n)
                return (first and 15) to ByteArray(payload.size) { (payload[it].toInt() xor mask[it and 3].toInt()).toByte() }
            }
        }
    }

    fun testLiveSocketHandshakeFramingFragmentsPingAndClose() {
        val pong = CountDownLatch(1); val got = mutableListOf<String>()
        Server { client, key ->
            Server.upgrade(client, key)
            val (op, text) = Server.read(client); assertEquals(1, op); synchronized(got) { got += String(text) }
            Server.frame(client, 2, "{\"a\":".toByteArray(), fin = false) // binary, fragmented, with a ping between
            Server.frame(client, 9, "p".toByteArray())
            Server.frame(client, 0, "1}".toByteArray())
            val (pongOp, pongBody) = Server.read(client); if (pongOp == 10 && String(pongBody) == "p") pong.countDown()
            Server.frame(client, 1, ByteArray(300) { 'x'.code.toByte() })
            Server.frame(client, 8, byteArrayOf(3, -24))
            Server.read(client) // close echo
        }.use { server ->
            val socket = server.connect(headers = mapOf("x-goog-api-key" to "fixture-key"))
            socket.send("hello")
            assertEquals("{\"a\":1}", socket.receive())
            assertTrue(pong.await(5, TimeUnit.SECONDS))
            assertEquals(300, socket.receive()!!.length)
            assertNull(socket.receive())
            socket.close()
            assertEquals(listOf("hello"), got)
            assertTrue("the key travels in a header, never the path", server.headers.contains("x-goog-api-key: fixture-key"))
            assertTrue(server.headers.first().startsWith("GET /live HTTP/1.1"))
        }
    }

    fun testLiveSocketRefusesBadAcceptAndMapsHttpStatus() {
        Server { client, _ -> client.getOutputStream().write("HTTP/1.1 101 Switching Protocols\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: x\r\n\r\n".toByteArray()) }.use { server ->
            try { server.connect(); fail("a 101 without Upgrade: websocket is not a WebSocket") } catch (_: IllegalStateException) { }
        }
        Server { client, _ -> client.getOutputStream().write("HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n".toByteArray()) }.use { server ->
            try { server.connect(); fail("an upgrade without an accept header is refused") } catch (_: IllegalStateException) { }
        }
        // Google's endpoint returns a non-RFC accept value (measured); it is accepted on the fixed TLS origin.
        Server { client, key -> Server.upgrade(client, key, accept = "9/ogW52peQ5mvk95YySGaf72sDU=") }.use { server -> server.connect().close() }
        Server { client, _ -> client.getOutputStream().write("HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n".toByteArray()) }.use { server ->
            try { server.connect(); fail() } catch (error: CloudFailure) { assertEquals(403, error.status); assertTrue(error.message!!.contains("key")) }
        }
        Server { client, _ -> client.getOutputStream().write("HTTP/1.1 302 Found\r\nLocation: https://example.com/\r\n\r\n".toByteArray()) }.use { server ->
            try { server.connect(); fail() } catch (error: CloudFailure) { assertTrue(error.message!!.contains("refused to forward")) }
        }
    }

    fun testGeminiLiveSetupInterimFinalTurnStateAndAudioEnvelope() {
        val texts = Collections.synchronizedList(mutableListOf<Pair<String, Boolean>>()); val complete = CountDownLatch(2)
        val sawEnd = CountDownLatch(1); val audio = mutableListOf<JSONObject>()
        Server { client, key ->
            Server.upgrade(client, key)
            val setup = JSONObject(String(Server.read(client).second)).getJSONObject("setup")
            assertEquals("models/${GeminiLive.MODEL}", setup.getString("model"))
            assertEquals("he-IL", setup.getJSONObject("inputAudioTranscription").getJSONArray("languageCodes").getString(0))
            Server.frame(client, 2, "{\"setupComplete\":{}}".toByteArray())
            audio += JSONObject(String(Server.read(client).second))
            for (m in listOf("""{"voiceActivity":{"type":"ACTIVITY_START","audioOffset":"0.4s"}}""", """{"serverContent":{"interimInputTranscription":{"text":"שלום"}}}""",
                """{"serverContent":{"inputTranscription":{"text":"שלום עולם"}}}""", """{"serverContent":{"generationComplete":true}}""")) Server.frame(client, 2, m.toByteArray())
            if (JSONObject(String(Server.read(client).second)).getJSONObject("realtimeInput").optBoolean("audioStreamEnd")) sawEnd.countDown()
            Server.frame(client, 1, """{"serverContent":{"generationComplete":true}}""".toByteArray())
            Thread.sleep(200)
        }.use { server ->
            val listener = object : GeminiLive.Listener {
                override fun text(fragment: String, final: Boolean) { texts += fragment to final }
                override fun complete() { complete.countDown() }
                override fun goAway() {}
                override fun failed(error: Exception) {}
            }
            val live = GeminiLive.connect("fixture-key", "he", listener) { headers -> assertEquals("fixture-key", headers["x-goog-api-key"]); server.connect(GeminiLive.PATH, headers) }
            live.audio(byteArrayOf(1, 2, 3, 4), 1, 2)
            Thread.sleep(300)
            assertFalse("a completed generation closes the turn", live.turnOpen)
            live.end()
            assertTrue(sawEnd.await(5, TimeUnit.SECONDS)); assertTrue(complete.await(5, TimeUnit.SECONDS))
            assertEquals(listOf("שלום" to false, "שלום עולם" to true), texts.toList())
            val data = audio.single().getJSONObject("realtimeInput").getJSONObject("audio")
            assertEquals("audio/pcm;rate=16000", data.getString("mimeType"))
            assertTrue(android.util.Base64.decode(data.getString("data"), 0).contentEquals(byteArrayOf(2, 3)))
            live.close()
        }
    }
}
