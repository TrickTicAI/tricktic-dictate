@file:Suppress("DEPRECATION")
package com.metrix.writer

import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Process
import android.test.InstrumentationTestCase
import android.test.InstrumentationTestRunner
import org.json.JSONObject
import java.io.File
import java.util.UUID

/** Opt-in process-death fixture, run in two instrumentation invocations on an owned emulator. */
class CacheCrashTest : InstrumentationTestCase() {
    fun testProcessDeathAfterDeletionIntent() {
        val phase = (instrumentation as InstrumentationTestRunner).arguments.getString("writerCrashPhase") ?: return
        val context = instrumentation.targetContext
        val receipt = File(context.filesDir, "cache-crash-fixture.json")
        val store = SessionStore(File(context.filesDir, "recordings"), context)
        if (phase == "seed") {
            check(!receipt.exists())
            val item = store.create("en"); store.audio(item).appendBytes(byteArrayOf(1, 2)); store.finishCapture(item, null)
            val id = item.getString("id")
            val uri = AudioExportProvider.snapshot(context, store, id)
            context.grantUriPermission("com.android.shell", uri, Intent.FLAG_GRANT_READ_URI_PERMISSION)
            // Simulate the result-cache state of a dying client. Kernel ownership dies with it.
            val lease = PhoneResultCache.lease(context, true)!!
            val attempt = UUID.randomUUID().toString()
            PhoneResultCache.file(context, attempt).writeText("synthetic result")
            receipt.outputStream().use { out ->
                out.write(JSONObject().put("id", id).put("uri", uri.toString()).put("attempt", attempt).toString().toByteArray())
                out.fd.sync()
            }
            store.save(JSONObject().put("id", id).put("capture_state", "discarded"))
            Process.killProcess(Process.myPid())
            lease.close()
            fail("Expected process death")
        } else {
            check(phase == "verify")
            val saved = JSONObject(receipt.readText()); val id = saved.getString("id"); val uri = Uri.parse(saved.getString("uri"))
            // Provider startup, before opening History, must already have completed cleanup.
            assertFalse(File(context.cacheDir, "audio-exports/$id").exists())
            assertFalse(File(store.directory, "$id.wav").exists())
            assertFalse(PhoneResultCache.file(context, saved.getString("attempt")).exists())
            assertEquals("discarded", store.load(id).getString("capture_state"))
            val uid = context.packageManager.getApplicationInfo("com.android.shell", 0).uid
            assertEquals(PackageManager.PERMISSION_DENIED, context.checkUriPermission(uri, -1, uid, Intent.FLAG_GRANT_READ_URI_PERMISSION))
            try { context.contentResolver.openInputStream(uri)?.close(); fail("Deleted snapshot reopened") }
            catch (_: java.io.FileNotFoundException) { }
            File(store.directory, "$id.json").delete(); receipt.delete()
        }
    }
}
