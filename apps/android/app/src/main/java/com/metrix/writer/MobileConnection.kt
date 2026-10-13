package com.metrix.writer

import android.content.Context
import android.content.pm.ApplicationInfo
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.AtomicFile
import org.json.JSONObject
import java.io.File
import java.io.IOException
import java.net.HttpURLConnection
import java.net.URI
import java.net.URL
import java.security.KeyStore
import java.security.MessageDigest
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec
import javax.net.ssl.SSLException

/** A credential can only authorize its canonical origin. No redirect or TLS overrides. */
class MobileConnection(private val context: Context) {
    private val preferences = context.getSharedPreferences("mobile_connection", Context.MODE_PRIVATE)
    fun endpoint(): String = preferences.getString("endpoint", "")!!
    fun selection(): String = preferences.getString("selection", "")!!
    fun language(): String = preferences.getString("language", "he")!!
    fun normalize(value: String): String {
        val uri = try { URI(value.trim()) } catch (_: Exception) { throw ConnectionFailure("endpoint", "Enter a valid HTTPS server address.") }
        val debug = context.applicationInfo.flags and ApplicationInfo.FLAG_DEBUGGABLE != 0
        val test = debug && uri.scheme == "http" && uri.host == "10.0.2.2" && uri.port in listOf(18795, 18796)
        if (uri.host == null || uri.userInfo != null || uri.query != null || uri.fragment != null ||
            uri.rawPath !in listOf("", "/") || (uri.scheme != "https" && !test) || uri.port == 0 || uri.port > 65535)
            throw ConnectionFailure("endpoint", "Use an HTTPS server origin without a path, login, query or fragment.")
        return URI(uri.scheme, null, uri.host.lowercase(), if (uri.port == 443 && uri.scheme == "https") -1 else uri.port, null, null, null).toASCIIString()
    }
    private fun file(endpoint: String): AtomicFile {
        val hash = MessageDigest.getInstance("SHA-256").digest(endpoint.toByteArray()).joinToString("") { "%02x".format(it) }
        return AtomicFile(File(context.noBackupFilesDir, "mobile-$hash.credential"))
    }
    private fun key(): SecretKey = synchronized(MobileConnection::class.java) {
        val store = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        (store.getKey(KEY_ALIAS, null) as? SecretKey) ?: KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore").apply {
            init(KeyGenParameterSpec.Builder(KEY_ALIAS, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM).setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE).setKeySize(256).build())
        }.generateKey()
    }
    fun saveCredential(origin: String, token: String) {
        val endpoint = normalize(origin)
        if (!TOKEN.matches(token)) throw ConnectionFailure("authentication", "Enter the device credential issued by your $PRODUCT_NAME server owner.")
        val cipher = Cipher.getInstance("AES/GCM/NoPadding").apply { init(Cipher.ENCRYPT_MODE, key()); updateAAD(endpoint.toByteArray()) }
        val encrypted = cipher.doFinal(token.toByteArray())
        val atomic = file(endpoint); val out = atomic.startWrite()
        try { out.write(cipher.iv.size); out.write(cipher.iv); out.write(encrypted); atomic.finishWrite(out) }
        catch (e: Exception) { atomic.failWrite(out); throw e }
    }
    private fun credential(endpoint: String): String {
        try {
            val bytes = file(endpoint).readFully(); val size = bytes[0].toInt()
            require(size == 12 && bytes.size > size + 16)
            val cipher = Cipher.getInstance("AES/GCM/NoPadding").apply {
                init(Cipher.DECRYPT_MODE, key(), GCMParameterSpec(128, bytes.copyOfRange(1, size + 1))); updateAAD(endpoint.toByteArray())
            }
            return String(cipher.doFinal(bytes.copyOfRange(size + 1, bytes.size)), Charsets.UTF_8)
        } catch (_: Exception) { throw ConnectionFailure("authentication", "Pair this endpoint again. Saved audio and pending requests are retained.") }
    }
    fun forget(origin: String) { file(normalize(origin)).delete(); if (endpoint() == normalize(origin)) preferences.edit().clear().commit() }
    fun select(origin: String, model: String, language: String) {
        check(preferences.edit().putString("endpoint", normalize(origin)).putString("selection", model).putString("language", language).commit())
    }
    /** Cached capability consent allows capture offline; server validation remains authoritative. */
    fun captureChoice(): JSONObject {
        val origin = normalize(endpoint())
        val body = JSONObject(preferences.getString("capabilities:" + origin, "{}")!!)
        val models = body.optJSONArray("models") ?: throw ConnectionFailure("selection", "Connect and choose a cloud model. Audio is saved for retry.")
        val model = (0 until models.length()).map { models.getJSONObject(it) }.firstOrNull {
            val choice = it.getJSONObject("selection")
            "${choice.getString("provider")}/${choice.getString("model")}" == selection() &&
                it.optBoolean("configured") && it.optString("execution_location") == "cloud" &&
                (0 until it.getJSONArray("input_languages").length()).any { n -> it.getJSONArray("input_languages").getString(n) == language() }
        } ?: throw ConnectionFailure("selection", "Choose a configured cloud model and supported speech language. Audio is saved for retry.")
        return JSONObject().put("server", origin).put("selection", model.getJSONObject("selection"))
            .put("input_language", language()).put("options", CompanionPreferences(context).options(model.getJSONObject("selection").getString("provider"), model.getJSONObject("selection").getString("model"), language()))
            .put("delivery", CompanionPreferences(context).delivery())
    }
    fun open(origin: String, path: String, method: String): HttpURLConnection = openWithToken(normalize(origin), path, method, credential(normalize(origin)))
    private fun openWithToken(origin: String, path: String, method: String, token: String): HttpURLConnection {
        require(path.startsWith("/api/v1/") && !path.contains('?') && !path.contains('#') && !path.contains(".."))
        if (!TOKEN.matches(token)) throw ConnectionFailure("authentication", "Invalid device credential.")
        return (URL(origin + path).openConnection() as HttpURLConnection).apply {
            requestMethod = method; connectTimeout = 10000; readTimeout = 30000; instanceFollowRedirects = false
            setRequestProperty("Authorization", "Bearer $token")
        }
    }
    /** A candidate token is persisted only after this server confirms the mobile protocol. */
    fun check(origin: String, candidate: String): JSONObject {
        val endpoint = normalize(origin)
        val connection = if (candidate.isBlank()) open(endpoint, "/api/v1/mobile/connection", "GET")
            else openWithToken(endpoint, "/api/v1/mobile/connection", "GET", candidate.trim())
        try {
            val body = response(connection).getJSONObject("data")
            if (body.optString("service") != "metrix-writer-mobile" || body.optInt("mobile_version") != 1 || body.optJSONArray("models") == null)
                throw ConnectionFailure("compatibility", "This server does not support $PRODUCT_NAME mobile API version 1.")
            if (candidate.isNotBlank()) saveCredential(endpoint, candidate.trim())
            check(preferences.edit().putString("capabilities:" + endpoint, body.toString()).commit())
            return body
        } finally { connection.disconnect() }
    }
    companion object {
        private const val KEY_ALIAS = "writer.mobile.credentials.v1"
        private val TOKEN = Regex("mw1\\.[0-9a-f-]{36}\\.[0-9a-f]{64}")
        @JvmStatic fun response(connection: HttpURLConnection): JSONObject {
            when (val code = connection.responseCode) {
                401, 403 -> throw ConnectionFailure("authentication", "Device credential rejected or revoked. Ask the owner to pair this endpoint again.")
                in 300..399 -> throw ConnectionFailure("redirect", "Server redirected the request. Verify and pair the final HTTPS address explicitly.")
                404, 405, 426 -> throw ConnectionFailure("compatibility", "This endpoint does not support the required $PRODUCT_NAME mobile API.")
                in 500..599 -> throw ConnectionFailure("server", "$PRODUCT_NAME server is unavailable. Retry later; saved audio is retained.")
                !in 200..299 -> throw ConnectionFailure("request", "Server rejected the request ($code). Saved audio is retained.")
            }
            val bytes = connection.inputStream.use { it.readNBytes(1024 * 1024 + 1) }
            if (bytes.size > 1024 * 1024) throw ConnectionFailure("compatibility", "Server response exceeded the supported size.")
            val body = try { JSONObject(String(bytes, Charsets.UTF_8)) } catch (_: Exception) { throw ConnectionFailure("compatibility", "Server returned an incompatible response.") }
            if (body.optString("api_version") != "1") throw ConnectionFailure("compatibility", "Server API version is incompatible.")
            return body
        }
        @JvmStatic fun explain(error: Exception): String = when (error) {
            is ConnectionFailure -> error.message!!
            is SSLException -> "TLS certificate validation failed. Check the HTTPS hostname, certificate and device clock."
            is java.net.UnknownHostException, is java.net.ConnectException, is java.net.SocketTimeoutException, is java.net.NoRouteToHostException -> "Offline or server unreachable. Check the network and endpoint, then retry."
            else -> "Connection could not complete. Saved audio and pending requests are retained."
        }
    }
}
class ConnectionFailure(val category: String, message: String) : IOException(message)
