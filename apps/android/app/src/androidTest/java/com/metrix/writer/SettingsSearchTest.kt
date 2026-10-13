@file:Suppress("DEPRECATION")
package com.metrix.writer

import android.test.AndroidTestCase
import com.metrix.writer.ui.SettingsSearch

/** Matching for the Settings search field; WriterUiTest covers opening a result. */
class SettingsSearchTest : AndroidTestCase() {
    private fun titles(query: String) = SettingsSearch.search(query).map { it.title }

    fun testMisspellingsStillMatch() {
        assertTrue(titles("Cleeboard").contains("Text insertion"))
        assertEquals("Microphone", titles("microfone").first())
        assertTrue(titles("languge").contains("Speech language"))
    }

    fun testEveryWordMustMatchAndTitlesRankFirst() {
        assertEquals("Theme", titles("dark mode").first())
        assertEquals("Microphone", titles("mic").first())
        assertTrue(titles("delete history").contains("Your data"))
        assertEquals(emptyList<String>(), titles("clipboard nonsenseword"))
        assertEquals(emptyList<String>(), titles("   "))
    }

    fun testOptionsButtonsAndDescriptionsAreSearchable() {
        assertEquals("Temporary hiding", titles("snooze").first())
        assertTrue(titles("dock to nearest edge").contains("Placement"))
        assertTrue(titles("accessibility consent").contains("Text insertion"))
        assertTrue(titles("privacy policy").contains("Legal"))
        // Self-update and sideload help exist only in the dev build; the Play build must not mention them.
        val dev = BuildConfig.FLAVOR == "dev"
        assertEquals(dev, titles("restricted settings").contains("Android won't let me enable insertion"))
        assertEquals(dev, titles("check for updates").contains("$PRODUCT_NAME updates"))
        // Optional Google sign-in is reachable only in the dev build until MW-RELEASE-12 (MW-OUATH-4).
        assertEquals(dev, titles("sign in with google").contains("Google account"))
        assertEquals(dev, titles("logout").contains("Google account"))
        assertTrue(titles("ocean").contains("Theme"))
    }

    fun testPolishingInstructionsAreSearchable() {
        assertEquals("Writing style", titles("writing style").first())
        assertTrue(titles("prompt").contains("Instructions"))
        assertTrue(titles("rewrite strength").contains("Writing style"))
        assertTrue(titles("punctuation only").contains("Style"))
        assertTrue(titles("offline dictation polishing").contains("Polishing"))
    }

    fun testProvidersAndModelsFromTheCatalogsAreSearchable() {
        val entries = SettingsSearch.index + SettingsSearch.catalog(listOf("whisper-base-q5"))
        CloudProviders.all.forEach { provider ->
            assertTrue(provider.name, SettingsSearch.search(provider.name, entries).any { it.title == provider.name })
        }
        assertEquals("whisper-base-q5", SettingsSearch.search("base q5", entries).first().title)
    }

    fun testKeysAreFoundByTheWordsPeopleUse() {
        val entries = SettingsSearch.index + SettingsSearch.catalog(emptyList())
        listOf("api key", "manage keys", "token").forEach { assertEquals(it, "API keys", SettingsSearch.search(it, entries).first().title) }
        assertEquals("OpenRouter key", SettingsSearch.search("openrouter key", entries).first().title)
    }

    fun testEveryEntryOpensAKnownPage() {
        (SettingsSearch.index + SettingsSearch.catalog(listOf("model"))).forEach { assertTrue(it.page, it.page in SettingsSearch.pageNames) }
    }
}
