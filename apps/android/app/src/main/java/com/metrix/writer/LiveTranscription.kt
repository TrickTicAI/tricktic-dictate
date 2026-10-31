package com.metrix.writer

import android.os.SystemClock
import android.util.Base64
import org.json.JSONArray
import org.json.JSONObject
import java.io.BufferedInputStream
import java.io.EOFException
import java.io.IOException
import java.io.InputStream
import java.io.OutputStream
import java.net.Socket
import java.security.SecureRandom
import javax.net.ssl.SSLSocket
import javax.net.ssl.SSLSocketFactory

/**
 * Minimal RFC 6455 client for one fixed origin. No redirects, no extensions, no key in the URL.
 * Server messages are bounded; text and binary frames are both UTF-8 JSON (Gemini Live sends either).
 */
internal class LiveSocket private constructor(private val socket: Socket) : AutoCloseable {
    private val input: InputStream = BufferedInputStream(socket.getInputStream(), 16384)
    private val output: OutputStream = socket.getOutputStream()
    private val random = SecureRandom()
    @Volatile private var closed = false

    @Synchronized fun send(text: String) {
        if (closed) throw IOException("Live connection closed")
        frame(1, text.toByteArray(Charsets.UTF_8))
    }
    private fun frame(opcode: Int, payload: ByteArray) {
        val header = java.io.ByteArrayOutputStream(14)
        header.write(0x80 or opcode)
        when {
            payload.size < 126 -> header.write(0x80 or payload.size)
            payload.size <= 0xffff -> { header.write(0x80 or 126); header.write(payload.size ushr 8); header.write(payload.size and 255) }
            else -> { header.write(0x80 or 127); for (shift in 56 downTo 0 step 8) header.write(((payload.size.toLong() ushr shift) and 255).toInt()) }
        }
        val mask = ByteArray(4).also(random::nextBytes); header.write(mask)
        val masked = ByteArray(payload.size) { (payload[it].toInt() xor mask[it and 3].toInt()).toByte() }
        output.write(header.toByteArray()); output.write(masked); output.flush()
    }
    /** The next complete data message, or null after a close frame. Answers pings itself. */
    fun receive(): String? {
        val message = java.io.ByteArrayOutputStream()
        var started = false
        while (true) {
            val first = read(); val second = read()
            check(second and 0x80 == 0) { "Server frames must not be masked" }
            check(first and 0x70 == 0) { "Unexpected WebSocket extension bits" }
            val opcode = first and 15
            var length = (second and 127).toLong()
            if (length == 126L) length = ((read() shl 8) or read()).toLong()
            else if (length == 127L) { length = 0; repeat(8) { length = (length shl 8) or read().toLong() } }
            check(length in 0..MAX_MESSAGE) { "Live message exceeded the supported size" }
            val payload = ByteArray(length.toInt()); readFully(payload)
            when (opcode) {
                8 -> { closed = true; runCatching { synchronized(this) { frame(8, payload.copyOf(minOf(2, payload.size))) } }; return null }
                9 -> { synchronized(this) { if (!closed) frame(10, payload) }; continue }
                10 -> continue
                0, 1, 2 -> {
                    check(opcode == 0 == started) { "Unexpected WebSocket continuation" }
                    started = true
                    check(message.size() + payload.size <= MAX_MESSAGE) { "Live message exceeded the supported size" }
                    message.write(payload)
                    if (first and 0x80 != 0) return message.toString("UTF-8")
                }
                else -> error("Unsupported WebSocket frame")
            }
        }
    }
    private fun read(): Int = input.read().also { if (it < 0) throw EOFException("Live connection ended") }
    private fun readFully(bytes: ByteArray) {
        var at = 0
        while (at < bytes.size) { val n = input.read(bytes, at, bytes.size - at); if (n < 0) throw EOFException("Live connection ended"); at += n }
    }
    fun setReadTimeout(ms: Int) { socket.soTimeout = ms }
    override fun close() {
        if (!closed) { closed = true; runCatching { synchronized(this) { frame(8, byteArrayOf(3, -24)) } } } // 1000 normal closure
        runCatching { socket.close() }
    }

    companion object {
        const val MAX_MESSAGE = 1L shl 20
        /** TLS with hostname verification; [plain] is only for in-process test servers. */
        fun tls(host: String): Socket = (SSLSocketFactory.getDefault().createSocket(host, 443) as SSLSocket).apply {
            sslParameters = sslParameters.apply { endpointIdentificationAlgorithm = "HTTPS" }
            soTimeout = 15000; startHandshake()
        }
        fun open(host: String, path: String, headers: Map<String, String>, connect: () -> Socket = { tls(host) }): LiveSocket {
            require(path.startsWith("/") && '?' !in path && '#' !in path)
            val socket = connect()
            try {
                socket.soTimeout = 15000
                val nonce = ByteArray(16).also(SecureRandom()::nextBytes)
                val key = Base64.encodeToString(nonce, Base64.NO_WRAP)
                val request = StringBuilder("GET $path HTTP/1.1\r\nHost: $host\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n")
                    .append("Sec-WebSocket-Key: $key\r\nSec-WebSocket-Version: 13\r\n")
                headers.forEach { (name, value) -> require('\r' !in value && '\n' !in value); request.append("$name: $value\r\n") }
                socket.getOutputStream().apply { write(request.append("\r\n").toString().toByteArray(Charsets.ISO_8859_1)); flush() }
                // Read only the handshake head, byte by byte, so no frame bytes are consumed early.
                val head = StringBuilder(); val stream = socket.getInputStream()
                while (!head.endsWith("\r\n\r\n")) {
                    val c = stream.read(); if (c < 0) throw EOFException("Live handshake ended")
                    head.append(c.toChar()); check(head.length <= 16384) { "Live handshake too large" }
                }
                val lines = head.split("\r\n")
                val status = lines.first().split(" ").getOrNull(1)?.toIntOrNull() ?: 0
                if (status != 101) throw CloudFailure(when (status) {
                    400 -> "Gemini rejected the live request. Saved audio is retained."
                    401, 403 -> "Gemini key was rejected. Check it in Settings › API keys. Saved audio is retained."
                    404 -> "Gemini live transcription is unavailable to this key. Saved audio is retained."
                    429 -> "Gemini quota or rate limit reached. Saved audio is retained."
                    in 300..399 -> "Gemini redirected the live request; $PRODUCT_NAME refused to forward your key."
                    else -> "Gemini live connection failed (HTTP $status). Saved audio is retained."
                }, status)
                val fields = lines.drop(1).filter { ':' in it }.associate { it.substringBefore(':').trim().lowercase() to it.substringAfter(':').trim() }
                // Google's endpoint answers with an accept value that is not the RFC 6455 digest of our key
                // (measured 2026-10-26; the reference Python client tolerates it too). On TLS to a fixed,
                // hostname-verified origin, a 101 upgrade with an accept header is the acknowledgement.
                check(fields["upgrade"].equals("websocket", true) && !fields["sec-websocket-accept"].isNullOrEmpty()) { "Live handshake was not acknowledged" }
                check(fields["sec-websocket-extensions"].isNullOrEmpty()) { "Unexpected WebSocket extension" }
                return LiveSocket(socket)
            } catch (error: Exception) { runCatching { socket.close() }; throw error }
        }
    }
}

/** One live recognition connection, as the streaming coordinator uses it. */
internal interface LiveConnection : AutoCloseable {
    /** Speech has started that the provider has not answered with a completed generation yet. */
    val turnOpen: Boolean
    fun audio(bytes: ByteArray, offset: Int, count: Int)
    fun end()
}

/** Gemini Live input transcription over [LiveSocket]. Text is reported in arrival order; nothing is logged. */
internal class GeminiLive(private val socket: LiveSocket, private val listener: Listener) : LiveConnection {
    interface Listener {
        /** [final] text completes one voice turn; interim text is that turn's cumulative partial and is replaced. */
        fun text(fragment: String, final: Boolean)
        /** A turn or generation completed after the last audio this connection was given. */
        fun complete()
        /** The server asked the client to reconnect soon. */
        fun goAway()
        fun failed(error: Exception)
    }
    @Volatile private var closing = false
    /**
     * Whether speech has started that the provider has not answered with a completed generation yet.
     * Turns can last minutes (one 206 s turn in the 5-minute probe), so this, not silence, decides the tail.
     */
    @Volatile override var turnOpen = false; private set
    private val reader = Thread({ read() }, "writer-live-reader").apply { isDaemon = true }

    private fun read() {
        try {
            socket.setReadTimeout(0)
            while (true) {
                val raw = socket.receive() ?: break
                val message = JSONObject(raw)
                message.optJSONObject("voiceActivity")?.optString("type")?.let { if ("START" in it.uppercase()) turnOpen = true }
                message.optJSONObject("serverContent")?.let { content ->
                    content.optJSONObject("interimInputTranscription")?.optString("text")?.takeIf { it.isNotEmpty() }?.let { turnOpen = true; listener.text(it, false) }
                    content.optJSONObject("inputTranscription")?.optString("text")?.takeIf { it.isNotEmpty() }?.let { listener.text(it, true) }
                    if (content.optBoolean("turnComplete") || content.optBoolean("generationComplete")) { turnOpen = false; listener.complete() }
                }
                if (message.has("goAway")) listener.goAway()
                if (message.has("error")) throw CloudFailure("Gemini live transcription reported an error. Saved audio is retained.")
            }
            if (!closing) listener.failed(EOFException("Live connection closed"))
        } catch (error: Exception) { if (!closing) listener.failed(error) }
    }
    override fun audio(bytes: ByteArray, offset: Int, count: Int) {
        socket.send("{\"realtimeInput\":{\"audio\":{\"data\":\"" + Base64.encodeToString(bytes, offset, count, Base64.NO_WRAP) +
            "\",\"mimeType\":\"audio/pcm;rate=16000\"}}}")
    }
    override fun end() = socket.send("{\"realtimeInput\":{\"audioStreamEnd\":true}}")
    override fun close() { closing = true; socket.close() }

    companion object {
        const val HOST = "generativelanguage.googleapis.com"
        const val PATH = "/ws/google.ai.generativelanguage.v1beta.GenerativeService.BidiGenerateContent"
        const val MODEL = "gemini-3.5-transcribe-live"
        fun languageCodes(language: String): JSONArray = when (language) {
            "en" -> JSONArray().put("en-US"); "he" -> JSONArray().put("he-IL"); else -> JSONArray()
        }
        /** Connects, sends the setup and waits for its acknowledgement before audio is accepted. */
        fun connect(apiKey: String, language: String, listener: Listener, model: String = MODEL,
            open: (Map<String, String>) -> LiveSocket = { LiveSocket.open(HOST, PATH, it) }): GeminiLive {
            require(DirectCloud.MODEL.matches(model))
            val socket = open(mapOf("x-goog-api-key" to apiKey))
            try {
                socket.send(JSONObject().put("setup", JSONObject().put("model", "models/$model")
                    .put("generationConfig", JSONObject().put("responseModalities", JSONArray().put("TEXT")))
                    .put("inputAudioTranscription", JSONObject().put("languageCodes", languageCodes(language)))).toString())
                socket.setReadTimeout(10000)
                val reply = JSONObject(socket.receive() ?: throw EOFException("Live setup was refused"))
                check(reply.has("setupComplete")) { "Live setup was not acknowledged" }
                return GeminiLive(socket, listener).also { it.reader.start() }
            } catch (error: Exception) { socket.close(); throw error }
        }
        fun now() = SystemClock.elapsedRealtime()
    }
}
