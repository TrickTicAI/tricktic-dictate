@file:Suppress("DEPRECATION")
package com.metrix.writer

import android.app.Activity
import android.app.Instrumentation
import android.app.UiAutomation
import android.content.Intent
import android.os.Bundle
import android.os.SystemClock
import android.os.ParcelFileDescriptor
import android.view.View
import android.view.ViewGroup
import android.view.accessibility.AccessibilityNodeInfo
import android.widget.Button
import android.widget.Spinner
import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.net.HttpURLConnection
import java.net.URL

/** Screen events → durable preferences → real JobScheduler/HTTP path. Test APK only. */
class CompanionRunner : Instrumentation() {
    private lateinit var ui: UiAutomation
    private lateinit var privateDir: File
    private lateinit var config: JSONObject
    private lateinit var manager: SessionManager
    private var phase = "run"
    private var selectedSession = ""
    private var step = "setup"
        set(value) { field = value; if (::privateDir.isInitialized) File(privateDir, "step.txt").writeText(value) }
    override fun onCreate(arguments: Bundle?) { super.onCreate(arguments); phase = arguments?.getString("phase") ?: "run"; start() }
    private fun <T> main(block: () -> T): T {
        var value: T? = null; var failure: Throwable? = null
        runOnMainSync { try { value = block() } catch (error: Throwable) { failure = error } }; failure?.let { throw it }
        @Suppress("UNCHECKED_CAST") return value as T
    }
    private fun views(view: View): List<View> = listOf(view) + if (view is ViewGroup) (0 until view.childCount).flatMap { views(view.getChildAt(it)) } else emptyList()
    private fun launch(type: Class<out Activity>): Activity = startActivitySync(Intent(targetContext, type).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)).also { activity ->
        waitForIdleSync(); SystemClock.sleep(300)
        if (type == HistoryActivity::class.java && selectedSession.isNotEmpty()) {
            main { (views(activity.window.decorView).first { it is Button && it.tag == selectedSession } as Button).performClick() }
            waitForIdleSync()
        }
    }
    private fun button(activity: Activity, name: String) { main { (views(activity.window.decorView).first { it is Button && it.text.toString() == name } as Button).performClick() }; waitForIdleSync(); SystemClock.sleep(200) }
    private fun nodes(): List<AccessibilityNodeInfo> {
        ui.clearCache()
        fun walk(node: AccessibilityNodeInfo): List<AccessibilityNodeInfo> = listOf(node) + (0 until node.childCount).flatMap { node.getChild(it)?.let(::walk) ?: emptyList() }
        return ui.rootInActiveWindow?.let(::walk) ?: emptyList()
    }
    private fun shell(command: String): ByteArray = ParcelFileDescriptor.AutoCloseInputStream(ui.executeShellCommand(command)).use { it.readBytes() }
    private fun fill(description: String, text: String) {
        val node = nodes().first { it.contentDescription?.toString() == description }
        check(node.performAction(AccessibilityNodeInfo.ACTION_SET_TEXT, Bundle().apply { putCharSequence(AccessibilityNodeInfo.ACTION_ARGUMENT_SET_TEXT_CHARSEQUENCE, text) }))
    }
    private fun dialogSave() { check(nodes().first { it.text?.toString()?.equals("Save", true) == true && it.isClickable }.performAction(AccessibilityNodeInfo.ACTION_CLICK)); SystemClock.sleep(300) }
    private fun dictionary(trigger: String) {
        val activity = launch(DictionaryActivity::class.java)
        button(activity, "Add hint"); fill("Word or name", "Metrix עברית"); dialogSave()
        button(activity, "Add correction"); fill("Spoken text to correct", "English"); fill("Replacement spelling", "Metrix"); dialogSave()
        button(activity, "Add snippet"); fill("Spoken trigger", trigger); fill("Exact inserted text", "חתימה 🌍\nAlex"); dialogSave()
        main { activity.finish() }
    }
    private fun resolved(id: String): JSONObject {
        val deadline = SystemClock.elapsedRealtime() + 180000
        while (SystemClock.elapsedRealtime() < deadline) {
            val item = manager.store.load(id)
            if (!manager.isBusy() && !item.has("pending") && !item.has("pending_postprocess") && item.getJSONArray("attempts").length() > 0) return item
            SystemClock.sleep(100)
        }; error("Request did not resolve")
    }
    private fun control(mode: String) {
        val c = URL(config.getString("control")).openConnection() as HttpURLConnection
        try { c.requestMethod = "POST"; c.doOutput = true; c.outputStream.use { it.write(JSONObject().put("mode", mode).toString().toByteArray()) }; check(c.responseCode == 200) } finally { c.disconnect() }
    }
    override fun onStart() {
        try {
            privateDir = File(targetContext.noBackupFilesDir, "cloud-proof")
            config = JSONObject(File(privateDir, "config.json").readText()); ui = getUiAutomation()
            manager = SessionManager.get(targetContext)
            val mobile = MobileConnection(targetContext); val prefs = CompanionPreferences(targetContext)
            mobile.check(config.getString("origin"), config.getString("token"))
            if (phase == "resume") {
                step = "history after process recreation"
                val saved = JSONObject(File(privateDir, "companion.json").readText()); val item = manager.store.load(saved.getString("id")); selectedSession = item.getString("id")
                check(item.getJSONArray("attempts").toString() == saved.getString("attempts"))
                check(prefs.document().toString() == saved.getString("dictionary")); check(prefs.delivery() == "cleaned")
                check(mobile.language() == "mixed")
                val history = launch(HistoryActivity::class.java)
                check(main { views(history.window.decorView).any { it.contentDescription == "Original text" } })
                main { history.window.decorView.layoutDirection = View.LAYOUT_DIRECTION_RTL }
                waitForIdleSync()
                check(main { views(history.window.decorView).filterIsInstance<Button>().all { it.width > 0 && it.width <= history.window.decorView.width } })
                ui.takeScreenshot()?.let { bitmap -> File(privateDir, "history-rtl.png").outputStream().use { bitmap.compress(android.graphics.Bitmap.CompressFormat.PNG, 100, it) } }
                targetContext.contentResolver.openInputStream(android.net.Uri.parse(saved.getString("export")))!!.use { check(it.readBytes().contentEquals(manager.store.audio(item).readBytes())) }
                if (item.has("pending_postprocess")) {
                    step = "resume immutable cleanup after process recreation"
                    control("success")
                    val before = item.getJSONObject("pending_postprocess").toString()
                    check(item.getJSONObject("pending_postprocess").getString("server") != mobile.endpoint())
                    button(history, "Resume text processing")
                    val resumed = resolved(item.getString("id"))
                    check(!resumed.has("pending_postprocess"))
                    check(CompanionProcessing.raw(resumed.getJSONArray("attempts").getJSONObject(0)) == CompanionProcessing.raw(item.getJSONArray("attempts").getJSONObject(0)))
                    check(JSONObject(before).getString("server") == config.getString("origin"))
                }
                main { history.finish() }
                File(privateDir, "resume-passed").writeText("History, settings, originals and export survived process recreation")
            } else {
                prefs.saveDocument(JSONObject("{\"hints\":[],\"corrections\":[],\"snippets\":[]}")); prefs.save("off", "raw")
                val clip = config.getJSONArray("clips").getJSONObject(0)
                mobile.select(config.getString("origin"), config.getString("selection"), "en")
                step = "speech language selected on pairing screen"
                val pairing = launch(PairingActivity::class.java)
                button(pairing, "Pair / check connection")
                repeat(100) { if (main { (views(pairing.window.decorView).first { it.contentDescription == "Speech language" } as Spinner).count > 0 }) return@repeat; SystemClock.sleep(100) }
                main {
                    val language = views(pairing.window.decorView).first { it.contentDescription == "Speech language" } as Spinner
                    language.setSelection((0 until language.count).first { language.getItemAtPosition(it).toString() == "Hebrew + English" })
                }
                button(pairing, "Use selected model"); check(mobile.language() == "mixed")
                step = "screen-selected cleanup and delivery"
                val settings = launch(CompanionSettingsActivity::class.java)
                button(settings, "Check available cleanup models")
                repeat(100) { if (main { (views(settings.window.decorView).first { it.contentDescription == "Cleanup model" } as Spinner).count > 1 }) return@repeat; SystemClock.sleep(100) }
                main {
                    (views(settings.window.decorView).first { it.contentDescription == "Cleanup model" } as Spinner).setSelection(1)
                    (views(settings.window.decorView).first { it.contentDescription == "Delivery text" } as Spinner).setSelection(2)
                }
                button(settings, "Save dictation settings"); check(prefs.cleanup() != "off"); check(prefs.delivery() == "cleaned"); main { settings.finish() }
                step = "dictionary UI saves distinct entries"
                dictionary(config.optString("snippet_trigger", "Fixture"))
                check(prefs.vocabulary().getJSONArray("hints").length() == 1); check(prefs.vocabulary().getJSONArray("snippets").length() == 1)
                val item = manager.store.create("mixed"); selectedSession = item.getString("id"); val wav = File(privateDir, clip.getString("file")).readBytes(); manager.store.audio(item).writeBytes(wav); manager.store.finishCapture(item, null)
                step = "history retry uses next-request settings through service"
                val history = launch(HistoryActivity::class.java)
                button(history, "Retry saved audio with current settings")
                val completed = resolved(item.getString("id")); val job = completed.getJSONArray("attempts").getJSONObject(0)
                val options = job.getJSONObject("request").getJSONObject("options")
                check(options.getString("input_language") == "mixed"); check(options.getJSONObject("vocabulary").toString() == prefs.vocabulary().toString())
                check(CompanionProcessing.raw(job).isNotBlank()); check(job.has("dictionary_text")); check(job.has("cleaned_text"))
                check(job.getString("dictionary_text").contains("חתימה 🌍\nAlex")); check(job.getString("cleaned_text").contains("חתימה 🌍\nAlex"))
                check(manager.store.audio(completed).readBytes().contentEquals(wav))
                val raw = CompanionProcessing.raw(job); val cleaned = job.getString("cleaned_text")
                if (!config.optBoolean("live")) {
                    step = "cleanup failure keeps previous outputs and raw success"
                    control("cleanup-failed"); main { history.finish() }; SystemClock.sleep(500)
                    val reopened = launch(HistoryActivity::class.java)
                    button(reopened, "Clean up attempt 1")
                    val failed = resolved(item.getString("id")); val same = failed.getJSONArray("attempts").getJSONObject(0)
                    check(CompanionProcessing.raw(same) == raw); check(same.getString("cleaned_text") == cleaned)
                    check(same.getJSONArray("processing_attempts").getJSONObject(1).getJSONObject("cleanup").getString("state") == "failed")
                    main { reopened.finish() }
                }
                step = "export and restart checkpoint"
                if (!config.optBoolean("live")) {
                    step = "uncertain cleanup checkpoint retains original selection"
                    control("cleanup-drop")
                    main { history.finish() }
                    val retryScreen = launch(HistoryActivity::class.java)
                    button(retryScreen, "Clean up attempt 1")
                    val deadline = SystemClock.elapsedRealtime() + 45000
                    while (manager.isBusy() && SystemClock.elapsedRealtime() < deadline) SystemClock.sleep(100)
                    check(manager.store.load(item.getString("id")).has("pending_postprocess"))
                    prefs.save("off", "cleaned"); mobile.select("https://changed.invalid", "gemini/changed", "mixed")
                    main { retryScreen.finish() }
                }
                val saved = manager.store.load(item.getString("id"))
                main { history.finish() }
                val exportScreen = launch(HistoryActivity::class.java)
                button(exportScreen, "Copy Original")
                check(main { targetContext.getSystemService(android.content.ClipboardManager::class.java).primaryClip!!.getItemAt(0).text.toString() } == raw)
                button(exportScreen, "Export audio")
                var save: AccessibilityNodeInfo? = null
                val deadline = SystemClock.elapsedRealtime() + 10000
                while (save == null && SystemClock.elapsedRealtime() < deadline) { save = nodes().firstOrNull { it.text?.toString()?.equals("Save", true) == true && it.isClickable }; SystemClock.sleep(100) }
                check(save != null) { "System export picker not shown" }
                check(save!!.performAction(AccessibilityNodeInfo.ACTION_CLICK))
                SystemClock.sleep(1000)
                val exported = shell("cat /sdcard/Download/Writer-${item.getString("id")}.wav")
                check(exported.contentEquals(wav)) { "System document export differs from saved audio" }
                main { exportScreen.finish() }
                val uri = AudioExportProvider.snapshot(targetContext, manager.store, saved.getString("id"))
                File(privateDir, "companion.json").writeText(JSONObject().put("id", item.getString("id")).put("attempts", saved.getJSONArray("attempts").toString()).put("dictionary", prefs.document().toString()).put("export", uri.toString()).toString())
                File(privateDir, "receipt.json").writeText(JSONArray().put(JSONObject().put("mode", "mixed").put("clip", clip.getString("id")).put("session_id", item.getString("id")).put("job_id", job.getString("id")).put("timing", job.getJSONObject("client_timing")).put("delivery", JSONObject()).put("exact_delivery", false)).toString())
                main { history.finish() }
            }
            finish(Activity.RESULT_OK, Bundle().apply { putString("stream", "PASS: companion $phase; screen settings, dictionary, cleanup, history and export verified\n") })
        } catch (error: Throwable) {
            File(privateDir, "failure.txt").writeText("$step\n" + error.stackTraceToString())
            finish(Activity.RESULT_CANCELED, Bundle().apply { putString("stream", "FAIL at $step (${error.javaClass.simpleName}); inspect private receipt\n") })
        }
    }
}
