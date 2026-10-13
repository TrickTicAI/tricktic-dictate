@file:Suppress("DEPRECATION")
package com.metrix.writer

import android.app.UiAutomation
import android.content.pm.ApplicationInfo
import android.os.ParcelFileDescriptor
import android.test.InstrumentationTestCase
import org.json.JSONObject
import java.io.File

/** Separate instrumentation only; refuses phones and anything except the isolated upgrade fixture. */
class ReleaseCombinedTest : InstrumentationTestCase() {
    fun testSignedOfflineInferenceAndRetainedModel() {
        val app = instrumentation.targetContext
        val ui = instrumentation.getUiAutomation(UiAutomation.FLAG_DONT_SUPPRESS_ACCESSIBILITY_SERVICES)
        fun shell(command: String) = ParcelFileDescriptor.AutoCloseInputStream(ui.executeShellCommand(command))
        check(app.packageName == "com.metrix.writer")
        check(app.applicationInfo.flags and ApplicationInfo.FLAG_DEBUGGABLE == 0)
        check(shell("getprop ro.kernel.qemu").bufferedReader().use { it.readText().trim() } == "1")
        check(File(app.noBackupFilesDir, "upgrade-baseline.json").isFile)
        check(shell("settings get global airplane_mode_on").bufferedReader().use { it.readText().trim() } == "1")
        val network = app.getSystemService(android.net.ConnectivityManager::class.java)
        repeat(100) { if (network.activeNetwork != null) android.os.SystemClock.sleep(100) }
        check(network.activeNetwork == null)
        // Upgrade already proved decryption. Only this known fake account may be removed
        // so the ordinary credential/engine-switch fixtures can run afterwards.
        val cloud = DirectCloud(app)
        if (cloud.configured()) {
            check(cloud.key(cloud.snapshot()) == "writer-upgrade-fixture-key-not-a-real-secret-0001")
            cloud.forget()
        }
        val models = PhoneModels(app)
        val receipt = File(app.noBackupFilesDir, "combined-release.json")
        val store = SessionStore(File(app.filesDir, "recordings"))
        if (!models.file("base").exists()) {
            shell("cat /data/local/tmp/writer-release-proof/base.bin").use {
                models.import("base", it, { false }, { _, _ -> })
            }
        }
        models.verify("base")
        val item = store.create("en")
        shell("cat /data/local/tmp/writer-release-proof/speech.wav").use { input ->
            store.audio(item).outputStream().use { input.copyTo(it) }
        }
        store.finishCapture(item, null)
        val audioHash = PhoneModels.hash(store.audio(item))
        val options = CompanionPreferences(app).options("phone", "base", "en")
        options.getJSONObject("cleanup").put("mode", "gemini/gemini-3.8-flash")
        store.preparePhone(item, options, models.snapshot("base", "en"), "raw")
        PhoneEngine(app, store).execute(item, "phone")
        val completed = store.load(item.getString("id"))
        val job = completed.getJSONArray("attempts").getJSONObject(0)
        check(job.getJSONObject("status").getString("state") == "succeeded")
        check(CompanionProcessing.raw(job).contains("country", ignoreCase = true))
        check(job.getString("execution_location") == "phone")
        check(job.has("deferred_cleanup") && !completed.has("pending_postprocess"))
        check(!job.has("server_origin") && !completed.has("pending_cloud"))
        check(PhoneModels.hash(store.audio(completed)) == audioHash)
        // A later unavailable runtime must preserve the actual successful original and all audio.
        val original = job.toString()
        store.preparePhone(completed, options, models.snapshot("base", "en").put("runtime_revision", "unavailable"), "raw")
        PhoneEngine(app, store).execute(completed, "phone")
        val failed = store.load(item.getString("id"))
        check(failed.getJSONArray("attempts").getJSONObject(0).toString() == original)
        check(failed.getJSONArray("attempts").getJSONObject(1).getJSONObject("status").getString("state") == "failed")
        check(PhoneModels.hash(store.audio(failed)) == audioHash)
        receipt.writeText(JSONObject().put("session", item.getString("id"))
            .put("model_sha256", PhoneModels.hash(models.verify("base")))
            .put("audio_sha256", audioHash).put("metadata_sha256", PhoneModels.hash(File(store.directory, failed.getString("id") + ".json")))
            .toString())
    }
    fun testRetainedModelAudioAndResult() {
        val app = instrumentation.targetContext
        check(app.packageName == "com.metrix.writer")
        check(app.applicationInfo.flags and ApplicationInfo.FLAG_DEBUGGABLE == 0)
        check(android.os.Build.HARDWARE.contains("ranchu") || android.os.Build.HARDWARE.contains("goldfish"))
        check(File(app.noBackupFilesDir, "upgrade-baseline.json").isFile)
        val saved = JSONObject(File(app.noBackupFilesDir, "combined-release.json").readText())
        val models = PhoneModels(app)
        check(PhoneModels.hash(models.verify("base")) == saved.getString("model_sha256"))
        val store = SessionStore(File(app.filesDir, "recordings"))
        val item = store.load(saved.getString("session"))
        check(PhoneModels.hash(store.audio(item)) == saved.getString("audio_sha256"))
        check(PhoneModels.hash(File(store.directory, item.getString("id") + ".json")) == saved.getString("metadata_sha256"))
    }

}
