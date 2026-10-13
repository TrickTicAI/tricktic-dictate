package com.metrix.writer

import android.app.Instrumentation
import android.os.Bundle
import org.json.JSONObject
import java.io.File
import java.security.MessageDigest

/** Explicit public-clip/live-provider evidence on an isolated debug emulator only. */
class DirectLiveRunner : Instrumentation() {
    override fun onCreate(arguments: Bundle?) {
        super.onCreate(arguments)
        Thread {
            val result=Bundle()
            try {
                check(targetContext.packageName=="com.metrix.writer.debug")
                check(android.os.Build.HARDWARE.contains("ranchu") || android.os.Build.HARDWARE.contains("goldfish"))
                val input=File(targetContext.filesDir,"direct-live-key.private")
                val provider = arguments?.getString("provider") ?: "gemini"
                val spec = CloudProviders.get(provider)
                val model = spec.speech.first().id
                val account=DirectCloud(targetContext, provider)
                try { account.saveKey(input.readText().trim()) } finally { input.delete() }
                account.saveSelection(model,"en")
                DictationChoice(targetContext).saveLanguage("en")
                CompanionPreferences(targetContext).save(provider + "/" + spec.cleanup.first().id,"raw")
                val manager=SessionManager.get(targetContext)
                val store=manager.store
                val source=File(targetContext.filesDir,"direct-live.wav")
                val item=store.create("en");source.copyTo(store.audio(item),overwrite=true);store.finishCapture(item,null)
                account.prepare(store,item)
                val started=android.os.SystemClock.elapsedRealtime()
                val done=java.util.concurrent.CountDownLatch(1)
                manager.runJob(item.getString("id"), Runnable { done.countDown() })
                check(done.await(180,java.util.concurrent.TimeUnit.SECONDS)) { "Job manager timed out" }
                val final=store.load(item.getString("id"))
                check(!final.has("pending")) { final.optString("pending_cloud_error","Request stayed pending") }
                check(!final.has("pending_postprocess")) { "Direct cleanup stayed pending" }
                val job=final.getJSONArray("attempts").getJSONObject(0)
                check(job.getString("transport")=="direct_$provider" && job.getString("cleaned_text").isNotBlank())
                val raw=CompanionProcessing.raw(job)
                val receipt=JSONObject().put("passed",true).put("session",item.getString("id"))
                    .put("provider",provider).put("model",model).put("job_manager_dispatch",true).put("direct_https",true).put("writer_server_used",false)
                    .put("elapsed_ms",android.os.SystemClock.elapsedRealtime()-started)
                    .put("audio_sha256",MessageDigest.getInstance("SHA-256").digest(store.audio(item).readBytes()).joinToString(""){"%02x".format(it)})
                    .put("raw_characters",raw.length).put("raw_and_cleaned_retained",true)
                File(targetContext.filesDir,"direct-live-receipt.json").writeText(receipt.toString())
                result.putString("result","PASS")
                finish(0,result)
            } catch (error:Exception) {
                result.putString("result","FAIL: "+GeminiClient.explain(error));finish(1,result)
            }
        }.start()
    }
}
