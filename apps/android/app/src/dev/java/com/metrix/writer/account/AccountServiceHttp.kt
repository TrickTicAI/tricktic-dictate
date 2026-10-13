package com.metrix.writer.account

import android.net.Uri
import com.metrix.writer.BuildConfig
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import org.json.JSONObject
import java.io.IOException
import java.io.InputStream
import java.net.HttpURLConnection
import java.net.URL

/**
 * The trusted cleanup service and the Firestore account read over HTTPS. The service URL comes from
 * the private build configuration; without it every control reports not configured. Only fixed
 * error kinds leave this class: no body, URL, token, UID or email is logged or rethrown.
 */
internal class AccountServiceHttp(
    private val baseUrl: String = BuildConfig.WRITER_ACCOUNT_SERVICE_URL,
    private val firestoreUrl: String = "https://firestore.googleapis.com/v1",
) : AccountServiceApi {
    override val configured get() = baseUrl.isNotEmpty()

    override suspend fun readAccount(account: AccountKey, idToken: String): RemoteAccount? {
        val url = "$firestoreUrl/projects/${Uri.encode(account.projectId)}/databases/(default)/documents/users/${Uri.encode(account.uid)}"
        val (status, body) = call("GET", url, idToken, null)
        return when {
            status == 404 -> null
            status == 401 -> throw ServiceFailure(ControlError.UNAUTHENTICATED)
            status == 429 || status >= 500 -> throw ServiceFailure(ControlError.UNAVAILABLE)
            status !in 200..299 -> throw ServiceFailure(ControlError.UNAVAILABLE)
            else -> try { RemoteAccount.fromFirestore(body ?: throw ServiceFailure(ControlError.INVALID_RESPONSE)) }
                catch (_: org.json.JSONException) { throw ServiceFailure(ControlError.INVALID_RESPONSE) }
                catch (_: NumberFormatException) { throw ServiceFailure(ControlError.INVALID_RESPONSE) }
        }
    }

    override suspend fun startCleanup(idToken: String, request: JSONObject) = receipt(service("POST", "/v1/cleanup", idToken, request))

    override suspend fun cleanupStatus(idToken: String, operationId: String) =
        receipt(service("GET", "/v1/cleanup/${Uri.encode(operationId)}", idToken, null))

    override suspend fun revokeSessions(idToken: String, uid: String) {
        val body = service("POST", "/v1/sessions/revoke", idToken, JSONObject().put("schema_version", 1).put("uid", uid))
        if (!body.optBoolean("revoked")) throw ServiceFailure(ControlError.INVALID_RESPONSE)
    }

    private fun receipt(body: JSONObject) = body.optJSONObject("receipt") ?: throw ServiceFailure(ControlError.INVALID_RESPONSE)

    private suspend fun service(method: String, path: String, idToken: String, body: JSONObject?): JSONObject {
        if (!configured) throw ServiceFailure(ControlError.NOT_CONFIGURED)
        val (status, reply) = call(method, baseUrl + path, idToken, body)
        if (status in 200..299) return reply ?: throw ServiceFailure(ControlError.INVALID_RESPONSE)
        throw ServiceFailure(ControlError.of(reply?.optString("error"))?.takeIf { it != ControlError.NOT_CONFIGURED }
            ?: if (status == 429 || status >= 500) ControlError.UNAVAILABLE else ControlError.INVALID_RESPONSE)
    }

    private suspend fun call(method: String, url: String, idToken: String, body: JSONObject?): Pair<Int, JSONObject?> = withContext(Dispatchers.IO) {
        val connection = try { URL(url).openConnection() as HttpURLConnection } catch (_: IOException) { throw ServiceFailure(ControlError.OFFLINE) }
        try {
            connection.instanceFollowRedirects = false
            connection.connectTimeout = 15_000
            // The service waits up to ten seconds for a small cleanup to finish.
            connection.readTimeout = 30_000
            connection.requestMethod = method
            connection.useCaches = false
            connection.setRequestProperty("Authorization", "Bearer $idToken")
            connection.setRequestProperty("Accept", "application/json")
            if (body != null) {
                val bytes = body.toString().toByteArray(Charsets.UTF_8)
                connection.doOutput = true
                connection.setRequestProperty("Content-Type", "application/json")
                connection.setFixedLengthStreamingMode(bytes.size)
                connection.outputStream.use { it.write(bytes) }
            }
            val status = connection.responseCode
            val stream = if (status >= 400) connection.errorStream else connection.inputStream
            val text = stream?.use { read(it) }
            status to text?.let { runCatching { JSONObject(it) }.getOrNull() }
        } catch (failure: ServiceFailure) { throw failure } catch (_: IOException) {
            throw ServiceFailure(ControlError.OFFLINE)
        } finally { connection.disconnect() }
    }

    private fun read(stream: InputStream): String {
        val bytes = stream.readNBytes(LIMIT + 1)
        if (bytes.size > LIMIT) throw ServiceFailure(ControlError.INVALID_RESPONSE)
        return String(bytes, Charsets.UTF_8)
    }

    private companion object { const val LIMIT = 64 * 1024 }
}
