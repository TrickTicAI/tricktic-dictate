@file:Suppress("DEPRECATION")
package com.metrix.writer

import android.test.AndroidTestCase

/** MW-SETTINGS-8: the phone's polishing prompt, its Super default, styles and validation. */
class PolishInstructionsTest : AndroidTestCase() {
    private val prefs get() = CompanionPreferences(context)
    private lateinit var before: Map<String, *>

    override fun setUp() {
        super.setUp()
        before = context.getSharedPreferences("companion", 0).all
        context.getSharedPreferences("companion", 0).edit()
            .remove("polish_style").remove("polish_instructions").remove("polish_formatting").commit()
    }

    override fun tearDown() {
        val edit = context.getSharedPreferences("companion", 0).edit().clear()
        before.forEach { (key, value) -> when (value) { is String -> edit.putString(key, value); is Boolean -> edit.putBoolean(key, value) } }
        edit.commit()
        super.tearDown()
    }

    fun testFreshInstallPolishesLikeTheComputer() {
        assertEquals("super", prefs.polishStyle())
        assertEquals(PolishPrompts.SUPER, prefs.polish().instructions)
        assertEquals(PolishFormatting(), prefs.polish().formatting)
        assertTrue(prefs.isPolishDefault())
        val prompt = prefs.polishSystemPrompt()
        assertTrue(prompt.startsWith(PolishPrompts.SUPER))
        assertTrue(prompt.contains("Formatting preferences:"))
        assertTrue(prompt.contains("Polish grammar and flow"))
        assertTrue(prompt.endsWith(PolishPrompts.TRAILER))
    }

    fun testLightKeepsThePunctuationOnlyPrompt() {
        prefs.savePolish("light", prefs.polish().instructions, prefs.polish().formatting)
        assertEquals("light", prefs.polishStyle())
        assertEquals(PolishPrompts.LIGHT, prefs.polishSystemPrompt())
        assertFalse(prefs.isPolishDefault())
    }

    fun testEditedInstructionsAndFormattingApplyToTheNextPrompt() {
        prefs.savePolish("super", "Fixture edited instructions.", PolishFormatting("concise", "single_line", false, false, false, false))
        assertFalse(prefs.isPolishDefault())
        val prompt = prefs.polishSystemPrompt()
        assertTrue(prompt.startsWith("Fixture edited instructions."))
        assertTrue(prompt.contains("concise finished prose"))
        assertTrue(prompt.contains("Keep editable output on one line"))
        assertTrue(prompt.contains("Keep enumerations inline"))
        assertTrue(prompt.contains("Preserve fillers, repetitions and false starts."))
        assertTrue(prompt.contains("Preserve spoken corrections as spoken"))
        assertTrue(prompt.contains("Do not convert spoken punctuation"))
        prefs.resetPolish()
        assertTrue(prefs.isPolishDefault())
        assertEquals(PolishPrompts.SUPER, prefs.polish().instructions)
    }

    fun testFormattingLinesCoverEveryPreference() {
        val lines = PolishFormatting().instructions().split("\n")
        assertEquals(7, lines.size)
        assertTrue(lines[0].contains("Polish grammar and flow"))
        assertTrue(lines[1].contains("paragraph breaks between distinct ideas"))
        assertTrue(lines[2].contains("vertical list"))
        assertTrue(lines[3].contains("Remove nonmeaningful fillers"))
        assertTrue(lines[4].contains("Tuesday, actually Thursday"))
        assertTrue(lines[5].contains("open quote"))
        assertTrue(lines[6].startsWith("These preferences override conflicting style requests above."))
        assertTrue(PolishFormatting("light", "automatic", true, true, true, true).instructions().contains("light grammatical and punctuation corrections"))
        assertTrue(PolishFormatting("polished", "automatic", false, true, true, true).instructions().contains("Keep enumerations inline"))
    }

    fun testValidationRejectsBlankOversizeAndNul() {
        assertNotNull(PolishPrompts.validate(""))
        assertNotNull(PolishPrompts.validate("   "))
        assertNotNull(PolishPrompts.validate("x".repeat(16385)))
        assertNotNull(PolishPrompts.validate("a" + 0.toChar() + "b"))
        assertNull(PolishPrompts.validate("Polish this dictation."))
        assertNull(PolishPrompts.validate("א".repeat(8192)))
        assertNull(PolishPrompts.validate(PolishPrompts.SUPER))
        try { prefs.savePolish("super", "   ", PolishFormatting()); fail("blank instructions saved") }
        catch (_: IllegalArgumentException) { }
        try { prefs.savePolish("bogus", "Polish this dictation.", PolishFormatting()); fail("bogus style saved") }
        catch (_: IllegalArgumentException) { }
        assertTrue(prefs.isPolishDefault())
    }

    fun testUnreadableStoredValuesReadAsDefault() {
        context.getSharedPreferences("companion", 0).edit()
            .putString("polish_style", "bogus").putString("polish_instructions", "   ").putString("polish_formatting", "not json").commit()
        assertEquals("super", prefs.polishStyle())
        assertEquals(PolishPrompts.defaults(), prefs.polish())
    }

    fun testPortableRoundTrip() {
        val portable = PolishPrompts.fromJson(PolishFormatting("concise", "single_line", false, true, false, true).let {
            org.json.JSONObject().put("instructions", "Portable fixture.").put("formatting", it.toJson())
        })
        assertEquals(PolishInstructions("Portable fixture.", PolishFormatting("concise", "single_line", false, true, false, true)), portable)
        assertNull(PolishPrompts.fromJson(org.json.JSONObject().put("instructions", "   ").put("formatting", PolishFormatting().toJson())))
        assertNull(PolishFormatting.fromJson(org.json.JSONObject().put("rewrite", "flowery")))
    }
    fun testValidationPermissionsAreFrozenAndLightDoesNotRemoveWords() {
        prefs.savePolish("super", prefs.polish().instructions, PolishFormatting(removeFillers = true, spokenPunctuation = true))
        val frozen = prefs.polishPolicy(null)
        assertTrue(frozen.getBoolean("remove_fillers")); assertTrue(frozen.getBoolean("spoken_punctuation"))
        prefs.savePolish("light", prefs.polish().instructions, prefs.polish().formatting)
        val light = prefs.polishPolicy(null)
        assertFalse(light.getBoolean("remove_fillers")); assertFalse(light.getBoolean("spoken_punctuation"))
        assertTrue(frozen.getBoolean("remove_fillers")); assertTrue(frozen.getBoolean("spoken_punctuation"))
    }

}
