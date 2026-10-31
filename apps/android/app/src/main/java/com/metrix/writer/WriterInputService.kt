package com.metrix.writer

import android.accessibilityservice.AccessibilityService
import android.accessibilityservice.InputMethod
import android.os.Handler
import android.os.Looper
import android.provider.Settings
import android.text.InputType
import android.text.Spanned
import android.view.accessibility.AccessibilityEvent
import android.view.accessibility.AccessibilityNodeInfo
import android.view.inputmethod.EditorInfo
import android.view.inputmethod.InputConnection
import android.widget.Toast
import com.metrix.writer.DeliveryStore.Companion.markOutcome
import java.io.File
import java.util.UUID
import com.metrix.writer.ui.RecoveryNotification
import com.metrix.writer.ui.deliveryReason

/**
 * Only the focused editor is inspected. No screen traversal, clipboard paste, key or Send action.
 * Insert Anywhere (MW-72): a result goes into the editor focused when it is ready, not the editor
 * focused when recording started. By design, moving to another field before the text
 * arrives inserts it there. Password, numeric, private and Writer's own editors are never targets.
 */
class WriterInputService : AccessibilityService(), TextDelivery {
    private val handler = Handler(Looper.getMainLooper())
    private lateinit var editorInput: EditorInput
    private lateinit var bubble: WriterBubble
    private lateinit var deliveries: DeliveryStore
    private val consent by lazy { AccessibilityConsent(this) }
    private var composing = false
    private var verifying = false
    private var connected = false
    @Volatile private var editorCheck = "not_checked"
    @Volatile private var snapshotCheck = "not_checked"
    @Volatile private var snapshotComparison = "not_checked"
    @Volatile private var lastBindCheck = "not_called"
    @Volatile private var controlCheck = "not_checked"
    override fun dump(fd: java.io.FileDescriptor, writer: java.io.PrintWriter, args: Array<out String>?) {
        // Shell DUMP diagnostics contain only fixed reason codes and geometry, never editor text or credentials.
        if (args?.contains("latency-json") == true) {
            writer.println(LatencyMeasurements.read(File(filesDir, "recordings")).toString()); return
        }
        if (args?.contains("processing-json") == true) {
            // Content-free view of the newest session: stage, codes, timings and version kinds only.
            val latest = File(filesDir, "recordings").listFiles().orEmpty().filter { it.name.endsWith(".json") }.maxByOrNull { it.lastModified() }
            writer.println(latest?.let { runCatching { ProcessingState.view(filesDir, it.nameWithoutExtension).toJson() }.getOrNull() } ?: "{}"); return
        }
        writer.println("Writer editor check: $editorCheck")
        writer.println("Writer control check: $controlCheck")
        writer.println("Writer last bind check: $lastBindCheck")
        if (Looper.myLooper() == Looper.getMainLooper()) {
            val probeStarted = android.os.SystemClock.elapsedRealtime()
            snapshot(detail = true)
            writer.println("Writer current snapshot probe: $snapshotCheck probe_ms=${android.os.SystemClock.elapsedRealtime() - probeStarted}")
            writer.println("Writer snapshot comparison: $snapshotComparison")
        }
        try {
            val directory = File(filesDir, "recordings")
            directory.listFiles().orEmpty().filter { it.name.endsWith(".json") }.maxByOrNull { it.lastModified() }?.let { file ->
                val item = SessionStore(directory).load(file.nameWithoutExtension)
                item.optJSONObject("startup_timing")?.let { timing ->
                    val tap = timing.optLong("tap_elapsed_ms", -1)
                    if (tap >= 0) for (key in listOf("service_elapsed_ms", "prepared_elapsed_ms", "microphone_started_elapsed_ms", "first_audio_read_elapsed_ms", "first_audio_saved_elapsed_ms", "app_visible_elapsed_ms", "bubble_visible_elapsed_ms")) {
                        if (timing.has(key) && timing.optLong(key) >= tap) writer.println("Writer startup timing: tap_to_$key=${timing.getLong(key) - tap}")
                    }
                }
                val attempts = item.getJSONArray("attempts")
                val latest = if (attempts.length() > 0) attempts.getJSONObject(attempts.length() - 1) else null
                writer.println("Writer latest recording: id=${item.getString("id")} state=${item.optString("capture_state")} bytes=${item.optLong("byte_length")} duration_ms=${item.optLong("duration_ms")} origin=${item.optString("capture_origin")} target=${item.has("original_target")} pending=${item.has("pending")} attempts=${attempts.length()} latest_state=${latest?.optJSONObject("status")?.optString("state")}")
            }
        } catch (_: Exception) { writer.println("Writer recording diagnostics unavailable") }
        if (::deliveries.isInitialized) {
            deliveries.all().firstOrNull()?.let {
                writer.println("Writer latest delivery: state=${it.optString("state")} outcome=${DeliveryOutcome.of(it).code} reason_code=${it.optString("reason_code")} representation=${it.optString("representation")} reason=${it.optString("reason")} result_saved=${it.has("text")}")
                if (it.has("diagnostic")) writer.println("Writer latest delivery diagnostic: ${it.optString("diagnostic")}")
                // Allowlisted persisted monotonic durations only; never dump arbitrary record fields.
                it.optJSONObject("timing")?.let { timing ->
                    for (key in listOf("stop_to_result_ms", "stop_to_delivery_ms")) {
                        if (timing.has(key) && timing.optLong(key, -1) >= 0)
                            writer.println("Writer delivery timing: $key=${timing.getLong(key)}")
                    }
                }
            }
        }
        if (::bubble.isInitialized) writer.println("Writer overlay check: ${bubble.diagnostic}")
    }
    private fun unavailable(reason: String): AccessibilityNodeInfo? { editorCheck = reason; return null }
    private var unsupported: AccessibilityNodeInfo? = null
    private var eventEditor: AccessibilityNodeInfo? = null
    private data class Snapshot(val node: AccessibilityNodeInfo, val text: String, val start: Int, val end: Int,
        val offset: Int, val complete: Boolean, val fieldId: Int, val packageName: String, val windowId: Int,
        val representation: String, val matching: AccessibilityNodeInfo?, val nodeTextLength: Int, val selectionCorroborated: Boolean)

    override fun onCreateInputMethod(): InputMethod = EditorInput().also { editorInput = it }
    private inner class EditorInput : InputMethod(this@WriterInputService) {
        override fun onStartInput(attribute: EditorInfo, restarting: Boolean) {
            super.onStartInput(attribute, restarting); invalidate(); composing = false
        }
        override fun onFinishInput() { invalidate(); composing = false; super.onFinishInput() }
        override fun onUpdateSelection(oldSelStart: Int, oldSelEnd: Int, newSelStart: Int, newSelEnd: Int,
            candidatesStart: Int, candidatesEnd: Int) {
            composing = candidatesStart >= 0 && candidatesEnd >= 0
            if (oldSelStart != newSelStart || oldSelEnd != newSelEnd || composing) invalidate()
        }
    }
    override fun onServiceConnected() {
        super.onServiceConnected()
        deliveries = DeliveryStore(File(filesDir, "deliveries"))
        bubble = WriterBubble(this) {
            if (EditorFixture.enabled) EditorFixture.start(this)
            else if (CaptureService.isActive()) CaptureService.stop(this, false)
            else if (!CaptureService.start(this, "bubble")) bubble.block(CaptureService.message)
        }
        connected = true; instance = this
        EditorFixture.register(this)
        if (!consent.accepted) Toast.makeText(this, "$PRODUCT_NAME text insertion is paused. Open $PRODUCT_NAME → Settings → Permissions → Text insertion to review it.", Toast.LENGTH_LONG).show()
        handler.post(tick)
    }
    fun refreshControl() { if (connected) { handler.removeCallbacks(tick); handler.post(tick) } }
    private val tick = object : Runnable {
        override fun run() {
            if (!connected) return
            val node = eligibleNode()
            if (node == null) invalidate()
            val keyboard = windows.firstOrNull { it.type == android.view.accessibility.AccessibilityWindowInfo.TYPE_INPUT_METHOD }
            val keyboardBounds = android.graphics.Rect()
            keyboard?.getBoundsInScreen(keyboardBounds)
            val manager = SessionManager.get(this@WriterInputService)
            val capture = manager.capture
            val captureVisible = consent.accepted && CaptureService.isActive() && Settings.canDrawOverlays(this@WriterInputService) &&
                getSystemService(android.os.PowerManager::class.java).isInteractive &&
                !getSystemService(android.app.KeyguardManager::class.java).isKeyguardLocked
            controlCheck = "capture=$captureVisible composing=$composing unsupported=${node != null && node == unsupported} keyboardTop=${if (keyboard != null) keyboardBounds.top else null}"
            bubble.show(captureVisible || node != null && node != unsupported && !composing,
                verifying,
                if (keyboard != null) keyboardBounds.top else null,
                if (EditorFixture.enabled) "idle" else if (CaptureService.state != "idle") CaptureService.state else if (manager.state == "processing" && !manager.isBusy()) "idle" else manager.state, capture.level)
            handler.postDelayed(this, 300)
        }
    }
    override fun onAccessibilityEvent(event: AccessibilityEvent) {
        if (!consent.accepted) return // Nothing is read before the in-app disclosure is accepted.
        if (event.eventType == AccessibilityEvent.TYPE_VIEW_FOCUSED ||
            event.eventType == AccessibilityEvent.TYPE_VIEW_TEXT_SELECTION_CHANGED) {
            // Rich WebViews can emit selection events from a text child of the focused
            // editor. Inspect only that source's ancestry, never other screen content.
            var source = event.source
            for (depth in 0 until 32) {
                val node = source ?: break
                if (node.windowId != event.windowId || node.packageName?.toString() != event.packageName?.toString()) break
                if (node.isEditable && node.isFocused) { eventEditor = node; break }
                source = node.parent
            }
        }
        when (event.eventType) {
            AccessibilityEvent.TYPE_VIEW_TEXT_CHANGED, AccessibilityEvent.TYPE_VIEW_TEXT_SELECTION_CHANGED,
            AccessibilityEvent.TYPE_VIEW_FOCUSED, AccessibilityEvent.TYPE_WINDOW_STATE_CHANGED,
            AccessibilityEvent.TYPE_WINDOWS_CHANGED -> invalidate()
        }
    }
    /** Editor state moved on: re-evaluate an editor that previously failed verification. */
    private fun invalidate() { unsupported = null }
    override fun onInterrupt() { invalidate(); if (::bubble.isInitialized) bubble.hide() }
    override fun onDestroy() {
        connected = false; instance = null; invalidate(); handler.removeCallbacksAndMessages(null)
        EditorFixture.unregister(this)
        if (::bubble.isInitialized) bubble.release()
        super.onDestroy()
    }

    private fun sensitive(type: Int): Boolean {
        val variation = type and InputType.TYPE_MASK_VARIATION
        return type and InputType.TYPE_MASK_CLASS != InputType.TYPE_CLASS_TEXT ||
            variation in setOf(InputType.TYPE_TEXT_VARIATION_PASSWORD, InputType.TYPE_TEXT_VARIATION_VISIBLE_PASSWORD,
                InputType.TYPE_TEXT_VARIATION_WEB_PASSWORD)
    }
    private fun eligibleNode(): AccessibilityNodeInfo? {
        if (!connected) return unavailable("service_not_connected")
        if (!consent.accepted) return unavailable("consent_not_accepted")
        if (!Settings.canDrawOverlays(this)) return unavailable("overlay_not_allowed")
        if (!::editorInput.isInitialized) return unavailable("input_method_not_created")
        if (!getSystemService(android.os.PowerManager::class.java).isInteractive) return unavailable("screen_not_interactive")
        if (getSystemService(android.app.KeyguardManager::class.java).isKeyguardLocked) return unavailable("screen_locked")
        val info = editorInput.currentInputEditorInfo ?: return unavailable("editor_info_missing")
        if (!editorInput.currentInputStarted) return unavailable("input_not_started")
        // Focus outside any text field still starts input with TYPE_NULL: nothing to insert into.
        if (info.inputType == InputType.TYPE_NULL) return unavailable("no_text_editor")
        if (sensitive(info.inputType)) return unavailable("editor_type_excluded")
        if (info.imeOptions and EditorInfo.IME_FLAG_NO_PERSONALIZED_LEARNING != 0) return unavailable("private_editor")
        if (info.packageName == packageName) return unavailable("writer_own_editor")
        val window = windows.firstOrNull { it.isFocused && it.type == android.view.accessibility.AccessibilityWindowInfo.TYPE_APPLICATION }
            ?: return unavailable("focused_app_window_missing")
        val focus = window.root?.findFocus(AccessibilityNodeInfo.FOCUS_INPUT) ?: return unavailable("focused_node_missing")
        // WebView's native host can own FOCUS_INPUT while its focused editor is a virtual event source.
        val node = if (focus.isEditable) focus else {
            if (focus.className?.toString() != "android.webkit.WebView") return unavailable("focused_node_not_editable")
            eventEditor?.takeIf { it.refresh() && it.windowId == window.id && it.isFocused }
                ?: return unavailable("web_editor_event_missing")
        }
        if (!node.refresh() || !node.isEditable || !node.isFocused || !node.isEnabled || !node.isVisibleToUser) return unavailable("node_not_ready")
        if (node.isPassword || (node.inputType != InputType.TYPE_NULL && sensitive(node.inputType))) return unavailable("node_type_excluded")
        if (node.packageName?.toString() != info.packageName) return unavailable("editor_package_mismatch")
        if (android.os.Build.VERSION.SDK_INT >= 34 && node.isAccessibilityDataSensitive) return unavailable("node_sensitive")
        if (editorInput.currentInputConnection == null) return unavailable("input_connection_missing")
        editorCheck = "eligible"
        return node
    }
    // HTML accessibility text can collapse blank paragraph separators. Only the connection may
    // contain extra LFs in an existing LF run; all other characters and their order must agree.
    private fun paragraphTextAgrees(node: String, connection: String): Boolean {
        var i = 0; var j = 0
        while (i < node.length && j < connection.length) {
            if (node[i] != connection[j]) return false
            if (node[i] == '\n') {
                val ni = i; val cj = j
                while (i < node.length && node[i] == '\n') i++
                while (j < connection.length && connection[j] == '\n') j++
                if (j - cj < i - ni) return false
            } else { i++; j++ }
        }
        return i == node.length && j == connection.length
    }
    /**
     * Characters block editors represent differently between accessibility and the connection (S25
     * Notion): paragraph line feeds and embedded-object markers (inline emoji/mentions). All else must match.
     */
    private fun blockContent(value: String) = value.filter { it != '\n' && it != '\uFFFC' }
    private fun kinds(value: String): String = value.map { when (it) {
        '\uFFFC' -> "object_marker"; '\n' -> "line_feed"; '\r' -> "carriage_return"; '\t' -> "tab"; '\u00A0' -> "nbsp"
        else -> if (it.isWhitespace()) "space" else if (Character.getType(it) == Character.FORMAT.toInt()) "format" else "other"
    } }.groupingBy { it }.eachCount().toString()
    private fun kind(c: Char) = when (c) { '\n' -> "lf"; '\uFFFC' -> "obj"; '\u00A0' -> "nbsp"
        else -> if (c.isWhitespace()) "sp" else if (Character.getType(c) == Character.FORMAT.toInt()) "fmt" else "o" }
    /** Content-free LCS edit script from a to b (kinds and a-positions only) and where a's cursor maps in b. */
    private fun alignment(a: String, aCursor: Int, b: String): String {
        if (a.length > 600 || b.length > 600) return "too_large"
        val lcs = Array(a.length + 1) { IntArray(b.length + 1) }
        for (i in a.length - 1 downTo 0) for (j in b.length - 1 downTo 0)
            lcs[i][j] = if (a[i] == b[j]) lcs[i + 1][j + 1] + 1 else maxOf(lcs[i + 1][j], lcs[i][j + 1])
        val ops = mutableListOf<String>(); var i = 0; var j = 0; var mapped = -1
        while (i < a.length || j < b.length) {
            if (i == aCursor && mapped < 0) mapped = j
            when {
                i < a.length && j < b.length && a[i] == b[j] -> { i++; j++ }
                j < b.length && (i == a.length || lcs[i][j + 1] >= lcs[i + 1][j]) -> { ops += "+${kind(b[j])}@$i"; j++ }
                else -> { ops += "-${kind(a[i])}@$i"; i++ }
            }
        }
        if (aCursor == a.length && mapped < 0) mapped = b.length
        return "ops=${ops.take(24).joinToString(",")}${if (ops.size > 24) ",+" + (ops.size - 24) else ""} a_cursor_maps_to=$mapped"
    }
    /** Content-free difference between two editor representations: shared ends, differing kinds, counts before each cursor. */
    private fun difference(a: String, aCursor: Int, b: String, bCursor: Int): String {
        val prefix = a.commonPrefixWith(b).length
        val suffix = a.drop(prefix).commonSuffixWith(b.drop(prefix)).length
        fun before(text: String, cursor: Int) = if (cursor in 0..text.length) kinds(text.substring(0, cursor)) else "unknown"
        return "prefix=$prefix suffix=$suffix a_diff=${kinds(a.substring(prefix, a.length - suffix))} b_diff=${kinds(b.substring(prefix, b.length - suffix))} " +
            "a_before_cursor=${before(a, aCursor)} b_before_cursor=${before(b, bCursor)}"
    }
    /** Content-free shape of the focused node's same-window ancestors: class, editable, focused, length, agreement. */
    private fun ancestry(node: AccessibilityNodeInfo, connection: String, connectionCursor: Int): String {
        val parts = mutableListOf<String>()
        val started = android.os.SystemClock.elapsedRealtime()
        var current = node.parent
        repeat(12) {
            val n = current ?: return@repeat
            // WebView ancestors are cross-process reads; keep shell DUMP within its transfer timeout.
            if (android.os.SystemClock.elapsedRealtime() - started > 1500) { parts += "budget"; current = null; return@repeat }
            if (n.windowId != node.windowId || n.packageName?.toString() != node.packageName?.toString()) { current = null; return@repeat }
            val text = if (n.isShowingHintText) "" else n.text?.toString().orEmpty()
            val agrees = when { text.isEmpty() -> "-"; text == connection -> "exact"; paragraphTextAgrees(text, connection) -> "paragraphs"; else -> "no" }
            parts += "${n.className?.toString()?.substringAfterLast('.')}/e=${n.isEditable}/f=${n.isFocused}/len=${text.length}/sel=${n.textSelectionStart}:${n.textSelectionEnd}/agree=$agrees" +
                if (agrees == "no" && n.isEditable) "/{${difference(text, n.textSelectionStart, connection, connectionCursor)} ${alignment(text, n.textSelectionStart, connection)}}" else ""
            current = n.parent
        }
        return parts.joinToString(">") + " ancestry_ms=${android.os.SystemClock.elapsedRealtime() - started}"
    }
    private fun rejectSnapshot(reason: String): Snapshot? { snapshotCheck = reason; return null }
    /**
     * The keyboard's input connection is authoritative: it is the focused field Android routes typing
     * to (Insert Anywhere). Accessibility supplies focus, window and sensitivity, but rich editors
     * legitimately represent the same text differently (S25 Notion WebView: identical characters,
     * different line-feed placement and caret coordinates), so representation agreement is recorded,
     * never required. Insertion is verified only by reading the connection back.
     */
    /** Main thread: "eligible" when a focused editor can receive text now, else the reason code (instrumentation waits on it). */
    internal fun editorEligibility(): String = if (snapshot() != null) "eligible" else snapshotCheck
    private fun snapshot(detail: Boolean = false): Snapshot? {
        val node = eligibleNode() ?: return rejectSnapshot("editor_unavailable:$editorCheck")
        if (composing) return rejectSnapshot("composition_active")
        val nodeText = if (node.isShowingHintText) "" else node.text?.toString().orEmpty()
        val connection = editorInput.currentInputConnection ?: return rejectSnapshot("connection_missing")
        val around = connection.getSurroundingText(CONTEXT, CONTEXT, InputConnection.GET_TEXT_WITH_STYLES)
            ?: return rejectSnapshot("connection_read_failed")
        val text = around.text ?: return rejectSnapshot("connection_text_missing")
        if (text is Spanned && text.getSpans(0, text.length, Any::class.java).any {
                text.getSpanFlags(it) and Spanned.SPAN_COMPOSING != 0 }) return rejectSnapshot("composition_span_present")
        val start = around.selectionStart; val end = around.selectionEnd
        if (start !in 0..text.length || end !in 0..text.length) return rejectSnapshot("selection_range_invalid")
        if (!boundary(text, start) || !boundary(text, end)) return rejectSnapshot("selection_splits_codepoint")
        val inputText = text.toString()
        // Complete means the window holds the whole field: known start and nothing cut after the selection.
        val complete = around.offset == 0 && inputText.length - maxOf(start, end) < CONTEXT
        val info = editorInput.currentInputEditorInfo
        val (representation, matching) = representation(node, nodeText, inputText, complete)
        snapshotComparison = if (!detail || representation == "exact") representation else comparison(node, nodeText, inputText, start, end, around.offset)
        snapshotCheck = "verified"
        return Snapshot(node, inputText, start, end, around.offset, complete, info?.fieldId ?: -1, info?.packageName.orEmpty(),
            node.windowId, representation, matching, nodeText.length,
            node.textSelectionStart < 0 || representation != "exact" || (node.textSelectionStart == start && node.textSelectionEnd == end))
    }
    /** Content-free classification of how accessibility text relates to the connection text. */
    private fun representation(node: AccessibilityNodeInfo, nodeText: String, connection: String, complete: Boolean): Pair<String, AccessibilityNodeInfo?> {
        if (nodeText == connection) return "exact" to node
        if (!complete) return "windowed" to null
        fun lineFeedsOnly(a: String) = a.isNotEmpty() && blockContent(a) == blockContent(connection)
        if (nodeText.isNotEmpty() && paragraphTextAgrees(nodeText, connection)) return "paragraphs" to node
        if (lineFeedsOnly(nodeText)) return "line_feeds" to node
        // Chromium rich editors can focus a child while the editable root holds the connection text.
        val started = android.os.SystemClock.elapsedRealtime()
        var current = node.parent
        repeat(12) {
            val n = current ?: return@repeat
            if (android.os.SystemClock.elapsedRealtime() - started > 300 || n.windowId != node.windowId ||
                n.packageName?.toString() != node.packageName?.toString()) { current = null; return@repeat }
            if (n.isEditable) {
                val text = if (n.isShowingHintText) "" else n.text?.toString().orEmpty()
                if (text == connection) return "ancestor_exact" to n
                if (lineFeedsOnly(text)) return "ancestor_line_feeds" to n
            }
            current = n.parent
        }
        return (if (nodeText.isEmpty()) "node_empty" else "differs") to null
    }
    private fun comparison(node: AccessibilityNodeInfo, nodeText: String, inputText: String, start: Int, end: Int, offset: Int): String {
        val prefix = nodeText.commonPrefixWith(inputText).length
        val suffix = nodeText.drop(prefix).commonSuffixWith(inputText.drop(prefix)).length
        return "prefix=$prefix suffix=$suffix node_difference=${kinds(nodeText.substring(prefix, nodeText.length - suffix))} " +
            "connection_difference=${kinds(inputText.substring(prefix, inputText.length - suffix))} " +
            "node_length=${nodeText.length} connection_length=${inputText.length} node_hint=${node.isShowingHintText} " +
            "node_selection=${node.textSelectionStart}:${node.textSelectionEnd} connection_selection=$start:$end offset=$offset " +
            "node_class=${node.className?.toString()?.substringAfterLast('.')} node_children=${node.childCount} " +
            "input_variation=${Integer.toHexString((editorInput.currentInputEditorInfo?.inputType ?: 0) and InputType.TYPE_MASK_VARIATION)} " +
            "field_id_positive=${(editorInput.currentInputEditorInfo?.fieldId ?: 0) > 0} node_kinds=${kinds(nodeText)} " +
            "ancestry=${ancestry(node, inputText, start)}"
    }
    private fun boundary(text: CharSequence, index: Int) = index == 0 || index == text.length ||
        !(Character.isHighSurrogate(text[index - 1]) && Character.isLowSurrogate(text[index]))
    /**
     * Exact readback of [inserted] at the original selection. A complete field must equal [expected]
     * with the caret after the insertion. A windowed (long) field must show the insertion ending at
     * the caret, with the visible text on both sides identical to the original surroundings and, when
     * both offsets are known, the same absolute caret position.
     */
    private fun readback(before: Snapshot, after: Snapshot, inserted: String): Boolean {
        val lo = minOf(before.start, before.end); val hi = maxOf(before.start, before.end)
        val cursor = lo + inserted.length
        if (after.start != after.end) return false
        if (before.complete && after.complete)
            return after.text == before.text.substring(0, lo) + inserted + before.text.substring(hi) && after.start == cursor
        val offsetsAgree = before.offset < 0 || after.offset < 0 || after.offset + after.start == before.offset + cursor
        // Windowed: the visible text before the caret must end with the original prefix plus the insertion
        // (only its tail when the insertion exceeds the window) and the visible text after must begin
        // with the original suffix.
        val expectedBefore = before.text.substring(0, lo) + inserted; val expectedAfter = before.text.substring(hi)
        val visibleBefore = after.text.substring(0, after.start); val visibleAfter = after.text.substring(after.start)
        val left = minOf(visibleBefore.length, expectedBefore.length); val right = minOf(visibleAfter.length, expectedAfter.length)
        // Require real surrounding context, not an accidental match in a nearly empty window.
        val matches = left >= minOf(expectedBefore.length, inserted.length + READBACK_CONTEXT, CONTEXT) &&
            right >= minOf(expectedAfter.length, READBACK_CONTEXT) &&
            visibleBefore.takeLast(left) == expectedBefore.takeLast(left) && visibleAfter.take(right) == expectedAfter.take(right)
        if (offsetsAgree) return matches
        // The S25 Notion page counts absolute offsets differently once it outgrows the window (MW-ANDROID-06).
        // Without agreeing offsets, accept only the whole insertion plus original context before it, exactly.
        return matches && left >= minOf(expectedBefore.length, inserted.length + READBACK_CONTEXT)
    }
    /**
     * Block editors such as the S25 Notion page end keyboard input when a line feed splits a paragraph, so
     * no connection is left to read. The accessibility node that matched before the edit then proves it only
     * when it holds exactly the original text with the insertion at the original selection, in order, apart
     * from block separators. A windowed field, a different window or any other difference stays unconfirmed.
     */
    private fun closedReadback(before: Snapshot, inserted: String): Boolean {
        val node = before.matching ?: return false
        if (!before.complete || blockContent(inserted).isEmpty() || !node.refresh() || node.windowId != before.windowId || node.isShowingHintText) return false
        val lo = minOf(before.start, before.end); val hi = maxOf(before.start, before.end)
        return blockContent(node.text?.toString().orEmpty()) == blockContent(before.text.substring(0, lo) + inserted + before.text.substring(hi))
    }
    /** Content-free summary of a failed readback for the ledger. */
    private fun readbackDiagnostic(before: Snapshot, after: Snapshot?, inserted: String): String {
        // Input can end after an edit (S25 Notion closes it when a line feed splits a paragraph); then compare
        // the accessibility text that matched before, ignoring the block representation characters.
        if (after == null) return "after=$snapshotCheck " + (before.matching?.let { node ->
            val lo = minOf(before.start, before.end); val hi = maxOf(before.start, before.end)
            val refreshed = node.refresh()
            val text = if (node.isShowingHintText) "" else node.text?.toString().orEmpty()
            "closed_refresh=$refreshed closed_window_same=${node.windowId == before.windowId} closed_complete=${before.complete} " +
                difference(blockContent(before.text.substring(0, lo) + inserted + before.text.substring(hi)), -1, blockContent(text), -1)
        } ?: "no_matching_node")
        val lo = minOf(before.start, before.end); val hi = maxOf(before.start, before.end)
        val expected = before.text.substring(0, lo) + inserted + before.text.substring(hi)
        return "window_same=${after.windowId == before.windowId} field_same=${after.fieldId == before.fieldId && after.packageName == before.packageName} " +
            "complete=${before.complete}:${after.complete} offset=${before.offset}:${after.offset} expected_cursor=${lo + inserted.length} " +
            "after_selection=${after.start}:${after.end} " + difference(expected, lo + inserted.length, after.text, after.start)
    }
    /**
     * Call on the main thread when capture begins. Opens a delivery ledger whether or not an editor is
     * focused now; the opaque token is the originalTarget in TextDelivery. Eligibility is decided at delivery.
     */
    /** [inApp]: recorded on Writer's Home, so no focused field at delivery is a normal save, not a failure. */
    fun bind(sessionId: String, fixture: Boolean = false, inApp: Boolean = false): String? {
        check(Looper.myLooper() == Looper.getMainLooper())
        if (!connected) return null
        invalidate()
        lastBindCheck = if (snapshot() == null) snapshotCheck else "verified"
        if (lastBindCheck != "verified") unsupported = eligibleNode()
        val token = UUID.randomUUID().toString()
        deliveries.create(sessionId, token, fixture)
        if (inApp) deliveries.save(deliveries.load(token).put("in_app", true))
        return token
    }

    @Volatile var lastManualCheck = "not_called"
        private set
    /**
     * Explicit "Insert here" / "Insert anyway" for a saved result (U-3, D-5): types it into the field
     * focused now, through the same commit + read-back as [deliver]. Never automatic, never Send.
     * Refuses a verified ledger, an unconfirmed one unless [anyway], paused consent, a busy
     * verification and any field [deliver] would refuse. [lastManualCheck] holds the content-free reason.
     */
    fun insertSaved(sessionId: String, anyway: Boolean): DeliveryResult {
        check(Looper.myLooper() == Looper.getMainLooper())
        fun refuse(code: String): DeliveryResult { lastManualCheck = code; return DeliveryResult.UNSUPPORTED }
        val token = try { SessionStore(File(filesDir, "recordings")).load(sessionId).optString("original_target") } catch (_: Exception) { return refuse("session_unavailable") }
        val item = try { deliveries.load(token) } catch (_: Exception) { return refuse("ledger_unavailable") }
        if (item.getString("session_id") != sessionId || !item.has("text")) return refuse("nothing_saved")
        if (item.optString("state") == "delivered") { lastManualCheck = "already_delivered"; return DeliveryResult.DELIVERED }
        if (item.optString("state") == "unconfirmed" && !anyway) return refuse("unconfirmed_needs_anyway")
        if (!consent.accepted) return refuse("insertion_paused")
        if (verifying) return refuse("insertion_busy")
        val now = snapshot() ?: return refuse(snapshotCheck.removePrefix("editor_unavailable:"))
        val connection = editorInput.currentInputConnection ?: return refuse("connection_missing")
        item.put("manual_insert_at_ms", System.currentTimeMillis()).put("manual_insert_anyway", anyway)
        lastManualCheck = "issued"
        return commitAndVerify(sessionId, item, item.getString("text"), now, connection, manual = true)
    }

    /** Persist before attempting. PENDING also means asynchronous readback is in progress; consult the ledger. */
    override fun deliver(sessionId: String, text: String, originalTarget: String): DeliveryResult {
        check(Looper.myLooper() == Looper.getMainLooper())
        val item = try { deliveries.load(originalTarget) } catch (_: Exception) { return DeliveryResult.UNSUPPORTED }
        if (item.getString("session_id") != sessionId) return DeliveryResult.UNSUPPORTED
        if (item.has("text")) return if (item.optString("state") == "delivered") DeliveryResult.DELIVERED
            else DeliveryResult.PENDING_ORIGINAL_TARGET // Never execute a duplicate, including uncertain outcomes.
        item.put("text", text).markOutcome(DeliveryOutcome.SAVED_NOT_INSERTED, "no_focused_editor", "No insertable editor was focused")
        deliveries.save(item)
        val now = snapshot()
        val connection = editorInput.currentInputConnection
        val rejection: Triple<DeliveryOutcome, String, String>? = when {
            text.isEmpty() -> Triple(DeliveryOutcome.SAVED_NOT_INSERTED, "empty_result", "Empty result")
            !consent.accepted -> Triple(DeliveryOutcome.SAVED_NOT_INSERTED, "insertion_paused", "Text insertion is paused until it is accepted in $PRODUCT_NAME")
            verifying -> Triple(DeliveryOutcome.SAVED_NOT_INSERTED, "insertion_busy", "Another insertion is still being verified")
            now == null -> snapshotCheck.removePrefix("editor_unavailable:").let { code ->
                Triple(if (code in UNSUPPORTED_EDITORS) DeliveryOutcome.UNSUPPORTED else DeliveryOutcome.SAVED_NOT_INSERTED, code,
                    if (code in UNSUPPORTED_EDITORS) "The focused field does not accept $PRODUCT_NAME insertion ($code)" else "No insertable editor was focused ($snapshotCheck)")
            }
            connection == null -> Triple(DeliveryOutcome.SAVED_NOT_INSERTED, "connection_missing", "Editor connection unavailable")
            else -> null
        }
        // Home has no target field (MW-ANDROID-02): with no field focused, the text is simply saved in Writer.
        if (item.optBoolean("in_app") && now == null && rejection?.second in setOf("no_text_editor", "editor_info_missing", "writer_own_editor",
                "focused_app_window_missing", "focused_node_missing", "focused_node_not_editable")) {
            item.markOutcome(DeliveryOutcome.SAVED_NOT_INSERTED, "in_app", "Recorded in $PRODUCT_NAME; no text field was focused")
                .put("handled_at_ms", System.currentTimeMillis())
            deliveries.save(item); return DeliveryResult.PENDING_ORIGINAL_TARGET
        }
        if (rejection != null || now == null || connection == null) {
            rejection?.let { item.markOutcome(it.first, it.second, it.third) }; deliveries.save(item)
            recoveryNotice(item); return DeliveryResult.PENDING_ORIGINAL_TARGET
        }
        return commitAndVerify(sessionId, item, text, now, connection, manual = false)
    }

    /**
     * The one commit + read-back path, shared by automatic delivery and explicit "Insert here". The
     * ledger is marked unconfirmed before the edit, so a crash can never reissue it. [manual] keeps
     * delivery timing out of an explicit insert (it measures a person's choice, not latency).
     */
    private fun commitAndVerify(sessionId: String, item: org.json.JSONObject, text: String, now: Snapshot, connection: android.accessibilityservice.InputMethod.AccessibilityInputConnection, manual: Boolean): DeliveryResult {
        val lo = minOf(now.start, now.end); val hi = maxOf(now.start, now.end)
        // A length-limited editor would silently truncate; use the longest known field length.
        val fieldLength = maxOf(now.nodeTextLength, now.text.length)
        if (now.node.maxTextLength >= 0 && fieldLength - (hi - lo) + text.length > now.node.maxTextLength) {
            // An explicit insert into a too-short field changes nothing; the saved outcome stays as it was.
            if (manual) { lastManualCheck = "text_limit"; return DeliveryResult.UNSUPPORTED }
            item.markOutcome(DeliveryOutcome.UNSUPPORTED, "text_limit", "Result exceeds the editor text limit"); deliveries.save(item); recoveryNotice(item)
            return DeliveryResult.PENDING_ORIGINAL_TARGET
        }
        verifying = true
        item.markOutcome(DeliveryOutcome.UNCONFIRMED, "readback_pending", "Check the focused field before copying; insertion is unconfirmed")
            .put("representation", now.representation).put("selection_corroborated", now.selectionCorroborated)
        deliveries.save(item) // A crash from here cannot reissue the edit.
        if (!manual) item.optJSONObject("timing")?.put("insertion_started_elapsed_ms", android.os.SystemClock.elapsedRealtime())
        fun unconfirmed(code: String, diagnostic: String?) {
            verifying = false
            item.markOutcome(DeliveryOutcome.UNCONFIRMED, code, "Check the focused field before copying; insertion is unconfirmed")
            diagnostic?.let { item.put("diagnostic", it) }
            try { deliveries.save(item) } catch (_: Exception) { }
            recoveryNotice(item)
        }
        // AccessibilityInputConnection.commitText reports nothing; only readback can confirm the edit.
        try { connection.commitText(text, 1, null) }
        catch (_: Exception) { unconfirmed("commit_failed", null); return DeliveryResult.PENDING_ORIGINAL_TARGET }
        fun verify(remaining: Int) {
            val after = snapshot()
            val closed = after == null && closedReadback(now, text)
            if (closed || after != null && after.windowId == now.windowId && after.fieldId == now.fieldId &&
                after.packageName == now.packageName && readback(now, after, text)) {
                if (closed) item.markOutcome(DeliveryOutcome.INSERTED_VERIFIED, "readback_closed_exact", "Exact text read back after the editor ended input")
                else item.markOutcome(DeliveryOutcome.INSERTED_VERIFIED, "readback_exact", "Exact text and cursor read back")
                if (!manual) item.optJSONObject("timing")?.let { timing ->
                    val deliveredAt = android.os.SystemClock.elapsedRealtime()
                    timing.put("delivery_elapsed_ms", deliveredAt)
                    val sameBoot = timing.optInt("boot_count", -1) == android.provider.Settings.Global.getInt(contentResolver, android.provider.Settings.Global.BOOT_COUNT, -2)
                    if (sameBoot && timing.has("capture_stop_elapsed_ms")) timing.put("stop_to_delivery_ms", deliveredAt - timing.getLong("capture_stop_elapsed_ms"))
                    if (!sameBoot) { timing.remove("stop_to_delivery_ms"); timing.remove("insertion_started_elapsed_ms") }
                }
                try { deliveries.save(item) } catch (_: Exception) { verifying = false; recoveryNotice(item); return }
                verifying = false
                if (!manual) SessionManager.get(this).deliveryVerified(sessionId)
                RecoveryNotification.cancel(this, sessionId)
                refreshControl()
            } else if (remaining > 0) {
                handler.postDelayed({ verify(remaining - 1) }, 100)
            } else unconfirmed(if (after == null) "readback_unavailable" else if (after.windowId != now.windowId ||
                after.fieldId != now.fieldId || after.packageName != now.packageName) "editor_changed" else "readback_mismatch",
                readbackDiagnostic(now, after, text))
        }
        // Allow ~2 s for editors that re-render or restart input after an edit.
        handler.postDelayed({ verify(VERIFY_ATTEMPTS) }, 100)
        return DeliveryResult.PENDING_ORIGINAL_TARGET
    }

    /** Persistent recovery notification (D-4) for the ledger [item]; test fixtures stay silent. */
    private fun recoveryNotice(item: org.json.JSONObject) {
        if (item.optBoolean("fixture")) return
        RecoveryNotification.post(this, item.getString("session_id"), item.optString("state") == "unconfirmed", deliveryReason(item.optString("reason_code")))
    }
    companion object {
        private const val CONTEXT = 4096
        /** Minimum visible original context on each side for a windowed (long-field) readback. */
        private const val READBACK_CONTEXT = 256
        private const val VERIFY_ATTEMPTS = 20
        /** Focused fields Writer recognizes but never inserts into or cannot read back. */
        private val UNSUPPORTED_EDITORS = setOf("editor_type_excluded", "private_editor", "writer_own_editor",
            "node_type_excluded", "node_sensitive", "connection_read_failed", "connection_text_missing",
            "selection_range_invalid", "selection_splits_codepoint")
        @Volatile var instance: WriterInputService? = null
            private set
    }
}
