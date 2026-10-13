package com.metrix.writer

import android.test.AndroidTestCase
import java.util.UUID
import java.io.File
import java.security.MessageDigest

@Suppress("DEPRECATION")
class MobileConnectionTest : AndroidTestCase() {
    fun testCredentialEncryptedAndBoundToExactOrigin() {
        val origin = "https://${UUID.randomUUID()}.invalid"
        val token = "mw1.${UUID.randomUUID()}." + "a".repeat(64)
        val client = MobileConnection(context)
        client.saveCredential(origin, token)
        try {
            val reopened = MobileConnection(context)
            val connection = reopened.open(origin, "/api/v1/mobile/connection", "GET")
            assertEquals("Bearer $token", connection.getRequestProperty("Authorization"))
            assertFalse(connection.instanceFollowRedirects); connection.disconnect()
            val hash = MessageDigest.getInstance("SHA-256").digest(origin.toByteArray()).joinToString("") { "%02x".format(it) }
            val file = File(context.noBackupFilesDir, "mobile-$hash.credential")
            assertFalse(String(file.readBytes()).contains(token))
            val different = origin + ":444"
            try { reopened.open(different, "/api/v1/mobile/connection", "GET"); fail("Credential crossed origins") }
            catch (error: ConnectionFailure) { assertEquals("authentication", error.category) }
            // Moving the encrypted record to a different endpoint cannot defeat its AAD binding.
            val otherHash = MessageDigest.getInstance("SHA-256").digest(different.toByteArray()).joinToString("") { "%02x".format(it) }
            val moved = File(context.noBackupFilesDir, "mobile-$otherHash.credential")
            file.copyTo(moved)
            try {
                try { reopened.open(different, "/api/v1/mobile/connection", "GET"); fail("AAD binding missing") }
                catch (error: ConnectionFailure) { assertEquals("authentication", error.category) }
            } finally { moved.delete() }
        } finally { client.forget(origin) }
        try { client.open(origin, "/api/v1/mobile/connection", "GET"); fail("Forgotten token remained") }
        catch (error: ConnectionFailure) { assertEquals("authentication", error.category) }
    }
    fun testUnsafeEndpointsRejectedAndHttpsCanonicalized() {
        val client = MobileConnection(context)
        assertEquals("https://example.invalid", client.normalize("https://EXAMPLE.invalid:443/"))
        for (endpoint in listOf("http://example.invalid", "https://user:secret@example.invalid", "https://example.invalid/path", "https://example.invalid?token=x", "https://example.invalid#x", "http://10.0.2.2:80", "https://example.invalid:0", "https://example.invalid:99999")) {
            try { client.normalize(endpoint); fail("Unsafe endpoint accepted") } catch (_: ConnectionFailure) { }
        }
    }
}
