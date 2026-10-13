@file:Suppress("DEPRECATION")
package com.metrix.writer

import android.app.Activity
import android.app.Instrumentation
import android.app.UiAutomation
import android.os.Bundle
import android.os.ParcelFileDescriptor
import android.os.SystemClock
import android.view.accessibility.AccessibilityNodeInfo
import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.net.HttpURLConnection
import java.net.URL

/** Test APK only: saved speech feeds the existing native-source boundary; no app capture backdoor. */
class CloudDictationRunner : Instrumentation() {
    private lateinit var ui: UiAutomation
    private lateinit var config: JSONObject
    private lateinit var manager: SessionManager
    private lateinit var mobile: MobileConnection
    private lateinit var privateDir: File
    private var step = "configuration"
    private var phase = "run"
    private val receipts = JSONArray()
    override fun onCreate(arguments: Bundle?) { super.onCreate(arguments); phase = arguments?.getString("phase") ?: "run"; start() }
    private fun shell(command: String) = ParcelFileDescriptor.AutoCloseInputStream(ui.executeShellCommand(command)).bufferedReader().use { it.readText() }
    private fun pause(ms: Long = 500) = SystemClock.sleep(ms)
    private fun nodes(): List<AccessibilityNodeInfo> {
        ui.clearCache(); val result = mutableListOf<AccessibilityNodeInfo>()
        fun walk(node: AccessibilityNodeInfo) { result.add(node); for (i in 0 until node.childCount) node.getChild(i)?.let { walk(it) } }
        ui.windows.forEach { it.root?.let { node -> walk(node) } }; return result
    }
    private fun find(predicate: (AccessibilityNodeInfo) -> Boolean): AccessibilityNodeInfo {
        repeat(100) { nodes().firstOrNull(predicate)?.let { return it }; pause(100) }; error("UI element unavailable")
    }
    private fun click(node: AccessibilityNodeInfo) { var target = node; while (!target.isClickable && target.parent != null) target = target.parent; check(target.performAction(AccessibilityNodeInfo.ACTION_CLICK)); pause() }
    private fun action(text: String) = click(find { it.text?.toString()?.equals(text, true) == true })
    private fun <T> main(block: () -> T): T {
        var value: T? = null; var error: Throwable? = null
        runOnMainSync { try { value = block() } catch (e: Throwable) { error = e } }; error?.let { throw it }
        @Suppress("UNCHECKED_CAST") return value as T
    }
    private fun editor(): AccessibilityNodeInfo {
        shell("am force-stop com.metrix.writer.editor")
        shell("am start -W -n com.metrix.writer.editor/.EditorActivity"); pause()
        val node = find { it.contentDescription?.toString() == "Native editor 0" }
        click(node); pause(1000); return node
    }
    private fun fixtureControl(mode: String) {
        if (config.optBoolean("live")) return
        val connection = URL(config.getString("control")).openConnection() as HttpURLConnection
        try { connection.requestMethod = "POST"; connection.doOutput = true
            connection.outputStream.use { it.write(JSONObject().put("mode", mode).toString().toByteArray()) }
            check(connection.responseCode == 200)
        } finally { connection.disconnect() }
    }
    private fun capture(clip: JSONObject): Pair<String, AccessibilityNodeInfo> {
        mobile.select(config.getString("origin"), config.getString("selection"), clip.getString("mode"))
        if (config.optBoolean("processed_delivery")) CompanionPreferences(targetContext).also {
            it.save("gemini/gemini-3.8-flash", "cleaned")
            it.saveDocument(JSONObject().put("hints", JSONArray()).put("corrections", JSONArray())
                .put("snippets", JSONArray().put(JSONObject().put("phrase", "Fixture").put("text", "חתימה 🌍\nAlex").put("enabled", true))))
        }
        val node = editor()
        val pcm = File(privateDir, clip.getString("file")).readBytes().drop(44).toByteArray()
        // Reflection is confined to this separate test APK, using the already-tested Source interface.
        val field = AudioCapture::class.java.getDeclaredField("sources").apply { isAccessible = true }
        field.set(manager.capture, AudioCapture.SourceFactory {
            object : AudioCapture.Source {
                var offset = 0
                override fun start() {}
                override fun read(bytes: ByteArray, length: Int): Int {
                    val count = minOf(length, pcm.size - offset)
                    pcm.copyInto(bytes, 0, offset, offset + count); offset += count; return count
                }
                override fun capacity() = 6400
                override fun silenced() = false
                override fun route() = 1
                override fun close() {}
            }
        })
        val before = manager.store.restore().map { it.getString("id") }.toSet()
        click(find { it.contentDescription?.toString() == "TrickTic Dictate microphone" })
        find { it.contentDescription?.toString() == "TrickTic Dictate stop recording" && it.isEnabled }
        val id = manager.store.restore().single { it.getString("id") !in before }.getString("id")
        check(manager.store.load(id).has("original_target"))
        repeat(100) { if (manager.store.audio(manager.store.load(id)).length() >= pcm.size + 44) return@repeat; pause(10) }
        // Changing future defaults while recording cannot change this session's captured choice.
        mobile.select("https://different.invalid", "gemini/changed", "mixed")
        if (config.optBoolean("processed_delivery")) CompanionPreferences(targetContext).save("off", "raw")
        click(find { it.contentDescription?.toString() == "TrickTic Dictate stop recording" && it.isEnabled })
        repeat(100) { if (manager.store.load(id).optString("capture_state") == "complete") return@repeat; pause(10) }
        check(pcm.contentEquals(manager.store.audio(manager.store.load(id)).readBytes().drop(44).toByteArray()))
        return id to node
    }
    private fun resolved(id: String): JSONObject {
        val deadline = SystemClock.elapsedRealtime() + 180000
        while (SystemClock.elapsedRealtime() < deadline) {
            val item = manager.store.load(id)
            if (!item.has("pending") && item.getJSONArray("attempts").length() > 0 && !manager.isBusy()) return item
            pause(100)
        }; error("Result did not resolve")
    }
    private fun idle() { repeat(400) { if (!manager.isBusy()) return; pause(100) }; error("Request still busy") }
    private fun record(id: String, clip: JSONObject, delivered: Boolean, node: AccessibilityNodeInfo) {
        val item = resolved(id); pause(1500)
        val jobs = item.getJSONArray("attempts"); val job = jobs.getJSONObject(jobs.length() - 1)
        check(job.getJSONObject("status").getString("state") == "succeeded")
        val raw = job.getJSONObject("status").getJSONObject("result").getJSONObject("transcript").getString("raw_text")
        check(raw.isNotBlank())
        val deliveredText = CompanionProcessing.deliveryText(job)
        if (config.optBoolean("processed_delivery")) {
            check(job.getString("delivery_choice") == "cleaned")
            check(deliveredText != raw && job.has("cleaned_text"))
        }
        val ledger = DeliveryStore(File(targetContext.filesDir, "deliveries")).load(item.getString("original_target"))
        check(node.refresh())
        if (delivered) {
            check(ledger.getString("state") == "delivered")
            check(node.text.toString() == "English before " + deliveredText + "| after")
            check(ledger.getString("text") == deliveredText)
            check(ledger.getJSONObject("timing").has("stop_to_delivery_ms"))
        } else check(ledger.getString("state") != "delivered")
        receipts.put(JSONObject().put("session_id", id).put("clip", clip.getString("id"))
            .put("mode", clip.getString("mode")).put("job_id", job.getString("id"))
            .put("timing", job.getJSONObject("client_timing")).put("delivery", ledger)
            .put("raw_text", raw).put("field_text", node.text.toString()).put("exact_delivery", delivered))
    }
    override fun onStart() {
        try {
            privateDir = File(targetContext.noBackupFilesDir, "cloud-proof")
            config = JSONObject(File(privateDir, "config.json").readText())
            ui = getUiAutomation(UiAutomation.FLAG_DONT_SUPPRESS_ACCESSIBILITY_SERVICES)
            check(shell("getprop ro.kernel.qemu").trim() == "1")
            ui.serviceInfo = ui.serviceInfo.apply { flags = flags or 64 or 16 }
            manager = SessionManager.get(targetContext); mobile = MobileConnection(targetContext)
            // This suite asserts original-text delivery regardless of earlier companion smoke defaults.
            if (phase == "run") CompanionPreferences(targetContext).also {
                it.save("off", "raw"); it.saveDocument(JSONObject("{\"hints\":[],\"corrections\":[],\"snippets\":[]}"))
            }
            step = "pair protected endpoint"
            mobile.check(config.getString("origin"), config.getString("token"))
            if (phase == "resume") {
                step = "process recreation explicit resume"
                val saved = JSONObject(File(privateDir, "pending.json").readText())
                val id = saved.getString("id"); val before = manager.store.load(id).getJSONObject("pending").toString()
                val node = editor()
                fixtureControl("success"); main { manager.submit(id, 0) }
                val item = resolved(id)
                check(item.getJSONArray("attempts").getJSONObject(0).getJSONObject("request").toString() == before)
                pause(1500); check(node.refresh()); check(node.text.toString() == "English before | after")
                check(DeliveryStore(File(targetContext.filesDir, "deliveries")).load(item.getString("original_target")).getString("state") == "recovery")
                File(privateDir, "resume-passed").writeText("Exact pending request resumed without insertion")
            } else {
                step = "enable real editor service"
                AccessibilityConsent(targetContext).record(true)
                shell("am start -W -f 0x10008000 -a android.settings.ACCESSIBILITY_SETTINGS"); pause()
                action("Writer editor delivery")
                val toggle = find { it.isCheckable }
                if (toggle.isChecked) { click(toggle); action("Turn off") }
                click(find { it.isCheckable }); action("Allow")
                repeat(100) { if (WriterInputService.instance != null) return@repeat; pause(100) }
                check(WriterInputService.instance != null)
                val clips = config.getJSONArray("clips")
                for (i in 0 until clips.length()) {
                    val clip = clips.getJSONObject(i); step = "bubble Stop / exact ${clip.getString("mode")} delivery"
                    fixtureControl("success"); val (id, node) = capture(clip); record(id, clip, true, node)
                }
                if (!config.optBoolean("live")) {
                    val clip = clips.getJSONObject(0)
                    step = "typing while processing"
                    fixtureControl("delay"); val (id, node) = capture(clip)
                    check(node.performAction(AccessibilityNodeInfo.ACTION_SET_TEXT, Bundle().apply { putCharSequence(AccessibilityNodeInfo.ACTION_ARGUMENT_SET_TEXT_CHARSEQUENCE, "User kept typing") }))
                    record(id, clip, false, node); check(node.text.toString() == "User kept typing")
                    for (fault in listOf("partial", "drop", "wrong", "server")) {
                        step = "$fault response preserves exact request"
                        fixtureControl(fault); val (pendingId, pendingNode) = capture(clip); idle()
                        val saved = manager.store.load(pendingId); check(saved.has("pending"))
                        val request = saved.getJSONObject("pending").toString(); val endpoint = saved.getString("pending_server")
                        mobile.select("https://different.invalid", "gemini/changed", "mixed")
                        fixtureControl("success"); main { manager.submit(pendingId, 0) }
                        val item = resolved(pendingId)
                        check(item.getJSONArray("attempts").getJSONObject(0).getJSONObject("request").toString() == request)
                        check(endpoint == config.getString("origin"))
                        record(pendingId, clip, false, pendingNode); check(pendingNode.text.toString() == "English before | after")
                    }
                    step = "provider failure keeps earlier raw success on explicit fresh retry"
                    val previous = manager.store.restore().first { it.getJSONArray("attempts").length() > 0 }
                    val count = previous.getJSONArray("attempts").length(); val prior = previous.getJSONArray("attempts").toString()
                    val last = previous.getJSONArray("attempts").getJSONObject(count - 1)
                    manager.store.prepare(previous, config.getString("origin"), "gemini", "fixture-cloud", "mixed")
                    check(previous.getJSONObject("pending").getString("retry_of_job_id") == last.getString("id"))
                    check(previous.getJSONObject("pending").getString("idempotency_key") != last.getJSONObject("request").getString("idempotency_key"))
                    fixtureControl("failed"); main { manager.submit(previous.getString("id"), 0) }
                    val failed = resolved(previous.getString("id")); check(failed.getJSONArray("attempts").length() == count + 1)
                    check(failed.getJSONArray("attempts").getJSONObject(count).getJSONObject("status").getString("state") == "failed")
                    check(failed.getJSONArray("attempts").getJSONObject(count - 1).toString() == JSONArray(prior).getJSONObject(count - 1).toString())
                    step = "offline capture and delayed queue cannot insert"
                    fixtureControl("success")
                    shell("svc wifi disable"); shell("svc data disable"); pause(1800)
                    try {
                        val connectivity = targetContext.getSystemService(android.net.ConnectivityManager::class.java)
                        check(connectivity.activeNetwork == null)
                        val (offlineId, offlineNode) = capture(clip)
                        check(manager.store.load(offlineId).has("pending"))
                        check(manager.store.load(offlineId).getJSONArray("attempts").length() == 0)
                        shell("svc wifi enable")
                        record(offlineId, clip, false, offlineNode)
                        check(offlineNode.text.toString() == "English before | after")
                    } finally { shell("svc wifi enable"); shell("svc data enable") }
                    step = "unsupported capability choice is refused"
                    mobile.select(config.getString("origin"), config.getString("selection"), "unsupported")
                    try { mobile.captureChoice(); error("Unsupported language accepted") } catch (_: ConnectionFailure) { }
                    step = "process death pending preparation"
                    fixtureControl("drop"); val (pendingId, _) = capture(clip); idle()
                    check(manager.store.load(pendingId).has("pending"))
                    File(privateDir, "pending.json").writeText(JSONObject().put("id", pendingId).toString())
                }
                File(privateDir, "receipt.json").writeText(receipts.toString(2))
            }
            finish(Activity.RESULT_OK, Bundle().apply { putString("stream", "PASS: cloud dictation $phase; private receipts retained\n") })
        } catch (error: Throwable) {
            File(privateDir, "failure.txt").writeText("$step\n" + error.stackTraceToString())
            finish(Activity.RESULT_CANCELED, Bundle().apply { putString("stream", "FAIL at $step (${error.javaClass.simpleName}); inspect private receipt\n") })
        }
    }
}
