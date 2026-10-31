package com.metrix.writer

import androidx.compose.ui.graphics.asAndroidBitmap
import androidx.compose.ui.semantics.SemanticsActions
import androidx.compose.ui.semantics.SemanticsNode
import androidx.compose.ui.semantics.SemanticsProperties
import androidx.compose.ui.semantics.getOrNull
import androidx.compose.ui.test.*
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.text.TextLayoutResult
import androidx.compose.ui.text.style.ResolvedTextDirection
import androidx.test.ext.junit.runners.AndroidJUnit4
import com.metrix.writer.ui.AppearancePreferences
import org.json.JSONObject
import org.junit.*
import org.junit.runner.RunWith
import java.io.File
import java.util.UUID

/**
 * MW-ANDROID-04 renewal UI on the built app, driven by deterministic state fixtures: saved sessions,
 * delivery ledgers and live processing hints written through the real stores. No provider, network,
 * microphone or other app is used, and nothing here claims speech recognition works (MW-ANDROID-06).
 */
@RunWith(AndroidJUnit4::class)
class RenewalUiTest {
    @get:Rule val permissions = androidx.test.rule.GrantPermissionRule.grant(android.Manifest.permission.RECORD_AUDIO, android.Manifest.permission.POST_NOTIFICATIONS)
    @get:Rule val compose = createAndroidComposeRule<MainActivity>()
    private lateinit var saved: SavedPreferences
    private val created = mutableListOf<String>()
    private val manager get() = SessionManager.get(compose.activity)
    private val deliveries get() = DeliveryStore(File(compose.activity.filesDir, "deliveries"))

    @Before fun enter() {
        saved = SavedPreferences(compose.activity, "writer-consent", "writer-appearance")
        saved.clear("writer-consent")
        AccessibilityConsent(compose.activity).record(false)
        AppearancePreferences(compose.activity).onboarding = true
        compose.activityRule.scenario.recreate()
        compose.waitForIdle()
        if (compose.onAllNodesWithText("Explore first").fetchSemanticsNodes().isNotEmpty()) compose.onNodeWithText("Explore first").performClick()
        // ActivityScenario restores a disclosure opened before @Before changed consent; decline it like ComplianceUiTest does.
        if (compose.onAllNodesWithTag("accessibility-decline").fetchSemanticsNodes().isNotEmpty()) compose.onNodeWithTag("accessibility-decline").performClick()
    }
    @After fun leave() {
        // Fixtures with pending work are cleared first: deleteSaved refuses pending sessions, and a
        // leftover would sit in other suites' History (FloatingInputTest reads Needs attention).
        created.forEach { id ->
            ProcessingEvents.finish(id)
            runCatching { manager.store.load(id).also { it.remove("pending"); it.remove("pending_postprocess"); it.remove("pending_timing"); it.remove("pending_engine"); manager.store.save(it) } }
            runCatching { manager.store.deleteSaved(id) }
        }
        saved.restore()
    }

    private fun tab(name: String) { compose.onNodeWithTag("nav-$name").performClick() }
    private fun proof(name: String) {
        compose.waitForIdle()
        val directory = File(compose.activity.cacheDir, "ui-proof").apply { mkdirs() }
        val mode = compose.activity.resources.configuration.let { "${it.fontScale}-${it.layoutDirection}-${it.screenWidthDp}" }
        File(directory, "renewal-$name-$mode.png").outputStream().use {
            compose.onRoot().captureToImage().asAndroidBitmap().compress(android.graphics.Bitmap.CompressFormat.PNG, 100, it)
        }
    }

    /** A saved recording with synthetic audio and the given attempts; optionally a delivery ledger in [ledger] state. */
    private fun session(raw: String? = "Synthetic fixture sentence one.", cleaned: String? = null, language: String = "en", deliveryChoice: String = "raw",
                        state: String = "succeeded", error: String? = null, ledger: String? = null, reason: String = "no_focused_editor",
                        pasted: Boolean = false, polishOutcome: JSONObject? = null, configure: (JSONObject) -> Unit = {}): String {
        val item = manager.store.create(language)
        manager.store.audio(item).appendBytes(ByteArray(32000)); manager.store.finishCapture(item, null)
        val fresh = manager.store.load(item.getString("id"))
        if (raw != null || state != "succeeded") {
            val job = JSONObject().put("id", "job-" + UUID.randomUUID()).put("execution_location", "phone").put("delivery_choice", deliveryChoice)
                .put("request", JSONObject().put("idempotency_key", "attempt-" + UUID.randomUUID()).put("options", JSONObject().put("selection", JSONObject().put("provider", "phone").put("model", "base"))))
                .put("status", JSONObject().put("state", state).apply {
                    if (state == "succeeded") put("result", JSONObject().put("transcript", JSONObject().put("raw_text", raw)))
                    else put("error", JSONObject().put("message", error ?: "The phone model could not finish."))
                })
            cleaned?.let { job.put("cleaned_text", it) }
            polishOutcome?.let { job.put("polish_outcome", it) }
            if (pasted) job.put("polish_delivery", JSONObject().put("pasted", true).put("original_used", polishOutcome?.optBoolean("original_used") == true))
            fresh.getJSONArray("attempts").put(job)
        }
        if (ledger != null) {
            val token = UUID.randomUUID().toString()
            deliveries.create(fresh.getString("id"), token, true)
            val entry = deliveries.load(token)
            if (ledger != "awaiting_result") entry.put("text", cleaned ?: raw ?: "")
            entry.put("state", ledger).put("reason_code", reason).put("outcome", when (ledger) { "delivered" -> "inserted_verified"; "unconfirmed" -> "unconfirmed"; else -> "saved_not_inserted" })
            deliveries.save(entry)
            fresh.put("original_target", token)
        }
        configure(fresh)
        manager.store.save(fresh)
        created += fresh.getString("id")
        return fresh.getString("id")
    }

    private fun waitFor(tag: String) = compose.waitUntil(8000) { compose.onAllNodesWithTag(tag).fetchSemanticsNodes().isNotEmpty() }
    private fun open(id: String) {
        tab("History")
        compose.waitUntil(8000) { compose.onAllNodes(hasScrollToNodeAction()).fetchSemanticsNodes().isNotEmpty() }
        // The list keeps its first visible row anchored, so a newer row can sit above the viewport.
        compose.waitUntil(8000) { runCatching { compose.onNode(hasScrollToNodeAction()).performScrollToNode(hasTestTag("dictation-$id")) }.isSuccess }
        compose.onNodeWithTag("dictation-$id").performClick()
        waitFor("audio-player")
    }

    @Test fun disclosureHasExactlyOneAcceptOnFirstRunReviewAndRecreation() {
        // A fresh first launch (consent unset) opens the disclosure on Home.
        saved.clear("writer-consent")
        compose.activityRule.scenario.recreate(); compose.waitForIdle()
        // Restored Compose state from before the reset is not a first launch; open it through Home like a person would.
        if (compose.onAllNodesWithTag("accessibility-disclosure").fetchSemanticsNodes().isEmpty())
            compose.onNodeWithText("Type into other apps").performScrollTo().performClick()
        compose.onAllNodesWithTag("accessibility-accept").assertCountEquals(1)
        compose.activityRule.scenario.recreate()
        compose.onAllNodesWithTag("accessibility-accept").assertCountEquals(1)
        proof("disclosure")
        compose.onNodeWithTag("accessibility-decline").performClick()
        Assert.assertEquals("declined", AccessibilityConsent(compose.activity).choice)
        compose.onAllNodesWithTag("accessibility-disclosure").assertCountEquals(0)
        // Permissions reaches the same single host.
        tab("Settings"); compose.onNodeWithText("Permissions").performScrollTo().performClick()
        compose.onNodeWithText("Text insertion").performScrollTo().performClick()
        compose.onAllNodesWithTag("accessibility-accept").assertCountEquals(1)
        instrumentation().sendKeyDownUpSync(android.view.KeyEvent.KEYCODE_BACK)
        compose.waitUntil(5000) { compose.onAllNodesWithTag("accessibility-disclosure").fetchSemanticsNodes().isEmpty() }
        Assert.assertEquals("Dismissing records nothing", "declined", AccessibilityConsent(compose.activity).choice)
    }
    private fun instrumentation() = androidx.test.platform.app.InstrumentationRegistry.getInstrumentation()

    @Test fun savedNotInsertedStaysOnHomeAndHistoryUntilHandled() {
        val id = session(raw = "Fixture result that was not inserted.", ledger = "recovery", reason = "no_focused_editor")
        tab("Home"); waitFor("home-attention")
        compose.onNode(hasText("Saved, not inserted") and hasAnyAncestor(hasTestTag("home-attention"))).assertIsDisplayed()
        compose.onNodeWithText("No text field was focused when the text was ready. Nothing was typed.", substring = true).assertExists()
        proof("home-attention")
        // Navigation and recreation never lose it (no 1.8 s chip, no toast).
        tab("History"); tab("Home"); compose.activityRule.scenario.recreate(); waitFor("home-attention")
        // The badge counts every dictation needing attention on this device, including this fixture.
        compose.onNode(SemanticsMatcher("History badge") { it.config.getOrNull(SemanticsProperties.ContentDescription)?.any { d -> d.startsWith("History, ") && d.endsWith(" need attention") } == true }).assertExists()
        tab("History"); compose.onNodeWithTag("filter-attention").performClick(); waitFor("attention-$id")
        compose.onNodeWithTag("attention-$id").assertIsDisplayed(); proof("history-attention")
        val handled = hasText("Mark handled") and hasAnyAncestor(hasTestTag("attention-$id"))
        compose.onNode(hasScrollToNodeAction()).performScrollToNode(handled); compose.onNode(handled).performClick()
        compose.waitUntil(8000) { compose.onAllNodesWithTag("attention-$id").fetchSemanticsNodes().isEmpty() }
        Assert.assertTrue(deliveries.load(manager.store.load(id).getString("original_target")).has("handled_at_ms"))
        tab("Home"); compose.onAllNodes(hasText("Fixture result that was not inserted.") and hasAnyAncestor(hasTestTag("home-attention"))).assertCountEquals(0)
        // Handling it never typed or deleted anything: the dictation stays.
        Assert.assertEquals("recovery", deliveries.load(manager.store.load(id).getString("original_target")).getString("state"))
    }

    @Test fun copyingASavedResultRecordsRecoveryAndUnconfirmedHasNoInsert() {
        val id = session(raw = "Fixture text that may already be there.", ledger = "unconfirmed", reason = "readback_mismatch")
        tab("History"); compose.onNodeWithTag("filter-attention").performClick()
        val title = hasText("Check the field") and hasAnyAncestor(hasTestTag("attention-$id"))
        compose.waitUntil(8000) { runCatching { compose.onNode(hasScrollToNodeAction()).performScrollToNode(title) }.isSuccess }
        compose.onNode(title).assertIsDisplayed()
        compose.onAllNodesWithText("Insert here").assertCountEquals(0)
        compose.onAllNodesWithText("Insert anyway").assertCountEquals(0)
        compose.onNode(hasScrollToNodeAction()).performScrollToNode(hasText("Copy") and hasAnyAncestor(hasTestTag("attention-$id")))
        compose.onNode(hasText("Copy") and hasAnyAncestor(hasTestTag("attention-$id"))).performClick()
        compose.waitUntil(8000) { deliveries.load(manager.store.load(id).getString("original_target")).has("recovered_at_ms") }
        val clipboard = compose.activity.getSystemService(android.content.ClipboardManager::class.java)
        Assert.assertEquals("Fixture text that may already be there.", clipboard.primaryClip!!.getItemAt(0).coerceToText(compose.activity).toString())
        Assert.assertEquals("Copy never changes the ledger outcome", "unconfirmed", deliveries.load(manager.store.load(id).getString("original_target")).getString("state"))
    }

    @Test fun deliveryLineNamesWhatWasInsertedWhicheverVersionIsShown() {
        val id = session(raw = "um the original words", cleaned = "The polished words.", deliveryChoice = "cleaned", ledger = "delivered", pasted = true)
        open(id)
        compose.onNodeWithTag("entry-text").assertTextEquals("The polished words.")
        compose.onNodeWithTag("delivery-line").assertTextContains("Inserted · verified · Polished version", substring = true)
        proof("entry-inserted")
        compose.onNodeWithTag("version-original").performClick()
        compose.onNodeWithTag("entry-text").assertTextEquals("um the original words")
        compose.onNodeWithTag("delivery-line").assertTextContains("Showing: Original · Inserted: Polished · verified", substring = true)
        // Recreation keeps the selected version.
        compose.activityRule.scenario.recreate()
        compose.onNodeWithTag("entry-text").assertTextEquals("um the original words")
        compose.onAllNodesWithTag("entry-copy").assertCountEquals(1)
    }

    @Test fun originalInsertedAfterPolishTimeoutIsDistinctFromValidationRejection() {
        val timeout = session(raw = "Timed out original.", deliveryChoice = "cleaned", ledger = "delivered", pasted = true,
            polishOutcome = JSONObject().put("original_used", true).put("reason", "provider_timeout"))
        val rejected = session(raw = "Rejected original.", deliveryChoice = "cleaned",
            polishOutcome = JSONObject().put("original_used", true).put("reason", "invalid_provider_response"))
        open(timeout)
        compose.onNodeWithText("Original kept · polishing took longer than your wait limit").performScrollTo().assertIsDisplayed()
        compose.onNodeWithTag("delivery-line").assertTextContains("Original inserted · verified · polishing was not used", substring = true)
        proof("entry-original")
        compose.onNodeWithContentDescription("Back").performClick()
        open(rejected)
        compose.onNodeWithText("Polishing not used").performScrollTo().assertIsDisplayed()
        compose.onAllNodesWithText("Original kept · polishing took longer than your wait limit").assertCountEquals(0)
    }

    @Test fun skippedPolishingIsSavedAndNeverTyped() {
        // Insert original now, then polishing finished later: the polished version is saved, never typed.
        val late = session(raw = "Skipped original words.", cleaned = "Skipped polished words.", deliveryChoice = "cleaned", ledger = "delivered", pasted = true,
            polishOutcome = JSONObject().put("original_used", true).put("reason", "user_skipped").put("failure", "skipped"), configure = {
                it.getJSONArray("attempts").getJSONObject(0).put("late_polish_outcome", JSONObject().put("original_used", true).put("late", true)
                    .put("selected_model", JSONObject().put("provider", "gemini").put("model", "fixture")))
            })
        val skipped = session(raw = "Skipped before polishing ended.", deliveryChoice = "cleaned", ledger = "delivered", pasted = true,
            polishOutcome = JSONObject().put("original_used", true).put("reason", "user_skipped").put("failure", "skipped"))
        open(late)
        compose.onNodeWithTag("entry-text").assertTextEquals("Skipped original words.")
        compose.onNodeWithText("Polished version ready · not typed anywhere").performScrollTo().assertIsDisplayed()
        compose.onAllNodesWithTag("entry-polish-notice").assertCountEquals(0)
        compose.onNodeWithTag("delivery-line").assertTextContains("Original inserted · verified", substring = true)
        proof("entry-late-polish")
        compose.onNodeWithContentDescription("Back").performClick()
        open(skipped)
        compose.onNodeWithText("Original inserted · you skipped polishing").performScrollTo().assertIsDisplayed()
        compose.onAllNodesWithText("Polished version ready · not typed anywhere").assertCountEquals(0)
    }

    @Test fun keptAudioIsStoppedNotNeedsAttention() {
        val id = session(raw = null, configure = { it.put("capture_completion", (it.optJSONObject("capture_completion") ?: JSONObject()).put("kept", true)) })
        open(id)
        compose.onNodeWithText("Stopped without transcribing").performScrollTo().assertIsDisplayed()
        compose.onNode(hasText("Transcribe now", substring = true)).assertExists()
        compose.onAllNodesWithText("Insert here").assertCountEquals(0)
        compose.onAllNodesWithText("Not transcribed").assertCountEquals(0)
        proof("entry-kept")
        compose.onNodeWithContentDescription("Back").performClick()
        compose.waitUntil(8000) { runCatching { compose.onNode(hasScrollToNodeAction()).performScrollToNode(hasTestTag("dictation-$id")) }.isSuccess }
        // The row merges its descendants, so its own node carries the tag text.
        compose.onNode(hasTestTag("dictation-$id") and hasText("Stopped · audio kept", substring = true)).assertExists()
        compose.onNodeWithTag("filter-attention").performClick()
        compose.waitForIdle()
        compose.onAllNodesWithTag("attention-$id").assertCountEquals(0)
    }

    @Test fun failedTranscriptionInventsNoTextAndNamesTheRetryEngine() {
        val prefs = compose.activity.getSharedPreferences("phone-engine", 0); val before = prefs.all
        try {
            DictationChoice(compose.activity).save(true, "base", "en")
            val id = session(raw = null, state = "failed", error = "Network unavailable. Saved audio retained.")
            open(id)
            compose.onNodeWithTag("entry-failed").assertIsDisplayed()
            compose.onNodeWithText("Retry · On this phone · base").assertIsDisplayed()
            compose.onAllNodesWithTag("entry-text").assertCountEquals(0)
            compose.onNodeWithTag("audio-play").assertIsEnabled()
            proof("entry-failed")
        } finally {
            prefs.edit().clear().apply { before.forEach { (k, v) -> when (v) { is String -> putString(k, v); is Boolean -> putBoolean(k, v) } } }.commit()
        }
    }

    @Test fun homeBindsLiveStagesAndShowsTheOriginalWhilePolishing() {
        // Saved and queued: the live hint moves it to Transcribing; no percentage is invented.
        val queued = session(raw = null, configure = { it.put("pending", JSONObject().put("idempotency_key", "attempt-live").put("options", JSONObject())).put("pending_engine", JSONObject()) })
        ProcessingEvents.emit(queued, "attempt-live", ProcessingStage.RECOGNIZING)
        compose.activityRule.scenario.recreate()
        tab("Home"); compose.waitUntil(8000) { compose.onAllNodesWithText("Transcribing").fetchSemanticsNodes().isNotEmpty() }
        compose.onNodeWithText("On this phone · Your audio is saved on this phone").assertExists()
        compose.onAllNodes(hasProgressBarRangeInfo(androidx.compose.ui.semantics.ProgressBarRangeInfo.Indeterminate)).assertCountEquals(1)
        proof("home-transcribing")
        ProcessingEvents.finish(queued)
        // Original saved, polishing running: the original is copyable now.
        var attempt = ""
        val polishing = session(raw = "Original available before polishing ends.", configure = {
            val job = it.getJSONArray("attempts").getJSONObject(0); attempt = job.getJSONObject("request").getString("idempotency_key")
            it.put("pending_postprocess", JSONObject().put("cloud", JSONObject()))
        })
        ProcessingEvents.emit(polishing, attempt, ProcessingStage.POLISHING)
        compose.waitUntil(8000) { compose.onAllNodesWithTag("copy-original-now").fetchSemanticsNodes().isNotEmpty() }
        compose.onNodeWithText("Polishing").assertExists()
        compose.onNodeWithText("Stop polishing · keep original").assertExists()
        compose.onNodeWithTag("copy-original-now").performScrollTo().performClick()
        val clipboard = compose.activity.getSystemService(android.content.ClipboardManager::class.java)
        Assert.assertEquals("Original available before polishing ends.", clipboard.primaryClip!!.getItemAt(0).coerceToText(compose.activity).toString())
        proof("home-polishing")
        // Process death loses the live hint: unfinished work becomes recovery, never shown as running.
        ProcessingEvents.finish(polishing)
        compose.waitUntil(8000) { compose.onAllNodesWithTag("copy-original-now").fetchSemanticsNodes().isEmpty() }
    }

    @Test fun interruptedRecordingIsRecoveredAndNeverOffersInsertion() {
        val id = session(raw = null, configure = { it.put("pending", JSONObject().put("idempotency_key", "attempt-dead").put("options", JSONObject()))
            .put("pending_timing", JSONObject().put("request_elapsed_ms", 1L)) })
        tab("Home"); waitFor("home-recovered")
        compose.onNode(hasText("Recording recovered") and hasAnyAncestor(hasTestTag("home-recovered"))).assertIsDisplayed()
        open(id)
        compose.onNodeWithText("Writer stopped while this was in progress. The audio is saved. It will not be typed anywhere.").assertExists()
        compose.onAllNodesWithText("Insert here").assertCountEquals(0)
        proof("entry-recovered")
    }

    @Test fun largeHistoryPagesAndLongTranscriptIsNeverTruncated() {
        val long = (1..1500).joinToString(" ") { "word$it" } + " שלום עולם and release notes."
        val longId = session(raw = long)
        repeat(60) { session(raw = "Fixture history row $it.") }
        tab("History")
        compose.waitUntil(15000) { compose.onAllNodes(SemanticsMatcher("dictation row") { it.config.getOrNull(SemanticsProperties.TestTag)?.startsWith("dictation-") == true }).fetchSemanticsNodes().isNotEmpty() }
        // Pages of 50: the oldest rows arrive through Load more, which sits at the end of the lazy list.
        compose.onNode(hasScrollToNodeAction()).performScrollToNode(hasText("Load more dictations"))
        compose.onNodeWithText("Load more dictations").performClick()
        compose.waitUntil(15000) { compose.onAllNodesWithText("Load more dictations").fetchSemanticsNodes().isEmpty() }
        compose.onNode(hasScrollToNodeAction()).performScrollToNode(hasTestTag("dictation-$longId"))
        proof("history-large")
        compose.onNodeWithTag("history-search").performTextInput("word1500")
        waitFor("dictation-$longId")
        compose.onNodeWithTag("dictation-$longId").performClick(); waitFor("entry-text")
        compose.onNodeWithTag("entry-text").assertTextEquals(long)
        proof("entry-long")
    }

    @Test fun spokenTextResolvesDirectionFromContentAndMediaStaysLeftToRight() {
        val hebrew = session(raw = "שלום, זה טקסט בדיקה עם API ו-Gemini בתוכו.", language = "he")
        open(hebrew)
        val layouts = mutableListOf<TextLayoutResult>()
        compose.onNodeWithTag("entry-text").fetchSemanticsNode().config[SemanticsActions.GetTextLayoutResult].action!!.invoke(layouts)
        Assert.assertEquals(ResolvedTextDirection.Rtl, layouts.single().getParagraphDirection(0))
        val play = compose.onNodeWithTag("audio-play").fetchSemanticsNode().boundsInRoot.center.x
        val time = compose.onNodeWithText("0:00 /", substring = true).fetchSemanticsNode().boundsInRoot.center.x
        Assert.assertTrue("The player reads left to right in every layout direction", play < compose.onNodeWithContentDescription("Playback position").fetchSemanticsNode().boundsInRoot.center.x)
        Assert.assertTrue(time >= 0)
        proof("entry-hebrew")
        compose.onNodeWithContentDescription("Back").performClick()
        val english = session(raw = "An English fixture sentence.")
        open(english)
        layouts.clear()
        compose.onNodeWithTag("entry-text").fetchSemanticsNode().config[SemanticsActions.GetTextLayoutResult].action!!.invoke(layouts)
        Assert.assertEquals(ResolvedTextDirection.Ltr, layouts.single().getParagraphDirection(0))
    }

    /** Every actionable control on a screen has a label and is at least 48 × 48 dp. */
    private fun assertActionable(screen: String) {
        compose.waitForIdle()
        val density = compose.activity.resources.displayMetrics.density
        val problems = compose.onAllNodes(SemanticsMatcher.keyIsDefined(SemanticsActions.OnClick)).fetchSemanticsNodes().mapNotNull { node ->
            // Checked as laid out on screen; a control clipped by scrolling is checked where it is fully visible.
            val visible = node.boundsInRoot
            if (visible.width < node.size.width - 1 || visible.height < node.size.height - 1) return@mapNotNull null
            // Material controls may draw smaller than 48 dp and extend their touch area; the touch bounds are what a finger hits.
            val w = node.touchBoundsInRoot.width / density; val h = node.touchBoundsInRoot.height / density
            val label = label(node)
            when {
                label.isBlank() -> "$screen: unlabelled control ${w}x${h}dp"
                w < 47.5f || h < 47.5f -> "$screen: \"$label\" is ${"%.0f".format(w)}x${"%.0f".format(h)}dp"
                else -> null
            }
        }
        Assert.assertTrue(problems.joinToString("\n"), problems.isEmpty())
    }
    private fun label(node: SemanticsNode): String = listOfNotNull(
        node.config.getOrNull(SemanticsProperties.ContentDescription)?.joinToString(),
        node.config.getOrNull(SemanticsProperties.Text)?.joinToString { it.text },
        node.config.getOrNull(SemanticsProperties.EditableText)?.text,
    ).joinToString(" ").ifBlank { node.children.joinToString(" ") { label(it) } }

    @Test fun actionableControlsAreLabelledAndLargeEnough() {
        val id = session(raw = "Fixture for touch targets.", cleaned = "Fixture for touch targets, polished.", deliveryChoice = "cleaned", ledger = "recovery")
        tab("Home"); waitFor("home-attention"); assertActionable("Home")
        tab("History"); waitFor("dictation-$id"); assertActionable("History")
        compose.onNodeWithTag("filter-attention").performClick(); waitFor("attention-$id"); assertActionable("Needs attention")
        compose.onNodeWithTag("filter-all").performClick()
        compose.onNodeWithTag("dictation-$id").performScrollTo().performClick(); waitFor("audio-player"); assertActionable("Entry")
        tab("Settings"); assertActionable("Settings")
        compose.onNodeWithTag("settings-polishing").performScrollTo().performClick(); assertActionable("Polishing & output")
    }
}
