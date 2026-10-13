@file:Suppress("DEPRECATION")
package com.metrix.writer

import android.app.Instrumentation
import android.app.UiAutomation
import android.os.*
import android.view.accessibility.AccessibilityNodeInfo
import org.json.JSONArray
import org.json.JSONObject
import java.io.File

/** Real model + real bubble/JobScheduler/editor. Speech injection exists only in this test APK. */
class PhoneDictationRunner : Instrumentation() {
    private lateinit var ui: UiAutomation
    private lateinit var manager: SessionManager
    private lateinit var config: JSONObject
    private lateinit var privateDir: File
    private var phase = "run"
    private var step = "setup"
    private val receipts = JSONArray()
    override fun onCreate(arguments: Bundle?) { super.onCreate(arguments); phase = arguments?.getString("phase") ?: "run"; start() }
    private fun shell(command: String) = ParcelFileDescriptor.AutoCloseInputStream(ui.executeShellCommand(command)).bufferedReader().use { it.readText() }
    private fun pause(ms: Long = 300) = SystemClock.sleep(ms)
    private fun nodes(): List<AccessibilityNodeInfo> {
        ui.clearCache(); val result = mutableListOf<AccessibilityNodeInfo>()
        fun walk(n: AccessibilityNodeInfo) { result.add(n); for (i in 0 until n.childCount) n.getChild(i)?.let { walk(it) } }
        ui.windows.forEach { it.root?.let { root -> walk(root) } }; return result
    }
    private fun find(predicate: (AccessibilityNodeInfo) -> Boolean): AccessibilityNodeInfo {
        repeat(100) { nodes().firstOrNull(predicate)?.let { return it }; pause(100) }; error("UI element unavailable at $step; " + shell("dumpsys activity service ${targetContext.packageName}/com.metrix.writer.WriterInputService"))
    }
    private fun click(node: AccessibilityNodeInfo) { var n = node; while (!n.isClickable && n.parent != null) n = n.parent; check(n.performAction(AccessibilityNodeInfo.ACTION_CLICK)); pause() }
    private fun main(block: () -> Unit) { var error: Throwable? = null; runOnMainSync { try { block() } catch (e: Throwable) { error = e } }; error?.let { throw it } }
    private fun offline() {
        check(shell("settings get global airplane_mode_on").trim() == "1")
        check(shell("settings get global wifi_on").trim() == "0")
        check(targetContext.getSystemService(android.net.ConnectivityManager::class.java).activeNetwork == null) { "Active network found" }
    }
    private fun editor(): AccessibilityNodeInfo {
        shell("am force-stop com.metrix.writer.editor"); shell("am start -W -n com.metrix.writer.editor/.EditorActivity"); pause(800)
        val node = find { it.contentDescription?.toString() == "Native editor 0" }; click(node); pause(800); return node
    }
    /** Leaves the editor while recording, then returns before Stop; returns the field focused at Stop. */
    private fun capture(clip: JSONObject, beforeStop: (AccessibilityNodeInfo) -> AccessibilityNodeInfo = { it }): Pair<String, AccessibilityNodeInfo> {
        offline(); DictationChoice(targetContext).save(true, clip.getString("model"), clip.getString("mode"))
        val node = editor(); val pcm = File(privateDir, clip.getString("file")).readBytes().drop(44).toByteArray()
        AudioCapture::class.java.getDeclaredField("sources").apply { isAccessible = true }.set(manager.capture, AudioCapture.SourceFactory {
            object : AudioCapture.Source {
                var offset = 0
                override fun start() {}
                override fun read(bytes: ByteArray, length: Int): Int { val n = minOf(length, pcm.size-offset); pcm.copyInto(bytes,0,offset,offset+n); offset+=n; return n }
                override fun capacity() = 6400
                override fun silenced() = false
                override fun route() = 1
                override fun close() {}
            }
        })
        val before = manager.store.restore().map { it.getString("id") }.toSet()
        click(find { it.packageName?.toString() == targetContext.packageName && it.contentDescription?.toString() == "TrickTic Dictate microphone" })
        find { it.packageName?.toString() == targetContext.packageName && it.contentDescription?.toString() == "TrickTic Dictate stop recording" && it.isEnabled }
        val id = manager.store.restore().single { it.getString("id") !in before }.getString("id")
        val deadline = SystemClock.elapsedRealtime() + 10000
        while (manager.store.audio(manager.store.load(id)).length() < pcm.size + 44 && SystemClock.elapsedRealtime() < deadline) pause(20)
        check(manager.store.load(id).has("original_target"))
        // Future engine/language changes must not mutate this recording's phone choice.
        DictationChoice(targetContext).save(false, "base", "en")
        val target = beforeStop(node)
        click(find { it.packageName?.toString() == targetContext.packageName && it.contentDescription?.toString() == "TrickTic Dictate stop recording" && it.isEnabled })
        check(pcm.contentEquals(manager.store.audio(manager.store.load(id)).readBytes().drop(44).toByteArray()))
        return id to target
    }
    private fun resolved(id: String): JSONObject {
        val deadline = SystemClock.elapsedRealtime() + 240000
        while (SystemClock.elapsedRealtime() < deadline) {
            offline(); val item = manager.store.load(id)
            if (!item.has("pending") && item.getJSONArray("attempts").length() > 0 && !manager.isBusy()) return item
            pause(200)
        }; error("Phone result timeout")
    }
    private fun last(item: JSONObject) = item.getJSONArray("attempts").let { it.getJSONObject(it.length()-1) }
    private fun record(id: String, node: AccessibilityNodeInfo, delivered: Boolean, expected: (String) -> String = { "English before " + it + "| after" }) {
        val item = resolved(id); pause(1600); val job = last(item)
        check(job.getJSONObject("status").getString("state") == "succeeded")
        val raw = CompanionProcessing.raw(job); check(raw.isNotBlank()); check(job.getString("execution_location") == "phone")
        check(job.has("deferred_cleanup") && !job.has("server_origin") && !item.has("pending_postprocess"))
        // Live polish ran and fell back (no key in this offline run): insertion says so.
        check(item.getString("last_status").contains("corrected original"))
        val text = CompanionProcessing.deliveryText(job); check(node.refresh())
        val ledger = DeliveryStore(File(targetContext.filesDir, "deliveries")).load(item.getString("original_target"))
        if (delivered) { check(ledger.getString("state") == "delivered"); check(node.text.toString() == expected(text)) { "Field text differs from the expected insertion" } }
        else check(ledger.getString("state") != "delivered")
        receipts.put(JSONObject().put("session_id", id).put("job", job).put("field_text", node.text.toString())
            .put("exact_delivery", delivered).put("audio_sha256", PhoneModels.hash(manager.store.audio(item))))
    }
    override fun onStart() {
        try {
            privateDir = File(targetContext.noBackupFilesDir, "phone-proof"); config = JSONObject(File(privateDir, "config.json").readText())
            File(privateDir,"$phase-failure.json").delete(); File(privateDir,"$phase-results.json").delete()
            ui = getUiAutomation(UiAutomation.FLAG_DONT_SUPPRESS_ACCESSIBILITY_SERVICES)
            ui.serviceInfo = ui.serviceInfo.apply { flags = flags or 64 or 16 }
            if (phase == "transfer") {
                check(shell("getprop ro.kernel.qemu").trim() == "1")
                step = "wait for emulator network"
                val network = targetContext.getSystemService(android.net.ConnectivityManager::class.java)
                repeat(200) { if (network.activeNetwork == null) pause(100) }
                check(network.activeNetwork != null)
                step = "real HTTPS download pause and resume"
                val models = PhoneModels(targetContext); models.delete("base")
                var paused = false
                try { models.download("base", { paused }, { bytes, _ -> if (bytes >= 2 * 1024 * 1024) paused = true }) }
                catch (_: java.io.InterruptedIOException) { }
                val partial = models.partial("base").length()
                check(partial in 1 until models.model("base").getLong("bytes")); check(!models.file("base").exists())
                models.download("base", { false }, { _, _ -> }); models.verify("base")
                step = "bad import keeps installed model"
                val hash = PhoneModels.hash(models.file("base"))
                try { java.io.ByteArrayInputStream(byteArrayOf(1,2,3)).use { models.import("base",it,{false},{_,_->}) }; error("Bad import accepted") }
                catch (_: IllegalStateException) { }
                check(PhoneModels.hash(models.file("base")) == hash)
                File(privateDir,"transfer-results.json").writeText(JSONObject().put("passed",true).put("resumed_from_bytes",partial).put("sha256",hash).toString())
                finish(0,Bundle().apply { putString("stream","PHONE_TRANSFER_PASSED") }); return
            }
            offline(); manager = SessionManager.get(targetContext)
            if (shell("getprop ro.kernel.qemu").trim() != "1") check(config.optBoolean("physical_authorized"))
            val models = PhoneModels(targetContext)
            val clips = config.getJSONArray("clips")
            if (phase == "run") {
                step = "import pinned models offline"
                for (i in 0 until config.getJSONArray("models").length()) {
                    val model = config.getJSONArray("models").getJSONObject(i)
                    val modelId = model.getString("id")
                    // Exercise interrupted document streaming before completing the same pinned import.
                    models.partial(modelId).delete()
                    var pauseImport = false
                    try { File(privateDir, model.getString("file")).inputStream().use { models.import(modelId, it, { pauseImport }, { bytes, _ -> if (bytes >= 2 * 1024 * 1024) pauseImport = true }) } }
                    catch (_: java.io.InterruptedIOException) { }
                    check(models.partial(modelId).length() in 1 until models.model(modelId).getLong("bytes"))
                    File(privateDir, model.getString("file")).inputStream().use { models.import(modelId, it, { false }, { _, _ -> }) }
                    models.verify(modelId)
                }
            }
            step = "editor service"
            AccessibilityConsent(targetContext).record(true)
            val component = targetContext.packageName + "/com.metrix.writer.WriterInputService"
            val enabled = shell("settings get secure enabled_accessibility_services").trim().split(':').filter { it != "null" && it.isNotEmpty() }
            val others = enabled.filter { it != component }.joinToString(":")
            shell(if (others.isEmpty()) "settings delete secure enabled_accessibility_services" else "settings put secure enabled_accessibility_services $others")
            pause()
            shell("settings put secure enabled_accessibility_services " + (enabled + component).distinct().joinToString(":"))
            repeat(100) { if (WriterInputService.instance == null) pause(100) }
            check(WriterInputService.instance != null)
            CompanionPreferences(targetContext).also { it.save("gemini/gemini-3.8-flash", "cleaned"); it.saveDocument(JSONObject("""{"hints":[{"text":"Metrix"}],"corrections":[{"from":"country","to":"country 🌍"}],"snippets":[]}""")) }
            if (phase == "resume") {
                step = "explicit process recovery"
                val interrupted = JSONObject(File(privateDir, "interrupted.json").readText()); val id = interrupted.getString("id")
                val before = manager.store.load(id); check(before.getJSONObject("pending").toString() == interrupted.getString("pending"))
                check(PhoneModels.hash(manager.store.audio(before)) == interrupted.getString("audio_sha256"))
                val node = editor(); main { manager.submit(id, 0) }; record(id, node, false)
                check(last(manager.store.load(id)).getJSONObject("request").toString() == interrupted.getString("pending"))
                check(node.text.toString() == "English before | after")
            } else {
                step = "model management screen and import picker"
                main { targetContext.startActivity(android.content.Intent(targetContext, PhoneModelsActivity::class.java).addFlags(android.content.Intent.FLAG_ACTIVITY_NEW_TASK)) }
                find { it.text?.toString()?.startsWith("Models run on this phone") == true }
                repeat(8) {
                    if (nodes().none { it.text?.toString() == "Verify file" }) {
                        nodes().firstOrNull { it.isScrollable }?.performAction(AccessibilityNodeInfo.ACTION_SCROLL_FORWARD); pause()
                    }
                }
                click(find { it.text?.toString()?.equals("Verify file", true) == true })
                find { it.text?.toString()?.startsWith("SHA-256 verified") == true }
                repeat(8) { nodes().firstOrNull { it.isScrollable }?.performAction(AccessibilityNodeInfo.ACTION_SCROLL_BACKWARD); pause(80) }
                click(find { it.text?.toString()?.startsWith("Import file") == true })
                find { it.packageName?.toString() == "com.google.android.documentsui" || it.packageName?.toString() == "com.android.documentsui" }
                shell("input keyevent 4")
                step = "pending cloud queue cannot block phone inference"
                val cloud = manager.store.create("en"); manager.store.finishCapture(cloud, null)
                manager.store.prepare(cloud, "https://offline.invalid", "gemini", "unconfigured", "en")
                main { manager.submit(cloud.getString("id"), 0) }
                DictationChoice(targetContext).save(true, clips.getJSONObject(0).getString("model"), clips.getJSONObject(0).getString("mode"))
                check(!manager.isBusy())
                for (i in 0 until clips.length()) { step = "real ${clips.getJSONObject(i).getString("mode")} bubble inference"; val (id,node) = capture(clips.getJSONObject(i)); record(id,node,true) }
                check(manager.store.load(cloud.getString("id")).getJSONObject("pending").toString() == cloud.getJSONObject("pending").toString())
                check(manager.store.load(cloud.getString("id")).getJSONArray("attempts").length() == 0)
                val clip = clips.getJSONObject(0)
                // Insert Anywhere (MW-72): leave for another app while recording, return, tap the field, Stop.
                step = "away and back to the same field"
                val (away,awayNode) = capture(clip) { _ ->
                    shell("am start -W -a android.settings.SETTINGS"); pause(1200)
                    shell("am start -W -n com.metrix.writer.editor/.EditorActivity"); pause(800)
                    find { it.contentDescription?.toString() == "Native editor 0" }.also { click(it); pause(800) }
                }
                record(away,awayNode,true)
                step = "field recreated between start and stop"
                val (recreated,recreatedNode) = capture(clip) { _ -> editor() }
                record(recreated,recreatedNode,true)
                step = "user kept typing before the result"
                val (stale,node) = capture(clip)
                check(node.performAction(AccessibilityNodeInfo.ACTION_SET_TEXT, Bundle().apply { putCharSequence(AccessibilityNodeInfo.ACTION_ARGUMENT_SET_TEXT_CHARSEQUENCE,"User kept typing") }))
                record(stale,node,true) { "User kept typing$it" }
                val original = manager.store.load(stale); val audioHash = PhoneModels.hash(manager.store.audio(original)); val prior = original.getJSONArray("attempts").getJSONObject(0).toString()
                fun retry(): String {
                    DictationChoice(targetContext).save(true, clip.getString("model"), clip.getString("mode"))
                    val fresh = manager.store.load(stale); DictationChoice(targetContext).prepare(manager.store,fresh); main { manager.submit(stale,0) }; return stale
                }
                step = "missing model"
                val model = clip.getString("model"); val installed = models.file(model); val parked = File(models.directory,"$model.held")
                check(installed.renameTo(parked))
                try { check(last(resolved(retry())).getJSONObject("status").getString("state") == "failed") } finally { check(parked.renameTo(installed)) }
                step = "corrupt model"
                java.io.RandomAccessFile(installed,"rw").use { file -> val b=file.read(); file.seek(0); file.write(b xor 255)
                    try { check(last(resolved(retry())).getJSONObject("status").getString("state") == "failed") } finally { file.seek(0); file.write(b) } }
                step = "cancel saved inference"
                retry(); pause(300); main { manager.cancelWork() }
                resolved(stale); check(last(manager.store.load(stale)).getJSONObject("status").getString("state") == "failed")
                check(audioHash == PhoneModels.hash(manager.store.audio(manager.store.load(stale))))
                check(prior == manager.store.load(stale).getJSONArray("attempts").getJSONObject(0).toString())
                step = "native worker death keeps main process and saved attempts"
                retry(); pause(1000)
                val pid = shell("pidof ${targetContext.packageName}:phone_inference").trim()
                check(pid.matches(Regex("[0-9]+")))
                shell("run-as ${targetContext.packageName} kill -9 $pid")
                check(last(resolved(stale)).getJSONObject("status").getString("state") == "failed")
                check(audioHash == PhoneModels.hash(manager.store.audio(manager.store.load(stale))))
                step = "model deletion keeps audio"
                models.delete(model); check(audioHash == PhoneModels.hash(manager.store.audio(manager.store.load(stale))))
                val asset = (0 until config.getJSONArray("models").length()).map { config.getJSONArray("models").getJSONObject(it) }.first { it.getString("id")==model }
                File(privateDir,asset.getString("file")).inputStream().use { models.import(model,it,{false},{_,_->}) }
                step = "interrupt actual worker"
                val (id,_) = capture(clip)
                val deadline = SystemClock.elapsedRealtime()+10000
                while (shell("pidof ${targetContext.packageName}:phone_inference").trim().isEmpty() && SystemClock.elapsedRealtime()<deadline) pause(50)
                val pending = manager.store.load(id)
                check(pending.has("pending"))
                File(privateDir,"interrupted.json").writeText(JSONObject().put("id",id).put("pending",pending.getJSONObject("pending").toString()).put("audio_sha256",PhoneModels.hash(manager.store.audio(pending))).toString())
            }
            File(privateDir,"$phase-results.json").writeText(JSONObject().put("passed",true).put("phase",phase).put("receipts",receipts).toString())
            finish(0, Bundle().apply { putString("stream", "PHONE_${phase.uppercase()}_PASSED") })
        } catch (error: Throwable) {
            File(privateDir,"$phase-failure.json").writeText(JSONObject().put("step",step).put("error",error.toString()).put("stack",error.stackTraceToString()).toString())
            finish(1,Bundle().apply { putString("stream", "PHONE_FAILED at $step: ${error.javaClass.simpleName}") })
        }
    }
}
