package com.metrix.writer

import org.json.JSONObject

/** Snippets ignore case/whitespace; corrections stay literal. Exact values are never rescanned. */
object OfflineVocabulary {
    private data class Rule(val priority: Int, val phrase: String, val text: String)
    private fun alphanumeric(c: Int): Boolean = Character.isAlphabetic(c) || Character.isDigit(c) || Character.getType(c) == Character.LETTER_NUMBER.toInt() || Character.getType(c) == Character.OTHER_NUMBER.toInt()
    private fun word(c: Int): Boolean = alphanumeric(c) || c == 95 ||
        c in 0x0300..0x036f || c in 0x0591..0x05bd || c == 0x05bf || c in 0x05c1..0x05c2 || c in 0x05c4..0x05c5 || c == 0x05c7 || c in 0x0610..0x061a || c in 0x064b..0x065f || c in 0x1ab0..0x1aff || c in 0x1dc0..0x1dff || c in 0x200c..0x200d || c in 0x203f..0x2040 || c in 0x20d0..0x20ff || c in 0xfe20..0xfe2f
    data class Segment(val text: String, val exact: Boolean)
    fun process(raw: String, vocabulary: JSONObject): String = segments(raw, vocabulary).joinToString("") { it.text }
    private fun whitespace(c: Int) = c in 0x09..0x0d || Character.isSpaceChar(c) || c == 0x85
    private fun lower(c: Int) = String(Character.toChars(c)).lowercase(java.util.Locale.ROOT)
    private fun matchEnd(raw: String, offset: Int, rule: Rule): Int? {
        if (offset > 0 && word(raw.codePointBefore(offset))) return null
        var end = offset
        if (rule.priority == 0) {
            var trigger = 0
            while (trigger < rule.phrase.length) {
                val expected = rule.phrase.codePointAt(trigger)
                trigger += Character.charCount(expected)
                if (whitespace(expected)) {
                    while (trigger < rule.phrase.length && whitespace(rule.phrase.codePointAt(trigger))) {
                        trigger += Character.charCount(rule.phrase.codePointAt(trigger))
                    }
                    if (trigger < rule.phrase.length) {
                        while (end < raw.length) {
                            val separator = raw.codePointAt(end)
                            if (!whitespace(separator) && separator != ','.code && separator != '-'.code) break
                            end += Character.charCount(separator)
                        }
                    }
                } else {
                    if (end >= raw.length) return null
                    val actual = raw.codePointAt(end)
                    if (lower(actual) != lower(expected)) return null
                    end += Character.charCount(actual)
                }
            }
        } else {
            if (!raw.startsWith(rule.phrase, offset)) return null
            end += rule.phrase.length
        }
        return end.takeIf { it == raw.length || !word(raw.codePointAt(it)) }
    }
    fun segments(raw: String, vocabulary: JSONObject): List<Segment> {
        val rules = mutableListOf<Rule>()
        for ((priority, kind) in listOf("snippets", "corrections").withIndex()) {
            val rows = vocabulary.getJSONArray(kind)
            for (i in 0 until rows.length()) {
                val row = rows.getJSONObject(i)
                val phrase = row.getString(if (priority == 0) "phrase" else "from")
                require(phrase.isNotBlank())
                rules.add(Rule(priority, phrase, row.getString(if (priority == 0) "text" else "to")))
            }
        }
        rules.sortWith(compareBy<Rule> { it.priority }.thenByDescending { it.phrase.toByteArray(Charsets.UTF_8).size })
        if (rules.none { it.priority == 0 }) return scan(raw, rules, false)
        return scan(raw, rules, true)
    }

    private fun scan(raw: String, rules: List<Rule>, allowSnippets: Boolean): List<Segment> {
        val result = mutableListOf<Segment>()
        var start = 0
        var offset = 0
        while (offset < raw.length) {
            val match = rules.firstNotNullOfOrNull { rule -> matchEnd(raw, offset, rule)?.let { rule to it } }
            if (allowSnippets && (match?.first?.priority == 1 ||
                    (match == null && alphanumeric(raw.codePointAt(offset))))) {
                // Ordinary speech, including corrected words, cancels all snippet expansions.
                return scan(raw, rules.filter { it.priority == 1 }, false)
            }
            if (match == null) {
                offset += Character.charCount(raw.codePointAt(offset))
                continue
            }
            if (start < offset) result.add(Segment(raw.substring(start, offset), false))
            result.add(Segment(match.first.text, true))
            offset = match.second
            start = offset
        }
        if (start < raw.length) result.add(Segment(raw.substring(start), false))
        if (allowSnippets && result.any { it.exact }) {
            if (!result.first().exact) result.removeAt(0)
            if (!result.last().exact) result.removeAt(result.lastIndex)
            for (i in 1 until result.lastIndex) {
                val gap = result[i]
                if (!gap.exact && '\n' !in gap.text && gap.text.trim { whitespace(it.code) } == "," &&
                    plainWords(result[i - 1].text) && plainWords(result[i + 1].text)) {
                    result[i] = Segment(" ", false)
                }
            }
        }
        return result
    }

    private fun plainWords(value: String) = value.codePoints().allMatch { Character.isAlphabetic(it) || it == ' '.code }
}
