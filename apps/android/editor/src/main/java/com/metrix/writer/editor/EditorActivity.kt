package com.metrix.writer.editor

import android.app.Activity
import android.os.Bundle
import android.text.InputType
import android.webkit.WebView
import android.widget.EditText
import android.widget.LinearLayout
import android.widget.Button
import android.widget.ScrollView
import android.view.inputmethod.EditorInfo

/** Separate package/UID, no Writer bridge. Fixtures for later accessibility insertion acceptance. */
class EditorActivity : Activity() {
    // Fixture-only editor-owned IME operations. Writer's production input guards are never bypassed.
    private var holdComposition = false
    private val composition = object : android.content.BroadcastReceiver() {
        override fun onReceive(context: android.content.Context, intent: android.content.Intent) {
            val field = currentFocus as? EditText ?: return
            if (!intent.getBooleanExtra("inspect", false)) {
                val connection = field.onCreateInputConnection(EditorInfo()) ?: return
                holdComposition = intent.getBooleanExtra("compose", false)
                if (holdComposition) connection.setComposingRegion(0, 11)
                else connection.finishComposingText()
            }
            setResultData("region=" + android.view.inputmethod.BaseInputConnection.getComposingSpanStart(field.text) +
                ":" + android.view.inputmethod.BaseInputConnection.getComposingSpanEnd(field.text))
        }
    }
    override fun onDestroy() { unregisterReceiver(composition); super.onDestroy() }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        registerReceiver(composition, android.content.IntentFilter("com.metrix.writer.editor.COMPOSITION_FIXTURE"), RECEIVER_EXPORTED)
        var sent = 0
        val send = Button(this).apply { text = "Send count: 0"; setOnClickListener { sent++; text = "Send count: $sent" } }
        val root = LinearLayout(this).apply { orientation = LinearLayout.VERTICAL; setPadding(24, 48, 24, 24) }
        listOf("English before | after", "עברית לפני | אחרי", "Mixed עברית | English").forEachIndexed { index, text ->
            root.addView(object : EditText(this) {
                override fun onInitializeAccessibilityNodeInfo(info: android.view.accessibility.AccessibilityNodeInfo) {
                    super.onInitializeAccessibilityNodeInfo(info)
                    if (intent.getBooleanExtra("missingSelection", false)) info.setTextSelection(-1, -1)
                    if (intent.getBooleanExtra("placeholderAsText", false) && (this.text.isNullOrEmpty() || intent.getBooleanExtra("wrongNodeText", false))) {
                        info.text = "Message"; info.hintText = null; info.isShowingHintText = false
                    }
                }
                override fun onCreateInputConnection(outAttrs: EditorInfo): android.view.inputmethod.InputConnection? {
                    val connection = super.onCreateInputConnection(outAttrs)
                    if (intent.getBooleanExtra("wrongFieldId", false)) outAttrs.fieldId = android.R.id.content
                    return connection?.let { target ->
                        object : android.view.inputmethod.InputConnectionWrapper(target, false) {
                            // Keep a genuine editable composing span for the dedicated negative case.
                            // Modern keyboards otherwise finish our synthetic composition immediately.
                            override fun finishComposingText(): Boolean =
                                if (holdComposition) true else super.finishComposingText()
                        }
                    }
                }
            }.apply { id = if (index == 0 && intent.getBooleanExtra("placeholderAsText", false)) com.metrix.writer.editor.R.id.placeholder_editor else 100 + index; contentDescription = "Native editor $index"; setText(text); setSelection(text.indexOf('|'))
                setOnEditorActionListener { _, _, _ -> sent++; send.text = "Send count: $sent"; true }
                // Deliberately refuse one fixture through the editor's real filter, despite a valid connection.
                filters = arrayOf(android.text.InputFilter { source, start, end, _, _, _ ->
                    if (source.subSequence(start, end).toString() == "REJECTED FIXTURE") "" else null
                })
                if (!intent.getBooleanExtra("fullscreen", false)) imeOptions = EditorInfo.IME_FLAG_NO_FULLSCREEN
                if (index == 0) {
                    when (intent.getStringExtra("securityType")) {
                        "password" -> inputType = InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_VARIATION_PASSWORD
                        "visiblePassword" -> inputType = InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_VARIATION_VISIBLE_PASSWORD
                        "webPassword" -> inputType = InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_VARIATION_WEB_PASSWORD
                        "numberPassword" -> inputType = InputType.TYPE_CLASS_NUMBER or InputType.TYPE_NUMBER_VARIATION_PASSWORD
                        "private" -> imeOptions = imeOptions or EditorInfo.IME_FLAG_NO_PERSONALIZED_LEARNING
                        "sensitive" -> if (android.os.Build.VERSION.SDK_INT >= 34) setAccessibilityDataSensitive(android.view.View.ACCESSIBILITY_DATA_SENSITIVE_YES)
                    }
                    if (intent.hasExtra("securityType")) postDelayed({
                        requestFocus()
                        getSystemService(android.view.inputmethod.InputMethodManager::class.java).showSoftInput(this, 0)
                    }, 400)
                }
            })
        }
        root.addView(EditText(this).apply { id = 103; hint = "Sensitive field"; inputType = InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_VARIATION_PASSWORD })
        root.addView(object : WebView(this) {
            override fun onCreateInputConnection(outAttrs: EditorInfo): android.view.inputmethod.InputConnection? {
                val connection = super.onCreateInputConnection(outAttrs) ?: return null
                if (!intent.getBooleanExtra("richParagraphs", false)) return connection
                // This tiny fixture mirrors Gmail's known full-context offset. The emulator's
                // older WebView supplies the same real HTML text/selection but an unknown offset.
                return object : android.view.inputmethod.InputConnectionWrapper(connection, false) {
                    override fun getSurroundingText(beforeLength: Int, afterLength: Int, flags: Int): android.view.inputmethod.SurroundingText? {
                        val around = super.getSurroundingText(beforeLength, afterLength, flags) ?: return null
                        return if (around.offset == -1 && beforeLength >= 256 && afterLength >= 256 && around.text.length < 256)
                            android.view.inputmethod.SurroundingText(around.text, around.selectionStart, around.selectionEnd, 0)
                        else around
                    }
                }
            }
        }.apply {
            contentDescription = "Independent browser editor"
            val rich = intent.getBooleanExtra("richParagraphs", false)
            if (rich) id = com.metrix.writer.editor.R.id.rich_editor_host
            val html = if (rich) "<div id='editor' role='textbox' aria-multiline='true' contenteditable='true'>Before<div><br></div><div>After</div></div>"
                else "<label>Browser editor<textarea dir='auto'>Web before | אחרי</textarea></label>"
            loadDataWithBaseURL("https://writer-fixture.invalid", "<meta name='viewport' content='width=device-width'>" + html, "text/html", "UTF-8", null)
        }, LinearLayout.LayoutParams(-1, 240))
        root.addView(EditText(this).apply {
            contentDescription = "Unsupported numeric editor"; inputType = InputType.TYPE_CLASS_NUMBER; setText("12345")
        })
        root.addView(EditText(this).apply {
            contentDescription = "Private editor"; setText("Private fixture"); imeOptions = EditorInfo.IME_FLAG_NO_PERSONALIZED_LEARNING
        })
        root.addView(send)
        setContentView(ScrollView(this).apply { addView(root) })
    }
}
