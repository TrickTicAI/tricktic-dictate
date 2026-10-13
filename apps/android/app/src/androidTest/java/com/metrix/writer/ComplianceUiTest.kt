package com.metrix.writer

import android.app.Instrumentation
import android.content.Context
import android.content.Intent
import android.provider.Settings
import androidx.compose.ui.test.*
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.junit4.createEmptyComposeRule
import androidx.test.core.app.ActivityScenario
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import com.metrix.writer.ui.*
import org.junit.*
import org.junit.runner.RunWith
import java.io.File
import java.util.concurrent.CopyOnWriteArrayList

/** Records outgoing Accessibility-settings and browser starts without opening them. */
internal class OutgoingStarts : Instrumentation.ActivityMonitor() {
    val intents = CopyOnWriteArrayList<Intent>()
    override fun onStartActivity(intent: Intent): Instrumentation.ActivityResult? =
        if (intent.action == Settings.ACTION_ACCESSIBILITY_SETTINGS || intent.action == Intent.ACTION_VIEW) {
            intents += intent; Instrumentation.ActivityResult(0, null)
        } else null
    fun accessibility() = intents.count { it.action == Settings.ACTION_ACCESSIBILITY_SETTINGS }
    fun links() = intents.filter { it.action == Intent.ACTION_VIEW }.map { it.dataString }
}

/** Saves a full-screen capture, dialogs included, for the Play declaration and demo video. */
internal fun screenshot(context: Context, name: String) {
    InstrumentationRegistry.getInstrumentation().waitForIdleSync()
    Thread.sleep(350) // Android dialog-window transitions run outside Compose's idle clock.
    val directory = File(context.cacheDir, "ui-proof").apply { mkdirs() }
    val mode = context.resources.configuration.let { "${it.fontScale}-${it.layoutDirection}" }
    val bitmap = InstrumentationRegistry.getInstrumentation().uiAutomation.takeScreenshot()
    File(directory, "$name-$mode.png").outputStream().use { bitmap.compress(android.graphics.Bitmap.CompressFormat.PNG, 100, it) }
}

/** Keeps the tests' consent and onboarding choices from leaking into the device's real settings. */
internal class SavedPreferences(context: Context, vararg names: String) {
    private val saved = names.associateWith { context.getSharedPreferences(it, 0).let { prefs -> prefs to prefs.all } }
    fun clear(name: String) { saved.getValue(name).first.edit().clear().commit() }
    fun restore() = saved.values.forEach { (prefs, values) ->
        prefs.edit().clear().apply {
            values.forEach { (key, value) -> when (value) { is String -> putString(key, value); is Boolean -> putBoolean(key, value); is Long -> putLong(key, value); is Int -> putInt(key, value) } }
        }.commit()
    }
}

@RunWith(AndroidJUnit4::class)
class ComplianceUiTest {
    @get:Rule val permissions = androidx.test.rule.GrantPermissionRule.grant(android.Manifest.permission.RECORD_AUDIO, android.Manifest.permission.POST_NOTIFICATIONS)
    @get:Rule val compose = createAndroidComposeRule<MainActivity>()
    private val instrumentation = InstrumentationRegistry.getInstrumentation()
    private val starts = OutgoingStarts()
    private lateinit var saved: SavedPreferences

    @Before fun enter() {
        saved = SavedPreferences(compose.activity, "writer-consent", "writer-appearance")
        saved.clear("writer-consent")
        AccessibilityConsent(compose.activity).record(false) // First-launch auto-disclosure is tested separately below.
        AppearancePreferences(compose.activity).onboarding = true
        instrumentation.addMonitor(starts)
        compose.activityRule.scenario.recreate()
        compose.waitForIdle()
        // ActivityScenario restores Compose state created before @Before changed the preferences.
        if (compose.onAllNodesWithText("Explore first").fetchSemanticsNodes().isNotEmpty()) compose.onNodeWithText("Explore first").performClick()
        if (compose.onAllNodesWithTag("accessibility-decline").fetchSemanticsNodes().isNotEmpty()) compose.onNodeWithTag("accessibility-decline").performClick()
    }
    @After fun leave() { instrumentation.removeMonitor(starts); saved.restore() }

    private fun tab(name: String) { compose.onNodeWithTag("nav-$name").performClick() }
    private fun openTextInsertion() {
        tab("Settings"); compose.onNodeWithText("Permissions").performScrollTo().performClick()
        compose.onNodeWithText("Text insertion").performScrollTo().performClick()
    }
    private fun consent() = AccessibilityConsent(compose.activity).choice

    @Test fun acceptInTheDisclosureIsRequiredBeforeAccessibilitySettingsOpen() {
        openTextInsertion()
        compose.onNodeWithTag("accessibility-disclosure").assertIsDisplayed()
        Assert.assertEquals("Nothing opens before a choice", 0, starts.accessibility())
        screenshot(compose.activity, "accessibility-disclosure")
        compose.onNodeWithText("Where the data goes").assertExists()
        compose.onNodeWithText("You can turn the service off at any time", substring = true).performScrollTo().assertIsDisplayed()
        screenshot(compose.activity, "accessibility-disclosure-data")
        compose.onNodeWithTag("accessibility-accept").performClick()
        Assert.assertEquals("accepted", consent())
        Assert.assertEquals("Accept shows the setup steps first", 0, starts.accessibility())
        compose.onNodeWithText("Continue").performClick()
        compose.waitUntil(5000) { starts.accessibility() == 1 }
    }

    @Test fun declineIsRememberedKeepsRecordingAndCanBeReviewedAgain() {
        openTextInsertion()
        compose.onNodeWithTag("accessibility-decline").performClick()
        Assert.assertEquals("declined", consent())
        compose.onNodeWithText("Declined · record in TrickTic Dictate and copy from History. Tap to review.").performScrollTo().assertIsDisplayed()
        screenshot(compose.activity, "accessibility-declined")
        compose.activityRule.scenario.recreate()
        compose.onNodeWithText("Declined · record in TrickTic Dictate and copy from History. Tap to review.").performScrollTo().assertIsDisplayed()

        val context = compose.activity.applicationContext
        val engine = context.getSharedPreferences("phone-engine", 0)
        val before = engine.all
        val manager = SessionManager.get(context)
        var id: String? = null
        DictationChoice(context).save(true, "base", "en") // Never contact a live provider in UI tests.
        try {
            tab("Home"); compose.onNodeWithTag("start-recording").performClick()
            compose.waitUntil(5000) { CaptureService.state == "recording" }
            id = CaptureService.activeSessionId!!
            Thread.sleep(500)
            compose.onNodeWithContentDescription("TrickTic Dictate stop recording").performClick()
            compose.waitUntil(15000) { !CaptureService.isActive() && !manager.isBusy() }
            Assert.assertTrue("Audio is saved without text insertion", manager.store.audio(manager.store.load(id!!)).length() > 44)
            // The unconfigured phone model fails the attempt; wait for that before deleting.
            compose.waitUntil(15000) { manager.store.load(id!!).let { !it.has("pending") && !it.has("pending_postprocess") } && !manager.isBusy() }
        } finally {
            if (CaptureService.isActive()) compose.activityRule.scenario.onActivity { CaptureService.stop(it, true) }
            manager.cancelWork()
            engine.edit().clear().apply { before.forEach { (key, value) -> when (value) { is String -> putString(key, value); is Boolean -> putBoolean(key, value) } } }.commit()
            id?.let { runCatching { manager.store.deleteSaved(it) } }
        }
        Assert.assertEquals("Declining never opens Accessibility settings", 0, starts.accessibility())

        openTextInsertion()
        compose.onNodeWithTag("accessibility-disclosure").assertIsDisplayed()
        instrumentation.sendKeyDownUpSync(android.view.KeyEvent.KEYCODE_BACK)
        compose.waitUntil(5000) { compose.onAllNodesWithTag("accessibility-disclosure").fetchSemanticsNodes().isEmpty() }
        Assert.assertEquals("Dismissing records nothing", "declined", consent())
        Assert.assertEquals(0, starts.accessibility())
    }

    @Test fun homeOpensDisclosureDirectlyAndBackDoesNotConsent() {
        tab("Home")
        compose.onNodeWithText("Type into other apps").performScrollTo().performClick()
        compose.onNodeWithTag("accessibility-disclosure").assertIsDisplayed()
        instrumentation.sendKeyDownUpSync(android.view.KeyEvent.KEYCODE_BACK)
        compose.waitUntil(5000) { compose.onAllNodesWithTag("accessibility-disclosure").fetchSemanticsNodes().isEmpty() }
        Assert.assertEquals("declined", consent())
        Assert.assertEquals(0, starts.accessibility())
        compose.onNodeWithTag("start-recording").assertExists()
        compose.onNodeWithText("Type into other apps").performScrollTo().performClick()
        compose.onNodeWithTag("accessibility-accept").performClick()
        Assert.assertEquals("accepted", consent())
        Assert.assertEquals(0, starts.accessibility())
        compose.onNodeWithText("Continue").performClick()
        compose.waitUntil(5000) { starts.accessibility() == 1 }
    }

    @Test fun serviceTurnedOnInAndroidSettingsStaysPausedUntilAccepted() {
        fun shell(command: String) = android.os.ParcelFileDescriptor.AutoCloseInputStream(instrumentation.uiAutomation.executeShellCommand(command)).bufferedReader().use { it.readText() }.trim()
        val component = "${compose.activity.packageName}/com.metrix.writer.WriterInputService"
        val before = shell("settings get secure enabled_accessibility_services")
        try {
            shell("settings put secure enabled_accessibility_services $component")
            compose.activityRule.scenario.recreate()
            compose.onNodeWithText("Text insertion is paused").performScrollTo().assertIsDisplayed()
            screenshot(compose.activity, "home-insertion-paused")
            compose.onNodeWithText("Text insertion is paused").performClick()
            compose.onNodeWithTag("accessibility-disclosure").assertIsDisplayed()
            compose.onNodeWithTag("accessibility-accept").performClick()
            Assert.assertEquals("accepted", consent())
            Assert.assertEquals("Already on: Accept resumes it without opening Settings", 0, starts.accessibility())
            compose.onAllNodesWithText("Enable TrickTic Dictate · voice insertion").assertCountEquals(0)
            tab("Home"); compose.onAllNodesWithText("Text insertion is paused").assertCountEquals(0)
        } finally {
            shell(if (before.isEmpty() || before == "null") "settings delete secure enabled_accessibility_services" else "settings put secure enabled_accessibility_services $before")
        }
    }

    @Test fun privacyAndTermsOpenTheSharedUrlsFromAboutAndSetup() {
        tab("Settings"); compose.onNodeWithText("Storage & about").performScrollTo().performClick()
        compose.onNodeWithTag("privacy-link").performScrollTo().performClick()
        compose.onNodeWithTag("terms-link").performScrollTo().performClick()
        screenshot(compose.activity, "about-legal")
        compose.onNodeWithContentDescription("Back").performClick()
        compose.onNodeWithTag("settings-setup").performScrollTo().performClick()
        compose.onNodeWithTag("privacy-link").performScrollTo().performClick()
        compose.onNodeWithTag("terms-link").performScrollTo().performClick()
        screenshot(compose.activity, "setup-legal")
        compose.waitUntil(5000) { starts.links().size == 4 }
        Assert.assertEquals(listOf(LegalLinks.PRIVACY, LegalLinks.TERMS, LegalLinks.PRIVACY, LegalLinks.TERMS), starts.links())
        Assert.assertEquals("https://legal.tricktic.com/tricktic-dictate/privacy", LegalLinks.PRIVACY)
    }

    @Test fun licensesListBundledLibrariesModelsAndFullTexts() {
        tab("Settings"); compose.onNodeWithText("Storage & about").performScrollTo().performClick()
        compose.onNodeWithTag("licenses-link").performScrollTo().performClick()
        compose.onNodeWithText("Built with open source.").assertIsDisplayed()
        screenshot(compose.activity, "licenses")
        val rows = compose.activity.assets.open("licenses/dependencies.tsv").bufferedReader().use { it.readLines() }.filter { it.isNotBlank() }
        Assert.assertTrue(rows.size > 50)
        // Google's sign-in libraries, under Google's SDK terms, ship only in the dev build (MW-OUATH-4).
        val dev = BuildConfig.FLAVOR == "dev"
        val google = setOf("Android-SDK-License", "Play-Core-Terms", "Play-Integrity-Terms")
        Assert.assertTrue(rows.all { it.substringAfter('\t') == "Apache-2.0" || (dev && it.substringAfter('\t') in google) })
        Assert.assertEquals(dev, rows.any { it.startsWith("com.google.firebase:firebase-auth:") && it.endsWith("\tAndroid-SDK-License") })
        compose.onNodeWithTag("license-dependencies").performScrollTo().assert(hasText("androidx.compose.ui:ui:", substring = true))
        if (dev) compose.onNodeWithTag("license-dependencies").assert(hasText("com.google.firebase:firebase-auth:24.2.0 · Android SDK License", substring = true))
        compose.onNodeWithText("whisper.cpp v1.9.4, including ggml · MIT").performScrollTo().assertIsDisplayed()
        compose.onNodeWithText("base · ggml-base.bin", substring = true).performScrollTo().assertIsDisplayed()
        mapOf("Apache-2.0" to "TERMS AND CONDITIONS FOR USE, REPRODUCTION, AND DISTRIBUTION",
            "LLVM-exception" to "LLVM Exceptions to the Apache 2.0 License",
            "whisper.cpp" to "The ggml authors",
            "Whisper-models" to "Copyright (c) 2022 OpenAI").plus(if (dev) mapOf("Google-SDK-terms" to "https://developer.android.com/studio/terms") else emptyMap()).forEach { (id, expected) ->
            compose.onNodeWithTag("license-$id").performScrollTo().performClick()
            compose.onNodeWithTag("license-text-$id").performScrollTo().assert(hasText(expected, substring = true))
        }
        if (!dev) compose.onNodeWithTag("license-Google-SDK-terms").assertDoesNotExist()
        screenshot(compose.activity, "licenses-text")
    }
}

@RunWith(AndroidJUnit4::class)
class OnboardingLegalTest {
    @get:Rule val compose = createEmptyComposeRule()
    @Test fun exploringFirstShowsSeparateDisclosureWithoutOpeningSettings() {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val context = instrumentation.targetContext
        val saved = SavedPreferences(context, "writer-appearance", "writer-consent")
        saved.clear("writer-consent")
        AppearancePreferences(context).onboarding = false
        val starts = OutgoingStarts()
        instrumentation.addMonitor(starts)
        try {
            ActivityScenario.launch(MainActivity::class.java).use { scenario ->
                compose.onNodeWithText("Explore first").performClick()
                compose.onNodeWithTag("accessibility-disclosure").assertIsDisplayed()
                compose.onNodeWithTag("privacy-link").assertDoesNotExist() // Dedicated disclosure, not combined with welcome/legal.
                screenshot(context, "first-launch-disclosure")
                compose.onNodeWithText("The text and window information", substring = true).performScrollTo().assertIsDisplayed()
                screenshot(context, "first-launch-disclosure-data")
                Assert.assertEquals("unset", AccessibilityConsent(context).choice)
                Assert.assertEquals(0, starts.accessibility())
                compose.onNodeWithTag("accessibility-decline").performClick()
                Assert.assertEquals("declined", AccessibilityConsent(context).choice)
                compose.onNodeWithTag("start-recording").assertExists()
                scenario.recreate()
                compose.onNodeWithTag("accessibility-disclosure").assertDoesNotExist()
                compose.onNodeWithText("Type into other apps").performScrollTo().performClick()
                compose.onNodeWithTag("accessibility-disclosure").assertIsDisplayed()
                compose.onNodeWithTag("accessibility-decline").performClick()
                Assert.assertEquals(0, starts.accessibility())
            }
        } finally { instrumentation.removeMonitor(starts); saved.restore() }
    }

    @Test fun backOnFreshDisclosureLeavesConsentUnsetAndHomeCanRetry() {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val context = instrumentation.targetContext
        val saved = SavedPreferences(context, "writer-appearance", "writer-consent")
        saved.clear("writer-consent")
        AppearancePreferences(context).onboarding = true
        val starts = OutgoingStarts()
        instrumentation.addMonitor(starts)
        try {
            ActivityScenario.launch(MainActivity::class.java).use {
                compose.onNodeWithTag("accessibility-disclosure").assertIsDisplayed()
                instrumentation.sendKeyDownUpSync(android.view.KeyEvent.KEYCODE_BACK)
                compose.waitUntil(5000) { compose.onAllNodesWithTag("accessibility-disclosure").fetchSemanticsNodes().isEmpty() }
                Assert.assertEquals("unset", AccessibilityConsent(context).choice)
                compose.onNodeWithTag("start-recording").assertExists()
                compose.onNodeWithText("Type into other apps").performScrollTo().performClick()
                compose.onNodeWithTag("accessibility-disclosure").assertIsDisplayed()
                compose.onNodeWithTag("accessibility-decline").performClick()
                Assert.assertEquals(0, starts.accessibility())
            }
        } finally { instrumentation.removeMonitor(starts); saved.restore() }
    }

    @Test fun setupFirstAlsoShowsDisclosureBeforeAnyAccessibilitySettings() {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val context = instrumentation.targetContext
        val saved = SavedPreferences(context, "writer-appearance", "writer-consent")
        saved.clear("writer-consent")
        AppearancePreferences(context).onboarding = false
        val starts = OutgoingStarts()
        instrumentation.addMonitor(starts)
        try {
            ActivityScenario.launch(MainActivity::class.java).use {
                compose.onNodeWithText("Set up TrickTic Dictate").performClick()
                compose.onNodeWithTag("accessibility-disclosure").assertIsDisplayed()
                Assert.assertEquals(0, starts.accessibility())
                compose.onNodeWithTag("accessibility-accept").performClick()
                Assert.assertEquals("accepted", AccessibilityConsent(context).choice)
                Assert.assertEquals(0, starts.accessibility())
                compose.onNodeWithText("Continue").performClick()
                compose.waitUntil(5000) { starts.accessibility() == 1 }
            }
        } finally { instrumentation.removeMonitor(starts); saved.restore() }
    }

    @Test fun welcomeOffersPrivacyAndTermsBeforeSetup() {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val saved = SavedPreferences(context, "writer-appearance")
        val starts = OutgoingStarts()
        InstrumentationRegistry.getInstrumentation().addMonitor(starts)
        AppearancePreferences(context).onboarding = false
        try {
            ActivityScenario.launch(MainActivity::class.java).use {
                compose.onNodeWithText("Your voice. Your words.").assertIsDisplayed()
                screenshot(context, "welcome-legal")
                compose.onNodeWithTag("privacy-link").performClick()
                compose.onNodeWithTag("terms-link").performClick()
                compose.waitUntil(5000) { starts.links().size == 2 }
                Assert.assertEquals(listOf(LegalLinks.PRIVACY, LegalLinks.TERMS), starts.links())
            }
        } finally {
            InstrumentationRegistry.getInstrumentation().removeMonitor(starts)
            saved.restore()
        }
    }
}
