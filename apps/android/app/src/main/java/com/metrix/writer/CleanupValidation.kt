package com.metrix.writer

import java.util.Locale

/** Conservative dictation checks; never a proof of semantic equivalence. See cleanup-meaning.md. */
internal object CleanupValidation {
    private val cues = listOf(
        "open parentheses" to "(",
        "close parentheses" to ")",
        "open parenthesis" to "(",
        "close parenthesis" to ")",
        "open quote" to "\"“",
        "close quote" to "\"”",
        "double quote" to "\"“”",
        "single quote" to "'‘’",
        "new paragraph" to "\n",
        "new line" to "\n",
        "dot dot dot" to "…",
        "question mark" to "?",
        "exclamation mark" to "!",
        "פתח מירכאות" to "\"“",
        "סגור מירכאות" to "\"”",
        "פתח מרכאות" to "\"“",
        "סגור מרכאות" to "\"”",
        "פתח סוגריים" to "(",
        "סגור סוגריים" to ")",
        "פסקה חדשה" to "\n",
        "שורה חדשה" to "\n",
        "סימן שאלה" to "?",
        "סימן קריאה" to "!",
        "שלוש נקודות" to "…",
        "נקודתיים" to ":",
        "פסיק" to ",",
        "נקודה" to ".",
        "colon" to ":",
        "comma" to ",",
        "ellipsis" to "…",
        "apostrophe" to "'’"
    )
    private val contractions = mapOf(
        "don't" to "do not", "doesn't" to "does not", "didn't" to "did not",
        "can't" to "can not", "cannot" to "can not", "won't" to "will not", "isn't" to "is not",
        "aren't" to "are not", "wasn't" to "was not", "weren't" to "were not",
        "shouldn't" to "should not", "wouldn't" to "would not", "couldn't" to "could not",
        "mustn't" to "must not", "hasn't" to "has not", "haven't" to "have not", "hadn't" to "had not",
    )
    private data class Tokens(val values: MutableList<String> = mutableListOf(), val gaps: MutableList<StringBuilder> = mutableListOf(StringBuilder())) {
        fun push(value: String) { values.add(value); gaps.add(StringBuilder()) }
    }
    private fun digit(c: Char) = c in '0'..'9'
    private fun letter(c: Char) = c.isLetter() || c in '\u0300'..'\u036f' || c in '\u0591'..'\u05c7'
    private fun formatting(c: Char) = c.isWhitespace() || c in ".,;:!?\"'’‘“”„«»()[]{}…-–—"
    private fun numericStart(text: String, i: Int) = text.getOrNull(i)?.let { digit(it) || it in ".," && text.getOrNull(i+1)?.let(::digit) == true } == true
    private fun tokens(text: String): Tokens {
        val result = Tokens(); var i = 0
        while (i < text.length) {
            val c = text[i]
            if (!c.isWhitespace() && (i == 0 || text[i-1].isWhitespace())) {
                val end = (i until text.length).firstOrNull { text[it].isWhitespace() } ?: text.length
                val chunk = text.substring(i, end)
                if (chunk.contains("://") || chunk.startsWith("www.") || chunk.contains('@')) {
                    result.push("x:$chunk"); i = end; continue
                }
            }
            if (c in "-−–—" && !numericStart(text, i+1) && (i+1 until text.length).firstOrNull { !text[it].isWhitespace() }?.let { numericStart(text, it) } == true) {
                result.push("s:-"); i++; continue
            }
            if (numericStart(text, i) || c in "+-−–—" && numericStart(text, i+1)) {
                val start = i; i++
                while (i < text.length && (digit(text[i]) || text[i] in ".,:/-" && text.getOrNull(i+1)?.let(::digit) == true)) i++
                result.push("n:" + text.substring(start, i).map { if (it in "−–—") '-' else it }.joinToString(""))
            } else if (letter(c)) {
                val start = i; i++
                while (i < text.length && (letter(text[i]) || text[i].isDigit() || text[i] in "'’" && text.getOrNull(i+1)?.let(::letter) == true)) i++
                val value = text.substring(start, i).replace('’', '\'').lowercase(Locale.ROOT)
                (contractions[value] ?: value).split(" ").forEach { result.push("w:$it") }
            } else if (!formatting(c)) {
                result.push("s:$c"); i++
            } else {
                result.gaps.last().append(c); i++
            }
        }
        var at = 0
        while (at < result.values.size) {
            if (result.values[at] in listOf("w:minus", "w:מינוס", "w:plus", "w:פלוס") && result.values.getOrNull(at+1)?.startsWith("n:") == true && result.gaps[at+1].isBlank()) {
                val sign = if (result.values[at] in listOf("w:minus", "w:מינוס")) "-" else "+"
                result.values[at] = "n:$sign" + result.values[at+1].substring(2)
                result.values.removeAt(at+1); result.gaps.removeAt(at+1)
            }
            if (result.values[at].startsWith("n:") && result.values.getOrNull(at+1) in listOf("w:point", "w:נקודה") && result.values.getOrNull(at+2)?.let { it.startsWith("n:") && it.substring(2).all(::digit) } == true && result.gaps[at+1].isBlank() && result.gaps[at+2].isBlank()) {
                result.values[at] += "." + result.values[at+2].substring(2)
                repeat(2) { result.values.removeAt(at+1); result.gaps.removeAt(at+1) }
            }
            at++
        }
        return result
    }
    fun preserves(original: String, output: String, spoken: Boolean, fillers: Boolean): Boolean {
        if (output.isBlank() || '\u0000' in output || output.toByteArray().size > original.toByteArray().size * 8L + 8192L) return false
        val source = tokens(original); val target = tokens(output); var i = 0; var j = 0
        while (i < source.values.size) {
            if (target.values.getOrNull(j) == source.values[i]) { i++; j++; continue }
            if (fillers && source.values[i] in listOf("w:um", "w:uh", "w:אממ")) { i++; continue }
            val literal = source.values.subList((i-4).coerceAtLeast(0), i).any { it in listOf(
                "w:not", "w:no", "w:never", "w:לא", "w:אל", "w:בלי", "w:word", "w:literal", "w:called", "w:מילה", "w:המילה") }
            val cue = if (spoken && !literal) cues.firstOrNull { (cue, marks) ->
                val words = cue.split(" ").map { "w:$it" }
                i + words.size <= source.values.size && source.values.subList(i, i + words.size) == words && marks.any { it in target.gaps[j] }
            } else null
            if (cue != null) i += cue.first.split(" ").size else return false
        }
        return j == target.values.size
    }
}
