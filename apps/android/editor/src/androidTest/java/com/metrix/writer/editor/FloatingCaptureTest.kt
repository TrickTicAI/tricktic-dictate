@file:Suppress("DEPRECATION", "OVERRIDE_DEPRECATION")
package com.metrix.writer.editor

import android.app.UiAutomation
import android.graphics.Rect
import android.os.ParcelFileDescriptor
import android.os.SystemClock
import android.test.InstrumentationTestCase
import android.view.accessibility.AccessibilityNodeInfo
import org.json.JSONObject

/** Runs in the independent editor APK: Writer receives NO instrumentation start privileges. */
class FloatingCaptureTest : InstrumentationTestCase() {
    private lateinit var ui: UiAutomation
    private val app = "com.metrix.writer.debug"
    override fun setUp() {
        super.setUp()
        ui = instrumentation.getUiAutomation(UiAutomation.FLAG_DONT_SUPPRESS_ACCESSIBILITY_SERVICES)
        assertEquals("Emulator only", "1", shell("getprop ro.kernel.qemu").trim())
        assertEquals("com.metrix.writer.editor", instrumentation.targetContext.packageName)
        ui.serviceInfo = ui.serviceInfo.apply { flags = flags or 64 or 16 }
        shell("am force-stop $app"); writer()
        if (name != "test00VisibleOnboardingAndDeniedStart") enableEditorService()
    }
    private fun shell(command: String) = ParcelFileDescriptor.AutoCloseInputStream(ui.executeShellCommand(command)).bufferedReader().use { it.readText() }
    private fun pause() = SystemClock.sleep(700)
    private fun nodes(): List<AccessibilityNodeInfo> {
        ui.clearCache(); val result = mutableListOf<AccessibilityNodeInfo>()
        fun walk(n: AccessibilityNodeInfo) { result.add(n); for (i in 0 until n.childCount) n.getChild(i)?.let { walk(it) } }
        ui.windows.forEach { it.root?.let { n -> walk(n) } }; return result
    }
    private fun find(predicate: (AccessibilityNodeInfo) -> Boolean): AccessibilityNodeInfo {
        repeat(60) { nodes().firstOrNull(predicate)?.let { return it }; SystemClock.sleep(150) }
        fail("Expected UI element unavailable"); throw IllegalStateException()
    }
    private fun click(n0: AccessibilityNodeInfo) {
        var n = n0; while (!n.isClickable && n.parent != null) n = n.parent
        assertTrue(n.performAction(AccessibilityNodeInfo.ACTION_CLICK)); pause()
    }
    private fun text(value: String) {
        repeat(15) {
            nodes().firstOrNull { it.text?.toString()?.replace("’", "'")?.equals(value.replace("’", "'"), true) == true || it.contentDescription?.toString() == value }?.let { click(it); return }
            nodes().firstOrNull { it.isScrollable }?.performAction(AccessibilityNodeInfo.ACTION_SCROLL_FORWARD); pause()
        }
        fail("Missing visible action: $value")
    }
    private fun writer(activity: String = "MainActivity") { shell("am start -W -n $app/com.metrix.writer.$activity"); pause() }
    private fun editor() {
        shell("am start -W -n com.metrix.writer.editor/.EditorActivity"); pause()
        click(find { it.contentDescription?.toString() == "Native editor 0" })
        // Expire Activity/background grace: a valid launch cannot depend on recently opening Writer.
        SystemClock.sleep(12000)
    }
    private fun sessions(): List<JSONObject> = shell("run-as $app ls files/recordings").lines()
        .filter { it.endsWith(".json") }.map { JSONObject(shell("run-as $app cat files/recordings/$it")) }
    private fun session(id: String) = JSONObject(shell("run-as $app cat files/recordings/$id.json"))
    private fun bytes(id: String): ByteArray = ParcelFileDescriptor.AutoCloseInputStream(
        ui.executeShellCommand("run-as $app cat files/recordings/$id.wav")).use { it.readBytes() }
    private fun start(): String {
        val before = sessions().map { it.getString("id") }.toSet()
        click(find { it.contentDescription?.toString() in setOf("TrickTic Dictate microphone", "Writer capture stopped; tap to start a new recording. Recover saved audio in Writer History.", "Writer result or request saved; open Writer to recover") })
        find { it.contentDescription?.toString() == "TrickTic Dictate stop recording" }
        val saved = sessions().single { it.getString("id") !in before }
        assertEquals("bubble", saved.getString("capture_origin"))
        assertEquals("capturing", saved.getString("capture_state"))
        assertTrue("Original editor token persisted", saved.has("original_target"))
        assertTrue("Another app retains its editor", find { it.contentDescription?.toString() == "Native editor 0" }.isFocused)
        assertTrue(shell("dumpsys activity services $app").contains("isForeground=true"))
        SystemClock.sleep(1100)
        return saved.getString("id")
    }
    private fun terminal(id: String, state: String): JSONObject {
        repeat(80) {
            val item = session(id)
            if (item.optString("capture_state") == state) return item
            SystemClock.sleep(100)
        }
        fail("Expected $state, got ${session(id).optString("capture_state")}"); throw IllegalStateException()
    }
    private fun assertAudio(id: String, prefix: ByteArray = byteArrayOf()) {
        val wav = bytes(id); val pcm = wav.drop(44).toByteArray()
        assertTrue(pcm.isNotEmpty()); assertEquals(0, pcm.size % 2)
        assertTrue(prefix.contentEquals(pcm.take(prefix.size).toByteArray()))
        val length = java.nio.ByteBuffer.wrap(wav, 40, 4).order(java.nio.ByteOrder.LITTLE_ENDIAN).int
        assertEquals(pcm.size, length)
        assertEquals(wav.size.toLong(), session(id).getLong("byte_length"))
    }
    private fun enableEditorService() {
        // Writer's service stays paused until its in-app disclosure is accepted.
        writer("FloatingSetupActivity"); text("Text insertion"); text("Accept")
        if (nodes().any { it.text?.toString() == "Cancel" }) text("Cancel")
        shell("am start -W -f 0x10008000 -a android.settings.ACCESSIBILITY_SETTINGS"); pause()
        text("TrickTic Dictate · voice insertion")
        val toggle = find { it.isCheckable }
        // Force-stop may leave a checked but disconnected accessibility service. Reconnect visibly.
        if (toggle.isChecked) { click(toggle); text("Turn off") }
        click(find { it.isCheckable }); text("Allow")
    }
    private fun micSettings(allow: Boolean) {
        // Clear the previous Settings subpage so repeated permission changes open App info.
        shell("am start -W -f 0x10008000 -a android.settings.APPLICATION_DETAILS_SETTINGS -d package:$app"); pause()
        text("Permissions"); text("Microphone")
        text(if (allow) "Allow only while using the app" else "Don't allow")
        nodes().firstOrNull { it.text?.toString()?.equals("Don't allow anyway", true) == true }?.let { click(it) }
    }
    fun test00VisibleOnboardingAndDeniedStart() {
        writer("FloatingSetupActivity")
        // Revoke through visible Android settings, independent of the app's previous grant.
        text("Microphone")
        nodes().firstOrNull { it.text?.toString() in setOf("Don’t allow", "Don't allow") }?.let { click(it) }
        micSettings(false)
        shell("am start -W -f 0x10008000 -a android.settings.action.MANAGE_OVERLAY_PERMISSION -d package:$app"); pause()
        if (nodes().any { it.text?.toString() == "TrickTic Dictate debug" }) text("TrickTic Dictate debug")
        val toggle = find { it.isCheckable }; if (!toggle.isChecked) click(toggle)
        enableEditorService()
        editor(); val before = sessions().size
        click(find { it.contentDescription?.toString() in setOf("TrickTic Dictate microphone", "Writer capture stopped; tap to start a new recording. Recover saved audio in Writer History.", "Writer result or request saved; open Writer to recover") }); pause()
        assertEquals(before, sessions().size)
        assertNotNull(find { it.contentDescription?.toString() == "TrickTic Dictate microphone" })
        micSettings(true)
        writer("FloatingSetupActivity"); text("Recording notifications")
        nodes().firstOrNull { it.text?.toString() == "Allow" }?.let { click(it) }
    }
    fun test01TapStartStopAndNotificationControls() {
        editor(); val id = start()
        val prefix = bytes(id).drop(44).toByteArray()
        click(find { it.contentDescription?.toString() == "TrickTic Dictate stop recording" })
        val saved = terminal(id, "complete"); assertAudio(id, prefix)
        pause()
        val timing = session(id).getJSONObject("startup_timing")
        val tap = timing.getLong("tap_elapsed_ms")
        val first = timing.getLong("first_audio_read_elapsed_ms")
        val durable = timing.getLong("first_audio_saved_elapsed_ms")
        val visible = timing.getLong("bubble_visible_elapsed_ms")
        assertTrue(first >= tap); assertTrue(durable >= first); assertTrue(visible >= durable)
        println("Writer emulator startup: first_audio_ms=${first-tap} saved_audio_ms=${durable-tap} bubble_visible_ms=${visible-tap}")
        assertEquals(id, saved.getJSONObject("capture_completion").getString("event_id"))
        assertFalse(saved.has("pending")); assertEquals(0, saved.getJSONArray("attempts").length())
        editor(); val second = start()
        ui.performGlobalAction(android.accessibilityservice.AccessibilityService.GLOBAL_ACTION_NOTIFICATIONS); pause()
        text("Stop and save"); terminal(second, "complete"); assertAudio(second)
        ui.performGlobalAction(android.accessibilityservice.AccessibilityService.GLOBAL_ACTION_BACK); pause()
    }
    fun test02CompanionDismissalRotationHomeAndAppSwitchKeepOneSession() {
        editor(); val id = start(); val initial = bytes(id).size
        writer(); shell("input keyevent KEYCODE_BACK"); pause()
        assertEquals("capturing", session(id).getString("capture_state"))
        writer()
        try { assertTrue(ui.setRotation(UiAutomation.ROTATION_FREEZE_90)); SystemClock.sleep(1800) }
        finally { ui.setRotation(UiAutomation.ROTATION_FREEZE_0); pause() }
        shell("input keyevent KEYCODE_HOME"); pause()
        assertEquals("capturing", session(id).getString("capture_state"))
        assertTrue(bytes(id).size > initial)
        writer(); click(find { it.contentDescription?.toString() == "TrickTic Dictate stop recording" }); terminal(id, "complete"); assertAudio(id)
        // There was one session throughout, not an Activity-owned restart.
        assertEquals(1, sessions().count { it.getString("id") == id })
    }
    fun test03LockKeepsRecordingUntilStop() {
        editor(); val id = start(); val prefix = bytes(id).drop(44).toByteArray()
        try {
            shell("input keyevent KEYCODE_SLEEP"); SystemClock.sleep(2500)
            assertEquals("capturing", session(id).getString("capture_state"))
        } finally { shell("input keyevent KEYCODE_WAKEUP"); shell("wm dismiss-keyguard"); pause() }
        // MW-103: the same session kept capturing while the screen was off.
        assertTrue(bytes(id).size - 44 > prefix.size)
        ui.performGlobalAction(android.accessibilityservice.AccessibilityService.GLOBAL_ACTION_NOTIFICATIONS); pause()
        text("Stop and save"); terminal(id, "complete"); assertAudio(id, prefix)
        assertFalse(session(id).has("capture_error"))
        ui.performGlobalAction(android.accessibilityservice.AccessibilityService.GLOBAL_ACTION_BACK); pause()
    }
    fun test04ProcessDeathRecoversExactCompleteFrames() {
        editor(); val id = start()
        shell("am force-stop $app")
        val before = bytes(id).drop(44).toByteArray()
        writer(); terminal(id, "interrupted"); assertAudio(id, before)
        assertEquals(before.size + 44, bytes(id).size)
        assertFalse(shell("dumpsys activity services $app").contains("CaptureService"))
    }
    fun test05ExplicitDiscardIsDurable() {
        editor(); val id = start()
        ui.performGlobalAction(android.accessibilityservice.AccessibilityService.GLOBAL_ACTION_NOTIFICATIONS); pause()
        text("Discard"); terminal(id, "discarded")
        assertFalse(session(id).has("capture_completion"))
        assertFalse(shell("run-as $app ls files/recordings").contains("$id.wav"))
        ui.performGlobalAction(android.accessibilityservice.AccessibilityService.GLOBAL_ACTION_BACK); pause()
        shell("am force-stop $app"); writer()
        assertEquals("discarded", session(id).getString("capture_state"))
        assertFalse(shell("run-as $app ls files/recordings").contains("$id.wav"))
    }
    fun test06PermissionRevocationRetainsAndRecovers() {
        editor(); val id = start(); val prefix = bytes(id).drop(44).toByteArray()
        micSettings(false); writer(); terminal(id, "interrupted"); assertAudio(id, prefix)
        micSettings(true)
    }
    fun test07StorageFailureRetainsSavedFrames() {
        editor(); val id = start()
        // Cause a real AtomicFile staging failure inside this disposable emulator's own session.
        shell("run-as $app mkdir -p files/recordings/$id.json.new/keep")
        click(find { it.contentDescription?.toString() == "TrickTic Dictate stop recording" })
        find { it.contentDescription?.toString() == "TrickTic Dictate microphone" }
        val before = bytes(id).drop(44).toByteArray()
        assertEquals("capturing", session(id).getString("capture_state"))
        assertFalse(session(id).has("capture_completion"))
        shell("run-as $app rmdir files/recordings/$id.json.new/keep files/recordings/$id.json.new")
        writer(); terminal(id, "interrupted"); assertAudio(id, before)
    }
    fun test08ServiceInterruptionSavesAndDoesNotRestart() {
        editor(); val id = start(); val before = bytes(id).drop(44).toByteArray()
        shell("run-as $app /system/bin/am stopservice --user 0 -n $app/com.metrix.writer.CaptureService")
        // am writes its result to stderr on these images; assert actual durable state instead.
        terminal(id, "interrupted"); assertAudio(id, before)
        SystemClock.sleep(1000)
        assertFalse(shell("dumpsys activity services $app").contains("com.metrix.writer.CaptureService"))
    }

}
