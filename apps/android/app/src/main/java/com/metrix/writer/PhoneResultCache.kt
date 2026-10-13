package com.metrix.writer

import android.content.Context
import java.io.Closeable
import java.io.File
import java.io.RandomAccessFile
import java.nio.channels.OverlappingFileLockException
import java.util.UUID

/** Kernel leases survive neither process death nor reboot. Never unlink the lock inode. */
object PhoneResultCache {
    fun file(context: Context, attempt: String) = File(context.cacheDir, "phone-${UUID.fromString(attempt)}.txt")

    private val held = mutableSetOf<String>()

    @Synchronized fun lease(context: Context, shared: Boolean): Closeable? {
        val key = context.filesDir.canonicalPath
        // Never open/close a second descriptor while this process owns a POSIX lock:
        // closing it can release the first descriptor's kernel lock on Android/Linux.
        if (key in held) return null
        val handle = RandomAccessFile(File(context.filesDir, "phone-results.lock"), "rw")
        try {
            val lock = try { handle.channel.tryLock(0, Long.MAX_VALUE, shared) }
                catch (_: OverlappingFileLockException) { null }
            if (lock == null) { handle.close(); return null }
            held.add(key)
            return Closeable { synchronized(this) {
                try { lock.release() } finally { handle.close(); held.remove(key) }
            } }
        } catch (error: Exception) { handle.close(); throw error }
    }

    fun <T> deleting(context: Context, block: () -> T): T {
        val lease = checkNotNull(lease(context, false)) { "Phone inference is still active. Retry deletion when it finishes." }
        return lease.use { sweepUnlocked(context); block() }
    }

    /** An idle pending request is retryable from audio; only live workers own temporary results. */
    fun sweep(context: Context): Boolean {
        val lease = lease(context, false) ?: return false
        lease.use { sweepUnlocked(context) }
        return true
    }

    private fun sweepUnlocked(context: Context) {
        val files = checkNotNull(context.cacheDir.listFiles()) { "Cannot inspect temporary results; retry deletion." }
        files.filter { it.name.matches(Regex("phone-[0-9a-f-]{36}\\.txt")) }.forEach {
            check(!it.exists() || it.delete()) { "Temporary result deletion needs retry." }
        }
    }
}
