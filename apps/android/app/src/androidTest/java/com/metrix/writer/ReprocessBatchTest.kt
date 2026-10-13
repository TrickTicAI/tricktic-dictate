@file:Suppress("DEPRECATION")
package com.metrix.writer

import android.test.InstrumentationTestCase
import android.util.AtomicFile
import androidx.activity.compose.setContent
import androidx.compose.ui.test.*
import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.util.UUID
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicInteger

/** Real AtomicFile/SQLite/native filesystem; fake model execution cannot insert text. */
class ReprocessBatchTest : InstrumentationTestCase() {
    private val context get() = instrumentation.targetContext
    private lateinit var root: File
    private lateinit var store: SessionStore
    override fun setUp() { super.setUp(); root=File(context.cacheDir,"mw113-${UUID.randomUUID()}").also{check(it.mkdirs())}; store=SessionStore(File(root,"recordings")) }
    override fun tearDown() { root.deleteRecursively(); super.tearDown() }
    private fun choice() = JSONObject().put("cloud",JSONObject().put("provider","gemini").put("credential_id",UUID(0,1).toString()).put("transport_version",1))
        .put("server",DirectCloud.ORIGIN).put("options",JSONObject().put("selection",JSONObject().put("provider","gemini").put("model","fixture-stt"))
            .put("input_language","mixed").put("output_language",JSONObject().put("mode","preserve"))
            .put("vocabulary",JSONObject().put("hints",JSONArray()).put("corrections",JSONArray()).put("snippets",JSONArray()))
            .put("cleanup",JSONObject().put("mode","off")))
        .put("policy",JSONObject().put("selection",JSONObject.NULL).put("cloud",JSONObject()).put("system_prompt","frozen prompt v1"))
    private fun source(raw:String="hello שלום 42"):JSONObject {
        val item=store.create("mixed");store.audio(item).appendBytes(ByteArray(320));store.finishCapture(item,null)
        item.getJSONArray("attempts").put(result(item,choice(),raw));store.save(item);return item
    }
    private fun result(version:JSONObject,choice:JSONObject,raw:String="hello שלום 42"):JSONObject {
        val id=UUID.randomUUID().toString()
        return JSONObject().put("id",id).put("execution_location","cloud").put("delivery_choice","raw")
            .put("request",JSONObject().put("recording_id",version.getString("id")).put("idempotency_key",id).put("options",choice.getJSONObject("options")))
            .put("status",JSONObject().put("state","succeeded").put("result",JSONObject().put("transcript",JSONObject().put("raw_text",raw))))
    }
    private fun queue(process:(JSONObject,JSONObject,String)->JSONObject) = ReprocessBatch(context,store,File(root,"queue"),processor=process)
    private fun wait(queue:ReprocessBatch,id:String):JSONObject {
        val deadline=System.currentTimeMillis()+5000
        while(queue.active!=null && System.currentTimeMillis()<deadline) Thread.sleep(5)
        assertNull("worker stopped",queue.active);return queue.load(id)
    }
    fun testMixedFailuresMissingAudioAndReusePreserveOriginals() {
        val good=source();val bad=source();val missing=source();store.audio(missing).delete()
        java.io.RandomAccessFile(store.audio(bad),"rw").use { it.seek(44); it.write(7) }
        val before=store.load(good.getString("id")).toString();val audio=ReprocessBatch.hash(store.audio(good))
        val calls=AtomicInteger();val queue=queue{version,choice,_ -> calls.incrementAndGet();if(version.getJSONObject("historical_source").getString("session_id")==bad.getString("id")) error("Synthetic failure");result(version,choice)}
        val ids=listOf(good,bad,missing).map{it.getString("id")};val batch=queue.plan(ids,"transcribe",choice());val id=batch.getString("id")
        assertEquals("unavailable",batch.getJSONArray("items").getJSONObject(2).getString("state"))
        try{queue.start(id,false);fail("Paid acknowledgement required")}catch(_:IllegalStateException){}
        queue.start(id,true);val first=wait(queue,id);val rows=first.getJSONArray("items")
        assertEquals("succeeded",rows.getJSONObject(0).getString("state"));assertEquals("failed",rows.getJSONObject(1).getString("state"));assertEquals(2,calls.get())
        val next=queue.plan(ids,"transcribe",choice());assertTrue(next.getJSONArray("items").getJSONObject(0).getBoolean("reuse_available"));assertFalse(next.getJSONArray("items").getJSONObject(1).getBoolean("reuse_available"))
        queue.start(next.getString("id"),true);val second=wait(queue,next.getString("id"));assertEquals(3,calls.get())
        val version=second.getJSONArray("items").getJSONObject(0);assertEquals(rows.getJSONObject(0).getString("version_session_id"),version.getString("reused_from"))
        assertFalse(store.load(version.getString("version_session_id")).has("original_target"));assertFalse(store.load(version.getString("version_session_id")).has("auto_submit"))
        assertEquals(before,store.load(good.getString("id")).toString());assertEquals(audio,ReprocessBatch.hash(store.audio(good)))
        assertEquals("hello שלום 42",CompanionProcessing.raw(store.load(version.getString("version_session_id")).getJSONArray("attempts").getJSONObject(0)))
    }
    fun testFingerprintInvalidatesModelLanguageVocabularySnippetsAndPrompts() {
        val item=source();val calls=AtomicInteger();val queue=queue{version,c,_ -> calls.incrementAndGet();result(version,c)};val ids=listOf(item.getString("id"))
        val initial=queue.plan(ids,"transcribe",choice());queue.start(initial.getString("id"),true);wait(queue,initial.getString("id"))
        for(change in listOf("model","language","vocabulary","snippets","prompt")) {
            val changed=choice();val options=changed.getJSONObject("options")
            when(change){"model"->options.getJSONObject("selection").put("model","changed");"language"->options.put("input_language","en");"vocabulary"->options.getJSONObject("vocabulary").getJSONArray("hints").put("different");"snippets"->options.getJSONObject("vocabulary").getJSONArray("snippets").put(JSONObject().put("phrase","test").put("text","exact"));else->changed.getJSONObject("policy").put("system_prompt","v2")}
            assertFalse(change,queue.plan(ids,"transcribe",changed).getJSONArray("items").getJSONObject(0).getBoolean("reuse_available"))
        }
        val before=queue.plan(ids,"transcribe",choice());store.audio(item).appendBytes(byteArrayOf(1,2));queue.start(before.getString("id"),true)
        assertEquals("failed",wait(queue,before.getString("id")).getJSONArray("items").getJSONObject(0).getString("state"));assertEquals(1,calls.get())
    }
    fun testCancellationStopsLaterItemsAndKeepsSources() {
        val first=source();val second=source();val entered=CountDownLatch(1);val release=CountDownLatch(1);val calls=AtomicInteger()
        val queue=queue{version,c,_ -> calls.incrementAndGet();entered.countDown();check(release.await(5,TimeUnit.SECONDS));result(version,c)}
        val batch=queue.plan(listOf(first,second).map{it.getString("id")},"transcribe",choice());queue.start(batch.getString("id"),true)
        assertTrue(entered.await(5,TimeUnit.SECONDS));queue.cancel(batch.getString("id"));release.countDown();val canceled=wait(queue,batch.getString("id"))
        assertEquals("canceled",canceled.getString("state"));assertEquals(1,calls.get());assertEquals("canceled",canceled.getJSONArray("items").getJSONObject(1).getString("state"))
        assertTrue(store.audio(first).isFile);assertEquals("hello שלום 42",CompanionProcessing.raw(store.load(first.getString("id")).getJSONArray("attempts").getJSONObject(0)))
    }
    fun testRestartPausesAndExplicitResumeCompletesWithNoAutomaticSubmission() {
        val item=source();val calls=AtomicInteger();val process={version:JSONObject,c:JSONObject,_:String -> calls.incrementAndGet();result(version,c)}
        val queue=queue(process);val batch=queue.plan(listOf(item.getString("id")),"transcribe",choice());val id=batch.getString("id")
        batch.put("state","running");batch.getJSONArray("items").getJSONObject(0).put("state","running")
        AtomicFile(File(root,"queue/$id.json")).let { file ->val out=file.startWrite();out.write(batch.toString().toByteArray());out.fd.sync();file.finishWrite(out) }
        val reopened=queue(process);assertEquals("paused",reopened.load(id).getString("state"));assertEquals("interrupted",reopened.load(id).getJSONArray("items").getJSONObject(0).getString("state"));assertEquals(0,calls.get())
        reopened.start(id,true);assertEquals("succeeded",wait(reopened,id).getJSONArray("items").getJSONObject(0).getString("state"));assertEquals(1,calls.get())
    }
    fun testTextOnlyCleanupAndBoundsKeepRawSeparate() {
        val item=source();val before=store.load(item.getString("id")).toString();store.audio(item).delete()
        val queue=queue{version,c,operation ->assertEquals("cleanup",operation);result(version,c).put("dictionary_text","Hi שלום 42")}
        val batch=queue.plan(listOf(item.getString("id")),"cleanup",choice());assertFalse(batch.getBoolean("paid_work"));queue.start(batch.getString("id"),false)
        val done=wait(queue,batch.getString("id"));val entry=done.getJSONArray("items").getJSONObject(0);assertEquals("succeeded",entry.getString("state"))
        val result=store.load(entry.getString("version_session_id")).getJSONArray("attempts").getJSONObject(0);assertEquals("hello שלום 42",CompanionProcessing.raw(result));assertEquals("Hi שלום 42",result.getString("dictionary_text"))
        assertEquals(before,store.load(item.getString("id")).toString())
        for(ids in listOf(emptyList(),List(33){UUID.randomUUID().toString()},listOf(item.getString("id"),item.getString("id")))) {
            try{queue.plan(ids,"cleanup",choice());fail("Queue bounds")}catch(_:IllegalArgumentException){}
        }
        queue.forgetSources(setOf(item.getString("id")));assertTrue(queue.list().isEmpty())
    }
    fun testRealStageCoordinatorsUseFakeProvidersAndNeverCacheCleanupFailure() {
        val item=source();val before=store.load(item.getString("id")).toString()
        val calls=AtomicInteger();val failCleanup=java.util.concurrent.atomic.AtomicBoolean(true)
        val queue=ReprocessBatch(context,store,File(root,"real-queue"),cloudFactory={ object:CloudClient {
            override val usesFiles=false
            override fun cancel() {}
            override fun checkModel(model:String) {}
            override fun transcribeAudio(audio:File,options:JSONObject):String {calls.incrementAndGet();assertEquals("mixed",options.getString("input_language"));return "hello שלום 42"}
            override fun cleanup(model:String,text:String,reasoning:String,systemPrompt:String):String {calls.incrementAndGet();if(failCleanup.get())throw java.io.IOException("Synthetic failure");return "Hello שלום 42."}
        } })
        val raw=queue.plan(listOf(item.getString("id")),"transcribe",choice());queue.start(raw.getString("id"),true)
        assertEquals("succeeded",wait(queue,raw.getString("id")).getJSONArray("items").getJSONObject(0).getString("state"))
        val frozen=choice();val selection=JSONObject().put("provider","openai").put("model","gpt-4.1-mini").put("reasoning","off")
        frozen.getJSONObject("options").put("cleanup",JSONObject().put("mode","enabled").put("selection",selection))
        frozen.put("policy",JSONObject().put("selection",selection).put("cloud",JSONObject().put("provider","openai").put("credential_id",UUID(0,2).toString()).put("transport_version",1))
            .put("fallback",JSONObject.NULL).put("fallback_after_ms",1000).put("timeout_ms",3000).put("system_prompt","Frozen cleanup instructions").put("spoken_punctuation",true).put("remove_fillers",false))
        val failed=queue.plan(listOf(item.getString("id")),"cleanup",frozen);queue.start(failed.getString("id"),true)
        assertEquals("failed",wait(queue,failed.getString("id")).getJSONArray("items").getJSONObject(0).getString("state"))
        failCleanup.set(false)
        val next=queue.plan(listOf(item.getString("id")),"cleanup",frozen);assertFalse(next.getJSONArray("items").getJSONObject(0).getBoolean("reuse_available"));queue.start(next.getString("id"),true)
        val entry=wait(queue,next.getString("id")).getJSONArray("items").getJSONObject(0);assertEquals("succeeded",entry.getString("state"));assertEquals(3,calls.get())
        val version=store.load(entry.getString("version_session_id"));val job=version.getJSONArray("attempts").getJSONObject(0)
        assertEquals("hello שלום 42",CompanionProcessing.raw(job));assertEquals("Hello שלום 42.",job.getString("cleaned_text"));assertEquals("fixture-stt",job.getJSONObject("request").getJSONObject("options").getJSONObject("selection").getString("model"))
        // Sync projects cleanup versions under the original raw record, with dictionary -> cleaned ancestry.
        val journal=com.metrix.writer.sync.TranscriptStore(context)
        assertFalse("A version must wait for its retained original",journal.capture(version))
        assertTrue(journal.records().values.none{it.optString("session")==version.getString("id")})
        assertTrue(journal.capture(item));assertTrue(journal.capture(version))
        val projected=journal.records().values.filter{it.optString("session")==version.getString("id")}
        assertTrue(projected.isNotEmpty());assertTrue(projected.all{it.getJSONObject("data").getString("kind")!="original"})
        val rootId=com.metrix.writer.sync.TranscriptStore.stable("original:${item.getJSONArray("attempts").getJSONObject(0).getString("id")}")
        assertTrue(projected.all{it.getJSONObject("data").getString("original_record_id")==rootId})
        val corrected=projected.first{it.getJSONObject("data").getString("kind")=="corrected"}
        val cleaned=projected.first{it.getJSONObject("data").getString("kind")=="cleaned"}
        assertEquals(corrected.getString("id"),cleaned.getJSONObject("data").getString("parent_record_id"))
        val again=queue.plan(listOf(version.getString("id")),"cleanup",frozen);queue.start(again.getString("id"),true)
        val againEntry=wait(queue,again.getString("id")).getJSONArray("items").getJSONObject(0)
        assertEquals("succeeded",againEntry.getString("state"))
        val againVersion=store.load(againEntry.getString("version_session_id"));journal.capture(againVersion)
        val repeated=journal.records().values.filter{it.optString("session")==againVersion.getString("id")}
        assertTrue(repeated.isNotEmpty());assertTrue(repeated.all{it.getJSONObject("data").getString("original_record_id")==rootId})
        journal.deleteSession(againVersion.getString("id"))
        journal.deleteSession(version.getString("id"));journal.deleteSession(item.getString("id"))
        assertEquals(before,store.load(item.getString("id")).toString());assertFalse(version.has("original_target"));assertFalse(version.has("auto_submit"))
    }
}

@org.junit.runner.RunWith(androidx.test.ext.junit.runners.AndroidJUnit4::class)
class ReprocessBatchUiTest {
    @get:org.junit.Rule val compose = androidx.compose.ui.test.junit4.createAndroidComposeRule<MainActivity>()
    @org.junit.Test fun selectionProgressComparisonAndOriginalRetention() {
        val context=compose.activity
        val manager=SessionManager.get(context);val store=manager.store
        val source=store.create("mixed");store.audio(source).appendBytes(ByteArray(320));store.finishCapture(source,null)
        val jobId=UUID.randomUUID().toString()
        val prefs=CompanionPreferences(context)
        val original=JSONObject().put("id",jobId).put("execution_location","cloud").put("delivery_choice","raw")
            .put("request",JSONObject().put("options",prefs.options("gemini","fixture-stt","mixed")))
            .put("status",JSONObject().put("state","succeeded").put("result",JSONObject().put("transcript",JSONObject().put("raw_text","Original hello שלום"))))
        source.getJSONArray("attempts").put(original);store.save(source)
        val originalBytes=store.load(source.getString("id")).toString();val audio=ReprocessBatch.hash(store.audio(source))
        val directory=File(context.cacheDir,"mw113-ui-${UUID.randomUUID()}")
        val queue=ReprocessBatch(context,store,directory,processor={version,choice,_ -> JSONObject(original.toString()).put("id",UUID.randomUUID().toString())
            .put("request",JSONObject().put("recording_id",version.getString("id")).put("options",choice.getJSONObject("options"))).put("dictionary_text","New hello שלום") })
        val vm=com.metrix.writer.ui.WriterViewModel(context.application)
        compose.activity.runOnUiThread { compose.activity.setContent {
            com.metrix.writer.ui.WriterTheme {
                com.metrix.writer.ui.ReprocessHistory(vm,com.metrix.writer.ui.WriterState(records=listOf(com.metrix.writer.ui.Dictation.from(source))),{},queue)
            }
        } }
        try {
            compose.onNodeWithText("Reprocess selected recordings").performClick()
            compose.onNodeWithText("Saved raw text",substring=true).performClick()
            compose.onAllNodes(isToggleable())[0].performClick()
            compose.onNodeWithText("Review batch plan (1)").performScrollTo().performClick()
            compose.waitUntil(5000){compose.onAllNodesWithText("Start reviewed batch").fetchSemanticsNodes().isNotEmpty()}
            compose.onNodeWithText("Start reviewed batch").performScrollTo().performClick()
            compose.waitUntil(5000){compose.onAllNodesWithText("Compare version").fetchSemanticsNodes().isNotEmpty()}
            compose.onNodeWithText("Item 1 · succeeded").performScrollTo().assertIsDisplayed()
            compose.onNodeWithText("Compare version").performScrollTo().performClick()
            compose.onNodeWithText("Original hello שלום",useUnmergedTree=true).assertIsDisplayed()
            compose.onNodeWithText("Raw text:\nOriginal hello שלום\n\nNew result:\nNew hello שלום").assertIsDisplayed()
            org.junit.Assert.assertEquals(originalBytes,store.load(source.getString("id")).toString())
            org.junit.Assert.assertEquals(audio,ReprocessBatch.hash(store.audio(source)))
            org.junit.Assert.assertFalse(File(context.filesDir,"deliveries").listFiles().orEmpty().any{it.name.endsWith(".json")})
        } finally {
            queue.list().forEach{batch ->val rows=batch.getJSONArray("items");(0 until rows.length()).forEach{i->runCatching{store.deleteSaved(rows.getJSONObject(i).getString("version_session_id"))}}}
            store.deleteSaved(source.getString("id"));directory.deleteRecursively()
        }
    }
}

/** Run prepare and resume in separate instrumentation processes with force-stop between them. */
class ReprocessProcessRestartTest : InstrumentationTestCase() {
    private val context get()=instrumentation.targetContext
    private val root get()=File(context.cacheDir,"mw113-process-restart")
    private fun options()=JSONObject().put("selection",JSONObject().put("provider","gemini").put("model","fixture-stt"))
        .put("input_language","mixed").put("output_language",JSONObject().put("mode","preserve"))
        .put("vocabulary",JSONObject().put("hints",JSONArray()).put("corrections",JSONArray()).put("snippets",JSONArray())).put("cleanup",JSONObject().put("mode","off"))
    private fun result(version:JSONObject,choice:JSONObject)=JSONObject().put("id",UUID.randomUUID().toString()).put("execution_location","cloud").put("delivery_choice","raw")
        .put("request",JSONObject().put("recording_id",version.getString("id")).put("options",choice.getJSONObject("options")))
        .put("status",JSONObject().put("state","succeeded").put("result",JSONObject().put("transcript",JSONObject().put("raw_text","Synthetic hello שלום"))))
    fun testPrepareInterruptedProcess() {
        check(!root.exists()) { "Use the MW-113 owned emulator with an empty restart fixture." };check(root.mkdirs())
        val store=SessionStore(File(root,"recordings"));val calls=File(root,"calls");calls.writeText("0")
        val started=CountDownLatch(1)
        val queue=ReprocessBatch(context,store,File(root,"queue"),processor={version,choice,_ ->
            val count=calls.readText().toInt()+1;calls.writeText(count.toString())
            if(count==2){started.countDown();CountDownLatch(1).await()};result(version,choice) })
        val ids=(1..2).map { n->val item=store.create("mixed");store.audio(item).appendBytes(ByteArray(320){n.toByte()});store.finishCapture(item,null);item.getString("id") }
        val choice=JSONObject().put("cloud",JSONObject().put("provider","gemini").put("credential_id",UUID(0,1).toString()).put("transport_version",1)).put("server",DirectCloud.ORIGIN).put("options",options())
        val plan=queue.plan(ids,"transcribe",choice);queue.start(plan.getString("id"),true);assertTrue(started.await(5,TimeUnit.SECONDS))
        val persisted=queue.load(plan.getString("id"));assertEquals("succeeded",persisted.getJSONArray("items").getJSONObject(0).getString("state"));assertEquals("running",persisted.getJSONArray("items").getJSONObject(1).getString("state"))
        File(root,"receipt.json").writeText(JSONObject().put("id",plan.getString("id")).put("first_version",persisted.getJSONArray("items").getJSONObject(0).getString("version_session_id")).toString())
    }
    fun testResumeAfterProcessDeath() {
        val receipt=JSONObject(File(root,"receipt.json").readText());val store=SessionStore(File(root,"recordings"));val calls=File(root,"calls")
        val queue=ReprocessBatch(context,store,File(root,"queue"),processor={version,choice,_ ->calls.writeText((calls.readText().toInt()+1).toString());result(version,choice)})
        val paused=queue.load(receipt.getString("id"));assertEquals("paused",paused.getString("state"));assertEquals("interrupted",paused.getJSONArray("items").getJSONObject(1).getString("state"));assertEquals("2",calls.readText())
        queue.start(receipt.getString("id"),true)
        val deadline=System.currentTimeMillis()+5000;while(queue.active!=null&&System.currentTimeMillis()<deadline)Thread.sleep(5)
        assertNull(queue.active);val done=queue.load(receipt.getString("id"));assertEquals("complete",done.getString("state"));assertEquals("3",calls.readText());assertEquals(receipt.getString("first_version"),done.getJSONArray("items").getJSONObject(0).getString("version_session_id"))
        assertEquals("Synthetic hello שלום",CompanionProcessing.raw(store.load(receipt.getString("first_version")).getJSONArray("attempts").getJSONObject(0)));root.deleteRecursively()
    }
}
