package com.metrix.writer

import android.content.Context
import android.os.SystemClock
import android.util.AtomicFile
import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.io.IOException
import java.io.InterruptedIOException
import java.util.UUID
import java.util.concurrent.ArrayBlockingQueue
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

/**
 * Recognition while recording (MW-ANDROID-01 direction: live streaming, chunked fallback).
 *
 * Audio goes to Gemini Live as it is saved. The recording is split into live segments at pauses
 * (rotation before the provider's connection limit); a segment whose connection drained cleanly is a
 * stable, durably checkpointed piece. When a live connection fails, that segment and all later audio
 * switch to chunked batch requests cut at pauses, sent as soon as each chunk is spoken. Pieces never
 * overlap, so no word is counted twice at a seam. The saved WAV stays authoritative: a process death
 * resumes from the checkpoint by re-planning only the audio no stable piece covers.
 *
 * Partial (interim) text is held in memory only and is never a transcript.
 */
internal class StreamingRecognition(
    val sessionId: String,
    private val audio: File,
    /** Pending-style options: Gemini batch model, input language and vocabulary hints. */
    private val options: JSONObject,
    private val checkpoints: Checkpoints,
    private val live: ((GeminiLive.Listener) -> LiveConnection)?,
    private val ranges: () -> RangeClient,
    private val limits: Limits = Limits(),
) : AudioCapture.FrameSink {
    data class Limits(
        val rotateMinBytes: Long = 240L * 32000, val rotateMaxBytes: Long = 290L * 32000,
        val drainTimeoutMs: Long = 8000, val drainGraceMs: Long = 400,
        val queueBytes: Int = 2 * 1024 * 1024, val chunkTimeoutMs: Long = 25_000,
        val quietRms: Double = 300.0,
    )
    class Outcome(val text: String, val receipt: JSONObject)

    private enum class State { CONNECTING, LIVE, DRAINING, STABLE, FAILED }
    private inner class Segment(val start: Long) : GeminiLive.Listener {
        @Volatile var end = -1L
        @Volatile var state = State.CONNECTING
        @Volatile var connection: LiveConnection? = null
        val finals = mutableListOf<String>()
        @Volatile var interim = ""
        @Volatile var endSentAt = 0L
        @Volatile var lastMessageAt = 0L
        @Volatile var signal = ""
        val settled = CountDownLatch(1)
        override fun text(fragment: String, final: Boolean) {
            lastMessageAt = SystemClock.elapsedRealtime()
            synchronized(this) { if (final) { finals += fragment; interim = "" } else interim = fragment }
        }
        override fun complete() { lastMessageAt = SystemClock.elapsedRealtime(); if (endSentAt > 0) drained("generation_complete") }
        override fun goAway() { goAwayRequested = true }
        override fun failed(error: Exception) {
            // With no open turn, the provider closes the connection after the stream ends (probe: code 1008).
            if (endSentAt > 0 && connection?.turnOpen == false) drained("closed_after_end")
            else fail(this, if (error is CloudFailure) "provider_${error.status}" else if (error is java.io.EOFException || error.message == "dropped") "connection_lost" else "connection_lost:${error.javaClass.simpleName}")
        }
        fun drained(how: String) {
            synchronized(this@StreamingRecognition) {
                if (state != State.DRAINING) return
                state = State.STABLE; signal = how
                tailMs = maxOf(tailMs, SystemClock.elapsedRealtime() - endSentAt)
                runCatching { connection?.close() }
                checkpoint(piece(start, end, "live", text()))
            }
            settled.countDown()
        }
        fun text() = synchronized(this) { ChunkedTranscriber.join(finals) }
    }

    private val queue = ArrayBlockingQueue<Any>(4096)
    @Volatile private var queuedBytes = 0
    private val segments = mutableListOf<Segment>()
    @Volatile private var current: Segment? = null
    @Volatile private var goAwayRequested = false
    private var planner = ChunkPlanner(0)
    private val cuts = mutableListOf(0L)
    @Volatile private var chunker: ChunkedTranscriber? = null
    @Volatile private var chunkedFrom = -1L
    @Volatile private var written = 0L
    @Volatile private var ended = -1L
    @Volatile private var endedAt = 0L
    @Volatile private var cancelled = false
    @Volatile private var liveFailure: String? = null
    @Volatile private var tailMs = 0L
    @Volatile private var quietFrames = 0
    private val finished = CountDownLatch(1)
    private val worker = Thread({ work() }, "writer-stream").apply { isDaemon = true }

    fun start(): StreamingRecognition { worker.start(); return this }

    // AudioCapture thread: copy and return; overflow switches to the saved-file path.
    override fun frames(bytes: ByteArray, count: Int, dataOffset: Long) {
        if (cancelled) return
        if (queuedBytes + count > limits.queueBytes || !queue.offer(Frame(bytes.copyOf(count), dataOffset))) {
            overflowed = true; return
        }
        synchronized(this) { queuedBytes += count }
    }
    override fun finished(dataBytes: Long, complete: Boolean) { queue.put(End(dataBytes)) }
    private class Frame(val bytes: ByteArray, val offset: Long)
    private class End(val bytes: Long)
    @Volatile private var overflowed = false
    /** After dropped frames, the remaining audio is planned from the saved file once capture ends. */
    @Volatile private var fileMode = false

    private fun work() {
        try {
            if (live == null) chunkFrom(0, "live_unavailable") else open(0)
            while (true) {
                val next = queue.take()
                if (cancelled) return
                if (next is End) { end(next.bytes); return }
                try { frame(next as Frame) }
                catch (error: Exception) {
                    // Never lose the End marker: plan the rest from the saved WAV and keep draining.
                    synchronized(this) { fileMode = true; chunkFrom(current?.start ?: cuts.last(), "stream_error:${error.javaClass.simpleName}") }
                }
            }
        } catch (_: InterruptedException) { }
        finally { finished.countDown() }
    }
    private fun frame(next: Frame) {
        synchronized(this) { queuedBytes -= next.bytes.size }
        if (fileMode) return
        if (overflowed || next.offset != written) {
            // Frames were dropped: stop planning from memory and re-plan the rest from the saved WAV at Stop.
            synchronized(this) { fileMode = true; chunkFrom(current?.start ?: cuts.last(), if (overflowed) "queue_overflow" else "frame_gap") }
            return
        }
        written = next.offset + next.bytes.size
        val newCuts = planner.feed(next.bytes, 0, next.bytes.size)
        synchronized(this) {
            for (cut in newCuts) { cuts += cut; if (chunkedFrom >= 0) chunks().submit(cuts[cuts.size - 2], cut) }
        }
        val segment = current
        if (chunkedFrom < 0 && segment != null) {
            // Audio sent while connecting is buffered by the queue; connect() is synchronous below.
            try { segment.connection!!.audio(next.bytes, 0, next.bytes.size) }
            catch (_: Exception) { fail(segment, "send_failed"); return }
            quietFrames = if (next.bytes.size >= 1600 && ChunkPlanner.rms(next.bytes, next.bytes.size) <= limits.quietRms) quietFrames + 1 else 0
            val length = written - segment.start
            if (length >= limits.rotateMaxBytes || (length >= limits.rotateMinBytes || goAwayRequested) && quietFrames >= 2) rotate(written)
        }
    }


    private fun open(at: Long) {
        val segment = Segment(at)
        synchronized(this) { segments += segment; current = segment }
        try {
            segment.connection = live!!(segment)
            synchronized(this) { if (segment.state == State.CONNECTING) segment.state = State.LIVE }
        } catch (error: Exception) {
            fail(segment, if (error is CloudFailure) "provider_${error.status}" else "connect_failed:${error.javaClass.simpleName}")
        }
    }

    private fun rotate(at: Long) {
        val old = current ?: return
        quietFrames = 0; goAwayRequested = false
        endSegment(old, at)
        planner = ChunkPlanner(at); synchronized(this) { cuts.clear(); cuts += at }
        open(at)
    }

    private fun endSegment(segment: Segment, at: Long) {
        synchronized(this) {
            if (segment.state != State.LIVE) return
            segment.end = at; segment.state = State.DRAINING; segment.endSentAt = SystemClock.elapsedRealtime()
        }
        try { segment.connection!!.end() } catch (_: Exception) { fail(segment, "send_failed"); return }
        // The provider answers the last turn ~0.6 s after the stream ends; a silent tail has no turn to answer.
        Thread({
            while (!cancelled && segment.state == State.DRAINING) {
                val now = SystemClock.elapsedRealtime()
                if (now - segment.endSentAt > limits.drainTimeoutMs) { fail(segment, "drain_timeout"); break }
                if (!segment.connection!!.turnOpen && now - maxOf(segment.endSentAt, segment.lastMessageAt) >= limits.drainGraceMs) { segment.drained("quiet"); break }
                Thread.sleep(50)
            }
        }, "writer-stream-drain").apply { isDaemon = true }.start()
    }

    /** Switches [segment] (and, for the current one, all later audio) to chunked requests from saved audio. */
    private fun fail(segment: Segment, reason: String) {
        val replan: Pair<Long, Long>?
        synchronized(this) {
            if (segment.state == State.STABLE || segment.state == State.FAILED) return
            val wasCurrent = segment === current
            segment.state = State.FAILED
            if (liveFailure == null) liveFailure = reason
            runCatching { segment.connection?.close() }
            checkpoint(field = "live_failure" to liveFailure)
            replan = if (wasCurrent) { chunkFrom(segment.start, reason); null } else segment.start to segment.end
        }
        if (replan != null) Thread({
            try {
                val bounds = ChunkPlanner.plan(audio, replan.first, replan.second)
                for (i in 0 until bounds.size - 1) chunks().submit(bounds[i], bounds[i + 1])
            } catch (_: Exception) { /* Resume re-plans this range from the checkpoint. */ }
            finally { segment.settled.countDown() }
        }, "writer-stream-replan").apply { isDaemon = true }.start()
        else segment.settled.countDown()
    }

    @Synchronized private fun chunkFrom(at: Long, reason: String) {
        if (chunkedFrom >= 0) return
        if (liveFailure == null) liveFailure = reason
        // Also marks the session as resumable from a checkpoint before any range has finished.
        checkpoint(field = "live_failure" to liveFailure)
        chunkedFrom = at
        current?.let { if (it.state == State.LIVE || it.state == State.CONNECTING) { it.state = State.FAILED; runCatching { it.connection?.close() }; it.settled.countDown() } }
        current = null
        // Cuts already planned since this segment started are spoken: send them all at once.
        check(cuts.first() <= at) { "Chunk plan does not reach the failed segment" }
        val planned = cuts.filter { it >= at }
        for (i in 0 until planned.size - 1) chunks().submit(planned[i], planned[i + 1])
        if (ended >= 0) tail()
    }
    /** After Stop, everything after the last planned cut is one more range (or re-planned from the file). */
    @Synchronized private fun tail() {
        val last = cuts.last()
        if (ended <= last) return
        if (!fileMode) { chunks().submit(last, ended); return }
        val bounds = ChunkPlanner.plan(audio, last, ended)
        for (i in 0 until bounds.size - 1) chunks().submit(bounds[i], bounds[i + 1])
    }
    @Synchronized private fun chunks(): ChunkedTranscriber = chunker ?: ChunkedTranscriber(audio, options, ranges,
        timeoutMs = limits.chunkTimeoutMs, completed = { start, end, text -> checkpoint(piece(start, end, "chunk", text)) }).also { chunker = it }

    private fun end(bytes: Long) {
        synchronized(this) {
            ended = bytes; endedAt = SystemClock.elapsedRealtime()
            if (bytes != written && chunkedFrom < 0) { fileMode = true; chunkFrom(current?.start ?: cuts.last(), "frame_gap"); return }
            if (chunkedFrom >= 0) { tail(); return }
        }
        current?.let { endSegment(it, bytes) }
    }

    /** Partial preview: stable text plus the current turn's interim text. Never persisted as a transcript. */
    fun preview(): String = synchronized(this) {
        ChunkedTranscriber.join(segments.flatMap { s -> synchronized(s) { s.finals + listOfNotNull(s.interim.takeIf { it.isNotEmpty() }) } })
    }

    /** Waits for the tail and every fallback request; assembles non-overlapping pieces in audio order. */
    fun await(deadlineElapsedMs: Long, progress: (Int, Int) -> Unit = { _, _ -> }, stop: () -> Boolean = { false }): Outcome {
        val wait = { latch: CountDownLatch ->
            while (!latch.await(100, TimeUnit.MILLISECONDS)) {
                if (stop()) throw InterruptedIOException("Transcription stopped; saved audio and finished pieces retained.")
                if (SystemClock.elapsedRealtime() > deadlineElapsedMs) throw ChunkedTranscriber.ChunkFailure("Live transcription did not finish in time; saved audio retained.", "timeout", null)
            }
        }
        try {
            wait(finished)
            check(ended >= 0) { "Recording did not finish." }
            for (segment in synchronized(this) { segments.toList() }) wait(segment.settled)
            val pieces = mutableListOf<Triple<Long, Long, String>>()
            synchronized(this) { for (s in segments) if (s.state == State.STABLE) pieces += Triple(s.start, s.end, s.text()) }
            chunker?.let { pieces += it.await(deadlineElapsedMs, progress, stop).map { r -> Triple(r.start, r.end, r.text) } }
            pieces.sortBy { it.first }
            check(covers(pieces.map { it.first to it.second }, ended)) { "Recognized pieces do not cover the recording." }
            val receipt = JSONObject().put("method", when { chunker == null -> "live"; segments.none { it.state == State.STABLE } -> "chunked"; else -> "live_and_chunked" })
                .put("segments", segments.size).put("live_failure", liveFailure ?: JSONObject.NULL)
                .put("end_signal", segments.lastOrNull { it.state == State.STABLE }?.signal?.takeIf { it.isNotEmpty() } ?: JSONObject.NULL)
                .put("tail_ms", if (chunker == null) tailMs else JSONObject.NULL)
            chunker?.receipt()?.let { r -> r.keys().forEach { receipt.put(it, r.get(it)) } }
            return Outcome(ChunkedTranscriber.join(pieces.map { it.third }), receipt)
        } catch (error: Exception) {
            if (error is ChunkedTranscriber.ChunkFailure || error is IOException) throw error
            throw IOException(error.message ?: "Live transcription failed; saved audio retained.", error)
        }
    }

    fun cancel() {
        cancelled = true; queue.clear(); queue.offer(End(-1)); worker.interrupt()
        synchronized(this) { segments.forEach { s -> runCatching { s.connection?.close() }; synchronized(s) { s.finals.clear(); s.interim = "" }; s.settled.countDown() } }
        chunker?.cancel()
    }

    private fun piece(start: Long, end: Long, source: String, text: String) = JSONObject().put("start", start).put("end", end).put("source", source).put("text", text)
    /** Best effort: a failed checkpoint write only means more audio is re-requested after a process death. */
    private fun checkpoint(piece: JSONObject? = null, field: Pair<String, Any?>? = null) {
        if (cancelled) return
        runCatching { checkpoints.update(sessionId, options) { doc ->
            piece?.let { doc.getJSONArray("pieces").put(it) }
            field?.let { (key, value) -> doc.put(key, value ?: JSONObject.NULL) }
        } }
    }

    /** Durable per-session recognition checkpoints: app-private, deleted with the session. */
    class Checkpoints(val directory: File) {
        private fun file(id: String) = AtomicFile(File(directory, "${UUID.fromString(id)}.json"))
        @Synchronized fun load(id: String): JSONObject? = runCatching { JSONObject(String(file(id).readFully(), Charsets.UTF_8)) }.getOrNull()
        @Synchronized fun update(id: String, options: JSONObject, change: (JSONObject) -> Unit) {
            check(directory.isDirectory || directory.mkdirs())
            val doc = load(id)?.takeIf { it.optString("binding") == binding(options) }
                ?: JSONObject().put("version", 1).put("binding", binding(options)).put("pieces", JSONArray())
            change(doc)
            val atomic = file(id); val out = atomic.startWrite()
            try { out.write(doc.toString().toByteArray(Charsets.UTF_8)); out.fd.sync(); atomic.finishWrite(out) }
            catch (error: Exception) { atomic.failWrite(out); throw error }
        }
        @Synchronized fun delete(id: String) {
            val target = File(directory, "${UUID.fromString(id)}.json")
            AtomicFile(target).delete()
            check(listOf(target, File("${target.path}.bak"), File("${target.path}.new")).none { it.exists() }) { "Recognition checkpoint deletion needs retry." }
        }
        companion object {
            /** Pieces are reusable only for the same batch model and language. */
            fun binding(options: JSONObject) = options.getJSONObject("selection").getString("model") + "/" + options.getString("input_language")
            fun of(context: Context) = Checkpoints(File(context.filesDir, "recognition"))
        }
    }

    companion object {
        /**
         * Starts streaming for a capture whose frozen choice asked for it, or returns null (phone engine,
         * other providers, streaming off, key unavailable). Never delays or fails the capture.
         */
        fun begin(context: Context, store: SessionStore, item: JSONObject): StreamingRecognition? = runCatching {
            val choice = item.optJSONObject("capture_choice") ?: return null
            val cloud = choice.optJSONObject("cloud") ?: return null
            if (cloud.optString("recognition") != "streaming" || cloud.optString("provider") != "gemini") return null
            val key = DirectCloud(context, "gemini").key(cloud)
            val added = CustomTextModels(context).forProvider("gemini").map { it.model }.toSet()
            val options = JSONObject(choice.getJSONObject("options").toString())
            val language = options.getString("input_language")
            val model = cloud.optString("live_model", GeminiLive.MODEL)
            StreamingRecognition(item.getString("id"), store.audio(item), options, Checkpoints.of(context),
                live = { listener -> GeminiLive.connect(key, language, listener, model) },
                ranges = { GeminiClient(key, added, readTimeoutMs = 20_000) }).also { register(it); it.start() }
        }.getOrNull()
        /** Discard or deletion: stop sending immediately and drop in-memory text. */
        fun discard(sessionId: String) { remove(sessionId)?.cancel() }

        private val active = ConcurrentHashMap<String, StreamingRecognition>()
        fun get(sessionId: String): StreamingRecognition? = active[sessionId]
        /** Partial text while recording or finishing, for display only; null when not streaming. Never a transcript. */
        fun preview(sessionId: String): String? = active[sessionId]?.preview()
        fun register(recognition: StreamingRecognition) { active[recognition.sessionId] = recognition }
        fun remove(sessionId: String) = active.remove(sessionId)

        /** Pieces must tile [0, total) exactly: no gap (lost words) and no overlap (duplicated words). */
        fun covers(pieces: List<Pair<Long, Long>>, total: Long): Boolean {
            var at = 0L
            for ((start, end) in pieces.sortedBy { it.first }) { if (start != at || end <= start) return false; at = end }
            return at == total
        }

        /**
         * After process death: reuse durable pieces and re-request only uncovered audio, cut at pauses.
         * Returns null when there is nothing to resume from.
         */
        fun resume(sessionId: String, audio: File, options: JSONObject, checkpoints: Checkpoints, ranges: () -> RangeClient,
            deadlineElapsedMs: Long, timeoutMs: Long = 25_000, progress: (Int, Int) -> Unit = { _, _ -> }, stop: () -> Boolean = { false }): Outcome? {
            val doc = checkpoints.load(sessionId)?.takeIf { it.optString("binding") == Checkpoints.binding(options) } ?: return null
            val total = audio.length() - 44
            check(total > 0) { "Saved audio is empty." }
            val stored = doc.getJSONArray("pieces").let { array -> (0 until array.length()).map { array.getJSONObject(it) } }
                .filter { it.getLong("start") >= 0 && it.getLong("end") <= total && it.getLong("end") > it.getLong("start") }
                .sortedBy { it.getLong("start") }
            // Keep a non-overlapping prefix-ordered set; drop any piece that overlaps an earlier one.
            val kept = mutableListOf<JSONObject>(); var reach = 0L
            for (p in stored) if (p.getLong("start") >= reach) { kept += p; reach = p.getLong("end") }
            val chunker = ChunkedTranscriber(audio, options, ranges, timeoutMs = timeoutMs, completed = { start, end, text ->
                runCatching { checkpoints.update(sessionId, options) { it.getJSONArray("pieces").put(JSONObject().put("start", start).put("end", end).put("source", "chunk").put("text", text)) } }
            })
            var at = 0L; var reused = 0
            fun gap(until: Long) { if (until > at) { val bounds = ChunkPlanner.plan(audio, at, until); for (i in 0 until bounds.size - 1) chunker.submit(bounds[i], bounds[i + 1]) } }
            for (p in kept) { gap(p.getLong("start")); chunker.submit(p.getLong("start"), p.getLong("end"), p.getString("text")); reused++; at = p.getLong("end") }
            gap(total)
            val results = chunker.await(deadlineElapsedMs, progress, stop)
            val receipt = JSONObject().put("method", "resumed").put("reused_pieces", reused)
                .put("live_failure", doc.opt("live_failure") ?: "process_restarted")
            chunker.receipt().let { r -> r.keys().forEach { receipt.put(it, r.get(it)) } }
            return Outcome(ChunkedTranscriber.join(results.map { it.text }), receipt)
        }
    }
}
