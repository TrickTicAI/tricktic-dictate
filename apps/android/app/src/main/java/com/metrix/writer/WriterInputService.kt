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
import java.io.File
import java.util.UUID

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
        writer.println("Writer editor check: $editorCheck")
        writer.println("Writer control check: $controlCheck")
        writer.println("Writer last bind check: $lastBindCheck")
        if (Looper.myLooper() == Looper.getMainLooper()) {
            snapshot()
            writer.println("Writer current snapshot probe: $snapshotCheck")
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
                writer.println("Writer latest delivery: state=${it.optString("state")} reason=${it.optString("reason")} result_saved=${it.has("text")}")
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
    private data class Snapshot(val node: AccessibilityNodeInfo, val text: String, val start: Int, val end: Int, val fieldId: Int)

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
            else if (!CaptureService.start(this, "bubble")) Toast.makeText(this, CaptureService.message, Toast.LENGTH_LONG).show()
        }
        connected = true; instance = this
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
        if (::bubble.isInitialized) bubble.hide()
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
    private fun webEditor(node: AccessibilityNodeInfo): Boolean {
        if (node.viewIdResourceName.isNullOrEmpty()) return false
        var parent = node.parent
        repeat(32) {
            val current = parent ?: return false
            if (current.windowId != node.windowId || current.packageName?.toString() != node.packageName?.toString()) return false
            if (current.className?.toString() == "android.webkit.WebView") return true
            parent = current.parent
        }
        return false
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
    private fun rejectSnapshot(reason: String): Snapshot? { snapshotCheck = reason; return null }
    private fun snapshot(): Snapshot? {
        val node = eligibleNode() ?: return rejectSnapshot("editor_unavailable:$editorCheck")
        if (composing) return rejectSnapshot("composition_active")
        val nodeText = if (node.isShowingHintText) "" else node.text?.toString().orEmpty()
        if (nodeText.length > CONTEXT * 2) return rejectSnapshot("context_too_large")
        val connection = editorInput.currentInputConnection ?: return rejectSnapshot("connection_missing")
        val around = connection.getSurroundingText(CONTEXT, CONTEXT, InputConnection.GET_TEXT_WITH_STYLES)
            ?: return rejectSnapshot("connection_read_failed")
        val text = around.text ?: return rejectSnapshot("connection_text_missing")
        if (text is Spanned && text.getSpans(0, text.length, Any::class.java).any {
                text.getSpanFlags(it) and Spanned.SPAN_COMPOSING != 0 }) return rejectSnapshot("composition_span_present")
        // Some editors omit accessibility selection. The active input connection is authoritative,
        // but only with full exact text agreement for this same focused node. Corroborate each node
        // selection endpoint when supplied; missing node metadata never permits truncated text.
        val start = around.selectionStart; val end = around.selectionEnd
        if (around.offset !in -1..0) return rejectSnapshot("context_offset_incomplete")
        if (text.toString() != nodeText) {
            val inputText = text.toString()
            fun spaces(value: String) = value.map { if (it.isWhitespace()) ' ' else it }.joinToString("")
            fun formats(value: String) = value.filterNot { Character.getType(it) == Character.FORMAT.toInt() }
            val prefix = nodeText.commonPrefixWith(inputText).length
            val suffix = nodeText.drop(prefix).commonSuffixWith(inputText.drop(prefix)).length
            fun kinds(value: String): String = value.map { when (it) {
                '\uFFFC' -> "object_marker"; '\n' -> "line_feed"; '\r' -> "carriage_return"; '\t' -> "tab"; '\u00A0' -> "nbsp"
                else -> if (it.isWhitespace()) "space" else if (Character.getType(it) == Character.FORMAT.toInt()) "format" else "other"
            } }.groupingBy { it }.eachCount().toString()
            snapshotComparison = "prefix=$prefix suffix=$suffix node_difference=${kinds(nodeText.substring(prefix,nodeText.length-suffix))} " +
                "connection_difference=${kinds(inputText.substring(prefix,inputText.length-suffix))} " +
                "node_length=${nodeText.length} connection_length=${inputText.length} node_hint=${node.isShowingHintText} " +
                "node_selection=${node.textSelectionStart}:${node.textSelectionEnd} connection_selection=$start:$end offset=${around.offset} " +
                "same_trimmed=${nodeText.trim() == inputText.trim()} same_spaces=${spaces(nodeText) == spaces(inputText)} " +
                "same_without_format=${formats(nodeText) == formats(inputText)} node_starts_connection=${nodeText.startsWith(inputText)} " +
                "connection_starts_node=${inputText.startsWith(nodeText)} node_resource_present=${node.viewIdResourceName != null}"
            // Some native editors expose placeholder/description text without marking it as a hint.
            // Accept only an explicitly empty full input connection at 0:0, with omitted node
            // selection and the same positive native field resource ID. Never ignore non-empty
            // connection text, an unknown offset, or a different/unidentifiable field.
            val info = editorInput.currentInputEditorInfo
            val emptyFieldCheck = when {
                text.isNotEmpty() -> "connection_not_empty"
                around.offset != 0 -> "empty_offset_unknown"
                start != 0 || end != 0 -> "empty_selection_nonzero"
                node.textSelectionStart != -1 || node.textSelectionEnd != -1 -> "node_selection_present"
                info == null || info.fieldId <= 0 -> "native_field_id_missing"
                else -> try {
                    val name = node.viewIdResourceName
                    when {
                        name == null || !name.startsWith(info.packageName + ":id/") -> "node_resource_missing"
                        packageManager.getResourcesForApplication(info.packageName).getIdentifier(name, null, null) != info.fieldId -> "native_field_id_mismatch"
                        else -> "verified"
                    }
                } catch (_: Exception) { "native_resource_unavailable" }
            }
            val webParagraphs = info != null && info.fieldId > 0 &&
                info.inputType and InputType.TYPE_MASK_VARIATION == InputType.TYPE_TEXT_VARIATION_WEB_EDIT_TEXT &&
                around.offset == 0 && inputText.length - maxOf(start, end) < CONTEXT &&
                node.textSelectionStart >= 0 && node.textSelectionEnd >= 0 && webEditor(node) &&
                paragraphTextAgrees(nodeText, inputText)
            if (emptyFieldCheck != "verified" && !webParagraphs) return rejectSnapshot("text_mismatch:$emptyFieldCheck")
        }
        if ((node.textSelectionStart >= 0 && node.textSelectionStart != start) ||
            (node.textSelectionEnd >= 0 && node.textSelectionEnd != end)) return rejectSnapshot("selection_mismatch")
        if (start !in 0..text.length || end !in 0..text.length) return rejectSnapshot("selection_range_invalid")
        if (!boundary(text, start) || !boundary(text, end)) return rejectSnapshot("selection_splits_codepoint")
        snapshotCheck = "verified"
        snapshotComparison = "verified"
        return Snapshot(node, text.toString(), start, end, editorInput.currentInputEditorInfo?.fieldId ?: -1)
    }
    private fun boundary(text: CharSequence, index: Int) = index == 0 || index == text.length ||
        !(Character.isHighSurrogate(text[index - 1]) && Character.isLowSurrogate(text[index]))
    /**
     * Call on the main thread when capture begins. Opens a delivery ledger whether or not an editor is
     * focused now; the opaque token is the originalTarget in TextDelivery. Eligibility is decided at delivery.
     */
    fun bind(sessionId: String, fixture: Boolean = false): String? {
        check(Looper.myLooper() == Looper.getMainLooper())
        if (!connected) return null
        invalidate()
        lastBindCheck = if (snapshot() == null) snapshotCheck else "verified"
        if (lastBindCheck != "verified") unsupported = eligibleNode()
        val token = UUID.randomUUID().toString()
        deliveries.create(sessionId, token, fixture)
        return token
    }

    /** Persist before attempting. PENDING also means asynchronous readback is in progress; consult the ledger. */
    override fun deliver(sessionId: String, text: String, originalTarget: String): DeliveryResult {
        check(Looper.myLooper() == Looper.getMainLooper())
        val item = try { deliveries.load(originalTarget) } catch (_: Exception) { return DeliveryResult.UNSUPPORTED }
        if (item.getString("session_id") != sessionId) return DeliveryResult.UNSUPPORTED
        if (item.has("text")) return if (item.optString("state") == "delivered") DeliveryResult.DELIVERED
            else DeliveryResult.PENDING_ORIGINAL_TARGET // Never execute a duplicate, including uncertain outcomes.
        item.put("text", text).put("state", "recovery").put("reason", "No insertable editor was focused")
        deliveries.save(item)
        val now = snapshot()
        val connection = editorInput.currentInputConnection
        val rejection = when {
            text.isEmpty() -> "Empty result"
            !consent.accepted -> "Text insertion is paused until it is accepted in $PRODUCT_NAME"
            verifying -> "Another insertion is still being verified"
            now == null -> "No insertable editor was focused ($snapshotCheck)"
            connection == null -> "Editor connection unavailable"
            else -> null
        }
        if (rejection != null || now == null || connection == null) {
            item.put("reason", rejection); deliveries.save(item)
            recoveryNotice(); return DeliveryResult.PENDING_ORIGINAL_TARGET
        }
        val lo = minOf(now.start, now.end); val hi = maxOf(now.start, now.end)
        val expected = now.text.substring(0, lo) + text + now.text.substring(hi)
        val cursor = lo + text.length
        if (now.node.maxTextLength >= 0 && expected.length > now.node.maxTextLength) {
            item.put("reason", "Result exceeds the editor text limit"); deliveries.save(item); recoveryNotice()
            return DeliveryResult.PENDING_ORIGINAL_TARGET
        }
        verifying = true
        item.put("state", "unconfirmed").put("reason", "Check the focused field before copying; insertion is unconfirmed")
        deliveries.save(item) // A crash from here cannot reissue the edit.
        item.optJSONObject("timing")?.put("insertion_started_elapsed_ms", android.os.SystemClock.elapsedRealtime())
        try { connection.commitText(text, 1, null) }
        catch (_: Exception) { verifying = false; recoveryNotice(); return DeliveryResult.PENDING_ORIGINAL_TARGET }
        fun verify(remaining: Int) {
            val after = snapshot()
            if (after != null && after.node == now.node && after.node.windowId == now.node.windowId &&
                after.text == expected && after.start == cursor && after.end == cursor && after.fieldId == now.fieldId) {
                item.put("state", "delivered").put("reason", "Exact text and cursor read back")
                item.optJSONObject("timing")?.let { timing ->
                    val deliveredAt = android.os.SystemClock.elapsedRealtime()
                    timing.put("delivery_elapsed_ms", deliveredAt)
                    val sameBoot = timing.optInt("boot_count", -1) == android.provider.Settings.Global.getInt(contentResolver, android.provider.Settings.Global.BOOT_COUNT, -2)
                    if (sameBoot && timing.has("capture_stop_elapsed_ms")) timing.put("stop_to_delivery_ms", deliveredAt - timing.getLong("capture_stop_elapsed_ms"))
                    if (!sameBoot) { timing.remove("stop_to_delivery_ms"); timing.remove("insertion_started_elapsed_ms") }
                }
                try { deliveries.save(item) } catch (_: Exception) { verifying = false; recoveryNotice(); return }
                verifying = false
                SessionManager.get(this).deliveryVerified(sessionId)
                refreshControl()
            } else if (remaining > 0) {
                handler.postDelayed({ verify(remaining - 1) }, 100)
            } else { verifying = false; recoveryNotice() }
        }
        handler.postDelayed({ verify(7) }, 100)
        return DeliveryResult.PENDING_ORIGINAL_TARGET
    }
    private fun recoveryNotice() {
        Toast.makeText(this, "Saved. Open $PRODUCT_NAME → History to copy.", Toast.LENGTH_LONG).show()
    }
    companion object {
        private const val CONTEXT = 4096
        @Volatile var instance: WriterInputService? = null
            private set
    }
}
