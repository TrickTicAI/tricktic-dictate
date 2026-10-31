package com.metrix.writer

import android.os.SystemClock
import org.json.JSONObject
import java.io.File
import java.io.IOException
import java.io.InterruptedIOException
import java.io.RandomAccessFile
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.Executors
import java.util.concurrent.Future
import java.util.concurrent.ScheduledExecutorService
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicInteger
import kotlin.math.sqrt

/** One saved-PCM range as its own provider request. Offsets are WAV data offsets (after the 44-byte header). */
interface RangeClient {
    fun transcribeRange(audio: File, start: Long, end: Long, options: JSONObject): String
    fun cancel()
}

/**
 * Cuts at the quietest 100 ms frame 20–30 s after the previous cut, as measured in the
 * MW-ANDROID-01 trial. Fed incrementally while recording or from a saved file; it keeps at most
 * one window of frame levels, never the audio.
 */
internal class ChunkPlanner(origin: Long) {
    private val levels = ArrayList<Double>(HIGH + 1)
    private val pending = ByteArray(FRAME.toInt()); private var pendingCount = 0
    var lastCut = origin; private set
    /** Total data bytes fed, as an offset. */
    var position = origin; private set

    /** Returns any new cut offsets. */
    fun feed(bytes: ByteArray, offset: Int, count: Int): List<Long> {
        var cuts: MutableList<Long>? = null
        var at = offset; val end = offset + count
        while (at < end) {
            val n = minOf(end - at, pending.size - pendingCount)
            System.arraycopy(bytes, at, pending, pendingCount, n); pendingCount += n; at += n; position += n
            if (pendingCount == pending.size) {
                levels += rms(pending, pendingCount); pendingCount = 0
                if (levels.size > HIGH) {
                    var best = LOW
                    for (i in LOW until HIGH) if (levels[i] < levels[best]) best = i
                    lastCut += (best + 1) * FRAME
                    repeat(best + 1) { levels.removeAt(0) }
                    (cuts ?: mutableListOf<Long>().also { cuts = it }).add(lastCut)
                }
            }
        }
        return cuts ?: emptyList()
    }

    companion object {
        const val FRAME = 3200L // 100 ms of 16 kHz mono PCM16
        private const val LOW = 200; private const val HIGH = 300
        fun rms(bytes: ByteArray, count: Int): Double {
            var sum = 0.0
            for (i in 0 until count - 1 step 2) { val s = ((bytes[i].toInt() and 255) or (bytes[i + 1].toInt() shl 8)).toShort().toDouble(); sum += s * s }
            return sqrt(sum / maxOf(1, count / 2))
        }
        /** Plans [start, end) of a saved WAV without holding more than one read buffer. */
        fun plan(audio: File, start: Long, end: Long): List<Long> {
            val planner = ChunkPlanner(start); val cuts = mutableListOf(start)
            RandomAccessFile(audio, "r").use { input ->
                input.seek(44 + start); val buffer = ByteArray(64 * 1024); var remaining = end - start
                while (remaining > 0) {
                    val n = input.read(buffer, 0, minOf(remaining, buffer.size.toLong()).toInt())
                    if (n <= 0) throw IOException("Saved audio is shorter than expected")
                    cuts += planner.feed(buffer, 0, n); remaining -= n
                }
            }
            return cuts + end
        }
    }
}

/**
 * Rejects chunk responses that repeat themselves (MW-ANDROID-01: random per-request duplication, e.g.
 * 49 then 103 words for the same 26 s). Read speech legitimately reaches ~190 words/min (LibriSpeech,
 * measured in-app), so the absolute ceiling sits above natural dictation and the relative check compares
 * with this recording's own accepted ranges.
 */
internal object DuplicationGuard {
    const val MAX_WORDS_PER_MINUTE = 280.0
    const val MAX_RELATIVE_RATE = 1.6
    private const val SPAN = 8
    fun words(text: String) = text.lowercase().split(Regex("[^\\p{L}\\p{N}']+")).filter { it.isNotEmpty() }
    fun rate(text: String, audioMs: Long) = words(text).size * 60_000.0 / audioMs
    /** [typical]: median words/min of ranges already accepted in this recording, when there are at least two. */
    fun reason(text: String, audioMs: Long, typical: Double? = null): String? {
        val words = words(text)
        if (audioMs >= 10_000 && (rate(text, audioMs) > MAX_WORDS_PER_MINUTE || typical != null && rate(text, audioMs) > typical * MAX_RELATIVE_RATE)) return "word_rate"
        val seen = HashSet<String>()
        for (i in 0..words.size - SPAN) if (!seen.add(words.subList(i, i + SPAN).joinToString(" "))) return "repeated_span"
        return null
    }
}

/**
 * Bounded parallel batch transcription of saved-audio ranges. Each range is requested as soon as it
 * is submitted, with a deadline, one retry for transport/abort failures and up to two re-requests
 * when the duplication guard rejects a response. Late responses never commit anything: callers read
 * results only through [await].
 */
internal class ChunkedTranscriber(
    private val audio: File,
    private val options: JSONObject,
    private val client: () -> RangeClient,
    private val parallel: Int = 4,
    private val timeoutMs: Long = 25_000,
    /** Called once per completed range, on a worker thread, for durable checkpointing. */
    private val completed: (Long, Long, String) -> Unit = { _, _, _ -> },
    private val retryDelayMs: Long = 500,
) {
    class Result(val start: Long, val end: Long, val text: String)
    class ChunkFailure(message: String, val code: String, val cause0: Exception?) : IOException(message, cause0)

    private val executor = Executors.newFixedThreadPool(parallel) { Thread(it, "writer-chunk").apply { isDaemon = true } }
    private val watchdog: ScheduledExecutorService = Executors.newSingleThreadScheduledExecutor { Thread(it, "writer-chunk-deadline").apply { isDaemon = true } }
    private val futures = linkedMapOf<Long, Pair<Long, Future<Result>>>()
    private val clients = ConcurrentHashMap.newKeySet<RangeClient>()
    @Volatile private var cancelled = false
    private val requests = AtomicInteger()
    /** Requests whose bytes may have reached the provider without a usable answer: possible extra charges. */
    private val uncertain = AtomicInteger()
    private val guardRejections = AtomicInteger()
    private val merged = AtomicInteger()
    private val suspected = AtomicInteger()
    private val rates = mutableListOf<Double>()
    private fun typical(): Double? = synchronized(rates) { if (rates.size < 2) null else rates.sorted()[rates.size / 2] }
    val submitted get() = synchronized(futures) { futures.size }
    val done get() = synchronized(futures) { futures.values.count { it.second.isDone } }

    /** Already-known text for a range (e.g. from a checkpoint) is used without a request. */
    fun submit(start: Long, end: Long, known: String? = null) {
        require(end > start)
        synchronized(futures) {
            if (start in futures) return
            futures[start] = end to if (known != null) java.util.concurrent.CompletableFuture.completedFuture(Result(start, end, known))
                else executor.submit<Result> { request(start, end) }
        }
    }

    private fun once(start: Long, end: Long): String {
        if (cancelled) throw InterruptedIOException("Transcription cancelled; saved audio retained")
        val api = client(); clients += api
        var timedOut = false
        val deadline = watchdog.schedule({ timedOut = true; api.cancel() }, timeoutMs, TimeUnit.MILLISECONDS)
        requests.incrementAndGet()
        try { return api.transcribeRange(audio, start, end, options) }
        catch (error: IOException) {
            uncertain.incrementAndGet()
            if (timedOut) throw ChunkFailure("Cloud request timed out; saved audio retained.", "timeout", error)
            throw error
        } catch (error: IllegalStateException) {
            // The provider answered without a complete transcript (e.g. finishReason OTHER): billed, not uncertain.
            throw ChunkFailure(error.message ?: "Gemini did not return a complete result.", "incomplete", null)
        } finally { deadline.cancel(false); clients -= api }
    }
    private fun retryable(error: Exception) = when (error) {
        is ChunkFailure -> error.code in setOf("timeout", "incomplete")
        is CloudFailure -> error.status == 0 || error.status >= 500 || error.status == 429 && (error.retryAfterMs ?: 0L) <= 5000L
        is InterruptedIOException -> false
        is IOException -> true
        else -> false
    }
    private fun request(start: Long, end: Long): Result {
        var failures = 0; var rejections = 0; var shortest: String? = null
        while (true) {
            try {
                val text = once(start, end)
                val reason = DuplicationGuard.reason(text, (end - start) / 32, typical())
                if (reason == null) {
                    if (end - start >= 10L * 32_000) synchronized(rates) { rates += DuplicationGuard.rate(text, (end - start) / 32) }
                    return Result(start, end, text).also { completed(start, end, text) }
                }
                guardRejections.incrementAndGet()
                if (shortest == null || DuplicationGuard.words(text).size < DuplicationGuard.words(shortest).size) shortest = text
                if (++rejections > 2) {
                    // Keep the most conservative answer rather than discarding the speech; flagged in the receipt.
                    suspected.incrementAndGet()
                    return Result(start, end, shortest).also { completed(start, end, it.text) }
                }
            } catch (error: Exception) {
                if (cancelled) throw InterruptedIOException("Transcription cancelled; saved audio retained")
                if (++failures > 1 || !retryable(error)) throw error
                Thread.sleep(retryDelayMs)
            }
        }
    }

    /**
     * Waits for every submitted range in order. A range that still fails is retried once merged with
     * its neighbour (the trial showed the same audio is accepted inside a larger window).
     */
    fun await(deadlineElapsedMs: Long, progress: (Int, Int) -> Unit = { _, _ -> }, stop: () -> Boolean = { false }): List<Result> {
        val ranges = synchronized(futures) { futures.entries.map { Triple(it.key, it.value.first, it.value.second) } }
        val results = arrayOfNulls<Result>(ranges.size); val failed = mutableListOf<Pair<Int, Exception>>()
        for ((index, range) in ranges.withIndex()) {
            while (true) {
                if (stop()) { cancel(); throw InterruptedIOException("Transcription stopped; saved audio and finished pieces retained.") }
                if (SystemClock.elapsedRealtime() > deadlineElapsedMs) { cancel(); throw ChunkFailure("Cloud transcription did not finish in time; saved audio retained.", "timeout", null) }
                try { results[index] = range.third.get(100, TimeUnit.MILLISECONDS); break }
                catch (_: java.util.concurrent.TimeoutException) { }
                catch (error: java.util.concurrent.ExecutionException) { failed += index to (error.cause as? Exception ?: IOException(error.cause)); break }
            }
            progress(index + 1, ranges.size)
        }
        val absorbed = HashSet<Int>()
        for ((index, error) in failed) {
            if (index in absorbed) continue
            if (cancelled || error !is ChunkFailure || error.code != "incomplete" || ranges.size < 2) throw error
            val (first, last) = if (index > 0 && index - 1 !in absorbed) index - 1 to index else index to index + 1
            if (last >= ranges.size) throw error
            val text = request(ranges[first].first, ranges[last].second).text
            merged.incrementAndGet()
            results[first] = Result(ranges[first].first, ranges[last].second, text); results[last] = null
            absorbed += first; absorbed += last
        }
        return results.filterNotNull().also { executor.shutdown(); watchdog.shutdown() }
    }
    fun cancel() {
        cancelled = true; clients.forEach { it.cancel() }
        synchronized(futures) { futures.values.forEach { it.second.cancel(true) } }
        executor.shutdownNow(); watchdog.shutdownNow()
    }
    fun receipt(): JSONObject = JSONObject().put("chunks", submitted).put("requests", requests.get()).put("uncertain_requests", uncertain.get())
        .put("guard_rejections", guardRejections.get()).put("merged_retries", merged.get()).put("duplication_suspected", suspected.get())

    companion object {
        /** Joins range texts in order; ranges are cut at pauses, so a single space separates them. */
        fun join(texts: List<String>) = texts.map { it.trim() }.filter { it.isNotEmpty() }.joinToString(" ")
    }
}
