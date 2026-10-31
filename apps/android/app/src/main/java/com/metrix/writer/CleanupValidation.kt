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
        // A spelling split, not a newly introduced negative instruction.
        "nevermind" to "never mind",
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
    /** Super preferences the rewriting guard needs (desktop `Formatting`). */
    data class Polish(val spoken: Boolean, val fillers: Boolean, val selfCorrections: Boolean, val lists: Boolean)

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

    // --- Super rewriting guard: a port of apps/api/src/cleanup/validation/polish.rs (MW-105/MW-118). ---
    // Super is a rewriting operation. Protect concrete values and polarity without requiring every
    // ordinary word to survive. This is not a semantic verifier; see docs/testing/cleanup-meaning.md.
    private val negations = setOf("not", "no", "never", "neither", "nor", "without", "לא", "אל", "אין", "אינו", "אינה", "אינם", "אינן", "בלי", "ללא")
    private fun negation(word: String) = word in negations
    private val protectedWords = negations + setOf(
        "zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten", "eleven", "twelve", "thirteen",
        "fourteen", "fifteen", "sixteen", "seventeen", "eighteen", "nineteen", "twenty", "thirty", "forty", "fifty", "sixty",
        "seventy", "eighty", "ninety", "hundred", "thousand", "million", "billion",
        "אפס", "אחד", "אחת", "שניים", "שתיים", "שני", "שתי", "שלוש", "שלושה", "ארבע", "ארבעה", "חמש", "חמישה", "שש", "שישה",
        "שבע", "שבעה", "שמונה", "תשע", "תשעה", "עשר", "עשרה", "עשרים", "שלושים", "ארבעים", "חמישים", "שישים", "שבעים",
        "שמונים", "תשעים", "מאה", "מאות", "אלף", "אלפים", "מיליון",
        "dollar", "dollars", "euro", "euros", "shekel", "shekels", "pound", "pounds", "percent",
        "דולר", "דולרים", "אירו", "שקל", "שקלים", "אחוז", "אחוזים",
        "today", "tomorrow", "yesterday", "am", "pm", "היום", "מחר", "אתמול",
        "monday", "tuesday", "wednesday", "thursday", "friday", "saturday", "sunday",
        "maybe", "perhaps", "possibly", "probably", "אולי", "כנראה", "yes", "כן")
    private val weekdays = setOf("w:monday", "w:tuesday", "w:wednesday", "w:thursday", "w:friday", "w:saturday", "w:sunday")
    private val articles = setOf("w:the", "w:a", "w:an", "w:to")
    private val pronounContractions = mapOf("w:i'm" to "i am", "w:you're" to "you are", "w:we're" to "we are", "w:they're" to "they are",
        "w:it's" to "it is", "w:that's" to "that is", "w:i've" to "i have", "w:we've" to "we have", "w:you've" to "you have",
        "w:they've" to "they have", "w:i'll" to "i will", "w:we'll" to "we will", "w:you'll" to "you will", "w:they'll" to "they will")
    private val numberWords = listOf("zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten", "eleven",
        "twelve", "thirteen", "fourteen", "fifteen", "sixteen", "seventeen", "eighteen", "nineteen", "twenty")

    private fun expand(values: List<String>) = values.flatMap { v -> pronounContractions[v]?.split(" ")?.map { "w:$it" } ?: listOf(v) }
    private fun correctable(v: String) = v.startsWith("n:") || v in weekdays
    private fun sourceValues(source: Tokens, p: Polish): List<String> {
        val values = expand(source.values).toMutableList()
        // Only explicit adjacent value corrections authorize dropping a protected value: "5, actually 10".
        if (p.selfCorrections) {
            var i = 1
            while (i + 1 < values.size) {
                if (values[i] in setOf("w:actually", "w:sorry", "w:בעצם", "w:סליחה") && correctable(values[i - 1]) && correctable(values[i + 1]) &&
                    values[i - 1].startsWith("n:") == values[i + 1].startsWith("n:")) {
                    values.removeAt(i - 1); values.removeAt(i - 1); i = maxOf(i - 1, 1)
                } else i++
            }
        }
        return values
    }
    private fun anchors(values: List<String>, p: Polish): List<String> = values.filterIndexed { i, v ->
        if (v == "w:am" || v == "w:pm") return@filterIndexed i > 0 && values[i - 1].startsWith("n:")
        // "like" can be a filler, but removing it after a subject can erase a stated preference.
        if (v == "w:like" && i > 0 && values[i - 1] in setOf("w:i", "w:we", "w:you", "w:they", "w:would")) return@filterIndexed true
        if (!v.startsWith("w:")) return@filterIndexed true
        val word = v.substring(2)
        word in protectedWords || word.any { it.isDigit() || Character.getType(it).toByte() in setOf(Character.LETTER_NUMBER, Character.OTHER_NUMBER) } || word.any { it in '\u0300'..'\u036f' || it in '\u0591'..'\u05c7' } ||
            !p.fillers && word in setOf("um", "uh", "אממ", "like")
    }
    /**
     * Content-free reason for differing anchors: the kind of the first value the polish dropped (or else
     * added), e.g. "anchors:number_word:dropped", or "anchors:order" when only their order changed.
     */
    private fun anchorDifference(source: List<String>, target: List<String>): String? {
        if (source == target) return null
        fun counts(values: List<String>) = values.groupingBy { it }.eachCount()
        val have = counts(source); val got = counts(target)
        // A symbol is a single punctuation glyph, never dictated words, so naming it stays content-free.
        fun code(value: String, how: String) = "anchors:${anchorKind(value)}:$how" + if (value.startsWith("s:")) ":U+%04X".format(value[2].code) else ""
        source.firstOrNull { (got[it] ?: 0) < have.getValue(it) }?.let { return code(it, "dropped") }
        target.firstOrNull { (have[it] ?: 0) < got.getValue(it) }?.let { return code(it, "added") }
        return "anchors:order"
    }
    private fun anchorKind(value: String): String {
        if (value.startsWith("n:")) return "digit"
        if (value.startsWith("x:")) return "link"
        if (value.startsWith("s:")) return "symbol"
        val word = value.substringAfter(':')
        return when {
            negation(word) -> "negation"
            word in protectedWords.filter { it in numberWords || it in setOf("thirty", "forty", "fifty", "sixty", "seventy", "eighty", "ninety", "hundred", "thousand", "million", "billion") || it.first() in '\u05d0'..'\u05ea' && it !in setOf("דולר", "דולרים", "אירו", "שקל", "שקלים", "אחוז", "אחוזים", "היום", "מחר", "אתמול", "אולי", "כנראה", "כן") } -> "number_word"
            word in setOf("dollar", "dollars", "euro", "euros", "shekel", "shekels", "pound", "pounds", "percent", "דולר", "דולרים", "אירו", "שקל", "שקלים", "אחוז", "אחוזים") -> "currency"
            word in setOf("today", "tomorrow", "yesterday", "am", "pm", "היום", "מחר", "אתמול") || "w:$word" in weekdays -> "time"
            word in setOf("maybe", "perhaps", "possibly", "probably", "yes", "אולי", "כנראה", "כן") -> "hedge"
            word in setOf("um", "uh", "אממ") -> "filler"
            word == "like" -> "like"
            word.any { it.isDigit() || Character.getType(it).toByte() in setOf(Character.LETTER_NUMBER, Character.OTHER_NUMBER) } -> "digit"
            else -> "diacritic"
        }
    }
    /** A deliberately narrow equivalence for inflection right after a negation. */
    private fun scopeWord(value: String): String {
        val word = value.removePrefix("w:")
        return when (word) {
            "is", "are", "was", "were", "been", "being" -> "be"
            "does", "did" -> "do"
            "has", "had" -> "have"
            else -> listOf("ing", "ed", "s").firstNotNullOfOrNull { suffix ->
                word.removeSuffix(suffix).takeIf { word.endsWith(suffix) && it.length >= 3 && it.all { c -> c.code < 128 } }
            } ?: word
        }
    }
    private fun negationScopes(values: List<String>) = values.mapIndexedNotNull { i, v ->
        val word = v.removePrefix("w:").takeIf { v.startsWith("w:") && negation(it) } ?: return@mapIndexedNotNull null
        word to values.drop(i + 1).firstOrNull { it !in articles }?.let(::scopeWord)
    }
    /** Drops a run that immediately repeats the run before it ("I don't know, I don't know"). */
    private fun collapseRepeats(values: List<String>): List<String> {
        val out = ArrayList<String>(values.size); var i = 0
        outer@ while (i < values.size) {
            for (len in 8 downTo 1) {
                if (out.size >= len && values.size >= i + len && out.subList(out.size - len, out.size) == values.subList(i, i + len)) { i += len; continue@outer }
            }
            out += values[i]; i++
        }
        return out
    }
    /** Spellings of the same protected value: "10am"/"10 a.m.", "today's"/"today", "five"/"5". */
    // One protected value, two spellings (S25, MW-ANDROID-06): Gemini writes "$150" where a polish writes
    // "150 dollars", and the reverse. Both normalize to the amount followed by its plural currency word.
    private val currencySymbols = mapOf("s:$" to "w:dollars", "s:€" to "w:euros", "s:₪" to "w:shekels", "s:£" to "w:pounds")
    private val currencySingular = mapOf("w:dollar" to "w:dollars", "w:euro" to "w:euros", "w:shekel" to "w:shekels", "w:pound" to "w:pounds")
    private fun normalize(input: List<String>): List<String> {
        val values = input.toMutableList(); var i = 0
        while (i < values.size) {
            currencySymbols[values[i]]?.takeIf { values.getOrNull(i + 1)?.startsWith("n:") == true }?.let { word -> values.removeAt(i); values.add(i + 1, word) }
            if (values[i] == "s:%" && values.getOrNull(i - 1)?.startsWith("n:") == true) values[i] = "w:percent"
            currencySingular[values[i]]?.let { values[i] = it }
            values[i].takeIf { it.startsWith("w:") && it.endsWith("'s") }?.let { it.substring(2, it.length - 2) }?.takeIf { it in protectedWords }?.let { values[i] = "w:$it" }
            if (values[i].startsWith("w:")) numberWords.indexOf(values[i].substring(2)).takeIf { it >= 0 }?.let { values[i] = "n:$it" }
            if (values[i].startsWith("n:") && values.getOrNull(i + 1) in setOf("w:a", "w:p") && values.getOrNull(i + 2) == "w:m") {
                values[i + 1] = values[i + 1] + "m"; values.removeAt(i + 2)
            }
            i++
        }
        return values
    }
    /** Numbers the model added as list markers ("1." at a line start) when lists are allowed. */
    private fun withoutListMarkers(target: Tokens, p: Polish) = target.values.filterIndexed { j, v ->
        !(p.lists && v.startsWith("n:") && (j == 0 || '\n' in target.gaps[j]) && target.gaps.getOrNull(j + 1)?.let { it.startsWith(".") || it.startsWith(")") } == true)
    }
    /** Every negation keeps its word and order and still governs its word among the next three output words. */
    private fun negationsKept(source: List<String>, target: List<String>): Boolean {
        val scopes = negationScopes(source)
        val windows = target.mapIndexedNotNull { i, v ->
            val word = v.removePrefix("w:").takeIf { v.startsWith("w:") && negation(it) } ?: return@mapIndexedNotNull null
            word to target.drop(i + 1).filter { it !in articles }.take(3).map(::scopeWord)
        }
        return scopes.size == windows.size && scopes.zip(windows).all { (s, t) -> s.first == t.first && (s.second?.let { it in t.second } ?: t.second.isEmpty()) }
    }
    private fun hasScript(text: String, hebrew: Boolean) = text.any { c -> if (hebrew) c in '\u05d0'..'\u05ea' else c in 'a'..'z' || c in 'A'..'Z' || c in '\u00c0'..'\u024f' }
    private fun cuesPreserved(source: Tokens, target: Tokens, p: Polish): Boolean {
        for ((cueText, marks) in cues) {
            val cue = cueText.split(" ").map { "w:$it" }
            for (i in 0..source.values.size - cue.size) {
                if (source.values.subList(i, i + cue.size) != cue) continue
                if ((0..target.values.size - cue.size).any { target.values.subList(it, it + cue.size) == cue }) continue // literal mention kept
                val literal = source.values.subList(maxOf(0, i - 4), i).any { v -> v.removePrefix("w:").let { negation(it) || it in setOf("word", "literal", "called", "מילה", "המילה") } }
                if (!p.spoken || literal) return false
                // Rendering must stay beside the same preceding/following word.
                val before = source.values.getOrNull(i - 1); val after = source.values.getOrNull(i + cue.size)
                val rendered = target.gaps.indices.any { j ->
                    marks.any { it in target.gaps[j] } && (before == null || target.values.getOrNull(j - 1) == before) && (after == null || target.values.getOrNull(j) == after)
                }
                if (!rendered) return false
            }
        }
        return true
    }
    fun preservesPolish(original: String, output: String, p: Polish): Boolean = polishRejection(original, output, p) == null
    /** Null when [output] is an acceptable polish; otherwise the first failed check as a content-free code. */
    fun polishRejection(original: String, output: String, p: Polish): String? {
        if (preserves(original, output, p.spoken, p.fillers)) return null
        if (output.isBlank() || '\u0000' in output || output.toByteArray().size > original.toByteArray().size * 3L + 128) return "size"
        if (listOf(false, true).any { hasScript(original, it) != hasScript(output, it) }) return "script"
        val source = tokens(original); val target = tokens(output)
        val values = normalize(collapseRepeats(sourceValues(source, p)))
        val targetValues = normalize(collapseRepeats(expand(withoutListMarkers(target, p))))
        anchorDifference(anchors(values, p), anchors(targetValues, p))?.let { return it }
        if (!negationsKept(values, targetValues)) return "negation"
        if (!cuesPreserved(source, target, p)) return "spoken_cues"
        // Still grounded in the dictation, not an answer or a replacement paragraph.
        val stop = setOf("w:a", "w:an", "w:the", "w:is", "w:are", "w:was", "w:were", "w:to", "w:and", "w:of")
        val sourceWords = values.filter { it.startsWith("w:") && it !in stop }.map(::scopeWord).toSet()
        val outputWords = targetValues.filter { it.startsWith("w:") && it !in stop }.map(::scopeWord)
        if (sourceWords.isNotEmpty() && (outputWords.isEmpty() || outputWords.count { it in sourceWords } * 2 < outputWords.size)) return "ungrounded"
        // Answering speaks as the assistant: first-person singular must appear in both or neither.
        val speaker = { v: List<String> -> v.any { it == "w:i" || it == "w:אני" } }
        return if (speaker(values) == speaker(targetValues)) null else "first_person"
    }
}
