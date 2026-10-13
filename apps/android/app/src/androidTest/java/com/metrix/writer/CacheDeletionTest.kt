@file:Suppress("DEPRECATION")
package com.metrix.writer

import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.test.InstrumentationTestCase
import android.os.ParcelFileDescriptor
import org.json.JSONObject
import java.io.File
import java.io.RandomAccessFile
import java.util.UUID
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

class CacheDeletionTest : InstrumentationTestCase() {
    private val context get() = instrumentation.targetContext
    private lateinit var store: SessionStore
    private val ids = mutableListOf<String>()
    override fun setUp() {
        super.setUp()
        store = SessionStore(File(context.filesDir, "recordings"), context)
    }
    override fun tearDown() {
        ids.forEach { id ->
            File(store.directory, "$id.wav").delete()
            File(store.directory, "$id.json").delete()
            File(context.cacheDir, "audio-exports/$id").deleteRecursively()
        }
        super.tearDown()
    }
    private fun saved(): JSONObject = store.create("en").also {
        store.audio(it).appendBytes(byteArrayOf(1, 2, 3, 4))
        store.finishCapture(it, null)
        ids.add(it.getString("id"))
    }
    private fun snapshot(item: JSONObject) = AudioExportProvider.snapshot(context, store, item.getString("id"))
    private fun cache(uri: Uri) = File(context.cacheDir, "audio-exports/${uri.pathSegments.joinToString("/")}")
    private fun cannotOpen(uri: Uri) {
        try { context.contentResolver.openInputStream(uri)?.close(); fail("Deleted export reopened") }
        catch (_: java.io.FileNotFoundException) { }
    }
    private fun grant(uri: Uri): Int {
        val recipient = "com.android.shell"
        val uid = context.packageManager.getApplicationInfo(recipient, 0).uid
        context.grantUriPermission(recipient, uri, Intent.FLAG_GRANT_READ_URI_PERMISSION)
        assertEquals(PackageManager.PERMISSION_GRANTED, context.checkUriPermission(uri, -1, uid, Intent.FLAG_GRANT_READ_URI_PERMISSION))
        return uid
    }
    fun testDeleteRemovesMultipleSnapshotsAndRevokesGrantedUriImmediately() {
        val item = saved(); val one = snapshot(item); val two = snapshot(item)
        val other = saved(); val retained = snapshot(other)
        val uid = grant(one)
        context.contentResolver.openInputStream(one)!!.use { assertTrue(it.readBytes().contentEquals(store.audio(item).readBytes())) }
        val before = ParcelFileDescriptor.AutoCloseInputStream(instrumentation.uiAutomation.executeShellCommand("content read --uri $one")).use { it.readBytes() }
        assertTrue("Granted separate UID can read", before.contentEquals(store.audio(item).readBytes()))
        store.deleteSaved(item.getString("id"))
        val after = ParcelFileDescriptor.AutoCloseInputStream(instrumentation.uiAutomation.executeShellCommand("content read --uri $one")).use { it.readBytes() }
        assertFalse("Revoked recipient cannot reopen", before.contentEquals(after))
        for (uri in listOf(one, two)) { cannotOpen(uri); assertFalse(cache(uri).exists()) }
        assertEquals(PackageManager.PERMISSION_DENIED, context.checkUriPermission(one, -1, uid, Intent.FLAG_GRANT_READ_URI_PERMISSION))
        assertTrue(cache(retained).exists()); assertTrue(store.audio(other).exists())
        try { snapshot(item); fail("Deleted source shared") } catch (_: IllegalStateException) { }
    }
    fun testEvictedSnapshotStillHasItsGrantRevoked() {
        val item = saved(); val uri = snapshot(item); val uid = grant(uri)
        assertTrue(cache(uri).delete())
        store.deleteSaved(item.getString("id"))
        assertEquals(PackageManager.PERMISSION_DENIED, context.checkUriPermission(uri, -1, uid, Intent.FLAG_GRANT_READ_URI_PERMISSION))
    }
    fun testRestartResumesTombstoneAndSweepsExpiredLegacyAndOrphanFiles() {
        val deleted = saved(); val uri = snapshot(deleted); val uid = grant(uri)
        val kept = saved(); val expired = snapshot(kept); val fresh = snapshot(kept)
        assertTrue(cache(expired).setLastModified(System.currentTimeMillis() - 25 * 60 * 60 * 1000L))
        val old = File(context.cacheDir, "audio-exports/${UUID.randomUUID()}.wav").apply { writeText("fixture") }
        val result = PhoneResultCache.file(context, UUID.randomUUID().toString()).apply { writeText("fixture") }
        // Durable crash boundary: process dies after the tombstone, before any unlink.
        store.save(JSONObject().put("id", deleted.getString("id")).put("capture_state", "discarded"))
        cannotOpen(uri)
        val reopened = SessionStore(store.directory, context)
        reopened.restore()
        assertEquals(0, reopened.restoreFailures)
        assertFalse(cache(uri).exists()); assertFalse(store.audio(deleted).exists())
        assertEquals(PackageManager.PERMISSION_DENIED, context.checkUriPermission(uri, -1, uid, Intent.FLAG_GRANT_READ_URI_PERMISSION))
        assertFalse(cache(expired).exists()); assertFalse(old.exists()); assertFalse(result.exists())
        assertTrue(cache(fresh).exists()); assertTrue(store.audio(kept).exists())
    }
    fun testPartialEraseReportsFailureAndKeepsTombstoneForRetry() {
        val item = saved(); val uri = snapshot(item)
        // A nonempty directory cannot be unlinked as a snapshot: deterministic I/O failure.
        val obstacle = File(cache(uri).parentFile, "blocked").apply { mkdir(); resolve("child").writeText("fixture") }
        try { store.deleteSaved(item.getString("id")); fail("Partial deletion reported success") } catch (_: IllegalStateException) { }
        assertEquals("discarded", store.load(item.getString("id")).getString("capture_state"))
        try { store.deleteAllSaved(); fail("Failed tombstone counted as complete") } catch (_: IllegalStateException) { }
        assertTrue(store.restoreFailures > 0)
        obstacle.deleteRecursively()
        SessionStore(store.directory, context).restore()
        assertFalse(store.audio(item).exists()); assertFalse(cache(uri).exists())
    }
    fun testBulkDeleteKeepsPendingAudioAndItsFreshShare() {
        val deleted = saved(); val uri = snapshot(deleted)
        val pending = saved(); val kept = snapshot(pending); val bytes = store.audio(pending).readBytes()
        store.prepare(pending, "https://example.invalid", "gemini", "fixture", "en")
        assertEquals(1, store.deleteAllSaved())
        cannotOpen(uri); assertFalse(cache(uri).exists())
        assertTrue(cache(kept).exists()); assertTrue(bytes.contentEquals(store.audio(pending).readBytes()))
    }
    fun testShareRacingDeletionCannotLeaveNewSnapshot() {
        val item = saved(); val start = CountDownLatch(1)
        var shared: Uri? = null; var unexpected: Throwable? = null
        val thread = Thread {
            start.await()
            try { shared = snapshot(item) } catch (_: IllegalStateException) { } catch (error: Throwable) { unexpected = error }
        }
        thread.start(); start.countDown(); store.deleteSaved(item.getString("id")); thread.join(5000)
        assertFalse(thread.isAlive); assertNull(unexpected)
        shared?.let { cannotOpen(it); assertFalse(cache(it).exists()) }
        assertFalse(File(context.cacheDir, "audio-exports/${item.getString("id")}").exists())
    }
    fun testLiveLeaseProtectsResultsAndAudioIncludingAgainstAnotherProcess() {
        val item = saved(); val bytes = store.audio(item).readBytes()
        val output = PhoneResultCache.file(context, UUID.randomUUID().toString()).apply { writeText("fixture") }
        val marker = File(context.cacheDir, "lease-${UUID.randomUUID()}")
        val testApk = context.packageManager.getApplicationInfo(context.packageName + ".test", 0).sourceDir
        val builder = ProcessBuilder("/system/bin/app_process", "/", "com.metrix.writer.CacheLeaseProcess",
            File(context.filesDir, "phone-results.lock").path, marker.path)
        builder.environment()["CLASSPATH"] = testApk + ":" + context.applicationInfo.sourceDir
        val child = builder.start()
        try {
            val deadline = System.nanoTime() + TimeUnit.SECONDS.toNanos(10)
            while (!marker.exists() && child.isAlive && System.nanoTime() < deadline) Thread.sleep(20)
            assertTrue("Separate process acquired its lease", marker.exists())
            assertFalse(PhoneResultCache.sweep(context)); assertTrue(output.exists())
            try { store.deleteSaved(item.getString("id")); fail("Active worker deleted") } catch (_: IllegalStateException) { }
            assertTrue(bytes.contentEquals(store.audio(item).readBytes()))
            // Same-process lease probes must not release the underlying kernel lock.
            val local = PhoneResultCache.lease(context, true)!!
            assertFalse(PhoneResultCache.sweep(context))
            local.close()
            assertFalse(PhoneResultCache.sweep(context))
        } finally { child.destroyForcibly(); child.waitFor(10, TimeUnit.SECONDS); marker.delete() }
        assertTrue(PhoneResultCache.sweep(context)); assertFalse(output.exists())
        assertTrue(bytes.contentEquals(store.audio(item).readBytes()))
    }
}

/** Test APK only: real second process, with a kernel lease released by SIGKILL. */
object CacheLeaseProcess {
    @JvmStatic fun main(args: Array<String>) {
        RandomAccessFile(args[0], "rw").use { file ->
            file.channel.lock(0, Long.MAX_VALUE, true).use {
                File(args[1]).writeText("ready")
                while (true) Thread.sleep(1000)
            }
        }
    }
}
