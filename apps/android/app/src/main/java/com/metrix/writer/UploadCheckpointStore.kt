package com.metrix.writer

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.AtomicFile
import org.json.JSONObject
import java.io.File
import java.security.KeyStore
import java.util.UUID
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/** Upload URLs are bearer capabilities: encrypted, backup-excluded, absent from history/export/logs. */
internal interface UploadRecovery {
    fun load(): JSONObject?
    fun save(value: JSONObject?)
    fun progress(confirmed: Long, total: Long, resumed: Boolean)
}
internal class UploadCheckpointStore(context: Context, sessionId: String, private val scope: String,
    private val report: (Long, Long, Boolean) -> Unit) : UploadRecovery {
    private val atomic = AtomicFile(File(context.noBackupFilesDir, "upload-${UUID.fromString(sessionId)}.checkpoint"))
    private fun key(): SecretKey = synchronized(UploadCheckpointStore::class.java) {
        val store = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        (store.getKey(ALIAS, null) as? SecretKey) ?: KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore").apply {
            init(KeyGenParameterSpec.Builder(ALIAS, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM).setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE).setKeySize(256).build())
        }.generateKey()
    }
    override fun load(): JSONObject? = runCatching {
        val bytes = atomic.readFully(); require(bytes.size > 28 && bytes.size < 32768)
        val cipher = Cipher.getInstance("AES/GCM/NoPadding").apply {
            init(Cipher.DECRYPT_MODE, key(), GCMParameterSpec(128, bytes.copyOfRange(0,12))); updateAAD(scope.toByteArray())
        }
        JSONObject(String(cipher.doFinal(bytes.copyOfRange(12,bytes.size)), Charsets.UTF_8))
    }.getOrNull()
    override fun save(value: JSONObject?) {
        if (value == null) { atomic.delete(); return }
        val cipher = Cipher.getInstance("AES/GCM/NoPadding").apply { init(Cipher.ENCRYPT_MODE,key()); updateAAD(scope.toByteArray()) }
        val bytes = cipher.iv + cipher.doFinal(value.toString().toByteArray(Charsets.UTF_8))
        val out = atomic.startWrite()
        try { out.write(bytes); out.fd.sync(); atomic.finishWrite(out); check(atomic.readFully().contentEquals(bytes)) { "Upload recovery could not be saved. Original audio remains." } }
        catch (error: Exception) { atomic.failWrite(out); throw error }
    }
    override fun progress(confirmed: Long, total: Long, resumed: Boolean) = report(confirmed,total,resumed)
    companion object {
        private const val ALIAS = "writer.upload.v1"
        fun delete(context: Context, id: String) { AtomicFile(File(context.noBackupFilesDir,"upload-${UUID.fromString(id)}.checkpoint")).delete() }
    }
}
