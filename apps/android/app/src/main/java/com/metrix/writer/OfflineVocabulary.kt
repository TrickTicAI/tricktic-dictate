package com.metrix.writer

import org.json.JSONObject

/** Snippets ignore case/whitespace; corrections stay literal. Exact values are never rescanned. */
object OfflineVocabulary {
    private data class Rule(val priority: Int, val phrase: String, val text: String)
    private fun word(c: Int): Boolean = Character.isLetterOrDigit(c) || Character.getType(c) == Character.LETTER_NUMBER.toInt() || Character.getType(c) == Character.OTHER_NUMBER.toInt() || c == 95 ||
        c in 0x0300..0x036f || c in 0x0591..0x05bd || c == 0x05bf || c in 0x05c1..0x05c2 || c in 0x05c4..0x05c5 || c == 0x05c7 || c in 0x0610..0x061a || c in 0x064b..0x065f || c in 0x1ab0..0x1aff || c in 0x1dc0..0x1dff || c in 0x200c..0x200d || c in 0x203f..0x2040 || c in 0x20d0..0x20ff || c in 0xfe20..0xfe2f
    data class Segment(val text: String, val exact: Boolean)
    fun process(raw: String, vocabulary: JSONObject): String = segments(raw, vocabulary).joinToString("") { it.text }
    private fun whitespace(c: Int) = Character.isWhitespace(c) || Character.isSpaceChar(c) || c == 0x85
    private fun lower(c: Int) = String(Character.toChars(c)).lowercase(java.util.Locale.ROOT)
    private fun matchEnd(raw: String, offset: Int, rule: Rule): Int? {
        if (offset > 0 && word(raw.codePointBefore(offset))) return null
        var end = offset
        if (rule.priority == 0) {
            var trigger = 0
            while (trigger < rule.phrase.length) {
                if (end >= raw.length) return null
                val expected = rule.phrase.codePointAt(trigger); val actual = raw.codePointAt(end)
                if (whitespace(expected) && whitespace(actual)) {
                    do { trigger += Character.charCount(rule.phrase.codePointAt(trigger)) }
                    while (trigger < rule.phrase.length && whitespace(rule.phrase.codePointAt(trigger)))
                    do { end += Character.charCount(raw.codePointAt(end)) }
                    while (end < raw.length && whitespace(raw.codePointAt(end)))
                } else {
                    if (lower(actual) != lower(expected)) return null
                    trigger += Character.charCount(expected); end += Character.charCount(actual)
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
        val result = mutableListOf<Segment>()
        val out = StringBuilder(); var offset = 0
        while (offset < raw.length) {
            val match = rules.firstNotNullOfOrNull { rule -> matchEnd(raw, offset, rule)?.let { rule to it } }
            if (match != null) {
                if (out.isNotEmpty()) { result.add(Segment(out.toString(), false)); out.clear() }
                result.add(Segment(match.first.text, true)); offset = match.second
            }
            else { val c = raw.codePointAt(offset); out.appendCodePoint(c); offset += Character.charCount(c) }
        }
        if (out.isNotEmpty()) result.add(Segment(out.toString(), false))
        // Match desktop's snippet-only join: discard recognizer marks outside the
        // exact values, but keep all characters of those values and between them.
        val onlyExact = result.any { it.exact } && result.none { segment ->
            !segment.exact && segment.text.codePoints().anyMatch { Character.isLetterOrDigit(it) ||
                Character.getType(it) == Character.LETTER_NUMBER.toInt() || Character.getType(it) == Character.OTHER_NUMBER.toInt() }
        }
        if (onlyExact) {
            if (result.firstOrNull()?.exact == false) result.removeAt(0)
            if (result.lastOrNull()?.exact == false) result.removeAt(result.lastIndex)
        }
        return result
    }
}
