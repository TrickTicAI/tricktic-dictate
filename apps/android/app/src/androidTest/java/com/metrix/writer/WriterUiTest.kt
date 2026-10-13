package com.metrix.writer

import androidx.compose.ui.test.*
import androidx.compose.ui.graphics.asAndroidBitmap
import java.io.File
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.test.ext.junit.runners.AndroidJUnit4
import org.junit.*
import org.junit.runner.RunWith
import com.metrix.writer.ui.*

@RunWith(AndroidJUnit4::class)
class WriterUiTest {
    @get:Rule val permissions = androidx.test.rule.GrantPermissionRule.grant(android.Manifest.permission.RECORD_AUDIO, android.Manifest.permission.POST_NOTIFICATIONS)
    @get:Rule val compose = createAndroidComposeRule<MainActivity>()
    @Before fun enter() {
        compose.waitForIdle()
        val expected = androidx.test.platform.app.InstrumentationRegistry.getArguments().getString("writerExpectedRtl")
        if (expected != null) Assert.assertEquals("Actual resource layout direction", expected.toInt(), compose.activity.resources.configuration.layoutDirection)
        if(compose.onAllNodesWithText("Explore first").fetchSemanticsNodes().isNotEmpty()) compose.onNodeWithText("Explore first").performClick()
    }
    private fun proof(name: String) {
        compose.waitForIdle()
        val directory = File(compose.activity.cacheDir, "ui-proof").apply { mkdirs() }
        val mode = compose.activity.resources.configuration.let { "${it.fontScale}-${it.layoutDirection}" }
        File(directory, "$name-$mode.png").outputStream().use {
            compose.onRoot().captureToImage().asAndroidBitmap().compress(android.graphics.Bitmap.CompressFormat.PNG, 100, it)
        }
    }
    private fun tab(name: String) { compose.onNodeWithTag("nav-$name").performClick() }
    @Test fun navigationStatisticsAndBackSurviveActivityRecreation() {
        tab("Statistics");compose.onNodeWithText("Your words, in perspective.").assertIsDisplayed()
        compose.onNodeWithText("30 days").performClick(); proof("statistics")
        compose.activityRule.scenario.recreate()
        compose.onNodeWithText("30 days").assertIsSelected()
        tab("Settings");proof("settings");compose.onNodeWithText("Appearance").performScrollTo().performClick()
        compose.onNodeWithText("Ink · warm dark").performScrollTo().performClick();proof("appearance-dark")
        compose.onNodeWithContentDescription("Back").performClick()
        compose.onNodeWithTag("settings-search").performScrollTo().assertIsDisplayed()
        tab("Home");compose.onNodeWithText("Make room for\nyour thoughts.").performScrollTo().assertIsDisplayed();proof("home-dark")
    }
    @Test fun settingsSearchFindsMisspelledSettingsAndOpensTheirPage() {
        tab("Settings")
        compose.onNodeWithTag("settings-search").performTextInput("Cleeboard")
        compose.onNodeWithText("Text insertion").assertIsDisplayed();proof("settings-search")
        compose.onNodeWithText("Text insertion").performClick()
        compose.onNodeWithText("Access").assertIsDisplayed()
        // Back returns to the same results, so another one can be opened.
        compose.onNodeWithContentDescription("Back").performClick()
        compose.onNodeWithText("Text insertion").assertIsDisplayed()
        compose.onNodeWithTag("settings-search").performTextClearance()
        compose.onNodeWithTag("settings-search").performTextInput("dark mode")
        compose.onNodeWithText("Theme").performClick()
        compose.onNodeWithText("Follow system").assertIsDisplayed()
        compose.onNodeWithContentDescription("Back").performClick()
        // Words inside a section find it, and opening it scrolls that section into view.
        compose.onNodeWithTag("settings-search").performTextClearance()
        compose.onNodeWithTag("settings-search").performTextInput("snooze")
        compose.onNodeWithText("Temporary hiding").performClick()
        compose.waitUntil(5000) { runCatching { compose.onNodeWithText("Hide now").assertIsDisplayed() }.isSuccess }
        compose.onNodeWithContentDescription("Back").performClick()
        compose.onNodeWithTag("settings-search").performTextClearance()
        compose.onNodeWithTag("settings-search").performTextInput("nothing like this")
        compose.onNodeWithText("No settings match “nothing like this”.").assertIsDisplayed()
    }
    @Test fun dictionaryCreateEditAndDeleteWithHebrewSnippet() {
        tab("Dictionary");compose.onNodeWithText("Snippets").performClick();compose.onNodeWithText("Add entry").performClick()
        compose.onNodeWithText("Spoken trigger").performTextInput("בדיקת ממשק")
        compose.onNodeWithText("Exact inserted text").performTextInput("שלום עולם\nHello world")
        compose.onNodeWithText("Save").performClick()
        compose.onNodeWithText("בדיקת ממשק").assertIsDisplayed();proof("dictionary")
        // Searching the inserted text finds the entry whose trigger you no longer remember.
        compose.onNodeWithTag("dictionary-search").performTextInput("hello world")
        compose.waitUntil(5000) { compose.onAllNodesWithText("בדיקת ממשק").fetchSemanticsNodes().isNotEmpty() }
        compose.onNodeWithTag("dictionary-search").performTextClearance()
        compose.onNodeWithTag("dictionary-search").performTextInput("nothing matches this")
        compose.onNodeWithText("No matching entries").assertIsDisplayed();proof("dictionary-search-empty")
        compose.onAllNodesWithText("בדיקת ממשק").assertCountEquals(0)
        // The empty state's own button sits below the fold at large accessibility fonts, so clear
        // from the field's always-visible control; both restore the unfiltered list.
        compose.onNodeWithContentDescription("Clear search").performClick()
        compose.waitUntil(5000) { compose.onAllNodesWithText("בדיקת ממשק").fetchSemanticsNodes().isNotEmpty() }
        compose.onAllNodesWithText("Delete").onLast().performClick();compose.onAllNodesWithText("Delete").onLast().performClick()
        compose.waitUntil(5000) { compose.onAllNodesWithText("בדיקת ממשק").fetchSemanticsNodes().isEmpty() }
    }
    @Test fun floatingPreviewSizeAndSnoozeAreRealPreferences() {
        tab("Settings");compose.onNodeWithText("Floating control").performScrollTo().performClick()
        compose.onNodeWithContentDescription("TrickTic Dictate microphone").performClick()
        compose.onNodeWithContentDescription("Live microphone waveform").assertIsDisplayed()
        Assert.assertTrue("Cancel stays physically left of finish in RTL too",
            compose.onNodeWithContentDescription("Cancel recording").fetchSemanticsNode().boundsInRoot.center.x <
                compose.onNodeWithContentDescription("TrickTic Dictate stop recording").fetchSemanticsNode().boundsInRoot.center.x)
        proof("floating-recording")
        compose.onNodeWithContentDescription("TrickTic Dictate stop recording").performClick()
        compose.onNodeWithContentDescription("Preparing text").assertIsDisplayed()
        compose.onNodeWithText("Idle").performClick()
        compose.onNodeWithText("30 minutes").performScrollTo().performClick()
        compose.onNodeWithText("Hide now").performScrollTo().performClick()
        Assert.assertTrue(FloatingPreferences(compose.activity).snoozed())
        compose.onNodeWithContentDescription("Enable floating control").performScrollTo().performClick()
        compose.onNodeWithText("Show now").performScrollTo().performClick()
        compose.onNodeWithContentDescription("Enable floating control").assertIsOn()
        Assert.assertFalse(FloatingPreferences(compose.activity).snoozed())
    }
    @Test fun homeRecordingSurvivesRecreationAndKeepsAudioWithoutAnEditor() {
        val context = compose.activity.applicationContext
        val preferences = context.getSharedPreferences("phone-engine", 0)
        val before = preferences.all
        val manager = SessionManager.get(context)
        var id: String? = null
        DictationChoice(context).save(true, "base", "en") // Never contact a live provider in UI tests.
        try {
            tab("Home"); compose.onNodeWithTag("start-recording").performClick()
            compose.waitUntil(5000) { CaptureService.state == "recording" }
            id = CaptureService.activeSessionId!!
            val started = CaptureService.startedAtElapsed
            compose.activityRule.scenario.recreate()
            Assert.assertEquals(id, CaptureService.activeSessionId)
            Assert.assertEquals(started, CaptureService.startedAtElapsed)
            compose.onNodeWithContentDescription("Live microphone waveform").assertIsDisplayed()
            proof("home-recording")
            compose.onNodeWithContentDescription("TrickTic Dictate stop recording").performClick()
            compose.waitUntil(15000) { !CaptureService.isActive() && !manager.isBusy() }
            val saved = manager.store.load(id!!)
            Assert.assertEquals("app", saved.getString("capture_origin"))
            Assert.assertEquals("phone", saved.getJSONObject("capture_choice").getString("server"))
            Assert.assertTrue(manager.store.audio(saved).length() > 44)
            tab("History"); proof("history")
        } finally {
            if(CaptureService.isActive()) compose.activityRule.scenario.onActivity { CaptureService.stop(it,true) }
            manager.cancelWork()
            preferences.edit().clear().apply {
                before.forEach { (key,value) -> when(value) { is String -> putString(key,value); is Boolean -> putBoolean(key,value) } }
            }.commit()
            id?.let { runCatching { manager.store.deleteSaved(it) } }
        }
    }
    @Test fun cloudKeyEntryIsProtectedAndNeverRestoredWithActivityState() {
        tab("Settings"); compose.onNodeWithText("API keys").performScrollTo().performClick()
        compose.onNodeWithTag("key-gemini").performScrollTo().performClick()
        compose.onNodeWithText("Paste API key").performScrollTo().performTextInput("ui-fixture-never-send")
        Assert.assertTrue(compose.activity.window.attributes.flags and android.view.WindowManager.LayoutParams.FLAG_SECURE != 0)
        compose.activityRule.scenario.recreate()
        val key = compose.onNodeWithText("Paste API key").fetchSemanticsNode().config[androidx.compose.ui.semantics.SemanticsProperties.EditableText].text
        Assert.assertEquals("", key)
        compose.onNodeWithContentDescription("Back").performClick(); compose.waitForIdle()
        Assert.assertEquals(0, compose.activity.window.attributes.flags and android.view.WindowManager.LayoutParams.FLAG_SECURE)
    }
    @Test fun polishingIsOneChoiceSavedAtOnceAndAsksForAKeyBeforePolishing() {
        val prefs = compose.activity.getSharedPreferences("companion", 0)
        val before = prefs.all
        try {
            prefs.edit().remove("cleanup").remove("cleanup_last").putString("delivery", "raw").commit()
            tab("Settings"); compose.onNodeWithTag("settings-polishing").performScrollTo().performClick()
            compose.onNodeWithTag("wording-dictionary").performClick(); compose.waitForIdle()
            compose.waitUntil(5000) { CompanionPreferences(compose.activity).delivery() == "dictionary" }
            Assert.assertEquals("off", CompanionPreferences(compose.activity).cleanup())
            proof("polishing")
            // No polishing model with a key yet: Polished opens the model list instead of saving a broken choice.
            compose.onNodeWithTag("wording-cleaned").performClick()
            compose.onNodeWithTag("polish-openrouter-anthropic/claude-haiku-4.5").performScrollTo().assertIsDisplayed()
            Assert.assertEquals("dictionary", CompanionPreferences(compose.activity).delivery())
            proof("polishing-models")
            // A keyless provider's model leads to its key page first.
            compose.onNodeWithTag("polish-openrouter-anthropic/claude-haiku-4.5").performScrollTo().performClick()
            compose.onNodeWithText("Paste API key").assertIsDisplayed()
            Assert.assertEquals("openrouter/anthropic/claude-haiku-4.5", com.metrix.writer.ui.settings.PendingChoice.polishing)
        } finally {
            com.metrix.writer.ui.settings.PendingChoice.polishing = null
            prefs.edit().clear().apply { before.forEach { (key, value) -> when (value) { is String -> putString(key, value); is Boolean -> putBoolean(key, value) } } }.commit()
        }
    }
    @Test fun polishingFallbackPersistsAndSeparatesSavedKeysFromVerifiedResponses() {
        val raw = compose.activity.getSharedPreferences("companion", 0)
        val before = raw.all
        try {
            val prefs = CompanionPreferences(compose.activity)
            prefs.save("gemini/gemini-3.8-flash", "cleaned"); prefs.saveReliability("off", 1000L, 3000L)
            tab("Settings"); compose.onNodeWithTag("settings-polishing").performScrollTo().performClick()
            compose.onNodeWithTag("polishing-fallback").performScrollTo().performClick()
            compose.onNodeWithTag("fallback-openai-gpt-4.1-mini").performScrollTo().performClick()
            compose.waitUntil(5000) { prefs.fallbackCleanup() == "openai/gpt-4.1-mini" }
            compose.onNodeWithTag("polishing-check-fallback").performScrollTo().assertIsNotEnabled()
            compose.onNodeWithText("5s", useUnmergedTree = true).performScrollTo().performClick()
            compose.waitUntil(5000) { prefs.polishTimeoutMs() == 5000L }
            compose.activityRule.scenario.recreate()
            compose.onNodeWithTag("polishing-fallback").performScrollTo().assertIsDisplayed()
            Assert.assertEquals("openai/gpt-4.1-mini", prefs.fallbackCleanup())
            Assert.assertEquals(5000L, prefs.polishTimeoutMs())
            proof("polishing-reliability")
        } finally {
            raw.edit().clear().apply { before.forEach { (key, value) -> when (value) { is String -> putString(key, value); is Boolean -> putBoolean(key, value); is Long -> putLong(key, value); is Int -> putInt(key, value) } } }.commit()
        }
    }
    @Test fun polishingInstructionsEditStyleAndReset() {
        val prefs = compose.activity.getSharedPreferences("companion", 0)
        val before = prefs.all
        try {
            CompanionPreferences(compose.activity).save("gemini/gemini-3.8-flash", "cleaned")
            tab("Settings"); compose.onNodeWithTag("settings-polishing").performScrollTo().performClick()
            compose.onNodeWithTag("polishing-instructions").performScrollTo().performClick()
            compose.onNodeWithTag("polish-instructions").assertIsDisplayed()
            val field = compose.onNodeWithTag("polish-instructions").fetchSemanticsNode()
                .config[androidx.compose.ui.semantics.SemanticsProperties.EditableText].text
            Assert.assertTrue(field.contains("ready-to-use writing"))
            compose.onNodeWithTag("polish-instructions").performTextClearance()
            compose.onNodeWithTag("polish-instructions").performTextInput("Fixture edited instructions.")
            compose.waitUntil(5000) { CompanionPreferences(compose.activity).polish().instructions == "Fixture edited instructions." }
            compose.onNodeWithTag("polish-style-light").performScrollTo().performClick()
            compose.waitUntil(5000) { CompanionPreferences(compose.activity).polishStyle() == "light" }
            compose.onNodeWithTag("polish-style-super").performScrollTo().performClick()
            compose.waitUntil(5000) { CompanionPreferences(compose.activity).polishStyle() == "super" }
            compose.onNodeWithTag("polish-rewrite-concise").performScrollTo().performClick()
            compose.waitUntil(5000) { CompanionPreferences(compose.activity).polish().formatting.rewrite == "concise" }
            proof("polishing-instructions")
            compose.onNodeWithTag("polish-reset").performScrollTo().performClick()
            compose.waitUntil(5000) { CompanionPreferences(compose.activity).isPolishDefault() }
        } finally {
            prefs.edit().clear().apply { before.forEach { (key, value) -> when (value) { is String -> putString(key, value); is Boolean -> putBoolean(key, value) } } }.commit()
        }
    }
    @Test fun speechEngineListsOnlySpeechProvidersAndPhoneModels() {
        tab("Settings"); compose.onNodeWithTag("settings-engine").performScrollTo().performClick()
        CloudProviders.transcribers.forEach { compose.onNodeWithTag("engine-${it.id}").performScrollTo().assertIsDisplayed() }
        compose.onAllNodesWithTag("engine-openrouter").assertCountEquals(0)
        compose.onNodeWithTag("active-engine").assertIsDisplayed()
        proof("speech-engine")
    }
    @Test fun apiKeysListsEveryProviderAndLinksFromPolishing() {
        tab("Settings"); compose.onNodeWithText("API keys").performScrollTo().performClick()
        CloudProviders.all.forEach { compose.onNodeWithTag("key-${it.id}").performScrollTo().assertIsDisplayed() }
        proof("api-keys")
        compose.onNodeWithTag("key-openrouter").performScrollTo().performClick()
        compose.onNodeWithText("No key saved").assertIsDisplayed()
        compose.onNodeWithTag("key-save").assertIsNotEnabled()
        Assert.assertTrue(compose.activity.window.attributes.flags and android.view.WindowManager.LayoutParams.FLAG_SECURE != 0)
    }
    @Test fun historyOriginalAndRecoveryRemainAvailable() {
        val manager=SessionManager.get(compose.activity)
        val item=manager.store.create("mixed");manager.store.audio(item).appendBytes(ByteArray(32000));manager.store.finishCapture(item,null)
        val job=org.json.JSONObject("""{"id":"ui-result","execution_location":"phone","request":{"options":{"selection":{"model":"base"}}},"status":{"state":"succeeded","result":{"transcript":{"raw_text":"שלום test transcript"}}},"cleaned_text":"שלום cleaned transcript"}""")
        item.getJSONArray("attempts").put(job);manager.store.save(item)
        try {
            manager.store.restore().forEach { Dictation.from(it) }
            tab("History")
            compose.onNodeWithText("Search transcripts").performTextInput("שלום test transcript")
            compose.waitUntil(5000) { compose.onAllNodesWithTag("dictation-${item.getString("id")}").fetchSemanticsNodes().isNotEmpty() }
            compose.onNodeWithTag("dictation-${item.getString("id")}").performScrollTo().performClick()
            compose.onNodeWithText("Spoken wording").performScrollTo().assertIsDisplayed()
            compose.onNodeWithText("שלום cleaned transcript").performScrollTo().assertIsDisplayed();proof("history-detail")
            compose.onNodeWithText("Copy Current wording").performScrollTo().performClick()
            // Each variant exports on its own, so raw and polished wording never share a file.
            compose.onNodeWithText("Export Spoken wording").performScrollTo().assertIsDisplayed()
            compose.onNodeWithText("Export Current wording").performScrollTo().assertIsDisplayed()
            // A failed retry selects the original without deleting the earlier polish.
            job.put("polish_outcome", org.json.JSONObject().put("original_used", true))
            manager.store.save(item); compose.activityRule.scenario.recreate()
            compose.onNodeWithText("Current wording · original").performScrollTo().assertIsDisplayed()
            compose.onNodeWithText("Earlier polished wording").performScrollTo().assertIsDisplayed()
            compose.onNodeWithText("Copy Current wording · original").performScrollTo().performClick()
            val clipboard = compose.activity.getSystemService(android.content.ClipboardManager::class.java)
            Assert.assertEquals("שלום test transcript", clipboard.primaryClip!!.getItemAt(0).coerceToText(compose.activity).toString())
            compose.onNodeWithText("Export Current wording · original").performScrollTo().assertIsDisplayed()
            compose.onNodeWithContentDescription("Back").performClick()
            tab("Statistics");compose.onNodeWithText("Words dictated").assertIsDisplayed()
        } finally { manager.store.deleteSaved(item.getString("id")) }
    }
    @Test fun permissionExplanationCanBeCancelledWithoutLeavingWriter() {
        tab("Settings"); compose.onNodeWithText("Permissions").performScrollTo().performClick()
        val consent = compose.activity.getSharedPreferences("writer-consent", 0)
        val before = consent.all
        try {
            compose.onNodeWithText("Text insertion").performScrollTo().performClick()
            compose.onNodeWithTag("accessibility-accept").performClick()
            compose.onAllNodesWithText("Enable TrickTic Dictate · voice insertion").onLast().assertIsDisplayed()
            compose.onNodeWithText("Cancel").performClick()
            compose.onNodeWithText("Access").performScrollTo().assertIsDisplayed()
            proof("permissions")
        } finally {
            consent.edit().clear().apply { before.forEach { (key, value) -> when (value) { is String -> putString(key, value); is Long -> putLong(key, value) } } }.commit()
        }
    }
    @Test fun placementAndNewThemePersistWithUnderstandableSettings() {
        tab("Settings"); compose.onNodeWithText("Floating control").performScrollTo().performClick()
        compose.onNodeWithText("Free placement · keep where I drag").performScrollTo().performClick()
        Assert.assertFalse(FloatingPreferences(compose.activity).dock)
        FloatingPreferences(compose.activity).position(.5f, .5f)
        compose.activityRule.scenario.recreate()
        Assert.assertFalse(FloatingPreferences(compose.activity).dock)
        Assert.assertEquals(.5f, FloatingPreferences(compose.activity).x)
        compose.onNodeWithContentDescription("Back").performClick()
        compose.onNodeWithText("Appearance").performScrollTo().performClick()
        compose.onNodeWithText("Ocean · deep blue").performScrollTo().performClick()
        Assert.assertEquals("ocean", AppearancePreferences(compose.activity).theme)
        proof("appearance-ocean")
        compose.onNodeWithText("Sage · calm green").performScrollTo().performClick()
        proof("appearance-sage")
    }

}
