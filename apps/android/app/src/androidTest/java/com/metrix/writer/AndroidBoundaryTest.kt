@file:Suppress("DEPRECATION")
package com.metrix.writer

import android.content.*
import android.content.pm.PackageManager
import android.net.Uri
import android.os.*
import android.test.InstrumentationTestCase
import java.io.File
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

/** Writer instrumentation prepares/observes state; attack calls run in the differently signed editor. */
class AndroidBoundaryTest : InstrumentationTestCase() {
    private val context get() = instrumentation.targetContext
    private val attacker = "com.metrix.writer.editor"
    override fun setUp() {
        super.setUp()
        assertTrue("Use an emulator or the distinct physical security-test package",
            Build.HARDWARE.contains("ranchu") || Build.HARDWARE.contains("goldfish") ||
                context.packageName.endsWith(".securityphone"))
        UpdaterProbe.prime(context)
        val pm = context.packageManager
        assertFalse(context.applicationInfo.uid == pm.getApplicationInfo(attacker, 0).uid)
        assertEquals("Resign the editor with an independent fixture key", PackageManager.SIGNATURE_NO_MATCH,
            pm.checkSignatures(context.packageName, attacker))
    }
    private fun attack(operation: String, uri: Uri? = null, activity: String? = null): Bundle {
        val done = CountDownLatch(1)
        var result: Bundle? = null
        val reply = object : ResultReceiver(Handler(Looper.getMainLooper())) {
            override fun onReceiveResult(code: Int, data: Bundle) { result = data; done.countDown() }
        }
        // Marshal as the framework class; the other APK cannot load this test's anonymous subclass.
        val parcel = Parcel.obtain()
        val remoteReply = try {
            reply.writeToParcel(parcel, 0); parcel.setDataPosition(0)
            ResultReceiver.CREATOR.createFromParcel(parcel)
        } finally { parcel.recycle() }
        context.startActivity(Intent().setClassName(attacker, "$attacker.HostileActivity")
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_MULTIPLE_TASK).setData(uri)
            .putExtra("target", context.packageName).putExtra("operation", operation)
            .putExtra("activity", activity).putExtra("reply", remoteReply))
        assertTrue("Hostile fixture did not reply", done.await(15, TimeUnit.SECONDS))
        return result!!.also { assertEquals(context.packageManager.getApplicationInfo(attacker, 0).uid, it.getInt("uid")) }
    }
    fun testSeparateUidCannotStartBindOrOpenPrivateComponents() {
        val result = attack("components")
        assertEquals(15, result.size()) // UID, eight service calls, six private activities.
        for (key in result.keySet() - "uid") assertEquals(key, "denied", result.getString(key))
        assertFalse(CaptureService.isActive())
    }
    fun testExportRequiresExactReadGrantAndRevocationTakesEffect() {
        val directory = File(context.cacheDir, "boundary-${java.util.UUID.randomUUID()}")
        val store = SessionStore(directory)
        val item = store.create("mixed")
        val source = store.audio(item).apply { appendBytes(byteArrayOf(1, 2, 3, 4)) }
        store.finishCapture(item, null)
        val original = source.readBytes()
        val id = item.getString("id")
        val uri = AudioExportProvider.snapshot(context, store, id)
        try {
            val denied = attack("export", uri)
            for (key in listOf("read", "query", "write", "sibling", "traversal")) assertEquals(key, "denied", denied.getString(key))
            context.grantUriPermission(attacker, uri, Intent.FLAG_GRANT_READ_URI_PERMISSION)
            val granted = attack("export", uri)
            assertEquals("allowed", granted.getString("read")); assertEquals(original.size, granted.getInt("bytes"))
            assertEquals("allowed", granted.getString("query"))
            for (key in listOf("write", "sibling", "traversal")) assertEquals(key, "denied", granted.getString(key))
            context.revokeUriPermission(uri, Intent.FLAG_GRANT_READ_URI_PERMISSION)
            assertEquals("denied", attack("export", uri).getString("read"))
            assertTrue(source.readBytes().contentEquals(original))
        } finally {
            context.revokeUriPermission(uri, Intent.FLAG_GRANT_READ_URI_PERMISSION)
            directory.deleteRecursively(); AudioExportProvider.erase(context, id)
        }
    }
    fun testForgedInstallBroadcastDoesNotChangeUpdaterOrLaunchCallback() {
        if (UpdaterProbe.PRESENT) assertFalse(UpdaterProbe.receiverExported(context))
        else try {
            context.packageManager.getReceiverInfo(ComponentName(context.packageName, "com.metrix.writer.InstallReceiver"), 0)
            fail("The Play build must not contain the install receiver")
        } catch (_: PackageManager.NameNotFoundException) { }
        val before = UpdaterProbe.state(context)
        val result = attack("callback")
        assertEquals(0, result.getInt("callbacks"))
        assertEquals(before, UpdaterProbe.state(context))
    }
    fun testExportedEntrypointsIgnoreRecordingAndInsertionExtras() {
        val files = File(context.filesDir, "recordings").list().orEmpty().toSet()
        val deliveries = File(context.filesDir, "deliveries").list().orEmpty().toSet()
        for (activity in listOf("MainActivity", "FloatingSetupActivity")) {
            assertEquals("allowed", attack("launch", activity = activity).getString("launch"))
            SystemClock.sleep(600)
            assertFalse(CaptureService.isActive())
            assertEquals(files, File(context.filesDir, "recordings").list().orEmpty().toSet())
            assertEquals(deliveries, File(context.filesDir, "deliveries").list().orEmpty().toSet())
        }
    }
}
