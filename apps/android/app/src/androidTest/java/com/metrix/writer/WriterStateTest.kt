@file:Suppress("DEPRECATION")
package com.metrix.writer

import android.test.InstrumentationTestCase
import com.metrix.writer.ui.Attempt
import com.metrix.writer.ui.Dictation
import com.metrix.writer.ui.TextEdit
import com.metrix.writer.ui.WriterStatistics
import com.metrix.writer.ui.WriterViewModel
import org.json.JSONArray
import org.json.JSONObject
import java.time.LocalDate
import java.time.ZoneId
import java.util.UUID
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

class WriterStateTest : InstrumentationTestCase() {
    private fun main(action: () -> Unit) = instrumentation.runOnMainSync(action)
    private val utc = ZoneId.of("UTC")
    private val today = LocalDate.parse("2026-09-14")
    private fun attempt(id: String, state: String, model: String = "base", location: String = "phone", latency: Long? = null, edits: List<TextEdit> = emptyList()) =
        Attempt(id, state, if (state == "succeeded") "one two three" else "", null, null, model, location, null, latency, edits)
    private fun record(id: String, date: String, attempts: List<Attempt>) =
        Dictation(id, LocalDate.parse(date).atStartOfDay(utc).toInstant().toEpochMilli(), 60000, "complete", false, false, attempts, null, null, "")
    private fun stats(records: List<Dictation>, range: Int = 7) = WriterStatistics.compute(records, range, today, utc)

    fun testModelSuccessRateCountsFinishedWorkOnly() {
        val edits = listOf(TextEdit("gemini", "flash", "complete", "succeeded", 1200),
            TextEdit("gemini", "flash", "incomplete", null, null),
            TextEdit("gemini", "flash", "replaced", null, null))
        val row = record("r", "2026-09-14", listOf(attempt("a", "succeeded", latency = 1000, edits = edits), attempt("b", "failed"), attempt("c", "running")))
        val speech = stats(listOf(row)).speechModels.single()
        assertEquals(3, speech.attempts); assertEquals(1, speech.successes); assertEquals(2, speech.finished); assertEquals(50, speech.rate)
        val text = stats(listOf(row)).textModels.single()
        // The replaced edit has an unknown outcome, so it counts as use but never as a finished failure.
        assertEquals(3, text.attempts); assertEquals(1, text.successes); assertEquals(2, text.finished); assertEquals(50, text.rate)
        val unfinished = stats(listOf(record("p", "2026-09-14", listOf(attempt("only", "running"))))).speechModels.single()
        assertEquals(1, unfinished.attempts); assertNull(unfinished.rate); assertNull(unfinished.median)
    }
    fun testMedianAndPercentileForOddAndEvenSampleCounts() {
        val odd = listOf(300L, 100L, 500L, 200L, 400L)
        assertEquals(300L, WriterStatistics.percentile(odd, 0.5)); assertEquals(500L, WriterStatistics.percentile(odd, 0.9))
        val even = listOf(400L, 100L, 300L, 200L)
        assertEquals(200L, WriterStatistics.percentile(even, 0.5)); assertEquals(400L, WriterStatistics.percentile(even, 0.9))
        assertEquals(100L, WriterStatistics.percentile(listOf(100L), 0.5)); assertNull(WriterStatistics.percentile(emptyList(), 0.5))
        val five = odd.mapIndexed { index, ms -> attempt("s$index", "succeeded", latency = ms) }
        val model = stats(listOf(record("r", "2026-09-14", five))).speechModels.single()
        assertEquals(300L, model.median); assertEquals(500L, model.p90); assertEquals(5, model.timings.size)
    }
    fun testUnmeasuredWorkIsExcludedFromTimingRatherThanCountedAsZero() {
        val edits = listOf(TextEdit("gemini", "flash", "complete", "succeeded", 2000), TextEdit("gemini", "flash", "complete", "succeeded", null))
        val attempts = listOf(attempt("timed", "succeeded", latency = 250, edits = edits), attempt("untimed", "succeeded"), attempt("failedTimed", "failed", latency = 900))
        val result = stats(listOf(record("r", "2026-09-14", attempts)))
        val speech = result.speechModels.single()
        assertEquals(3, speech.attempts); assertEquals(listOf(250L), speech.timings); assertEquals(250L, speech.median)
        val text = result.textModels.single()
        assertEquals(2, text.attempts); assertEquals(2, text.successes); assertEquals(listOf(2000L), text.timings); assertEquals(2000L, text.p90)
        val none = stats(listOf(record("n", "2026-09-14", listOf(attempt("x", "succeeded"))))).speechModels.single()
        assertTrue(none.timings.isEmpty()); assertNull(none.median); assertNull(none.p90)
    }
    fun testSavedReceiptsWithoutAFinishTimeAreParsedAsUnmeasured() {
        val job = JSONObject("""{"id":"job-1","execution_location":"cloud","status":{"state":"succeeded","result":{"transcript":{"raw_text":"hello there"}}},
            "client_timing":{"stop_to_result_ms":1200},
            "processing_attempts":[
              {"started_at_ms":1000,"finished_at_ms":2500,"selection":{"provider":"gemini","model":"flash"},"state":"complete","cleanup":{"state":"succeeded"}},
              {"started_at_ms":1000,"selection":{"provider":"gemini","model":"flash"},"state":"complete","cleanup":{"state":"succeeded"}},
              {"started_at_ms":1000,"finished_at_ms":1400,"selection":null,"state":"complete"}]}""")
        val item = JSONObject().put("id", UUID.randomUUID().toString()).put("capture_state", "complete")
            .put("created_at_ms", today.atStartOfDay(utc).toInstant().toEpochMilli()).put("duration_ms", 60000).put("attempts", JSONArray().put(job))
        val parsed = Dictation.from(item)
        assertEquals(1200L, parsed.attempts.single().latency)
        // The dictionary-only receipt named no text model and is not model use.
        assertEquals(2, parsed.attempts.single().edits.size)
        assertEquals(listOf(1500L, null), parsed.attempts.single().edits.map { it.elapsed })
        val text = stats(listOf(parsed)).textModels.single()
        assertEquals("flash", text.label); assertEquals(2, text.attempts); assertEquals(100, text.rate); assertEquals(listOf(1500L), text.timings)
    }
    fun testDailyModelBucketsFollowTheRecordingDate() {
        val edits = listOf(TextEdit("gemini", "flash", "complete", "succeeded", 900))
        val rows = listOf(record("old", "2026-09-07", listOf(attempt("out", "succeeded"))),
            record("first", "2026-09-08", listOf(attempt("f1", "succeeded"), attempt("f2", "failed"))),
            record("last", "2026-09-14", listOf(attempt("l1", "succeeded", model = "gpt-4o", location = "cloud", edits = edits))))
        val week = stats(rows)
        assertEquals(7, week.modelDays.size)
        assertEquals(LocalDate.parse("2026-09-08"), week.modelDays.first().first)
        assertEquals(LocalDate.parse("2026-09-14"), week.modelDays.last().first)
        assertEquals(mapOf("speech:base · phone" to 2), week.modelDays.first().second)
        // The edit belongs to its recording's day, like a retry, and never to the day it ran.
        assertEquals(mapOf("speech:gpt-4o · cloud" to 1, "text:gemini/flash" to 1), week.modelDays.last().second)
        assertTrue(week.modelDays.drop(1).dropLast(1).all { it.second.isEmpty() })
        assertEquals(listOf("base · phone", "gpt-4o · cloud"), week.speechModels.map { "${it.label} · ${it.detail}" })
        val month = stats(rows, 30)
        assertEquals(30, month.modelDays.size)
        assertEquals(1, month.modelDays.single { it.first == LocalDate.parse("2026-09-07") }.second.values.sum())
    }
    fun testOldBannerExpiryDoesNotDismissNewFeedback() {
        lateinit var vm: WriterViewModel
        main { vm = WriterViewModel(instrumentation.targetContext.applicationContext as android.app.Application); vm.notify("Old notice") }
        Thread.sleep(4000)
        main { vm.notify("New action needs attention", true) }
        Thread.sleep(800)
        assertEquals("New action needs attention", vm.state.value.message)
        main { vm.notify("Short notice") }
        Thread.sleep(4700)
        assertNull(vm.state.value.message)
    }
    fun testCompletionFromPreviousScreenCannotOverwriteNewAction() {
        lateinit var vm: WriterViewModel
        val began = CountDownLatch(1); val release = CountDownLatch(1)
        main {
            vm = WriterViewModel(instrumentation.targetContext.applicationContext as android.app.Application)
            vm.action { began.countDown(); release.await(3, TimeUnit.SECONDS); vm.notify("Stale completion") }
        }
        assertTrue(began.await(3, TimeUnit.SECONDS))
        main { vm.clearScreenMessage(); vm.notify("New screen") }
        release.countDown()
        repeat(30) { if (vm.state.value.actionBusy) Thread.sleep(100) }
        assertFalse(vm.state.value.actionBusy)
        assertEquals("New screen", vm.state.value.message)
        main { vm.clearScreenMessage() }
    }
}
