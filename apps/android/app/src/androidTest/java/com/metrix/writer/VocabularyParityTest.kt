@file:Suppress("DEPRECATION")
package com.metrix.writer

import android.test.InstrumentationTestCase
import org.json.JSONArray

class VocabularyParityTest : InstrumentationTestCase() {
    fun testSameCasesAsRustIncludingExactValuesAndHebrewBoundaries() {
        val cases = JSONArray(instrumentation.context.assets.open("vocabulary-parity.json").bufferedReader().use { it.readText() })
        for (i in 0 until cases.length()) {
            val case = cases.getJSONObject(i)
            assertEquals("case $i", case.getString("expected"), OfflineVocabulary.process(case.getString("raw"), case.getJSONObject("vocabulary")))
        }
    }
}
