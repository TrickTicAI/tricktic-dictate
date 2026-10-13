@file:Suppress("DEPRECATION")
package com.metrix.writer

import android.content.ComponentName
import android.content.Intent
import android.content.ServiceConnection
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import android.os.IBinder
import android.system.ErrnoException
import android.system.Os
import android.system.OsConstants
import android.test.AndroidTestCase
import android.util.Log
import androidx.test.platform.app.InstrumentationRegistry
import org.json.JSONObject
import java.io.File
import java.io.FileOutputStream
import java.util.UUID
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

/**
 * Public-release gate checks (MW-SEC-23) for a fixture-signed release build on an owned emulator.
 * play-gate.py selects the methods and supplies arguments; nothing here contacts a provider.
 */
class ReleaseGateTest : AndroidTestCase() {
    private val arguments get() = InstrumentationRegistry.getArguments()
    private val marker get() = File(context.filesDir, "release-gate.json")
    private val recordings get() = File(context.filesDir, "recordings")

    override fun setUp() {
        super.setUp()
        assertTrue("Release gate fixtures run only on an owned emulator",
            Build.HARDWARE.contains("ranchu") || Build.HARDWARE.contains("goldfish"))
    }

    /** Synthetic frames of one byte value; no microphone, so the saved prefix is checkable. */
    private class Frames(private val value: Byte) : AudioCapture.Source {
        val first = CountDownLatch(1)
        override fun start() {}
        override fun read(bytes: ByteArray, length: Int): Int {
            Thread.sleep(10)
            val count = minOf(length, 320)
            bytes.fill(value, 0, count); first.countDown(); return count
        }
        override fun capacity() = 640
        override fun silenced() = false
        override fun route() = 1
        override fun close() {}
    }

    private fun versionCode() = context.packageManager.getPackageInfo(context.packageName, 0).longVersionCode

    /** Phase 1 of the update check: seed data, then keep a capture writing until the package is replaced. */
    fun testSeedAndRecordUntilPackageReplaced() {
        val canary = arguments.getString("canary") ?: error("canary argument required")
        assertFalse("Run on a fresh fixture install", marker.exists())
        val store = SessionStore(recordings, context)
        val done = store.create("mixed")
        store.audio(done).appendBytes(ByteArray(3200) { 17 }); store.finishCapture(done, null)
        done.getJSONArray("attempts").put(JSONObject().put("raw_text", "synthetic gate transcript עברית"))
        store.save(done)
        DirectCloud(context, "openai").saveKey(canary)
        val active = store.create("en", reserveCapture = true)
        val source = Frames(42)
        AudioCapture(store) { source }.start(active) { _, _ -> }
        assertTrue(source.first.await(5, TimeUnit.SECONDS))
        while (store.audio(active).length() < 44 + 6400) Thread.sleep(20)
        val durable = store.audio(active).length()
        marker.writeText(JSONObject().put("completed", done.getString("id")).put("active", active.getString("id"))
            .put("durable_bytes", durable).put("version_code", versionCode()).toString())
        Log.i("WriterGate", "recording-in-flight ${active.getString("id")} $durable")
        // play-gate.py installs the newer build now; replacing the package kills this process.
        Thread.sleep(180_000)
        fail("The package was not replaced while the capture was writing")
    }

    /** Phase 2: the production startup path recovers the capture; data and credential survive the update. */
    fun testRecoveredAfterSkippedVersionUpdate() {
        val canary = arguments.getString("canary") ?: error("canary argument required")
        val seeded = JSONObject(marker.readText())
        assertTrue("Expected an update that skips at least one version",
            versionCode() >= seeded.getLong("version_code") + 2)
        val store = SessionManager.get(context).store
        val done = store.load(seeded.getString("completed"))
        assertEquals("complete", done.getString("capture_state"))
        assertTrue(store.audio(done).readBytes().drop(44).all { it == 17.toByte() })
        assertEquals("synthetic gate transcript עברית", done.getJSONArray("attempts").getJSONObject(0).getString("raw_text"))
        val active = store.load(seeded.getString("active"))
        assertEquals("interrupted", active.getString("capture_state"))
        val audio = store.audio(active).readBytes()
        assertTrue(audio.size >= seeded.getLong("durable_bytes"))
        assertTrue(audio.drop(44).take(seeded.getInt("durable_bytes") - 44).all { it == 42.toByte() })
        val cloud = DirectCloud(context, "openai")
        assertEquals(canary, cloud.key(cloud.snapshot()))
        cloud.forget(); store.deleteSaved(done.getString("id")); store.deleteSaved(active.getString("id"))
        assertTrue(marker.delete())
    }

    /** Fill the data partition, record until writes fail, then free space: saved frames stay recoverable. */
    fun testStorageExhaustionRetainsSavedAudio() {
        val ballast = File(context.filesDir, "release-gate-ballast").apply { deleteRecursively(); mkdirs() }
        val store = SessionStore(File(context.filesDir, "release-gate-storage"), context)
        var index = 0
        fun allocate(size: Long): Boolean = FileOutputStream(File(ballast, "${index++}")).use {
            try { Os.posix_fallocate(it.fd, 0, size); true }
            catch (error: ErrnoException) { assertEquals(OsConstants.ENOSPC, error.errno); false }
        }
        try {
            val item = store.create("en", reserveCapture = true)
            val reserve = File(ballast, "reserve")
            FileOutputStream(reserve).use { Os.posix_fallocate(it.fd, 0, 1L shl 20) }
            // Largest chunks first, down to 64 KiB, until the kernel reports ENOSPC.
            var chunk = 256L shl 20
            while (chunk >= 64 shl 10) if (!allocate(chunk)) chunk /= 4
            assertTrue(reserve.delete()) // About 1 MiB left for the first frames.
            val source = Frames(42); val finished = CountDownLatch(1)
            var error: String? = null
            AudioCapture(store) { source }.start(item) { _, e -> error = e; finished.countDown() }
            assertTrue(source.first.await(5, TimeUnit.SECONDS))
            val deadline = System.currentTimeMillis() + 120_000
            while (store.audio(item).length() < 44 + 6400 && finished.count > 0) Thread.sleep(20)
            // Android frees cache space when storage runs low; keep consuming it until capture fails.
            while (!finished.await(50, TimeUnit.MILLISECONDS)) {
                assertTrue("Capture never hit the full partition", System.currentTimeMillis() < deadline)
                allocate(64L shl 10)
            }
            assertNotNull(error)
            val saved = store.audio(item).readBytes()
            ballast.deleteRecursively()
            val recovered = SessionStore(store.directory, context).restore().single()
            assertEquals("interrupted", recovered.getString("capture_state"))
            val after = store.audio(recovered).readBytes()
            assertTrue("Saved frames were lost", after.size >= saved.size && saved.size > 44 + 6400)
            assertTrue(after.drop(44).take(saved.size - 44).all { it == 42.toByte() })
            Log.i("WriterGate", "storage-exhaustion saved=${saved.size} error=$error")
        } finally { ballast.deleteRecursively(); store.directory.deleteRecursively() }
    }

    /** A saved provider key never appears in this app's logs or in any file under its private storage. */
    fun testProviderKeyAbsentFromLogsAndPlainStorage() {
        val canary = arguments.getString("canary") ?: error("canary argument required")
        val cloud = DirectCloud(context, "openai")
        assertFalse("Use an unconfigured fixture app", cloud.configured())
        try {
            cloud.saveKey(canary)
            assertEquals(canary, cloud.key(cloud.snapshot()))
            cloud.accessStatus()
            val needles = listOf(canary.toByteArray(), canary.toByteArray(Charsets.UTF_16LE),
                android.util.Base64.encode(canary.toByteArray(), android.util.Base64.NO_WRAP))
            val roots = listOf(context.dataDir, context.createDeviceProtectedStorageContext().dataDir)
            for (file in roots.flatMap { it.walkTopDown().filter(File::isFile).toList() }) {
                if (file.length() > 64L shl 20) continue // Model weights; the key is never written there.
                val bytes = file.readBytes()
                for (needle in needles) assertFalse("Key bytes in ${file.name}", contains(bytes, needle))
            }
            val log = ProcessBuilder("logcat", "-d").redirectErrorStream(true).start().inputStream.readBytes()
            assertFalse("Key in this app's log", contains(log, canary.toByteArray()))
        } finally { cloud.forget() }
    }

    /** The Play app, as an ordinary other UID, cannot reach the dev build's files, components or exports. */
    fun testOtherBuildDataIsInaccessible() {
        val other = arguments.getString("other") ?: error("other argument required")
        val pm = context.packageManager
        val info = pm.getApplicationInfo(other, 0)
        assertFalse(info.uid == context.applicationInfo.uid)
        assertEquals(PackageManager.SIGNATURE_NO_MATCH, pm.checkSignatures(context.packageName, other))
        for (path in listOf("", "files", "files/recordings", "files/deliveries", "no_backup", "shared_prefs", "cache", "databases")) {
            assertNull(path, File(info.dataDir, path).list())
        }
        for (path in listOf("files/recordings", "shared_prefs/direct-cloud.xml", "no_backup")) {
            try { File(info.dataDir, path).readBytes(); fail("Read $path of $other") } catch (_: java.io.IOException) { }
        }
        for (name in listOf("CaptureService", "PhoneInferenceService")) {
            val intent = Intent().setComponent(ComponentName(other, "com.metrix.writer.$name"))
            try { context.startService(intent); fail("Started $name of $other") } catch (_: SecurityException) { }
            val connection = object : ServiceConnection {
                override fun onServiceConnected(name: ComponentName, service: IBinder) {}
                override fun onServiceDisconnected(name: ComponentName) {}
            }
            try { context.bindService(intent, connection, android.content.Context.BIND_AUTO_CREATE); fail("Bound $name of $other") }
            catch (_: SecurityException) { }
        }
        for (name in listOf("CloudSetupActivity", "HistoryActivity")) {
            val intent = Intent().setComponent(ComponentName(other, "com.metrix.writer.$name")).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            try { context.startActivity(intent); fail("Opened $name of $other") } catch (_: SecurityException) { }
        }
        val export = Uri.parse("content://$other.audioexports/${UUID.randomUUID()}/${UUID.randomUUID()}.wav")
        try { context.contentResolver.openInputStream(export)?.close(); fail("Opened an export of $other") }
        catch (_: SecurityException) { }
    }

    private fun contains(haystack: ByteArray, needle: ByteArray): Boolean {
        if (needle.isEmpty() || haystack.size < needle.size) return false
        outer@ for (i in 0..haystack.size - needle.size) {
            for (j in needle.indices) if (haystack[i + j] != needle[j]) continue@outer
            return true
        }
        return false
    }
}
