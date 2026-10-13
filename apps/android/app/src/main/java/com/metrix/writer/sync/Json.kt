package com.metrix.writer.sync

import org.json.JSONArray
import org.json.JSONObject
import java.nio.ByteBuffer
import java.nio.charset.CharacterCodingException
import java.nio.charset.CodingErrorAction

/** A document that is not valid contract JSON. Its message never contains the input. */
class ContractException(val reason: String) : Exception(reason)

/**
 * Strict JSON for the portable sync contract (MW-OUATH-1): malformed UTF-8, a byte-order mark,
 * duplicate object keys, unpaired surrogates, NUL and non-integer or unsafe numbers are refused
 * before any schema check. org.json silently keeps the last duplicate key, so it is not used
 * to parse untrusted contract input.
 */
object StrictJson {
    const val MAX_SAFE = 9007199254740991L
    private const val MAX_DEPTH = 32

    fun parse(bytes: ByteArray): Any {
        val text = try {
            Charsets.UTF_8.newDecoder().onMalformedInput(CodingErrorAction.REPORT).onUnmappableCharacter(CodingErrorAction.REPORT)
                .decode(ByteBuffer.wrap(bytes)).toString()
        } catch (_: CharacterCodingException) { throw ContractException("malformed_utf8") }
        if (text.startsWith(Char(0xFEFF))) throw ContractException("byte_order_mark")
        return Parser(text).document()
    }

    fun parseObject(bytes: ByteArray): JSONObject = parse(bytes) as? JSONObject ?: throw ContractException("not_an_object")

    private class Parser(val s: String) {
        var i = 0
        fun document(): Any {
            val value = value(0)
            space()
            if (i != s.length) throw ContractException("trailing_content")
            return value
        }
        fun space() { while (i < s.length && s[i] in " \t\r\n") i++ }
        fun value(depth: Int): Any {
            if (depth > MAX_DEPTH) throw ContractException("too_deep")
            space()
            if (i >= s.length) throw ContractException("truncated")
            return when (s[i]) {
                '{' -> obj(depth)
                '[' -> arr(depth)
                '"' -> string()
                't' -> literal("true", true)
                'f' -> literal("false", false)
                'n' -> literal("null", JSONObject.NULL)
                else -> number()
            }
        }
        fun literal(word: String, value: Any): Any {
            if (!s.startsWith(word, i)) throw ContractException("invalid_literal")
            i += word.length
            return value
        }
        fun obj(depth: Int): JSONObject {
            i++
            val result = JSONObject()
            space()
            if (i < s.length && s[i] == '}') { i++; return result }
            while (true) {
                space()
                if (i >= s.length || s[i] != '"') throw ContractException("expected_key")
                val key = string()
                if (result.has(key)) throw ContractException("duplicate_key")
                space()
                if (i >= s.length || s[i] != ':') throw ContractException("expected_colon")
                i++
                result.put(key, value(depth + 1))
                space()
                if (i >= s.length) throw ContractException("truncated")
                when (s[i++]) { ',' -> continue; '}' -> return result; else -> throw ContractException("expected_comma") }
            }
        }
        fun arr(depth: Int): JSONArray {
            i++
            val result = JSONArray()
            space()
            if (i < s.length && s[i] == ']') { i++; return result }
            while (true) {
                result.put(value(depth + 1))
                space()
                if (i >= s.length) throw ContractException("truncated")
                when (s[i++]) { ',' -> continue; ']' -> return result; else -> throw ContractException("expected_comma") }
            }
        }
        fun string(): String {
            i++
            val out = StringBuilder()
            while (true) {
                if (i >= s.length) throw ContractException("truncated")
                val c = s[i++]
                when {
                    c == '"' -> break
                    c == '\\' -> {
                        if (i >= s.length) throw ContractException("truncated")
                        when (val e = s[i++]) {
                            '"', '\\', '/' -> out.append(e)
                            'b' -> out.append('\b'); 'f' -> out.append('\u000C'); 'n' -> out.append('\n'); 'r' -> out.append('\r'); 't' -> out.append('\t')
                            'u' -> {
                                if (i + 4 > s.length) throw ContractException("truncated")
                                val hex = s.substring(i, i + 4)
                                if (!hex.all { it in "0123456789abcdefABCDEF" }) throw ContractException("invalid_escape")
                                out.append(hex.toInt(16).toChar()); i += 4
                            }
                            else -> throw ContractException("invalid_escape")
                        }
                    }
                    c < ' ' -> throw ContractException("control_character")
                    else -> out.append(c)
                }
            }
            val text = out.toString()
            if (!wellFormed(text)) throw ContractException("unpaired_surrogate")
            if (text.indexOf('\u0000') >= 0) throw ContractException("nul")
            return text
        }
        fun number(): Long {
            val start = i
            if (i < s.length && s[i] == '-') i++
            val digits = i
            while (i < s.length && s[i].isDigit()) i++
            if (i == digits || (s[digits] == '0' && i - digits > 1)) throw ContractException("invalid_number")
            if (i < s.length && s[i] in ".eE") throw ContractException("non_integer")
            val value = s.substring(start, i).toLongOrNull() ?: throw ContractException("unsafe_integer")
            if (value > MAX_SAFE || value < -MAX_SAFE) throw ContractException("unsafe_integer")
            return value
        }
    }

    /** True when every surrogate is part of a pair. */
    fun wellFormed(text: String): Boolean {
        var i = 0
        while (i < text.length) {
            val c = text[i]
            if (Character.isHighSurrogate(c)) {
                if (i + 1 >= text.length || !Character.isLowSurrogate(text[i + 1])) return false
                i += 2; continue
            }
            if (Character.isLowSurrogate(c)) return false
            i++
        }
        return true
    }

    /** Deep equality ignoring object key order; integers compare by value whatever their boxed type. */
    fun equal(a: Any?, b: Any?): Boolean {
        val x = if (a == JSONObject.NULL) null else a
        val y = if (b == JSONObject.NULL) null else b
        if (x == null || y == null) return x == null && y == null
        // Contract numbers are integers only.
        if (x is Number && y is Number) return x.toLong() == y.toLong() && x.toDouble() == y.toDouble()
        if (x is JSONObject && y is JSONObject) {
            if (x.length() != y.length()) return false
            return x.keys().asSequence().all { key -> y.has(key) && equal(x.get(key), y.get(key)) }
        }
        if (x is JSONArray && y is JSONArray) return x.length() == y.length() && (0 until x.length()).all { equal(x.get(it), y.get(it)) }
        return x == y
    }

    /** Stable text for a value: object keys sorted. Used for change detection, not transport. */
    fun canonical(value: Any?): String = when (value) {
        null, JSONObject.NULL -> "null"
        is JSONObject -> value.keys().asSequence().sorted().joinToString(",", "{", "}") { JSONObject.quote(it) + ":" + canonical(value.get(it)) }
        is JSONArray -> (0 until value.length()).joinToString(",", "[", "]") { canonical(value.get(it)) }
        is String -> JSONObject.quote(value)
        is Number -> value.toLong().toString()
        else -> value.toString()
    }

    /** A deep copy through canonical text, so later edits cannot alias stored documents. */
    fun copy(value: JSONObject): JSONObject = JSONObject(canonical(value))
}
