package com.metrix.writer

import android.os.SystemClock
import java.io.InputStream
import java.io.OutputStream
import java.net.HttpURLConnection
import java.net.URI
import java.security.MessageDigest

/** Shared limits for network, cached artifacts and PackageInstaller staging. */
internal object UpdateTransfer {
    const val MAX_FEED_BYTES = 64 * 1024L
    const val MAX_APK_BYTES = 256 * 1024 * 1024L
    const val SPACE_RESERVE = 64 * 1024 * 1024L

    fun manifestJson(text: String): org.json.JSONObject {
        require(text.toByteArray(Charsets.UTF_8).size <= MAX_FEED_BYTES) { "Release feed is too large" }
        // The contract is one flat object; reject deep JSON before the recursive parser runs.
        var quoted = false
        var escaped = false
        var depth = 0
        for (char in text) {
            if (quoted) {
                if (escaped) escaped = false
                else if (char == '\\') escaped = true
                else if (char == '"') quoted = false
            } else when (char) {
                '"' -> quoted = true
                '{' -> { depth++; require(depth <= 1) { "Nested release feed fields are not supported" } }
                '}' -> depth--
                '[', ']' -> throw IllegalArgumentException("Release feed arrays are not supported")
            }
        }
        return org.json.JSONObject(text)
    }

    fun url(value: String, feed: Boolean = false): java.net.URL {
        require(value.length in 1..2048) { "Invalid update URL length" }
        val uri = URI(value)
        require(uri.scheme == "https" && uri.host == "updates.tricktic.com" &&
            uri.port in listOf(-1, 443) && uri.rawUserInfo == null &&
            uri.rawQuery == null && uri.rawFragment == null && uri.normalize() == uri &&
            !uri.rawPath.contains('%') && !uri.rawPath.contains('\\')) { "Untrusted update URL" }
        require(if (feed) uri.path.matches(Regex("/writer/channels/android-[a-z0-9-]+\\.json"))
            else uri.path.startsWith("/writer/releases/") && uri.path.endsWith("/writer.apk")) {
            "Invalid update URL path"
        }
        return uri.toURL()
    }

    fun open(value: String, feed: Boolean = false): HttpURLConnection =
        (url(value, feed).openConnection() as HttpURLConnection).apply {
            connectTimeout = 10_000
            readTimeout = 10_000
            // The release host serves artifacts directly. No redirect, even to the same host.
            instanceFollowRedirects = false
            setRequestProperty("Accept-Encoding", "identity")
            setRequestProperty("User-Agent", "Metrix-Writer-Android/${BuildConfig.VERSION_NAME}")
        }

    fun response(conn: HttpURLConnection, max: Long, exact: Boolean = false) {
        check(conn.responseCode == HttpURLConnection.HTTP_OK) { "Update server returned HTTP ${conn.responseCode}" }
        check(conn.contentEncoding.isNullOrEmpty() || conn.contentEncoding.equals("identity", true)) {
            "Encoded update responses are not supported"
        }
        val length = conn.contentLengthLong
        check(length < 0 || (if (exact) length == max else length <= max)) { "Update response size mismatch" }
    }

    fun space(available: Long, bytes: Long) {
        require(bytes in 1..(2 * MAX_APK_BYTES)) { "Invalid APK byte length" }
        check(available >= bytes + SPACE_RESERVE) { "Insufficient storage space for update" }
    }

    /** Check before each write, even with no Content-Length or an endless/chunked body. */
    fun copy(input: InputStream, output: OutputStream, limit: Long, exact: Boolean = true,
             checkActive: () -> Unit = {}, beforeWrite: (Int) -> Unit = {},
             progress: (Long) -> Unit = {}): String {
        require(limit in 1..MAX_APK_BYTES)
        val digest = MessageDigest.getInstance("SHA-256")
        val deadline = SystemClock.elapsedRealtime() + if (limit <= MAX_FEED_BYTES) 30_000 else 10 * 60_000
        val buffer = ByteArray(16 * 1024)
        var count = 0L
        while (true) {
            checkActive()
            check(SystemClock.elapsedRealtime() < deadline) { "Update transfer timed out" }
            val read = input.read(buffer, 0, minOf(buffer.size.toLong(), limit - count + 1).toInt())
            if (read < 0) break
            if (read == 0) continue
            checkActive()
            check(SystemClock.elapsedRealtime() < deadline) { "Update transfer timed out" }
            check(count + read <= limit) { "Update exceeds byte limit" }
            beforeWrite(read)
            output.write(buffer, 0, read)
            digest.update(buffer, 0, read)
            count += read
            progress(count)
        }
        check(!exact || count == limit) { "Update file size mismatch" }
        return digest.digest().joinToString("") { "%02x".format(it) }
    }

    fun hashMatches(actual: String, expected: String) {
        if (!actual.equals(expected, true)) throw SecurityException("Update checksum mismatch")
    }
}
