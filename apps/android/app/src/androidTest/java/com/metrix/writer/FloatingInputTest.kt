@file:Suppress("DEPRECATION", "OVERRIDE_DEPRECATION")
package com.metrix.writer

import android.app.UiAutomation
import android.content.Intent
import android.graphics.Rect
import android.os.Bundle
import android.os.ParcelFileDescriptor
import android.os.SystemClock
import android.test.InstrumentationTestCase
import android.view.InputDevice
import android.view.MotionEvent
import android.view.accessibility.AccessibilityNodeInfo
import java.io.File
import java.util.UUID

/** Independent APK editors. UiAutomation uses public actions; Writer uses the real service/connection. */
class FloatingInputTest : InstrumentationTestCase() {
    private val FIXTURE = "Writer test שלום 🌍"
    private lateinit var ui: UiAutomation
    // Either flavor's debug build; MW-SEC-23 runs this suite on the Play flavor too.
    private val app get() = instrumentation.targetContext.packageName
    private val fixture = "com.metrix.writer.editor"
    override fun setUp() {
        super.setUp()
        check(app in setOf("com.metrix.writer.debug", "com.tricktic.dictate.debug")) { "Delivery fixtures require a debug app" }
        EditorFixture::class.java.getDeclaredField("enabled").apply { isAccessible = true }.setBoolean(null, true)
        AccessibilityConsent(instrumentation.targetContext).record(true) // The service is paused without it.
        ui = instrumentation.getUiAutomation(UiAutomation.FLAG_DONT_SUPPRESS_ACCESSIBILITY_SERVICES)
        // Release an injected pointer if a prior fixture assertion interrupted its gesture.
        MotionEvent.obtain(SystemClock.uptimeMillis(), SystemClock.uptimeMillis(), MotionEvent.ACTION_CANCEL, 0f, 0f, 0).let {
            it.source = InputDevice.SOURCE_TOUCHSCREEN; ui.injectInputEvent(it, true); it.recycle()
        }
        assertEquals("Emulator-only fixture suite", "1", shell("getprop ro.kernel.qemu").trim())
        ui.serviceInfo = ui.serviceInfo.apply { flags = flags or android.accessibilityservice.AccessibilityServiceInfo.FLAG_RETRIEVE_INTERACTIVE_WINDOWS or android.accessibilityservice.AccessibilityServiceInfo.FLAG_REPORT_VIEW_IDS }
    }
    private fun shell(command: String): String = ParcelFileDescriptor.AutoCloseInputStream(ui.executeShellCommand(command))
        .bufferedReader().use { it.readText() }
    private fun pause() { SystemClock.sleep(900) }
    private fun nodes(): List<AccessibilityNodeInfo> {
        if (android.os.Build.VERSION.SDK_INT >= 34) ui.clearCache()
        val all = mutableListOf<AccessibilityNodeInfo>()
        fun visit(n: AccessibilityNodeInfo) {
            if (android.os.Build.VERSION.SDK_INT == 33 && !n.refresh()) return
            all.add(n); for (i in 0 until n.childCount) n.getChild(i)?.let { visit(it) }
        }
        ui.windows.forEach { it.root?.let { root -> visit(root) } }
        return all
    }
    private fun find(predicate: (AccessibilityNodeInfo) -> Boolean): AccessibilityNodeInfo {
        repeat(40) { nodes().firstOrNull(predicate)?.let { return it }; SystemClock.sleep(150) }
        fail("Expected UI element unavailable"); throw IllegalStateException()
    }
    private fun click(n0: AccessibilityNodeInfo) {
        var n = n0
        while (!n.isClickable && n.parent != null) n = n.parent
        assertTrue("Settings click", n.performAction(AccessibilityNodeInfo.ACTION_CLICK)); pause()
    }
    private fun clickText(text: String) {
        repeat(12) {
            nodes().firstOrNull { it.text?.toString()?.equals(text, true) == true || it.contentDescription?.toString() == text }?.let { click(it); return }
            nodes().firstOrNull { it.isScrollable }?.performAction(AccessibilityNodeInfo.ACTION_SCROLL_FORWARD)
            pause()
        }
        fail("Missing visible action: $text")
    }
    private fun service(): WriterInputService {
        repeat(40) { WriterInputService.instance?.let { return it }; SystemClock.sleep(150) }
        fail("Writer service not connected"); throw IllegalStateException()
    }
    private fun <T> main(action: () -> T): T {
        var value: T? = null; var error: Throwable? = null
        instrumentation.runOnMainSync { try { value = action() } catch (e: Throwable) { error = e } }
        error?.let { throw it }
        @Suppress("UNCHECKED_CAST") return value as T
    }
    private fun editor(index: Int = 0): AccessibilityNodeInfo = find {
        it.packageName?.toString() == fixture && it.contentDescription?.toString() == "Native editor $index"
    }
    private fun launch(missingSelection: Boolean = false, placeholderAsText: Boolean = false, wrongFieldId: Boolean = false, wrongNodeText: Boolean = false, richParagraphs: Boolean = false) { shell("am force-stop $fixture"); shell("am start -W -n $fixture/.EditorActivity --ez missingSelection $missingSelection --ez placeholderAsText $placeholderAsText --ez wrongFieldId $wrongFieldId --ez wrongNodeText $wrongNodeText --ez richParagraphs $richParagraphs"); pause() }
    // The independent editor controls the real composing region for the dedicated negative test.
    private fun commitFixtureComposition() {
        shell("am broadcast -a $fixture.COMPOSITION_FIXTURE -p $fixture"); pause()
    }
    private fun prepare(text: String, start: Int, end: Int = start, browser: Boolean = false): AccessibilityNodeInfo {
        launch()
        val n = if (browser) find { it.packageName?.toString() == fixture && it.className?.toString() == "android.widget.EditText" && it.contentDescription == null && !it.isPassword } else editor()
        assertTrue(n.performAction(AccessibilityNodeInfo.ACTION_CLICK)); pause()
        assertTrue(n.performAction(AccessibilityNodeInfo.ACTION_SET_TEXT, Bundle().apply { putCharSequence(AccessibilityNodeInfo.ACTION_ARGUMENT_SET_TEXT_CHARSEQUENCE, text) })); pause()
        assertTrue(n.performAction(AccessibilityNodeInfo.ACTION_SET_SELECTION, Bundle().apply {
            putInt(AccessibilityNodeInfo.ACTION_ARGUMENT_SELECTION_START_INT, start)
            putInt(AccessibilityNodeInfo.ACTION_ARGUMENT_SELECTION_END_INT, end)
        })); pause()
        n.refresh(); assertEquals(text, n.text.toString()); assertEquals(start, n.textSelectionStart); assertEquals(end, n.textSelectionEnd)
        return n
    }
    private fun token(): Pair<String, String> {
        val session = UUID.randomUUID().toString()
        val token = main { service().bind(session, true) }
        if (token == null) {
            val diagnostic = main {
                val s = service(); val input = s.inputMethod!!
                val node = s.windows.firstOrNull { it.isFocused }?.root?.findFocus(AccessibilityNodeInfo.FOCUS_INPUT)
                val around = input.currentInputConnection?.getSurroundingText(4096, 4096, 1)
                val info = input.currentInputEditorInfo
                val resourceCheck = try {
                    "resolved=" + s.packageManager.getResourcesForApplication(info!!.packageName).getIdentifier(node?.viewIdResourceName, null, null)
                } catch (e: Exception) { e.javaClass.simpleName }
                "fieldId=${info?.fieldId}, resource=${node?.viewIdResourceName}, resourceCheck=$resourceCheck, inputType=${input.currentInputEditorInfo?.inputType}, nodeType=${node?.inputType}, editable=${node?.isEditable}, sensitive=${if (android.os.Build.VERSION.SDK_INT >= 34) node?.isAccessibilityDataSensitive else null}, nodeSel=${node?.textSelectionStart}:${node?.textSelectionEnd}, connectionSel=${around?.selectionStart}:${around?.selectionEnd}, offset=${around?.offset}, sameText=${around?.text?.toString() == node?.text?.toString()}"
            }
            fail("Eligible editor must bind: $diagnostic")
        }
        return session to token!!
    }
    private fun deliver(token: Pair<String, String>, text: String) = main { AndroidTextDelivery(instrumentation.targetContext).deliver(token.first, text, token.second) }
    private fun reason(token: Pair<String, String>): String = DeliveryStore(File(instrumentation.targetContext.filesDir, "deliveries")).load(token.second).optString("reason")
    private fun state(token: Pair<String, String>): String = DeliveryStore(File(instrumentation.targetContext.filesDir, "deliveries")).load(token.second).getString("state")
    private fun assertEditor(n: AccessibilityNodeInfo, text: String, start: Int, end: Int = start) {
        assertTrue(n.refresh()); assertEquals(text, n.text.toString()); assertEquals(start, n.textSelectionStart); assertEquals(end, n.textSelectionEnd)
    }
    private fun bubble(): AccessibilityNodeInfo = find { it.contentDescription?.toString() == "TrickTic Dictate microphone" }
    private fun tap(x: Float, y: Float, toX: Float = x, toY: Float = y) {
        val at = SystemClock.uptimeMillis()
        fun event(action: Int, time: Long, px: Float, py: Float) {
            val e = MotionEvent.obtain(at, time, action, px, py, 0); e.source = InputDevice.SOURCE_TOUCHSCREEN
            assertTrue(ui.injectInputEvent(e, true)); e.recycle()
        }
        event(MotionEvent.ACTION_DOWN, at, x, y)
        if (x != toX || y != toY) { SystemClock.sleep(100); event(MotionEvent.ACTION_MOVE, at + 100, toX, toY) }
        SystemClock.sleep(100); event(MotionEvent.ACTION_UP, at + 200, toX, toY)
    }

    fun test00EnableViaVisibleAndroidSettings() {
        shell("am start -W -f 0x10008000 -a android.settings.action.MANAGE_OVERLAY_PERMISSION -d package:$app"); pause()
        if (nodes().any { it.text?.toString() == "TrickTic Dictate debug" }) clickText("TrickTic Dictate debug")
        else if (nodes().any { it.text?.toString() == app }) clickText(app) // Settings may cache the package name after an APK replacement.
        val overlay = find { it.isCheckable }
        if (!overlay.isChecked) { click(overlay) }
        shell("am start -W -f 0x10008000 -a android.settings.ACCESSIBILITY_SETTINGS"); pause()
        clickText("TrickTic Dictate · voice insertion")
        for (attempt in 0 until 6) {
            if (WriterInputService.instance != null) break
            when {
                nodes().any { it.text?.toString() == "Allow" } -> { clickText("Allow"); service(); break }
                nodes().any { it.text?.toString() == "Turn off" } -> clickText("Turn off")
                nodes().any { it.text?.toString() == "Stop" } -> clickText("Stop")
                else -> clickText("Use TrickTic Dictate · voice insertion")
            }
            pause()
        }
        service()
        assertFalse(shell("settings get secure default_input_method").contains("metrix"))
    }
    fun test00bServiceStaysPausedUntilTheDisclosureIsAccepted() {
        service(); val consent = AccessibilityConsent(instrumentation.targetContext)
        val n = prepare("Paused | editor", 7); bubble()
        consent.record(false)
        try {
            main { service().refreshControl() }; pause()
            assertFalse("No floating control while paused", nodes().any { it.contentDescription?.toString() == "TrickTic Dictate microphone" })
            val session = UUID.randomUUID().toString()
            val target = main { service().bind(session, true) }!!
            main { AndroidTextDelivery(instrumentation.targetContext).deliver(session, "MUST NOT INSERT", target) }; pause()
            assertEditor(n, "Paused | editor", 7)
            assertEquals("recovery", state(session to target))
            assertEquals("Text insertion is paused until it is accepted in TrickTic Dictate", reason(session to target))
        } finally { consent.record(true) }
        main { service().refreshControl() }
        bubble() // Accepting resumes the connected service without turning it on again.
    }
    fun test01NativeInsertionUnicodeSelectionAndDuplicate() {
        service()
        for (sample in listOf(Triple("English before | after", 15, "test "), Triple("עברית לפני | אחרי", 11, "שלום "), Triple("Mixed עברית 🌍 | English", 14, "fixture שלום 🌍"))) {
            val n = prepare(sample.first, sample.second)
            val t = token(); deliver(t, sample.third); SystemClock.sleep(1200)
            val expected = sample.first.substring(0, sample.second) + sample.third + sample.first.substring(sample.second)
            assertEquals(DeliveryStore(File(instrumentation.targetContext.filesDir, "deliveries")).load(t.second).optString("reason"), "delivered", state(t)); assertEditor(n, expected, sample.second + sample.third.length)
            deliver(t, sample.third); pause(); assertEditor(n, expected, sample.second + sample.third.length)
        }
        val before = "Keep 🌍 selected עברית end"
        val n = prepare(before, 8, 21); val t = token(); deliver(t, "new שלום"); SystemClock.sleep(1200)
        assertEquals(DeliveryStore(File(instrumentation.targetContext.filesDir, "deliveries")).load(t.second).optString("reason"), "delivered", state(t)); assertEditor(n, before.substring(0, 8) + "new שלום" + before.substring(21), 16)
    }
    fun test01bMissingAccessibilitySelectionKeepsGuardedInsertion() {
        service(); launch(true, true)
        val n=editor(); assertTrue(n.performAction(AccessibilityNodeInfo.ACTION_CLICK)); pause()
        fun setText(text:String) {
            assertTrue(n.performAction(AccessibilityNodeInfo.ACTION_SET_TEXT, Bundle().apply {
                putCharSequence(AccessibilityNodeInfo.ACTION_ARGUMENT_SET_TEXT_CHARSEQUENCE,text)
            })); pause(); assertTrue(n.refresh()); assertEquals(-1,n.textSelectionStart)
        }
        fun select(start:Int,end:Int=start) {
            n.performAction(AccessibilityNodeInfo.ACTION_SET_SELECTION,Bundle().apply {
                putInt(AccessibilityNodeInfo.ACTION_ARGUMENT_SELECTION_START_INT,start)
                putInt(AccessibilityNodeInfo.ACTION_ARGUMENT_SELECTION_END_INT,end)
            });pause()
        }
        // Empty field, insertion amid existing Unicode text, and selected-range replacement.
        for ((before,start,end) in listOf(Triple("",0,0),Triple("Keep שלום end",5,5),Triple("Keep שלום end",5,9))) {
            setText(before);select(start,end);bubble()
            val t=token();deliver(t,"test 🌍");SystemClock.sleep(1200)
            val expected=before.substring(0,start)+"test 🌍"+before.substring(end)
            assertTrue(n.refresh());assertEquals(expected,n.text.toString());assertEquals("delivered",state(t))
            val around=main { service().inputMethod!!.currentInputConnection!!.getSurroundingText(4096,4096,0)!! }
            assertEquals(start+"test 🌍".length,around.selectionStart);assertEquals(around.selectionStart,around.selectionEnd)
            deliver(t,"test 🌍");pause();n.refresh();assertEquals(expected,n.text.toString())
        }
        // Insert Anywhere: a cursor moved after recording started receives the text at its new position.
        setText("Preserve this editor");select(4);val moved=token();select(1)
        deliver(moved,"X");SystemClock.sleep(1200);n.refresh()
        assertEquals("PXreserve this editor",n.text.toString());assertEquals(reason(moved),"delivered",state(moved))
    }
    fun test01cEmptyPlaceholderExceptionRequiresMatchingFieldAndActualEmptyText() {
        service()
        for ((wrongField,wrongText) in listOf(true to false,false to true)) {
            launch(true,true,wrongField,wrongText)
            val n=editor();assertTrue(n.performAction(AccessibilityNodeInfo.ACTION_CLICK));pause()
            n.performAction(AccessibilityNodeInfo.ACTION_SET_TEXT,Bundle().apply {
                putCharSequence(AccessibilityNodeInfo.ACTION_ARGUMENT_SET_TEXT_CHARSEQUENCE,if(wrongText) "Existing content" else "")
            });pause()
            val before=n.also { it.refresh() }.text?.toString()
            val t=token();deliver(t,"MUST NOT INSERT");pause()
            assertEquals("Mismatched field or nonempty text must never receive text","recovery",state(t))
            n.refresh();assertEquals(before,n.text?.toString())
        }
    }
    fun test01dRichParagraphPresentationKeepsExactConnectionTextAndStaleGuard() {
        service()
        launch(richParagraphs = true)
        val n = find { it.packageName?.toString() == fixture && it.viewIdResourceName == "editor" && it.isEditable }
        n.performAction(AccessibilityNodeInfo.AccessibilityAction.ACTION_SHOW_ON_SCREEN.id); pause()
        val bounds = Rect(); n.refresh(); n.getBoundsInScreen(bounds)
        tap(bounds.exactCenterX(),bounds.exactCenterY()); pause()
        fun around() = main { service().inputMethod!!.currentInputConnection!!.getSurroundingText(4096,4096,0)!! }
        val before = around().text.toString(); n.refresh()
        assertEquals("Before\n\nAfter", before)
        assertEquals("Before\nAfter", n.text.toString())
        shell("input keycombination 113 122"); pause()
        shell("input keyevent 22 22 22"); pause()
        val inconsistent = token(); deliver(inconsistent,"MUST NOT INSERT"); pause()
        assertEquals("Inconsistent WebView cursor metadata must not insert","recovery",state(inconsistent)); assertEquals(before,around().text.toString())
        // This older WebView corroborates the cursor at the first paragraph boundary.
        shell("input keyevent 22 22 22"); pause()
        val t = token(); deliver(t,"שלום"); SystemClock.sleep(1200)
        val expected = before.substring(0,6) + "שלום" + before.substring(6)
        assertTrue(n.refresh())
        assertEquals(shell("dumpsys activity service $app/com.metrix.writer.WriterInputService"),"delivered",state(t)); assertEquals(expected,around().text.toString()); assertEquals(10,around().selectionStart)
    }
    fun test02BrowserInsertionAndSelection() {
        service()
        for (text in listOf("English | after", "לפני | אחרי", "Mixed 🌍 | שלום")) {
            val pos = text.indexOf('|'); val n = prepare(text, pos, pos + 1, true)
            val t = token(); deliver(t, "test שלום 🌍"); SystemClock.sleep(1400)
            assertEquals(DeliveryStore(File(instrumentation.targetContext.filesDir, "deliveries")).load(t.second).optString("reason"), "delivered", state(t))
            assertEditor(n, text.replace("|", "test שלום 🌍"), pos + "test שלום 🌍".length)
        }
    }
    fun test03InsertsAtFocusWhenTheResultArrives() {
        service()
        // Another field focused after recording started receives the text; the first stays untouched.
        var n = prepare("Keep before after", 5); var t = token()
        val other = editor(1); assertTrue(other.performAction(AccessibilityNodeInfo.ACTION_CLICK)); pause()
        other.refresh(); val otherText = other.text.toString(); val otherCursor = other.textSelectionStart
        deliver(t, "moved "); SystemClock.sleep(1200); assertEquals(reason(t), "delivered", state(t))
        assertEditor(other, otherText.substring(0, otherCursor) + "moved " + otherText.substring(otherCursor), otherCursor + 6)
        assertEditor(n, "Keep before after", 5)
        // The user edited the field while text was processing: insert at the edited cursor.
        n = prepare("Keep before after", 5); t = token()
        n.performAction(AccessibilityNodeInfo.ACTION_SET_TEXT, Bundle().apply { putCharSequence(AccessibilityNodeInfo.ACTION_ARGUMENT_SET_TEXT_CHARSEQUENCE, "User changed this") }); pause()
        n.performAction(AccessibilityNodeInfo.ACTION_SET_SELECTION, Bundle().apply { putInt(AccessibilityNodeInfo.ACTION_ARGUMENT_SELECTION_START_INT, 4); putInt(AccessibilityNodeInfo.ACTION_ARGUMENT_SELECTION_END_INT, 4) }); pause()
        deliver(t, " late"); SystemClock.sleep(1200); assertEquals(reason(t), "delivered", state(t)); assertEditor(n, "User late changed this", 9)
        // Two sessions each deliver once, in arrival order.
        n = prepare("Keep before after", 5); val old = token(); val latest = token()
        deliver(old, "old "); SystemClock.sleep(1200); assertEquals("delivered", state(old))
        deliver(latest, "new "); SystemClock.sleep(1200); assertEquals("delivered", state(latest)); assertEditor(n, "Keep old new before after", 13)
        // Writer's own screen is never a target.
        n = prepare("Keep before after", 5); t = token(); shell("am start -W -f 0x10008000 -n $app/com.metrix.writer.MainActivity"); pause()
        deliver(t, "MUST NOT INSERT"); pause(); assertEquals("recovery", state(t))
        shell("am start -W -n $fixture/.EditorActivity"); pause(); n = editor(); n.refresh(); assertEquals("Keep before after", n.text.toString())
    }
    fun test03aFieldRecreatedBetweenStartAndStopStillReceivesText() {
        prepare("Before recreation", 6); val t = token()
        // Force-stop recreates the process, activity, window and field (like returning to a chat).
        val n = prepare("After recreation", 5)
        deliver(t, " again"); SystemClock.sleep(1200)
        assertEquals(reason(t), "delivered", state(t)); assertEditor(n, "After again recreation", 11)
    }
    fun test03bAuxiliaryAndFocusedWindowsBeforeReturningStillInsert() {
        val n = prepare("Keep before after", 5)
        val t = token()
        val context = instrumentation.targetContext
        val wm = context.getSystemService(android.view.WindowManager::class.java)
        val view = main { android.widget.TextView(context).apply { text = "Auxiliary test status" } }
        val params = android.view.WindowManager.LayoutParams(180, 90,
            android.view.WindowManager.LayoutParams.TYPE_APPLICATION_OVERLAY,
            android.view.WindowManager.LayoutParams.FLAG_NOT_FOCUSABLE or android.view.WindowManager.LayoutParams.FLAG_NOT_TOUCH_MODAL,
            android.graphics.PixelFormat.TRANSLUCENT).apply { gravity = android.view.Gravity.TOP or android.view.Gravity.LEFT }
        try {
            main { wm.addView(view, params) }; pause()
            main { view.sendAccessibilityEvent(android.view.accessibility.AccessibilityEvent.TYPE_WINDOW_STATE_CHANGED) }; pause()
            deliver(t, "test "); SystemClock.sleep(1200)
            assertEquals(reason(t), "delivered", state(t)); assertEditor(n, "Keep test before after", 10)
            val returned = token()
            main { params.flags = 0; wm.updateViewLayout(view, params); view.isFocusableInTouchMode = true; view.requestFocus() }; pause()
            main { wm.removeViewImmediate(view) }; pause()
            n.performAction(AccessibilityNodeInfo.ACTION_CLICK); pause()
            n.refresh(); val cursor = n.textSelectionStart
            deliver(returned, "back "); SystemClock.sleep(1200)
            assertEquals(reason(returned), "delivered", state(returned))
            assertEditor(n, "Keep test before after".substring(0, cursor) + "back " + "Keep test before after".substring(cursor), cursor + 5)
        } finally { main { if (view.isAttachedToWindow) wm.removeViewImmediate(view) } }
    }
    fun test03cNotificationShadeAndOtherAppAwayAndBackInserts() {
        val n = prepare("Keep before after", 5); val t = token()
        main { service().onAccessibilityEvent(android.view.accessibility.AccessibilityEvent.obtain(
            android.view.accessibility.AccessibilityEvent.TYPE_WINDOW_STATE_CHANGED).apply { packageName = "com.android.systemui" }) }
        assertTrue(ui.performGlobalAction(android.accessibilityservice.AccessibilityService.GLOBAL_ACTION_NOTIFICATIONS))
        SystemClock.sleep(300)
        assertTrue(ui.performGlobalAction(android.accessibilityservice.AccessibilityService.GLOBAL_ACTION_BACK)); pause()
        n.performAction(AccessibilityNodeInfo.ACTION_CLICK); pause()
        deliver(t, "test "); SystemClock.sleep(1200)
        assertEquals(reason(t), "delivered", state(t)); assertEditor(n, "Keep test before after", 10)
        // Leave for another app (Settings), then return to the same editor and tap it.
        val away = token()
        shell("am start -W -a android.settings.SETTINGS"); pause()
        shell("am start -W -n $fixture/.EditorActivity"); pause()
        val back = editor(); assertTrue(back.performAction(AccessibilityNodeInfo.ACTION_CLICK)); pause()
        back.refresh(); val cursor = back.textSelectionStart
        deliver(away, "home "); SystemClock.sleep(1200)
        assertEquals(reason(away), "delivered", state(away))
        assertEditor(back, "Keep test before after".substring(0, cursor) + "home " + "Keep test before after".substring(cursor), cursor + 5)
    }
    fun test03dScreenLockDuringPendingResultRetainsTextWithoutInsertion() {
        val n = prepare("Keep before after", 5); val t = token()
        try {
            shell("input keyevent KEYCODE_SLEEP"); pause()
            deliver(t, "Late result while locked"); pause()
            assertEquals("recovery", state(t))
            val saved = DeliveryStore(File(instrumentation.targetContext.filesDir, "deliveries")).load(t.second)
            assertEquals("Late result while locked", saved.getString("text"))
        } finally { shell("input keyevent KEYCODE_WAKEUP"); shell("wm dismiss-keyguard"); pause() }
        assertTrue(n.refresh()); assertEquals("Keep before after", n.text.toString())
        deliver(t, "Late result while locked"); pause()
        assertEquals("recovery", state(t)); assertTrue(n.refresh()); assertEquals("Keep before after", n.text.toString())
    }
    fun test04BubbleTouchDragAndSensitiveFields() {
        val keyboard = shell("settings get secure default_input_method")
        val n = prepare("Bubble | keeps focus", 7)
        var rect = Rect(); bubble().getBoundsInScreen(rect)

        tap(rect.exactCenterX(), rect.exactCenterY(), rect.exactCenterX() - 90, rect.exactCenterY() - 70); pause()
        assertEditor(n, "Bubble | keeps focus", 7)
        bubble().getBoundsInScreen(rect); tap(rect.exactCenterX(), rect.exactCenterY())
        SystemClock.sleep(4500)
        assertEditor(n, "Bubble " + FIXTURE + "| keeps focus", 7 + FIXTURE.length)
        val selected = prepare("Bubble | keeps selection", 7, 8)
        bubble().getBoundsInScreen(rect)
        tap(rect.exactCenterX(), rect.exactCenterY(), rect.exactCenterX() - 45, rect.exactCenterY() - 30); pause()
        assertEditor(selected, "Bubble | keeps selection", 7, 8)
        bubble().getBoundsInScreen(rect); tap(rect.exactCenterX(), rect.exactCenterY()); SystemClock.sleep(4500)
        assertEditor(selected, "Bubble " + FIXTURE + " keeps selection", 7 + FIXTURE.length)
        // Other screen touches stay available, including switching to another field.
        val other = editor(1); other.getBoundsInScreen(rect); tap(rect.exactCenterX(), rect.exactCenterY()); pause()
        assertTrue(other.refresh()); assertTrue(other.isFocused)
        val sensitive = token()
        find { it.packageName?.toString() == fixture && it.isPassword }.performAction(AccessibilityNodeInfo.ACTION_CLICK); pause()
        deliver(sensitive, "MUST NOT INSERT"); pause(); assertEquals("recovery", state(sensitive))
        assertFalse(nodes().any { it.contentDescription?.toString() == "TrickTic Dictate microphone" })
        assertEquals(keyboard, shell("settings get secure default_input_method"))
    }
    fun test04bSensitiveInputVariantsKeepDelayedTextInRecovery() {
        val variants = mutableListOf("password", "visiblePassword", "webPassword", "numberPassword", "private")
        if (android.os.Build.VERSION.SDK_INT >= 34) variants.add("sensitive")
        for (variant in variants) {
            prepare("Public before | after", 7)
            val pending = token()
            shell("am force-stop $fixture")
            shell("am start -W -n $fixture/.EditorActivity --es securityType $variant")
            SystemClock.sleep(1800)
            deliver(pending, "Sensitive result must be retained")
            pause()
            assertEquals(variant, "recovery", state(pending))
            val rejection = reason(pending)
            val expected = when (variant) {
                "private" -> listOf("private_editor")
                "sensitive" -> listOf("node_sensitive", "focused_node_missing", "focused_node_not_editable", "node_not_ready")
                else -> listOf("editor_type_excluded", "node_type_excluded")
            }
            assertTrue("$variant: $rejection", expected.any { rejection.contains(it) })
            assertEquals("Sensitive result must be retained", DeliveryStore(File(instrumentation.targetContext.filesDir, "deliveries")).load(pending.second).getString("text"))
            assertFalse(variant, nodes().any { it.contentDescription?.toString() == "TrickTic Dictate microphone" })
        }
        // test01 separately requires successful native insertion with exact surrounding text.
    }

    fun test04cActiveCompositionRetainsResultWithoutChangingEditor() {
        val n = prepare("Keep before after", 5)
        val pending = token()
        shell("am broadcast -a $fixture.COMPOSITION_FIXTURE -p $fixture --ez compose true"); pause()
        val region = shell("am broadcast -a $fixture.COMPOSITION_FIXTURE -p $fixture --ez inspect true")
        val span = Regex("region=(\\d+):(\\d+)").find(region)
        assertTrue("Fixture must retain a real composing region: $region",
            span != null && span.groupValues[2].toInt() > span.groupValues[1].toInt())
        deliver(pending, "MUST NOT REPLACE COMPOSITION"); pause()
        assertEquals(region, "recovery", state(pending))
        assertTrue(reason(pending), reason(pending).contains("composition"))
        assertEditor(n, "Keep before after", 5)
        commitFixtureComposition()
    }

    fun test04aDragToSnoozeAndRestoreKeepsEditorAndKeyboard() {
        val n = prepare("Hide without changing this", 5, 12)
        val prefs = FloatingPreferences(instrumentation.targetContext)
        prefs.showNow()
        val oldSize = prefs.size; val oldMinutes = prefs.minutes
        try {
            prefs.size = 32; prefs.minutes = 30; pause()
            val bounds = Rect(); bubble().getBoundsInScreen(bounds)
            val at = SystemClock.uptimeMillis()
            fun event(action: Int, x: Float, y: Float) {
                val e = MotionEvent.obtain(at, SystemClock.uptimeMillis(), action, x, y, 0)
                e.source = InputDevice.SOURCE_TOUCHSCREEN
                assertTrue(ui.injectInputEvent(e, true)); e.recycle(); SystemClock.sleep(150)
            }
            event(MotionEvent.ACTION_DOWN, bounds.exactCenterX(), bounds.exactCenterY())
            event(MotionEvent.ACTION_MOVE, bounds.exactCenterX() - 110, bounds.exactCenterY() - 90)
            val target = Rect()
            find { it.contentDescription?.toString() == "Drop to hide TrickTic Dictate for 30 minutes" }.getBoundsInScreen(target)
            val proof = File(instrumentation.targetContext.cacheDir, "overlay-proof").apply { mkdirs() }
            ui.takeScreenshot()!!.let { bitmap -> File(proof, "hide-target.png").outputStream().use { bitmap.compress(android.graphics.Bitmap.CompressFormat.PNG, 100, it) }; bitmap.recycle() }
            assertTrue("Hide target at screen bottom", target.centerY() > instrumentation.targetContext.resources.displayMetrics.heightPixels * .8f)
            event(MotionEvent.ACTION_MOVE, target.exactCenterX(), target.exactCenterY())
            event(MotionEvent.ACTION_UP, target.exactCenterX(), target.exactCenterY()); pause()
            assertTrue("Drop must hide: target=$target " + shell("dumpsys activity service $app/com.metrix.writer.WriterInputService"), prefs.snoozed())
            assertFalse(nodes().any { it.contentDescription?.toString() == "TrickTic Dictate microphone" })
            assertEditor(n, "Hide without changing this", 5, 12)
            assertTrue("Dragging preserves the visible keyboard", ui.windows.any { it.type == android.view.accessibility.AccessibilityWindowInfo.TYPE_INPUT_METHOD })
            prefs.showNow(); pause(); bubble()
            assertEditor(n, "Hide without changing this", 5, 12)
        } finally {
            MotionEvent.obtain(SystemClock.uptimeMillis(), SystemClock.uptimeMillis(), MotionEvent.ACTION_CANCEL, 0f, 0f, 0).let {
                it.source = InputDevice.SOURCE_TOUCHSCREEN; ui.injectInputEvent(it, true); it.recycle()
            }
            prefs.showNow(); prefs.size = oldSize; prefs.minutes = oldMinutes
        }
    }
    fun test05OverlayPermissionLossAndRecoveryCopy() {
        service(); val n = prepare("Keep before after", 5); val t = token()
        shell("am start -W -f 0x10008000 -a android.settings.action.MANAGE_OVERLAY_PERMISSION -d package:$app"); pause()
        if (nodes().any { it.text?.toString() == "TrickTic Dictate debug" }) clickText("TrickTic Dictate debug")
        else if (nodes().any { it.text?.toString() == app }) clickText(app) // Settings may cache the package name after an APK replacement.
        val toggle = find { it.isCheckable }; assertTrue(toggle.isChecked)
        click(toggle)
        deliver(t, "recovery fixture"); pause(); assertEquals("recovery", state(t))
        assertFalse(nodes().any { it.contentDescription?.toString() == "TrickTic Dictate microphone" })
        click(find { it.isCheckable })
        shell("am start -W -f 0x10008000 -n $app/com.metrix.writer.MainActivity"); pause(); if (nodes().any { it.text?.toString() == "Explore first" }) clickText("Explore first"); clickText("History"); clickText("Text to recover")
        clickText("Copy saved result")
        val clip = main { instrumentation.targetContext.getSystemService(android.content.ClipboardManager::class.java).primaryClip }
        assertEquals("recovery fixture", clip!!.getItemAt(0).text.toString())
    }
    fun test06RotationKeyboardInsetsAndPositionPersistence() {
        prepare("Rotation keeps a usable bubble", 4)
        val preferences = instrumentation.targetContext.getSharedPreferences("floating-control", 0)
        val x = preferences.getFloat("x", 1f); val y = preferences.getFloat("y", .6f)
        fun usable() {
            val bounds = Rect(); bubble().getBoundsInScreen(bounds)
            val screen = ui.takeScreenshot()!!
            assertTrue("Bubble within display", bounds.left >= 0 && bounds.top >= 0 && bounds.right <= screen.width && bounds.bottom <= screen.height)
            screen.recycle()
            val keyboard = ui.windows.firstOrNull { it.type == android.view.accessibility.AccessibilityWindowInfo.TYPE_INPUT_METHOD }
            if (keyboard != null) {
                val ime = Rect(); keyboard.getBoundsInScreen(ime)
                assertTrue("Bubble above keyboard: bubble=$bounds ime=$ime", bounds.bottom <= ime.top)
            }
        }
        usable()
        try {
            assertTrue(ui.setRotation(UiAutomation.ROTATION_FREEZE_90)); SystemClock.sleep(1800)
            editor().performAction(AccessibilityNodeInfo.ACTION_CLICK); pause(); usable()
            ui.performGlobalAction(android.accessibilityservice.AccessibilityService.GLOBAL_ACTION_BACK); pause(); usable()
            editor().performAction(AccessibilityNodeInfo.ACTION_CLICK); pause(); usable()
        } finally { ui.setRotation(UiAutomation.ROTATION_FREEZE_0); pause(); ui.setRotation(UiAutomation.ROTATION_UNFREEZE) }
        assertEquals(x, preferences.getFloat("x", 1f)); assertEquals(y, preferences.getFloat("y", .6f))
        // A fullscreen extract editor leaves no 64dp area above the keyboard: hide, then restore.
        shell("am force-stop $fixture"); shell("am start -W -n $fixture/.EditorActivity --ez fullscreen true"); pause()
        try {
            ui.setRotation(UiAutomation.ROTATION_FREEZE_90); pause()
            editor().performAction(AccessibilityNodeInfo.ACTION_CLICK); SystemClock.sleep(1600)
            val ime = Rect(); ui.windows.first { it.type == android.view.accessibility.AccessibilityWindowInfo.TYPE_INPUT_METHOD }.getBoundsInScreen(ime)
            assertTrue("Fullscreen fixture has no bubble space", ime.top < 168)
            assertFalse(nodes().any { it.contentDescription?.toString() == "TrickTic Dictate microphone" })
        } finally { ui.setRotation(UiAutomation.ROTATION_FREEZE_0); pause(); ui.setRotation(UiAutomation.ROTATION_UNFREEZE) }
        editor().performAction(AccessibilityNodeInfo.ACTION_CLICK); pause(); usable()
    }
    fun test07UnsupportedEditorsNeverReceiveTextAndReturningToOriginalInserts() {
        for (description in listOf("Unsupported numeric editor", "Private editor")) {
            launch()
            val field = find { it.contentDescription?.toString() == description }
            field.performAction(AccessibilityNodeInfo.ACTION_CLICK); pause()
            val t = token(); deliver(t, "MUST NOT INSERT"); pause()
            assertEquals("recovery", state(t)); assertTrue(field.refresh()); assertFalse(field.text.toString().contains("MUST NOT INSERT"))
            assertFalse(nodes().any { it.contentDescription?.toString() == "TrickTic Dictate microphone" })
        }
        val n = prepare("Keep before after", 5); val t = token()
        editor(1).performAction(AccessibilityNodeInfo.ACTION_CLICK); pause()
        n.performAction(AccessibilityNodeInfo.ACTION_CLICK); pause()
        n.performAction(AccessibilityNodeInfo.ACTION_SET_SELECTION, Bundle().apply {
            putInt(AccessibilityNodeInfo.ACTION_ARGUMENT_SELECTION_START_INT, 5); putInt(AccessibilityNodeInfo.ACTION_ARGUMENT_SELECTION_END_INT, 5)
        }); pause()
        deliver(t, "late "); SystemClock.sleep(1200); assertEquals(reason(t), "delivered", state(t)); assertEditor(n, "Keep late before after", 10)
        ui.performGlobalAction(android.accessibilityservice.AccessibilityService.GLOBAL_ACTION_BACK); pause()
        nodes().firstOrNull { it.isScrollable }?.performAction(AccessibilityNodeInfo.ACTION_SCROLL_FORWARD); pause()
        assertNotNull(find { it.text?.toString()?.equals("Send count: 0", true) == true })
    }
    fun test07aReadbackFailureNeverRetriesAnUnconfirmedEdit() {
        val n = prepare("Keep before after", 5); val t = token()
        deliver(t, "REJECTED FIXTURE"); SystemClock.sleep(1800)
        assertEquals("unconfirmed", state(t)); assertEditor(n, "Keep before after", 5)
        deliver(t, "different duplicate"); pause(); assertEditor(n, "Keep before after", 5)
        assertEquals("REJECTED FIXTURE", DeliveryStore(File(instrumentation.targetContext.filesDir, "deliveries")).load(t.second).getString("text"))
    }
    fun test08AccessibilityRevocationRetainsResultWithoutClipboardWrite() {
        shell("am start -W -f 0x10008000 -n $app/com.metrix.writer.MainActivity"); pause()
        main { instrumentation.targetContext.getSystemService(android.content.ClipboardManager::class.java)
            .setPrimaryClip(android.content.ClipData.newPlainText("test sentinel", "untouched sentinel")) }
        prepare("Original field", 4); val t = token()
        shell("am start -W -f 0x10008000 -a android.settings.ACCESSIBILITY_SETTINGS"); pause()
        clickText("TrickTic Dictate · voice insertion"); clickText("Use TrickTic Dictate · voice insertion"); clickText(if (android.os.Build.VERSION.SDK_INT == 33) "Stop" else "Turn off")
        assertNull(WriterInputService.instance)
        deliver(t, "Saved after revocation"); assertEquals("recovery", state(t))
        shell("am start -W -f 0x10008000 -n $app/com.metrix.writer.MainActivity"); pause(); if (nodes().any { it.text?.toString() == "Explore first" }) clickText("Explore first"); clickText("History"); clickText("Text to recover")
        assertNotNull(find { it.text?.toString() == "Saved after revocation" })
        val clip = main { instrumentation.targetContext.getSystemService(android.content.ClipboardManager::class.java).primaryClip }
        assertEquals("untouched sentinel", clip!!.getItemAt(0).text.toString())
        clickText("Copy saved result")
        val copied = main { instrumentation.targetContext.getSystemService(android.content.ClipboardManager::class.java).primaryClip }
        assertEquals("Saved after revocation", copied!!.getItemAt(0).text.toString())
    }

}
