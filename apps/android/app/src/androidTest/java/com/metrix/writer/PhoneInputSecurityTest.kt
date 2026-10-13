@file:Suppress("DEPRECATION")
package com.metrix.writer

import android.app.UiAutomation
import android.os.Bundle
import android.os.ParcelFileDescriptor
import android.os.SystemClock
import android.test.InstrumentationTestCase
import android.view.accessibility.AccessibilityNodeInfo
import java.io.File
import java.util.UUID

/** Explicit isolated release package only. No recording, cloud request, real key or personal editor. */
class PhoneInputSecurityTest : InstrumentationTestCase() {
    private lateinit var ui: UiAutomation
    private val fixture = "com.metrix.writer.editor"
    private val context get() = instrumentation.targetContext
    private lateinit var keyboard: String
    override fun setUp() {
        super.setUp()
        check(context.packageName == "com.metrix.writer.securityphone" && !BuildConfig.DEBUG)
        ui = instrumentation.getUiAutomation(UiAutomation.FLAG_DONT_SUPPRESS_ACCESSIBILITY_SERVICES)
        ui.serviceInfo = ui.serviceInfo.apply {
            flags = flags or android.accessibilityservice.AccessibilityServiceInfo.FLAG_RETRIEVE_INTERACTIVE_WINDOWS or
                android.accessibilityservice.AccessibilityServiceInfo.FLAG_REPORT_VIEW_IDS
        }
        keyboard = shell("settings get secure default_input_method")
    }
    override fun tearDown() {
        assertEquals("Keyboard selection changed", keyboard, shell("settings get secure default_input_method"))
        super.tearDown()
    }
    private fun shell(command: String) = ParcelFileDescriptor.AutoCloseInputStream(ui.executeShellCommand(command)).bufferedReader().use { it.readText() }
    private fun pause() = SystemClock.sleep(1000)
    private fun <T> main(action: () -> T): T {
        var result: T? = null
        instrumentation.runOnMainSync { result = action() }
        @Suppress("UNCHECKED_CAST") return result as T
    }
    private fun service(): WriterInputService {
        repeat(60) { WriterInputService.instance?.let { return it }; SystemClock.sleep(250) }
        error("Enable only TrickTic Dictate security test insertion in Android Settings")
    }
    private fun nodes(): List<AccessibilityNodeInfo> {
        if (android.os.Build.VERSION.SDK_INT >= 34) ui.clearCache()
        val out = mutableListOf<AccessibilityNodeInfo>()
        fun visit(n: AccessibilityNodeInfo) { out.add(n); for (i in 0 until n.childCount) n.getChild(i)?.let(::visit) }
        ui.windows.forEach { it.root?.let(::visit) }
        return out
    }
    private fun field(index: Int = 0): AccessibilityNodeInfo {
        repeat(30) {
            nodes().firstOrNull { it.packageName?.toString() == fixture && it.contentDescription?.toString() == "Native editor $index" }?.let { return it }
            SystemClock.sleep(200)
        }
        error("Independent editor unavailable")
    }
    private fun launch(extra: String = "") {
        shell("am force-stop $fixture")
        shell("am start -W -n $fixture/.EditorActivity $extra"); pause()
    }
    private fun prepare(): AccessibilityNodeInfo {
        launch()
        return field().also {
            assertTrue(it.performAction(AccessibilityNodeInfo.ACTION_CLICK)); pause()
            assertTrue(it.performAction(AccessibilityNodeInfo.ACTION_SET_TEXT, Bundle().apply {
                putCharSequence(AccessibilityNodeInfo.ACTION_ARGUMENT_SET_TEXT_CHARSEQUENCE, "Before | after שלום")
            })); pause()
            select(it, 7)
        }
    }
    private fun select(n: AccessibilityNodeInfo, position: Int) {
        assertTrue(n.performAction(AccessibilityNodeInfo.ACTION_SET_SELECTION, Bundle().apply {
            putInt(AccessibilityNodeInfo.ACTION_ARGUMENT_SELECTION_START_INT, position)
            putInt(AccessibilityNodeInfo.ACTION_ARGUMENT_SELECTION_END_INT, position)
        })); pause()
    }
    private fun token(): Pair<String, String> {
        val id = UUID.randomUUID().toString()
        val svc = service()
        return id to main { svc.bind(id, true) ?: error("Ordinary editor failed to bind") }
    }
    private fun deliver(t: Pair<String, String>, text: String) {
        main { AndroidTextDelivery(context).deliver(t.first, text, t.second) }; pause()
    }
    private fun result(t: Pair<String, String>) = DeliveryStore(File(context.filesDir, "deliveries")).load(t.second)
    private fun assertText(n: AccessibilityNodeInfo, expected: String) { assertTrue(n.refresh()); assertEquals(expected, n.text.toString()) }
    private fun click(n0: AccessibilityNodeInfo) {
        var n = n0
        while (!n.isClickable && n.parent != null) n = n.parent
        assertTrue(n.performAction(AccessibilityNodeInfo.ACTION_CLICK)); pause()
    }
    private fun clickText(text: String) {
        repeat(10) {
            nodes().firstOrNull { it.packageName?.toString() == "com.android.settings" &&
                it.text?.toString()?.equals(text, true) == true }?.let { click(it); return }
            nodes().firstOrNull { it.packageName?.toString() == "com.android.settings" && it.isScrollable }
                ?.performAction(AccessibilityNodeInfo.ACTION_SCROLL_FORWARD); pause()
        }
        error("Settings action unavailable: $text")
    }
    fun test00EnableOnlyIsolatedServiceViaVisibleSettings() {
        shell("am start -W -f 0x10008000 -a android.settings.ACCESSIBILITY_SETTINGS"); pause()
        clickText("Installed apps"); clickText("TrickTic Dictate security test insertion")
        for (attempt in 0 until 6) {
            if (WriterInputService.instance != null) return
            val options = nodes().filter { it.packageName?.toString() == "com.android.settings" }
            val confirmation = options.firstOrNull { it.text?.toString() in listOf("Allow", "Turn off", "Stop") }
            if (confirmation != null) click(confirmation)
            else click(options.first { it.isCheckable })
            pause()
        }
        service()
    }
    fun test01OrdinaryInsertionAndFocusedEditorChangeNeverSend() {
        val first = prepare(); val pending = token()
        assertTrue(nodes().any { it.packageName?.toString() == context.packageName && it.contentDescription?.toString() == "TrickTic Dictate microphone" })
        deliver(pending, "test 🌍 ")
        assertEquals(result(pending).optString("reason"), "delivered", result(pending).getString("state"))
        assertText(first, "Before test 🌍 | after שלום")
        deliver(pending, "duplicate"); assertText(first, "Before test 🌍 | after שלום")
        val next = token(); val second = field(1)
        assertTrue(second.performAction(AccessibilityNodeInfo.ACTION_CLICK)); pause(); select(second, 0)
        deliver(next, "next ")
        assertEquals(result(next).optString("reason"), "delivered", result(next).getString("state"))
        assertText(second, "next עברית לפני | אחרי")
        assertText(first, "Before test 🌍 | after שלום")
        ui.performGlobalAction(1); pause()
        repeat(4) { nodes().firstOrNull { it.packageName?.toString() == fixture && it.isScrollable }?.performAction(AccessibilityNodeInfo.ACTION_SCROLL_FORWARD); pause() }
        assertTrue(nodes().any { it.text?.toString()?.equals("Send count: 0", true) == true })
    }
    fun test02SensitiveFieldsRetainResults() {
        for (kind in listOf("password", "visiblePassword", "webPassword", "numberPassword", "private", "sensitive")) {
            prepare(); val pending = token()
            launch("--es securityType $kind")
            deliver(pending, "synthetic pending result")
            val item = result(pending)
            assertEquals(kind, "recovery", item.getString("state"))
            assertEquals("synthetic pending result", item.getString("text"))
            val allowed = when (kind) {
                "private" -> listOf("private_editor")
                "sensitive" -> listOf("node_sensitive", "focused_node_missing", "focused_node_not_editable", "node_not_ready")
                else -> listOf("editor_type_excluded", "node_type_excluded")
            }
            assertTrue("$kind: ${item.optString("reason")}", allowed.any { item.optString("reason").contains(it) })
            assertFalse(nodes().any { it.packageName?.toString() == context.packageName && it.contentDescription?.toString() == "TrickTic Dictate microphone" })
        }
    }
    fun test03RevocationPreservesTextAndClipboard() {
        shell("am start -W -n ${context.packageName}/com.metrix.writer.MainActivity"); pause()
        val clipboard = context.getSystemService(android.content.ClipboardManager::class.java)
        // Observe only equality; never log clipboard text or put fixture data on the user's clipboard.
        val before = main { clipboard.primaryClip }
        val n = prepare(); val pending = token()
        main { service().disableSelf() }
        repeat(40) { if (WriterInputService.instance != null) SystemClock.sleep(100) }
        assertNull(WriterInputService.instance)
        deliver(pending, "recover after revocation")
        assertEquals("recovery", result(pending).getString("state"))
        assertEquals("recover after revocation", result(pending).getString("text"))
        assertText(n, "Before | after שלום")
        shell("am start -W -n ${context.packageName}/com.metrix.writer.MainActivity"); pause()
        val after = main { clipboard.primaryClip }
        assertTrue("Clipboard changed", before?.toString() == after?.toString())
        assertFalse(shell("settings get secure enabled_accessibility_services").contains("com.metrix.writer.securityphone/"))
    }
}
