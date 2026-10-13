package com.metrix.writer

import android.content.ContentProvider
import android.content.ContentValues
import android.content.Context
import android.content.Intent
import android.database.MatrixCursor
import android.net.Uri
import android.os.ParcelFileDescriptor
import android.provider.OpenableColumns
import java.io.File
import java.io.FileNotFoundException
import java.util.UUID

/** Read-only, explicitly granted snapshots. No recording/credential directory is exposed. */
class AudioExportProvider : ContentProvider() {
    override fun onCreate(): Boolean {
        // A recipient can start the provider before the History UI after process death.
        val app = context!!
        runCatching { SessionStore(File(app.filesDir, "recordings"), app).restore() }
        return true
    }
    private fun file(uri: Uri): File {
        if (uri.authority != context!!.packageName + ".audioexports" || uri.pathSegments.size != 2 || !ID.matches(uri.pathSegments[0]) || !NAME.matches(uri.lastPathSegment.orEmpty())) throw FileNotFoundException()
        val metadata = File(context!!.filesDir, "recordings/${uri.pathSegments[0]}.json")
        if (metadata.exists() || File("${metadata.path}.bak").exists()) {
            val item = SessionStore(metadata.parentFile!!).load(uri.pathSegments[0])
            if (item.optString("capture_state") == "discarded") throw FileNotFoundException("Dictation deleted")
        }
        return File(context!!.cacheDir, "audio-exports/${uri.pathSegments[0]}/${uri.lastPathSegment}").also {
            if (!it.isFile || System.currentTimeMillis() - it.lastModified() > LIFETIME) throw FileNotFoundException("Export expired; share again from History")
        }
    }
    override fun getType(uri: Uri): String = synchronized(guard) { file(uri); "audio/wav" }
    override fun openFile(uri: Uri, mode: String): ParcelFileDescriptor = synchronized(guard) {
        if (mode != "r") throw FileNotFoundException("Read only")
        ParcelFileDescriptor.open(file(uri), ParcelFileDescriptor.MODE_READ_ONLY)
    }
    override fun query(uri: Uri, projection: Array<out String>?, selection: String?, selectionArgs: Array<out String>?, sortOrder: String?): MatrixCursor = synchronized(guard) {
        val file = file(uri); val columns = projection ?: arrayOf(OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE)
        MatrixCursor(columns).apply { addRow(columns.map { when(it) { OpenableColumns.DISPLAY_NAME -> "$PRODUCT_NAME recording.wav"; OpenableColumns.SIZE -> file.length(); else -> null } }) }
    }
    override fun insert(uri: Uri, values: ContentValues?): Uri? = throw UnsupportedOperationException()
    override fun update(uri: Uri, values: ContentValues?, selection: String?, selectionArgs: Array<out String>?) = throw UnsupportedOperationException()
    override fun delete(uri: Uri, selection: String?, selectionArgs: Array<out String>?) = throw UnsupportedOperationException()
    companion object {
        internal val guard = Any()
        private val ID = Regex("[0-9a-f-]{36}")
        private val NAME = Regex("[0-9a-f-]{36}\\.wav")
        private const val LIFETIME = 24 * 60 * 60 * 1000L
        private fun uri(context: Context, path: String) = Uri.parse("content://${context.packageName}.audioexports/$path")
        private fun remove(context: Context, file: File, path: String) {
            // Revoke even if Android has already evicted the cached file.
            context.revokeUriPermission(uri(context, path), Intent.FLAG_GRANT_READ_URI_PERMISSION)
            check(!file.exists() || file.delete()) { "Export deletion needs retry. Reopen History." }
        }
        internal fun erase(context: Context, id: String) = synchronized(guard) {
            val directory = File(context.cacheDir, "audio-exports/${UUID.fromString(id)}")
            if (directory.exists()) {
                checkNotNull(directory.listFiles()) { "Cannot inspect exports; retry deletion." }.forEach {
                    remove(context, it, "$id/${it.name}")
                }
                check(directory.delete()) { "Export deletion needs retry. Reopen History." }
            }
            // This also covers grants whose cache files Android evicted earlier.
            context.revokeUriPermission(uri(context, id), Intent.FLAG_GRANT_READ_URI_PERMISSION)
        }
        internal fun sweep(context: Context) = synchronized(guard) {
            val root = File(context.cacheDir, "audio-exports")
            if (!root.exists()) return@synchronized
            checkNotNull(root.listFiles()) { "Cannot inspect exports; retry deletion." }.forEach { entry ->
                if (entry.isDirectory && ID.matches(entry.name)) {
                    checkNotNull(entry.listFiles()) { "Cannot inspect exports; retry deletion." }.forEach { file ->
                        if (!NAME.matches(file.name) || System.currentTimeMillis() - file.lastModified() > LIFETIME)
                            remove(context, file, "${entry.name}/${file.name}")
                    }
                } else {
                    // Old flat snapshots have no source mapping: invalidate them on upgrade/startup.
                    remove(context, entry, entry.name)
                }
            }
        }
        fun snapshot(context: Context, store: SessionStore, id: String): Uri = synchronized(store) {
            synchronized(guard) {
                val item = store.load(id)
                check(item.optString("capture_state") in setOf("complete", "interrupted")) { "Save a dictation before sharing it." }
                sweep(context)
                val key = UUID.fromString(id).toString()
                val directory = File(context.cacheDir, "audio-exports/$key").apply { check(isDirectory || mkdirs()) }
                val target = File(directory, "${UUID.randomUUID()}.wav")
                try { store.audio(item).inputStream().use { input -> target.outputStream().use { output -> input.copyTo(output); output.fd.sync() } } }
                catch (error: Exception) { target.delete(); throw error }
                uri(context, "$key/${target.name}")
            }
        }
    }
}
