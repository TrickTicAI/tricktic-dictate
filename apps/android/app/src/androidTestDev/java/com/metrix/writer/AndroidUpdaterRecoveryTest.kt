@file:Suppress("DEPRECATION")
package com.metrix.writer

import android.content.Context
import android.content.pm.PackageInstaller
import android.test.InstrumentationTestCase
import java.io.File

/** Two phases separated by adb force-stop; uses only synthetic updater state. */
class AndroidUpdaterRecoveryTest : InstrumentationTestCase() {
    fun testPrepareInterruptedInstall() {
        val context = instrumentation.targetContext
        AndroidUpdater.getInstance(context)
        val prefs = context.getSharedPreferences(AndroidUpdater.PREFS_NAME, Context.MODE_PRIVATE)
        prefs.edit().clear().commit()
        val bytes = byteArrayOf(1, 2, 3)
        val expected = UpdateInfo("9.9.9", 999999, AndroidUpdater.EXPECTED_PACKAGE,
            AndroidUpdater.RETAINED_SIGNER_SHA256, "https://updates.tricktic.com/writer/releases/fixture/writer.apk",
            "0".repeat(64), bytes.size.toLong(), "", "")
        val dir = File(context.filesDir, "updates").apply { mkdirs() }
        val apk = File(dir, "writer-999999.apk").apply { writeBytes(bytes) }
        File(dir, "interrupted.apk.tmp").writeBytes(bytes)
        val params = PackageInstaller.SessionParams(PackageInstaller.SessionParams.MODE_FULL_INSTALL)
        params.setAppPackageName(AndroidUpdater.EXPECTED_PACKAGE)
        val id = context.packageManager.packageInstaller.createSession(params)
        context.packageManager.packageInstaller.openSession(id).use { session ->
            session.openWrite("writer_apk", 0, 3).use { it.write(bytes); session.fsync(it) }
        }
        prefs.edit().putString(AndroidUpdater.PREF_AVAILABLE_UPDATE, expected.toJson().toString())
            .putString(AndroidUpdater.PREF_DOWNLOADED_APK_PATH, apk.absolutePath)
            .putInt(AndroidUpdater.PREF_INSTALL_SESSION, id)
            .putString(AndroidUpdater.PREF_INSTALL_NONCE, "process-death-fixture")
            .putInt("test_old_pid", android.os.Process.myPid()).commit()
    }

    fun testVerifyAfterProcessDeath() {
        val context = instrumentation.targetContext
        val prefs = context.getSharedPreferences(AndroidUpdater.PREFS_NAME, Context.MODE_PRIVATE)
        val oldPid = prefs.getInt("test_old_pid", -1)
        assertTrue("Run the preparation phase first", oldPid > 0)
        assertTrue("Force-stop the app between phases", oldPid != android.os.Process.myPid())
        val updater = AndroidUpdater.getInstance(context)
        assertTrue(updater.state.value is UpdateState.Available)
        assertFalse(prefs.contains(AndroidUpdater.PREF_INSTALL_SESSION))
        assertFalse(prefs.contains(AndroidUpdater.PREF_DOWNLOADED_APK_PATH))
        assertTrue(File(context.filesDir, "updates").listFiles()!!.isEmpty())
        assertTrue(context.packageManager.packageInstaller.mySessions.isEmpty())
    }
}
