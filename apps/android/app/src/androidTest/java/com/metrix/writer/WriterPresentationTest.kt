@file:Suppress("DEPRECATION")
package com.metrix.writer

import android.test.AndroidTestCase
import com.metrix.writer.ui.*
import java.time.LocalDate
import java.time.ZoneId
import org.json.JSONObject
import java.util.UUID
import java.io.File

class WriterPresentationTest : AndroidTestCase() {
    private fun row(id: String, date: String, raw: String, duration: Long = 60000, latency: Long? = null, attempts: List<Attempt>? = null) = Dictation(id,LocalDate.parse(date).atStartOfDay(ZoneId.of("UTC")).toInstant().toEpochMilli(),duration,"complete",false,false,
        attempts ?: listOf(Attempt("job-$id","succeeded",raw,null,null,"base","phone",null)),latency,null,"")
    fun testStatisticsCountLatestSuccessOnceAfterFailedRetry() {
        val attempts=listOf(Attempt("a","succeeded","old text",null,null,"base","phone",null),Attempt("b","succeeded","שלום עולם hello",null,null,"base","phone",null),Attempt("c","failed","",null,null,"base","phone","offline"))
        val r=row("r","2026-09-14","",attempts=attempts)
        val s=WriterStatistics.compute(listOf(r,r),7,LocalDate.parse("2026-09-14"),ZoneId.of("UTC"))
        assertEquals(1,s.sessions);assertEquals(3,s.words);assertEquals(2,s.retries);assertEquals(2,s.successes);assertEquals(1,s.failures);assertEquals(3,s.pace);assertNull(s.medianLatency)
    }
    fun testDateRangeMissingTimingAndZeroDuration() {
        val s=WriterStatistics.compute(listOf(row("a","2026-09-07","outside"),row("b","2026-09-08","first day",0),row("c","2026-09-14","last day",60000,0),row("d","2026-09-15","future")),7,LocalDate.parse("2026-09-14"),ZoneId.of("UTC"))
        assertEquals(2,s.sessions);assertEquals(4,s.words);assertEquals(2,s.pace);assertEquals(0L,s.medianLatency);assertEquals(2,s.days.first().second);assertEquals(2,s.days.last().second)
        assertEquals(2000L,WriterStatistics.compute(listOf(row("low","2026-09-14","a",latency=1000),row("high","2026-09-14","b",latency=3000)),0,LocalDate.parse("2026-09-14")).medianLatency)
        assertNull(WriterStatistics.compute(emptyList(),0).pace)
        assertEquals(4,WriterStatistics.words("שָׁלוֹם, עולם! it's 2026"))
    }
    fun testFloatingGeometryKeepsPanelAboveKeyboardAndOnScreen() {
        assertEquals(0 to 44,FloatingGeometry.clamp(-20,900,214,56,360,100))
        assertEquals(0,FloatingGeometry.dock(20,48,360));assertEquals(312,FloatingGeometry.dock(260,48,360))
        assertTrue(FloatingGeometry.dropped(144,700,48,48,140,690,80))
        assertFalse(FloatingGeometry.dropped(0,700,48,48,140,690,80))
    }
    fun testSnoozeExpiresWithoutChangingRecordingAndPreferencesSurviveRecreation() {
        val prefs=FloatingPreferences(context)
        prefs.minutes=30;prefs.size=8;assertEquals(32,prefs.size);prefs.size=100;assertEquals(56,prefs.size)
        prefs.snooze(1000);assertTrue(FloatingPreferences(context).snoozed(1799999));assertFalse(prefs.snoozed(1801000))
        prefs.showNow();assertFalse(prefs.snoozed());prefs.size=40;prefs.minutes=10
    }
    fun testCombinedChoiceUsesDirectCloudAndDoesNotRewritePendingPhone() {
        val cloud=DirectCloud(context)
        assertFalse("Use an unconfigured emulator",cloud.configured())
        cloud.saveKey("fixture-only-not-a-real-secret-123456")
        val store=SessionStore(File(context.cacheDir,"combined-${UUID.randomUUID()}"))
        val choice=DictationChoice(context)
        val old=context.getSharedPreferences("phone-engine",0).all
        try {
            choice.save(false,"base","en");assertTrue(choice.capture().has("cloud"));assertFalse(choice.capture().has("engine"))
            val item=store.create("he");store.finishCapture(item,null)
            choice.save(true,"base","he");choice.prepare(store,item)
            val snapshot=store.load(item.getString("id")).toString()
            choice.save(false,"base","en");choice.prepare(store,store.load(item.getString("id")))
            assertEquals(snapshot,store.load(item.getString("id")).toString())
            cloud.replacePending(store,item.getString("id"))
            val replaced=store.load(item.getString("id"));val earlier=replaced.getJSONArray("attempts").getJSONObject(0)
            assertEquals("phone",earlier.getString("execution_location"));assertTrue(earlier.has("engine_snapshot"));assertTrue(replaced.has("pending_cloud"));assertFalse(replaced.has("pending_engine"))
        } finally {
            cloud.forget();store.directory.deleteRecursively()
            val edit=context.getSharedPreferences("phone-engine",0).edit().clear()
            old.forEach { (k,v)->when(v){is String->edit.putString(k,v);is Boolean->edit.putBoolean(k,v)} };edit.commit()
        }
    }
    fun testEngineSwitchesBothWaysAndCommitsDurably() {
        val cloud=DirectCloud(context,"openai")
        assertFalse("Use an unconfigured emulator",cloud.configured())
        val models=PhoneModels(context); val weights=models.file("base"); val placeholder=!weights.exists()
        val oldPhone=context.getSharedPreferences("phone-engine",0).all
        val oldCloud=DirectCloud.selectedProvider(context); val oldLanguage=context.getSharedPreferences("speech",0).getString("language",null)
        fun fresh()=DictationChoice(context) // Each read goes through committed preferences; process restart is proven in the UI smoke.
        try {
            val choice=DictationChoice(context)
            try { choice.useCloud("openai"); fail("Unconfigured cloud must not become active") } catch (_: IllegalStateException) {}
            if (placeholder) weights.writeBytes(ByteArray(8)) // Selection only checks presence; inference verifies hashes.
            cloud.saveKey("fixture-only-not-a-real-secret-123456")
            choice.saveLanguage("he")
            choice.usePhone("base")
            assertTrue(fresh().isPhone());assertEquals("base",fresh().model());assertEquals("base · on this phone",fresh().engineLabel())
            assertEquals("he",fresh().language()) // Switching engines keeps a language the model supports.
            assertEquals("phone",fresh().capture().getString("server"))
            choice.useCloud("openai")
            assertFalse(fresh().isPhone());assertEquals("openai",DirectCloud.selectedProvider(context))
            assertTrue(fresh().engineLabel().startsWith("OpenAI · "));assertTrue(fresh().capture().has("cloud"))
            assertEquals("openai",fresh().capture().getJSONObject("cloud").getString("provider"))
            choice.usePhone("base");assertTrue(fresh().isPhone())
            choice.useCloud("openai");assertFalse(fresh().isPhone())
        } finally {
            cloud.forget(); if (placeholder) weights.delete()
            val edit=context.getSharedPreferences("phone-engine",0).edit().clear()
            oldPhone.forEach { (k,v)->when(v){is String->edit.putString(k,v);is Boolean->edit.putBoolean(k,v)} };edit.commit()
            context.getSharedPreferences("direct-cloud",0).edit().putString("provider",oldCloud).commit()
            context.getSharedPreferences("speech",0).edit().apply { if (oldLanguage==null) remove("language") else putString("language",oldLanguage) }.commit()
        }
    }
    fun testPhoneTranscriptCleanupUsesTextOnlyAndKeepsOriginal() {
        val store=SessionStore(File(context.cacheDir,"cleanup-${UUID.randomUUID()}"))
        try {
            val item=store.create("mixed");store.finishCapture(item,null)
            val job=JSONObject().put("id","phone-result").put("execution_location","phone")
                .put("request",JSONObject().put("options",CompanionPreferences(context).options("phone","base","mixed")))
                .put("status",JSONObject("""{"state":"succeeded","result":{"transcript":{"raw_text":"שלום hello"}}}"""))
            item.getJSONArray("attempts").put(job);store.save(item)
            val before=PhoneModels.hash(store.audio(item))
            DirectCloudProcessing(context,store).prepare(item.getString("id"),"phone-result",null)
            assertFalse(store.load(item.getString("id")).has("pending"))
            DirectCloudProcessing(context,store).run(item.getString("id"))
            val saved=store.load(item.getString("id"))
            assertEquals(before,PhoneModels.hash(store.audio(saved)))
            assertEquals("שלום hello",CompanionProcessing.raw(saved.getJSONArray("attempts").getJSONObject(0)))
            assertFalse(saved.has("pending_postprocess"))
        } finally { store.directory.deleteRecursively() }
    }
}
