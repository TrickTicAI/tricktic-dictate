package com.metrix.writer

import android.content.Context
import org.json.JSONObject
import java.io.File
import java.io.FileOutputStream
import java.io.InputStream
import java.io.InterruptedIOException
import java.net.HttpURLConnection
import java.net.URL
import java.security.MessageDigest
import java.util.concurrent.locks.ReentrantLock

/** Only pinned, verified assets become usable. No weights in the APK or shared storage. */
class PhoneModels(context: Context) {
    val directory = File(context.filesDir, "phone-models").apply { check(isDirectory || mkdirs()) }
    val catalog = JSONObject(context.assets.open("phone-models.json").bufferedReader().use { it.readText() })
    val runtime: String = catalog.getJSONObject("runtime").getString("revision")
    fun models(): List<JSONObject> = catalog.getJSONArray("models").let { rows -> (0 until rows.length()).map { rows.getJSONObject(it) } }
    fun model(id: String) = requireNotNull(models().firstOrNull { it.getString("id") == id }) {
        "This phone model is no longer offered. Choose another engine; saved audio and transcripts are retained."
    }
    fun file(id: String): File { model(id); return File(directory, "$id.bin") }
    fun partial(id: String): File { model(id); return File(directory, "$id.part") }
    fun languages(id: String): List<String> = models().firstOrNull { it.getString("id") == id }
        ?.getJSONArray("evaluation_modes")?.let { modes -> (0 until modes.length()).map { modes.getString(it) } } ?: emptyList()
    fun limits(id: String) = when(id) {
        "base" -> "Experimental · English first. Fast on measured short speech; substantial Hebrew errors. Mixed quality unqualified."
        "turbo-q5_0" -> "Evaluation only · mixed speech took about a minute for 14 seconds. Hebrew errors remain."
        else -> "Experimental · substantial Hebrew errors; no measured benefit over base on the short English prompt."
    }
    fun snapshot(id: String, language: String): JSONObject {
        require(language in languages(id)) { "This phone model does not support the selected language." }
        return JSONObject(model(id).toString()).put("runtime_revision", runtime).put("backend", "cpu")
            .put("threads", Runtime.getRuntime().availableProcessors().coerceIn(1, 4)).put("execution_location", "phone").put("input_language", language)
            .put("flash_attn", true).put("no_context", true).put("translate", false).put("hints", "unsupported").put("decode", "greedy-best-of-1-no-temperature-fallback")
    }
    fun verify(id: String, cancelled: () -> Boolean = { false }): File {
        val asset = model(id); val path = file(id)
        check(path.isFile && path.length() == asset.getLong("bytes")) { "Phone model missing or incomplete. Download or import it in Phone models; audio is retained." }
        check(hash(path, cancelled) == asset.getString("sha256")) { "Phone model integrity check failed. Delete and reinstall it; audio is retained." }
        return path
    }
    fun <T> exclusive(action: () -> T): T {
        check(lock.tryLock()) { "A phone model is in use or being installed. Try again when it finishes." }
        try { return action() } finally { lock.unlock() }
    }
    fun delete(id: String) = exclusive {
        check(!file(id).exists() || file(id).delete()); check(!partial(id).exists() || partial(id).delete())
    }
    fun import(id: String, input: InputStream, cancelled: () -> Boolean, progress: (Long, Long) -> Unit) = exclusive {
        val part = partial(id); val expected = model(id).getLong("bytes")
        if (part.length() > expected) check(part.delete())
        val offset = part.length()
        checkSpace(expected - offset, directory.usableSpace)
        var skipped = 0L
        while (skipped < offset) {
            if (cancelled()) throw InterruptedIOException("Model import paused")
            val count = input.skip(offset - skipped)
            if (count > 0) skipped += count
            else { check(input.read() >= 0) { "Selected file is shorter than the partial import. Choose the complete pinned file." }; skipped++ }
        }
        FileOutputStream(part, offset > 0).use { out -> copy(input, out, offset, expected, cancelled, progress); out.fd.sync() }
        promote(id, cancelled)
    }
    fun download(id: String, cancelled: () -> Boolean, progress: (Long, Long) -> Unit) = exclusive {
        val asset = model(id); val part = partial(id); val total = asset.getLong("bytes")
        if (part.length() > total) check(part.delete())
        var offset = part.length()
        if (offset == total) { promote(id, cancelled); return@exclusive }
        checkSpace(total - offset, directory.usableSpace)
        var url = URL("https://huggingface.co/${asset.getString("repository")}/resolve/${asset.getString("revision")}/${asset.getString("filename")}")
        var connection: HttpURLConnection? = null
        try {
            for (redirect in 0..8) {
                if (cancelled()) throw InterruptedIOException("Model download paused")
                require(url.protocol == "https") { "Model downloads require HTTPS" }
                val c = url.openConnection() as HttpURLConnection; connection = c
                c.connectTimeout = 15000; c.readTimeout = 15000; c.instanceFollowRedirects = false
                c.setRequestProperty("Accept-Encoding", "identity")
                if (offset > 0) c.setRequestProperty("Range", "bytes=$offset-")
                val code = c.responseCode
                if (code in listOf(301, 302, 303, 307, 308)) {
                    check(redirect < 8) { "Too many model download redirects" }
                    url = URL(url, c.getHeaderField("Location") ?: error("Missing download location")); c.disconnect(); continue
                }
                check(code == 200 || code == 206) { "Download unavailable (HTTP $code). Saved partial download can be resumed." }
                if (code == 200) { offset = 0; checkSpace(total, directory.usableSpace + part.length()) }
                else check(c.getHeaderField("Content-Range") == "bytes $offset-${total - 1}/$total") { "Invalid resumed model range" }
                c.inputStream.use { input -> FileOutputStream(part, offset > 0).use { out -> copy(input, out, offset, total, cancelled, progress); out.fd.sync() } }
                promote(id, cancelled); return@exclusive
            }
        } finally { connection?.disconnect() }
    }
    private fun copy(input: InputStream, out: FileOutputStream, offset: Long, total: Long, cancelled: () -> Boolean, progress: (Long, Long) -> Unit) {
        val buffer = ByteArray(256 * 1024); var count = offset
        while (true) {
            if (cancelled()) throw InterruptedIOException("Model transfer paused; download can resume")
            val n = input.read(buffer); if (n < 0) break
            check(count + n <= total) { "Model exceeds pinned size" }
            checkSpace(n.toLong(), directory.usableSpace)
            out.write(buffer, 0, n); count += n; progress(count, total)
        }
        check(count == total) { "Download interrupted. Resume to finish the saved partial file." }
    }
    private fun promote(id: String, cancelled: () -> Boolean) {
        val asset = model(id); val part = partial(id)
        if (part.length() != asset.getLong("bytes") || hash(part, cancelled) != asset.getString("sha256")) {
            part.delete(); error("Model integrity check failed. Import the pinned file or download again.")
        }
        check(part.renameTo(file(id))) { "Could not install verified model" }
    }
    companion object {
        private val lock = ReentrantLock()
        fun checkSpace(required: Long, available: Long) { check(available >= required + 32L * 1024 * 1024) { "Not enough free storage. Keep 32 MiB free for recordings and metadata; free space then resume." } }
        fun hash(file: File, cancelled: () -> Boolean = { false }): String {
            val digest = MessageDigest.getInstance("SHA-256")
            file.inputStream().use { input -> val buffer = ByteArray(256 * 1024); while (true) { if (cancelled()) throw InterruptedIOException("Phone work cancelled"); val n = input.read(buffer); if (n < 0) break; digest.update(buffer, 0, n) } }
            return digest.digest().joinToString("") { "%02x".format(it) }
        }
    }
}
