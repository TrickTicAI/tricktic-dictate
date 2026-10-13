package com.metrix.writer

import android.graphics.Bitmap
import androidx.compose.ui.test.*
import androidx.compose.ui.test.junit4.createEmptyComposeRule
import androidx.test.core.app.ActivityScenario
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import com.metrix.writer.ui.AppearancePreferences
import org.json.JSONArray
import org.json.JSONObject
import org.junit.*
import org.junit.runner.RunWith
import java.io.File

/**
 * Google Play listing screenshots from the `play` build, driven by
 * apps/android/scripts/store-screenshots.py. Every dictation, dictionary entry and
 * key below is invented; nothing is transcribed and no provider is contacted.
 * The host script installs a fresh app, fixes the clock, the status bar and the
 * display size, and passes writerStoreScreenshots=1. Without that argument, this
 * test is skipped so it never seeds fixtures into an ordinary test run.
 */
@RunWith(AndroidJUnit4::class)
class StoreScreenshots {
    @get:Rule val compose = createEmptyComposeRule()
    private val instrumentation = InstrumentationRegistry.getInstrumentation()
    private val context get() = instrumentation.targetContext
    private val output get() = File(context.cacheDir, "store-screenshots")
    private var detail = "" // The dictation shown with its polished wording.

    @Before fun guard() {
        Assume.assumeTrue("Run through scripts/store-screenshots.py", InstrumentationRegistry.getArguments().getString("writerStoreScreenshots") == "1")
        Assert.assertTrue("Play listing fixtures need a fresh install", SessionManager.get(context).store.restore().isEmpty())
        Assert.assertTrue("The host script pins the clock", Math.abs(System.currentTimeMillis() - NOW) < 10 * 60_000)
    }

    @Test fun captureListingScreens() {
        seed()
        output.deleteRecursively(); output.mkdirs()
        instrumentation.uiAutomation.grantRuntimePermission(context.packageName, android.Manifest.permission.RECORD_AUDIO)
        instrumentation.uiAutomation.grantRuntimePermission(context.packageName, android.Manifest.permission.POST_NOTIFICATIONS)
        ActivityScenario.launch(MainActivity::class.java).use {
            compose.onNodeWithText("Make room for\nyour thoughts.").assertIsDisplayed()
            shot("01-home")

            tab("History"); compose.onNodeWithText("Search transcripts").assertIsDisplayed()
            shot("02-history")

            compose.onNodeWithTag("dictation-$detail").performClick()
            compose.onNodeWithText("Copy Polished wording").performScrollTo()
            shot("03-spoken-and-polished")

            tab("Dictionary"); compose.onNodeWithText("Snippets").performClick()
            compose.onNodeWithText("sign off").assertIsDisplayed()
            shot("04-dictionary")

            tab("Settings"); compose.onNodeWithText("Floating control").performScrollTo().performClick()
            compose.onNodeWithText("Recording").performClick()
            compose.onNodeWithContentDescription("Live microphone waveform").assertIsDisplayed()
            shot("05-floating-control")

            tab("Settings"); compose.onNodeWithText("Speech engine").performScrollTo().performClick()
            compose.onNodeWithTag("active-engine").assertIsDisplayed()
            shot("06-dictation-engine")

            // The last screen shows the Ink theme; the theme applies when the activity is recreated.
            AppearancePreferences(context).theme = "dark"
            it.recreate()
            tab("Statistics"); compose.onNodeWithText("Your words, in perspective.").assertIsDisplayed()
            compose.onNodeWithText("Show daily totals").performScrollTo()
            shot("07-statistics-ink")
        }
    }

    private fun tab(name: String) { compose.onNodeWithTag("nav-$name").performClick() }

    /** Whole-screen capture, status and navigation bars included, once the UI is idle. */
    private fun shot(name: String) {
        compose.waitForIdle()
        Thread.sleep(700) // Ripples and page transitions finish; the host has disabled animations.
        compose.waitForIdle()
        val image = instrumentation.uiAutomation.takeScreenshot() ?: error("Screenshot failed")
        File(output, "$name.png").outputStream().use { check(image.compress(Bitmap.CompressFormat.PNG, 100, it)) }
    }

    private fun seed() {
        AppearancePreferences(context).onboarding = true
        DirectCloud(context, "gemini").saveKey(FIXTURE_KEY)
        DirectCloud(context, "gemini").saveSelection("gemini-3.8-flash", "en")
        DictationChoice(context).saveLanguage("en")
        CompanionPreferences(context).saveDocument(JSONObject()
            .put("hints", JSONArray(listOf("TrickTic", "Kubernetes", "Tel Aviv", "sourdough").map { JSONObject().put("text", it).put("enabled", true) }))
            .put("corrections", JSONArray().put(JSONObject().put("from", "trick tick").put("to", "TrickTic").put("enabled", true)))
            .put("snippets", JSONArray(listOf(
                "sign off" to "Thanks, and talk soon!",
                "meeting link" to "https://meet.example.com/weekly-sync",
                "standup" to "Yesterday:\nToday:\nBlockers: none",
            ).map { (phrase, text) -> JSONObject().put("phrase", phrase).put("text", text).put("enabled", true) })))
        val store = SessionManager.get(context).store
        DICTATIONS.forEach { dictation ->
            val item = store.create(dictation.language)
            store.audio(item).appendBytes(ByteArray((dictation.seconds * 32_000).toInt()))
            store.finishCapture(item, null)
            val stop = dictation.created + dictation.seconds * 1000
            val timing = JSONObject().put("stop_to_result_ms", dictation.latency)
            val job = JSONObject().put("id", "listing-${dictation.key}").put("execution_location", "cloud").put("transport", "direct_gemini")
                .put("request", JSONObject().put("options", JSONObject().put("selection", JSONObject().put("provider", "gemini").put("model", "gemini-3.8-flash"))))
                .put("delivery_choice", if (dictation.polished == null) "raw" else "cleaned").put("client_timing", timing)
                .put("status", JSONObject().put("state", "succeeded").put("result", JSONObject().put("transcript", JSONObject().put("raw_text", dictation.spoken))))
            dictation.polished?.let { polished ->
                job.put("cleaned_text", polished)
                job.put("processing_attempts", JSONArray().put(JSONObject().put("state", "complete").put("started_at_ms", stop + dictation.latency)
                    .put("finished_at_ms", stop + dictation.latency + 900).put("cleanup", JSONObject().put("state", "succeeded"))
                    .put("selection", JSONObject().put("provider", "gemini").put("model", "gemini-3.8-flash"))))
            }
            item.put("created_at_ms", dictation.created).put("attempts", JSONArray().put(job)).put("last_timing", timing)
                .put("last_status", "Cloud transcript saved on this phone.")
            item.getJSONObject("capture_completion").put("saved_at_ms", stop)
            store.save(item)
            if (dictation.key == "draft") detail = item.getString("id")
        }
    }

    private data class Fixture(val key: String, val created: Long, val seconds: Long, val latency: Long, val spoken: String, val polished: String? = null, val language: String = "en")

    companion object {
        /** 2026-09-23 17:30 UTC. store-screenshots.py sets the emulator clock and time zone to this. */
        const val NOW = 1_790_184_600_000L
        private const val HOUR = 3_600_000L
        private const val DAY = 24 * HOUR
        private const val FIXTURE_KEY = "store-listing-fixture-key-never-sent"
        private val DICTATIONS = listOf(
            Fixture("concert", NOW - 18 * 60_000, 7, 820, "Remind me to pick up the concert tickets on Saturday and book a table for seven."),
            Fixture("draft", NOW - 2 * HOUR, 12, 1_140,
                "Thanks for the quick turnaround on the draft um I left two comments on the budget section and I think we're ready to share it with the team on Thursday",
                "Thanks for the quick turnaround on the draft! I left two comments on the budget section, and I think we're ready to share it with the team on Thursday."),
            Fixture("mixed", NOW - 4 * HOUR, 5, 940, "תזכורת לשלוח את ה-invoice ללקוח עד יום חמישי", language = "mixed"),
            Fixture("groceries", NOW - DAY - 3 * HOUR, 6, 760, "Groceries: oat milk, two lemons, fresh basil, sourdough bread and coffee beans."),
            Fixture("notes", NOW - DAY - 7 * HOUR, 21, 1_380,
                "Meeting notes. The launch moves to October sixth. Design owns the checklist, support updates the FAQ, and we review the numbers next Monday.",
                "Meeting notes: the launch moves to October 6. Design owns the checklist, support updates the FAQ, and we review the numbers next Monday."),
            Fixture("run", NOW - 2 * DAY - 4 * HOUR, 9, 880, "Idea for the weekend run: start at the harbor, loop through the park and finish at the bakery."),
            Fixture("email", NOW - 3 * DAY - 2 * HOUR, 16, 1_020,
                "Hi everyone, quick update, the new build is on the test phones and the crash from last week is fixed. Please try it before Friday.",
                "Hi everyone, quick update: the new build is on the test phones, and last week's crash is fixed. Please try it before Friday."),
            Fixture("reading", NOW - 4 * DAY - 5 * HOUR, 8, 790, "Add to the reading list: the history of the printing press and a beginner's guide to bread baking."),
            Fixture("gift", NOW - 5 * DAY - 1 * HOUR, 6, 700, "Gift ideas for the team party: plants, board games or a good coffee grinder."),
            Fixture("plan", NOW - 6 * DAY - 6 * HOUR, 14, 1_210, "Plan for tomorrow: finish the slides in the morning, call the printer after lunch and send the invoice before five."),
        )
    }
}
