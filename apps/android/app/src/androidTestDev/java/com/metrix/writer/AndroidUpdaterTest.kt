@file:Suppress("DEPRECATION")
package com.metrix.writer

import android.content.Context
import android.test.InstrumentationTestCase
import org.json.JSONObject
import java.io.File
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

class AndroidUpdaterTest : InstrumentationTestCase() {
    private lateinit var context: Context
    private lateinit var updater: AndroidUpdater

    override fun setUp() {
        super.setUp()
        context = instrumentation.targetContext
        updater = AndroidUpdater.getInstance(context)
        context.getSharedPreferences(AndroidUpdater.PREFS_NAME, Context.MODE_PRIVATE).edit().clear().commit()
        updater.reconcileState()
    }

    fun testManifestValidationValid() {
        val json = JSONObject().apply {
            put("version", "0.5.2")
            put("version_code", 21)
            put("package", "com.metrix.writer")
            put("signer_sha256", "e35e0ff5447132ced8c1228783a5f7cccf782af0f33af77e152f74e0a02f10fb")
            put("url", "https://updates.tricktic.com/writer/releases/0.5.2/writer.apk")
            put("sha256", "a".repeat(64))
            put("bytes", 25000000L)
            put("notes", "New features and fixes")
            put("pub_date", "2026-09-15T12:00:00Z")
        }

        val info = updater.parseAndValidateManifest(json)
        assertEquals("0.5.2", info.version)
        assertEquals(21, info.versionCode)
        assertEquals("com.metrix.writer", info.packageName)
        assertEquals("e35e0ff5447132ced8c1228783a5f7cccf782af0f33af77e152f74e0a02f10fb", info.signerSha256)
        assertEquals(25000000L, info.bytes)
        assertEquals("New features and fixes", info.notes)
    }

    fun testManifestValidationRejectsWrongPackage() {
        val json = JSONObject().apply {
            put("version", "0.5.2")
            put("version_code", 21)
            put("package", "com.other.app")
            put("signer_sha256", "e35e0ff5447132ced8c1228783a5f7cccf782af0f33af77e152f74e0a02f10fb")
            put("url", "https://updates.tricktic.com/writer/releases/0.5.2/writer.apk")
            put("sha256", "a".repeat(64))
            put("bytes", 25000000L)
        }

        try {
            updater.parseAndValidateManifest(json)
            fail("Expected SecurityException for mismatched package")
        } catch (e: SecurityException) {
            assertTrue(e.message?.contains("package") == true)
        }
    }

    fun testManifestValidationRejectsWrongSigner() {
        val json = JSONObject().apply {
            put("version", "0.5.2")
            put("version_code", 21)
            put("package", "com.metrix.writer")
            put("signer_sha256", "f".repeat(64))
            put("url", "https://updates.tricktic.com/writer/releases/0.5.2/writer.apk")
            put("sha256", "a".repeat(64))
            put("bytes", 25000000L)
        }

        try {
            updater.parseAndValidateManifest(json)
            fail("Expected SecurityException for wrong signer")
        } catch (e: SecurityException) {
            assertTrue(e.message?.contains("signer") == true)
        }
    }

    fun testManifestValidationRejectsBadSha256OrBytes() {
        val jsonBadHash = JSONObject().apply {
            put("version", "0.5.2")
            put("version_code", 21)
            put("package", "com.metrix.writer")
            put("signer_sha256", "e35e0ff5447132ced8c1228783a5f7cccf782af0f33af77e152f74e0a02f10fb")
            put("url", "https://updates.tricktic.com/writer/releases/0.5.2/writer.apk")
            put("sha256", "invalid-hash")
            put("bytes", 25000000L)
        }
        try {
            updater.parseAndValidateManifest(jsonBadHash)
            fail("Expected IllegalArgumentException for bad sha256")
        } catch (e: IllegalArgumentException) {
            assertTrue(e.message?.contains("sha256") == true)
        }

        val jsonBadBytes = JSONObject().apply {
            put("version", "0.5.2")
            put("version_code", 21)
            put("package", "com.metrix.writer")
            put("signer_sha256", "e35e0ff5447132ced8c1228783a5f7cccf782af0f33af77e152f74e0a02f10fb")
            put("url", "https://updates.tricktic.com/writer/releases/0.5.2/writer.apk")
            put("sha256", "a".repeat(64))
            put("bytes", -1L)
        }
        try {
            updater.parseAndValidateManifest(jsonBadBytes)
            fail("Expected IllegalArgumentException for negative bytes")
        } catch (e: IllegalArgumentException) {
            assertTrue(e.message?.contains("byte length") == true)
        }
    }

    fun testUpdateStateSerialization() {
        val info = UpdateInfo(
            version = "0.5.2",
            versionCode = 21,
            packageName = "com.metrix.writer",
            signerSha256 = "e35e0ff5447132ced8c1228783a5f7cccf782af0f33af77e152f74e0a02f10fb",
            url = "https://updates.tricktic.com/writer/releases/0.5.2/writer.apk",
            sha256 = "a".repeat(64),
            bytes = 1024L,
            notes = "Test notes",
            pubDate = "2026-09-15"
        )

        val json = info.toJson()
        val deserialized = UpdateInfo.fromJson(json)
        assertEquals(info.version, deserialized.version)
        assertEquals(info.versionCode, deserialized.versionCode)
        assertEquals(info.packageName, deserialized.packageName)
        assertEquals(info.signerSha256, deserialized.signerSha256)
        assertEquals(info.url, deserialized.url)
        assertEquals(info.sha256, deserialized.sha256)
        assertEquals(info.bytes, deserialized.bytes)
        assertEquals(info.notes, deserialized.notes)
    }

    fun testApkVerificationRejectsNonexistentOrCorruptFile() {
        val fakeFile = File(context.cacheDir, "nonexistent.apk")
        val info = UpdateInfo(
            version = "0.5.2",
            versionCode = 21,
            packageName = "com.metrix.writer",
            signerSha256 = "e35e0ff5447132ced8c1228783a5f7cccf782af0f33af77e152f74e0a02f10fb",
            url = "https://updates.tricktic.com/writer/releases/0.5.2/writer.apk",
            sha256 = "a".repeat(64),
            bytes = 100L,
            notes = "",
            pubDate = ""
        )

        try {
            updater.verifyApkFile(fakeFile, info)
            fail("Expected IllegalArgumentException for missing file")
        } catch (e: IllegalArgumentException) {
            assertTrue(e.message?.contains("does not exist") == true)
        }

        // Corrupt file (not a valid zip/apk)
        val corruptFile = File(context.cacheDir, "corrupt.apk")
        corruptFile.writeBytes(ByteArray(200) { it.toByte() })
        try {
            updater.verifyApkFile(corruptFile, info.copy(bytes = corruptFile.length(), sha256 = hash(corruptFile.readBytes())))
            fail("Expected SecurityException for corrupt apk")
        } catch (e: SecurityException) {
            assertTrue(e.message?.contains("archive info") == true)
        } finally {
            corruptFile.delete()
        }
    }

    fun testForeignCallbacksDoNotChangeState() {
        val before = updater.state.value
        updater.onInstallAborted(100, "User cancelled")
        updater.onInstallFailed(100, android.content.pm.PackageInstaller.STATUS_FAILURE_STORAGE, null)
        updater.onInstallSuccess(100)
        updater.onPendingUserAction(100, null)
        assertEquals(before, updater.state.value)
    }

    fun testReconcilePreservesLastChecked() {
        val prefs = context.getSharedPreferences(AndroidUpdater.PREFS_NAME, Context.MODE_PRIVATE)
        prefs.edit()
            .putLong(AndroidUpdater.PREF_LAST_CHECKED_MS, 1726400000000L)
            .remove(AndroidUpdater.PREF_AVAILABLE_UPDATE)
            .remove(AndroidUpdater.PREF_DOWNLOADED_APK_PATH)
            .apply()
        updater.reconcileState()
        val state = updater.state.value
        assertTrue(state is UpdateState.Idle)
        assertEquals(1726400000000L, (state as UpdateState.Idle).lastCheckedMs)
    }

    fun testManifestValidationRejectsBlankUrlAndZeroBytes() {
        val base = JSONObject().apply {
            put("version", "0.5.2")
            put("version_code", 21)
            put("package", "com.metrix.writer")
            put("signer_sha256", "e35e0ff5447132ced8c1228783a5f7cccf782af0f33af77e152f74e0a02f10fb")
            put("url", "https://updates.tricktic.com/writer/releases/0.5.2/writer.apk")
            put("sha256", "a".repeat(64))
            put("bytes", 25000000L)
        }
        try {
            updater.parseAndValidateManifest(JSONObject(base.toString()).put("url", "  "))
            fail("Expected IllegalArgumentException for blank artifact URL")
        } catch (e: IllegalArgumentException) {
            assertTrue(e.message?.contains("artifact URL") == true)
        }
        try {
            updater.parseAndValidateManifest(JSONObject(base.toString()).put("bytes", 0L))
            fail("Expected IllegalArgumentException for zero byte length")
        } catch (e: IllegalArgumentException) {
            assertTrue(e.message?.contains("byte length") == true)
        }
    }

    fun testUpdateInfoRoundTripDefaultsMissingNotes() {
        val json = JSONObject().apply {
            put("version", "0.5.2")
            put("version_code", 21)
            put("package", "com.metrix.writer")
            put("signer_sha256", "e35e0ff5447132ced8c1228783a5f7cccf782af0f33af77e152f74e0a02f10fb")
            put("url", "https://updates.tricktic.com/writer/releases/0.5.2/writer.apk")
            put("sha256", "a".repeat(64))
            put("bytes", 1024L)
        }
        val info = UpdateInfo.fromJson(json)
        assertEquals("", info.notes)
        assertEquals("", info.pubDate)
    }

    fun testFeedUrlIsPinnedHttpsUpdateHost() {
        updater.setFeedUrlOverride(null)
        val url = java.net.URL(updater.getFeedUrl())
        assertEquals("https", url.protocol)
        assertEquals("updates.tricktic.com", url.host)
        assertTrue(url.path.endsWith("android-stable.json"))
    }
    private fun hash(bytes: ByteArray) = java.security.MessageDigest.getInstance("SHA-256")
        .digest(bytes).joinToString("") { "%02x".format(it) }

    private fun info(bytes: ByteArray = byteArrayOf(1, 2, 3)) = UpdateInfo(
        "9.9.9", 999999, AndroidUpdater.EXPECTED_PACKAGE, AndroidUpdater.RETAINED_SIGNER_SHA256,
        "https://updates.tricktic.com/writer/releases/9.9.9/writer.apk", hash(bytes), bytes.size.toLong(), "", "")

    private fun rejected(block: () -> Unit) {
        try { block(); fail("Expected rejection") } catch (_: Exception) { }
    }

    fun testBoundedFlatJsonBeforeParsing() {
        rejected { UpdateTransfer.manifestJson(" ".repeat(UpdateTransfer.MAX_FEED_BYTES.toInt() + 1)) }
        rejected { UpdateTransfer.manifestJson("{\"x\":".repeat(1000) + "0" + "}".repeat(1000)) }
        rejected { UpdateTransfer.manifestJson("{\"x\":[]}") }
        val text = info().copy(notes = "Escaped \"quotes\" and [brackets] {braces}").toJson().toString()
        assertEquals(info().version, updater.parseAndValidateManifest(UpdateTransfer.manifestJson(text)).version)
    }

    fun testOversizedAndMalformedFeedFields() {
        for ((key, value) in listOf("notes" to "a".repeat(16385), "version" to "a".repeat(81),
            "version_code" to Long.MAX_VALUE, "version_code" to 2.5, "version_code" to "21",
            "bytes" to UpdateTransfer.MAX_APK_BYTES + 1, "bytes" to 1.5, "bytes" to "25")) {
            rejected { updater.parseAndValidateManifest(info().toJson().put(key, value)) }
        }
        val unknown = info().toJson().put("extra", "value")
        rejected { updater.parseAndValidateManifest(unknown) }
    }

    fun testOnlyPinnedHttpsDestinationsAndNoRedirects() {
        val good = info().url
        for (url in listOf(good.replace("https:", "http:"), good.replace("updates.tricktic.com", "evil.test"),
            good.replace("updates.tricktic.com", "updates.tricktic.com.evil.test"),
            good.replace("updates.tricktic.com", "user@updates.tricktic.com"),
            good.replace("updates.tricktic.com", "updates.tricktic.com:8443"),
            "$good?redirect=evil", "$good#fragment", good.replace("9.9.9/", "../"),
            good.replace("9.9.9/", "%2e%2e/"))) {
            rejected { UpdateTransfer.url(url) }
        }
        val conn = UpdateTransfer.open(good)
        assertFalse(conn.instanceFollowRedirects)
        conn.disconnect()
        for (code in listOf(301, 302, 303, 307, 308)) {
            val redirect = object : java.net.HttpURLConnection(java.net.URL(good)) {
                override fun connect() {}
                override fun disconnect() {}
                override fun usingProxy() = false
                override fun getResponseCode() = code
                override fun getHeaderField(name: String?) = if (name == "Location") "http://evil.test/writer.apk" else null
            }
            rejected { UpdateTransfer.response(redirect, 100) }
        }
    }

    fun testFeedOverrideRemainsConstrained() {
        rejected { updater.setFeedUrlOverride("http://evil.test/feed") }
        if (BuildConfig.DEBUG) {
            updater.setFeedUrlOverride("https://updates.tricktic.com/writer/channels/android-test.json")
            assertTrue(updater.getFeedUrl().endsWith("android-test.json"))
        } else {
            rejected { updater.setFeedUrlOverride("https://updates.tricktic.com/writer/channels/android-test.json") }
        }
        updater.setFeedUrlOverride(null)
        assertEquals(AndroidUpdater.DEFAULT_FEED_URL, updater.getFeedUrl())
    }

    fun testEndlessFeedAndApkStopBeforeExcessWrite() {
        for (limit in listOf(UpdateTransfer.MAX_FEED_BYTES, 100_000L)) {
            var reads = 0L
            val endless = object : java.io.InputStream() {
                override fun read() = 7
                override fun read(b: ByteArray, off: Int, len: Int): Int {
                    java.util.Arrays.fill(b, off, off + len, 7.toByte()); reads += len; return len
                }
            }
            val output = java.io.ByteArrayOutputStream()
            rejected { UpdateTransfer.copy(endless, output, limit, exact = false) }
            assertTrue(output.size() <= limit)
            assertEquals(limit + 1, reads)
        }
    }

    fun testTruncationCancellationAndFreeSpaceStopWrites() {
        val bytes = ByteArray(33000)
        rejected { UpdateTransfer.copy(bytes.inputStream(), java.io.ByteArrayOutputStream(), 33001) }
        val output = java.io.ByteArrayOutputStream()
        rejected {
            UpdateTransfer.copy(bytes.inputStream(), output, bytes.size.toLong(),
                checkActive = { if (output.size() > 0) throw kotlinx.coroutines.CancellationException() })
        }
        assertEquals(16384, output.size())
        val full = java.io.ByteArrayOutputStream()
        rejected {
            UpdateTransfer.copy(bytes.inputStream(), full, bytes.size.toLong(),
                beforeWrite = { UpdateTransfer.space(UpdateTransfer.SPACE_RESERVE, it.toLong()) })
        }
        assertEquals(0, full.size())
        rejected { UpdateTransfer.space(UpdateTransfer.SPACE_RESERVE + 99, 100) }
        UpdateTransfer.space(UpdateTransfer.SPACE_RESERVE + 100, 100)
        assertEquals(hash(bytes), UpdateTransfer.copy(bytes.inputStream(), java.io.ByteArrayOutputStream(), bytes.size.toLong()))
    }

    fun testDeclaredResponseSizeAndEncodingAreRejected() {
        for ((length, encoding) in listOf(101L to "identity", 99L to "identity", 100L to "gzip")) {
            val conn = object : java.net.HttpURLConnection(java.net.URL(info().url)) {
                override fun connect() {}
                override fun disconnect() {}
                override fun usingProxy() = false
                override fun getResponseCode() = 200
                override fun getContentLengthLong() = length
                override fun getContentEncoding() = encoding
            }
            rejected { UpdateTransfer.response(conn, 100, exact = true) }
        }
    }

    fun testCacheTamperAndInterruptedDownloadRecovery() {
        val dir = File(context.filesDir, "updates").apply { mkdirs() }
        val data = byteArrayOf(1, 2, 3)
        val expected = info(data)
        val apk = File(dir, "writer-${expected.versionCode}.apk").apply { writeBytes(byteArrayOf(3, 2, 1)) }
        val temp = File(dir, "interrupted.apk.tmp").apply { writeBytes(data) }
        val prefs = context.getSharedPreferences(AndroidUpdater.PREFS_NAME, Context.MODE_PRIVATE)
        prefs.edit().putString(AndroidUpdater.PREF_AVAILABLE_UPDATE, expected.toJson().toString())
            .putString(AndroidUpdater.PREF_DOWNLOADED_APK_PATH, apk.absolutePath).commit()
        try {
            updater.verifyApkFile(apk, expected)
            fail("Same-length tamper must fail")
        } catch (e: SecurityException) { assertTrue(e.message!!.contains("checksum")) }
        updater.reconcileState()
        assertTrue(updater.state.value is UpdateState.Available)
        assertFalse(apk.exists())
        assertFalse(temp.exists())
        assertFalse(prefs.contains(AndroidUpdater.PREF_DOWNLOADED_APK_PATH))
    }

    fun testInstallCallbackSessionNonceAndTerminalReplay() {
        val prefs = context.getSharedPreferences(AndroidUpdater.PREFS_NAME, Context.MODE_PRIVATE)
        prefs.edit().putInt(AndroidUpdater.PREF_INSTALL_SESSION, 123)
            .putString(AndroidUpdater.PREF_INSTALL_NONCE, "test-nonce").commit()
        val intent = updater.installStatusIntent(123, "test-nonce")
            .putExtra(android.content.pm.PackageInstaller.EXTRA_SESSION_ID, 123)
        assertEquals(context.packageName, intent.component!!.packageName)
        assertEquals(InstallReceiver::class.java.name, intent.component!!.className)
        assertTrue(updater.acceptsInstallStatus(intent))
        assertFalse(updater.acceptsInstallStatus(android.content.Intent(intent)
            .putExtra(android.content.pm.PackageInstaller.EXTRA_SESSION_ID, 456)))
        assertFalse(updater.acceptsInstallStatus(updater.installStatusIntent(123, "foreign")
            .putExtra(android.content.pm.PackageInstaller.EXTRA_SESSION_ID, 123)))
        assertFalse(updater.acceptsInstallStatus(android.content.Intent(intent).setPackage("foreign")))
        updater.onInstallFailed(456, android.content.pm.PackageInstaller.STATUS_FAILURE_STORAGE, null)
        assertTrue(updater.state.value is UpdateState.Idle)
        updater.onInstallFailed(123, android.content.pm.PackageInstaller.STATUS_FAILURE_STORAGE, null)
        assertEquals("Insufficient storage space to install update.", (updater.state.value as UpdateState.Error).message)
        assertFalse(updater.acceptsInstallStatus(intent))
        updater.onInstallSuccess(123)
        assertTrue(updater.state.value is UpdateState.Error)
    }

    fun testProcessRecoveryAbandonsUnsealedSession() {
        val installer = context.packageManager.packageInstaller
        val params = android.content.pm.PackageInstaller.SessionParams(android.content.pm.PackageInstaller.SessionParams.MODE_FULL_INSTALL)
        params.setAppPackageName(AndroidUpdater.EXPECTED_PACKAGE)
        val id = installer.createSession(params)
        try {
            context.getSharedPreferences(AndroidUpdater.PREFS_NAME, Context.MODE_PRIVATE).edit()
                .putString(AndroidUpdater.PREF_AVAILABLE_UPDATE, info().toJson().toString())
                .putInt(AndroidUpdater.PREF_INSTALL_SESSION, id)
                .putString(AndroidUpdater.PREF_INSTALL_NONCE, "recovery").commit()
            updater.reconcileState()
            assertTrue(updater.state.value is UpdateState.Available)
            assertNull(installer.getSessionInfo(id))
        } finally { runCatching { installer.abandonSession(id) } }
    }

    fun testMutableCallbackCannotBeRedirected() {
        val prefs = context.getSharedPreferences(AndroidUpdater.PREFS_NAME, Context.MODE_PRIVATE)
        prefs.edit().putInt(AndroidUpdater.PREF_INSTALL_SESSION, 124)
            .putString(AndroidUpdater.PREF_INSTALL_NONCE, "bound-nonce").commit()
        val callback = updater.installStatusCallback(124, "bound-nonce")
        try {
            assertFalse(callback.isImmutable)
            assertEquals(context.packageName, callback.creatorPackage)
            val fillIn = android.content.Intent("foreign.action").apply {
                component = android.content.ComponentName("foreign.package", "foreign.Receiver")
                `package` = "foreign.package"
                data = android.net.Uri.parse("writer-update://foreign")
                putExtra(android.content.pm.PackageInstaller.EXTRA_SESSION_ID, 124)
                putExtra(android.content.pm.PackageInstaller.EXTRA_STATUS, android.content.pm.PackageInstaller.STATUS_FAILURE_STORAGE)
            }
            callback.send(context, 0, fillIn)
            val deadline = android.os.SystemClock.elapsedRealtime() + 5000
            while (prefs.contains(AndroidUpdater.PREF_INSTALL_SESSION) && android.os.SystemClock.elapsedRealtime() < deadline) {
                Thread.sleep(20)
            }
            assertFalse("Explicit callback must reach our receiver with its original identity", prefs.contains(AndroidUpdater.PREF_INSTALL_SESSION))
            assertTrue(updater.state.value is UpdateState.Error)
        } finally { callback.cancel() }
    }

}
