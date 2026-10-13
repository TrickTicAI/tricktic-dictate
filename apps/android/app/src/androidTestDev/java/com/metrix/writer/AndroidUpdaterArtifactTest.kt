@file:Suppress("DEPRECATION")
package com.metrix.writer

import android.content.Context
import android.content.pm.PackageInstaller
import android.content.pm.PackageManager
import android.test.InstrumentationTestCase
import java.io.File
import java.security.MessageDigest

/** Run only on an owned emulator with the three fixtures in cache/updater-fixtures. */
class AndroidUpdaterArtifactTest : InstrumentationTestCase() {
    private val context get() = instrumentation.targetContext
    private val updater get() = AndroidUpdater.getInstance(context)
    private val prefs get() = context.getSharedPreferences(AndroidUpdater.PREFS_NAME, Context.MODE_PRIVATE)
    private fun fixture(name: String) = File(context.cacheDir, "updater-fixtures/$name.apk").also {
        assertTrue("Stage updater fixture $name (docs/testing/android-updates.md)", it.isFile)
    }
    private fun info(file: File): UpdateInfo {
        val archive = context.packageManager.getPackageArchiveInfo(file.absolutePath, 0)
        val digest = MessageDigest.getInstance("SHA-256")
        file.inputStream().use { input ->
            val bytes = ByteArray(16384)
            while (true) { val n = input.read(bytes); if (n < 0) break; digest.update(bytes, 0, n) }
        }
        return UpdateInfo(archive?.versionName ?: "9.9.9", archive?.longVersionCode?.toInt() ?: 999999, AndroidUpdater.EXPECTED_PACKAGE,
            AndroidUpdater.RETAINED_SIGNER_SHA256, "https://updates.tricktic.com/writer/releases/fixture/writer.apk",
            digest.digest().joinToString("") { "%02x".format(it) }, file.length(), "", "")
    }
    private fun rejected(message: String, block: () -> Unit) {
        try { block(); fail("Expected $message rejection") }
        catch (e: Exception) { assertTrue("Unexpected failure: ${e.message}", e.message?.contains(message) == true) }
    }
    override fun setUp() {
        super.setUp()
        updater // Initialize before setting up test state.
        prefs.edit().clear().commit()
        updater.reconcileState()
    }
    fun testRetainedSignerArtifactAndIdentityRejections() {
        val good = fixture("retained")
        val expected = info(good)
        updater.verifyApkFile(good, expected)
        rejected("version code") { updater.verifyApkFile(good, expected.copy(versionCode = 999999)) }
        rejected("version name") { updater.verifyApkFile(good, expected.copy(version = "wrong")) }
        val otherPackage = File(context.applicationInfo.sourceDir)
        rejected("package") { updater.verifyApkFile(otherPackage, info(otherPackage)) }
        val wrongSigner = fixture("wrong-signer")
        rejected("signer") { updater.verifyApkFile(wrongSigner, info(wrongSigner)) }
        val incompatible = fixture("future-sdk")
        // API 36 refuses to parse archives whose minSdk is too high.
        rejected("archive info") { updater.verifyApkFile(incompatible, info(incompatible)) }
    }
    fun testTamperedCacheCannotCreateInstallSession() {
        val good = fixture("retained")
        val expected = info(good)
        val dir = File(context.filesDir, "updates").apply { mkdirs() }
        val copy = good.copyTo(File(dir, "writer-${expected.versionCode}.apk"), overwrite = true)
        java.io.RandomAccessFile(copy, "rw").use { it.seek(100); it.write(99) }
        val before = context.packageManager.packageInstaller.mySessions.map { it.sessionId }
        rejected("checksum") { updater.commitSession(expected, copy) }
        assertFalse(copy.exists())
        assertEquals(before, context.packageManager.packageInstaller.mySessions.map { it.sessionId })
    }
    fun testRealInstallerCallbackAndSealedRecovery() {
        val good = fixture("retained")
        val expected = info(good)
        val id = updater.commitSession(expected, good)
        val installer = context.packageManager.packageInstaller
        try {
            updater.reconcileState()
            val recovered = updater.state.value as UpdateState.Installing
            assertEquals(id, recovered.sessionId)
            val deadline = android.os.SystemClock.elapsedRealtime() + 15000
            while ((updater.state.value as? UpdateState.Installing)?.pendingUserAction == false &&
                android.os.SystemClock.elapsedRealtime() < deadline) Thread.sleep(50)
            val pending = updater.state.value as UpdateState.Installing
            assertTrue("Real OS pending-user-action callback must be accepted", pending.pendingUserAction)
            installer.abandonSession(id)
            val end = android.os.SystemClock.elapsedRealtime() + 5000
            while (prefs.contains(AndroidUpdater.PREF_INSTALL_SESSION) && android.os.SystemClock.elapsedRealtime() < end) Thread.sleep(20)
            assertFalse(prefs.contains(AndroidUpdater.PREF_INSTALL_SESSION))
        } finally { runCatching { installer.abandonSession(id) } }
    }
}
