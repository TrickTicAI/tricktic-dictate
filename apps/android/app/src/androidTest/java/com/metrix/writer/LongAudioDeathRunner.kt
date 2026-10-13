@file:Suppress("DEPRECATION")
package com.metrix.writer

import android.app.Instrumentation
import android.app.Activity
import android.os.Bundle
import org.json.JSONObject
import java.io.File
import java.io.RandomAccessFile

/** Run seed and recovery separately: seed deliberately kills only this test APK's process. */
class LongAudioDeathRunner : Instrumentation() {
    private lateinit var arguments: Bundle
    private val target get() = targetContext
    override fun onCreate(args: Bundle?) { super.onCreate(args); arguments = args ?: Bundle(); start() }
    override fun onStart() {
        try {
            check(BuildConfig.PREPARATION_PROBE_ENABLED) { "Build explicit debug preparation probe" }
            when (arguments.getString("phase")) {
                "seed" -> seedAndDieWithLiveMapping()
                "recover" -> recoverAfterProcessDeathAndRetry()
                else -> error("Use explicit seed/recover phase")
            }
            finish(Activity.RESULT_OK, Bundle().apply { putString("stream", "LONG_AUDIO_RECOVERY_PASSED") })
        } catch (error: Throwable) {
            finish(Activity.RESULT_CANCELED, Bundle().apply { putString("stream", error.stackTraceToString()) })
        }
    }
    private fun equal(expected: Any?, actual: Any?) { check(expected == actual) { "Recovery mismatch" } }
    private fun seedAndDieWithLiveMapping() {
        val root = File(target.filesDir, "mw114-death-recordings")
        check(!root.exists()) { "Death fixture already exists; run recovery first" }
        val store = SessionStore(root)
        val item = store.create("mixed"); val audio = store.audio(item)
        RandomAccessFile(audio, "rw").use { out ->
            val bytes = 120 * 32000L; SessionStore.header(out, bytes)
            val buffer = ByteArray(65536) { 17 }; var left = bytes
            while (left > 0) { val count = minOf(left, buffer.size.toLong()).toInt(); out.write(buffer, 0, count); left -= count }
        }
        store.finishCapture(item, null)
        item.getJSONArray("attempts").put(JSONObject().put("id", "prior").put("raw_text", "prior transcript")); store.save(item)
        store.prepareDirect(item, CompanionPreferences(target).options("groq", "whisper-large-v3-turbo", "mixed"), JSONObject().put("provider", "groq").put("transport_version", 1).put("credential_id", java.util.UUID.randomUUID()), "raw")
        File(target.filesDir, "mw114-death.json").writeText(JSONObject().put("id", item.getString("id"))
            .put("sha256", PhoneModels.hash(audio)).put("pending", item.getJSONObject("pending")).put("attempts", item.getJSONArray("attempts")).toString())
        PreparationProbe().run(audio.path, 0, PreparationCheckpoint { _, mapped ->
            check(mapped > 0)
            File(target.filesDir, "mw114-death-live-mapping.flag").writeText("mapped")
            android.os.Process.killProcess(android.os.Process.myPid())
        })
        error("Death fixture returned")
    }
    private fun recoverAfterProcessDeathAndRetry() {
        val proof = JSONObject(File(target.filesDir, "mw114-death.json").readText())
        check(File(target.filesDir, "mw114-death-live-mapping.flag").exists())
        val store = SessionStore(File(target.filesDir, "mw114-death-recordings"))
        val item = store.restore().single(); val audio = store.audio(item)
        equal(proof.getString("id"), item.getString("id"))
        equal(proof.getString("sha256"), PhoneModels.hash(audio))
        equal(proof.getJSONObject("pending").toString(), item.getJSONObject("pending").toString())
        equal(proof.getJSONArray("attempts").toString(), item.getJSONArray("attempts").toString())
        equal(120 * 16000L, PreparationProbe().run(audio.path, 2, PreparationCheckpoint { _, _ -> }))
        check(store.directory.list()!!.none { it.contains("writer-pcm") })
        store.directory.deleteRecursively()
        File(target.filesDir, "mw114-death.json").delete(); File(target.filesDir, "mw114-death-live-mapping.flag").delete()
    }
}
