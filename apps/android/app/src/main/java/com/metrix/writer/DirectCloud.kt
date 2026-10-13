package com.metrix.writer

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.AtomicFile
import com.metrix.writer.sync.PortableChanges
import org.json.JSONObject
import java.io.File
import java.security.KeyStore
import java.util.UUID
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/** User-owned cloud key on this phone. Requests retain an opaque credential revision, never a key. */
class DirectCloud(private val context: Context, val provider: String = selectedProvider(context)) {
    private val prefs = context.getSharedPreferences("direct-cloud", Context.MODE_PRIVATE)
    init { CloudProviders.get(provider) }
    private fun pref(name: String) = if (provider == "gemini") name else "$provider-$name"
    fun model(): String = prefs.getString(pref("model"), CloudProviders.get(provider).speech.firstOrNull()?.id ?: "")!!
    fun language(): String = prefs.getString("language", "he")!!
    fun configured() = prefs.contains(pref("credential"))
    private fun file(id: String) = AtomicFile(File(context.noBackupFilesDir, "$provider-${UUID.fromString(id)}.credential"))
    private fun alias() = if (provider == "gemini") ALIAS else "writer.direct.$provider.v1"
    private fun secret(): SecretKey = synchronized(DirectCloud::class.java) {
        val store = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        (store.getKey(alias(), null) as? SecretKey) ?: KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore").apply {
            init(KeyGenParameterSpec.Builder(alias(), KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM).setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE).setKeySize(256).build())
        }.generateKey()
    }
    fun saveKey(value: String) {
        val key = value.trim()
        require(key.length in 20..512 && key.all { it.isLetterOrDigit() && it.code < 128 || it in "._-" }) { "Enter a valid ${CloudProviders.get(provider).name} API key." }
        val id = UUID.randomUUID().toString()
        val cipher = Cipher.getInstance("AES/GCM/NoPadding").apply { init(Cipher.ENCRYPT_MODE, secret()); updateAAD((CloudProviders.get(this@DirectCloud.provider).origin + id).toByteArray()) }
        val encrypted = cipher.doFinal(key.toByteArray())
        val atomic = file(id); val out = atomic.startWrite()
        try { out.write(cipher.iv); out.write(encrypted); atomic.finishWrite(out) }
        catch (error: Exception) { atomic.failWrite(out); throw error }
        check(prefs.edit().putString(pref("credential"), id).remove(pref("check")).commit())
        // Prior revisions remain encrypted for pending requests. Forget removes every revision explicitly.
    }
    fun saveSelection(model: String, language: String) {
        require(MODEL.matches(model) && language in listOf("en", "he", "mixed"))
        check(prefs.edit().putString(pref("model"), model).putString("language", language).commit())
        PortableChanges.changed(context)
    }
    fun snapshot(): JSONObject {
        val id = prefs.getString(pref("credential"), null) ?: throw ConnectionFailure("selection", "Add your ${CloudProviders.get(provider).name} key in Settings › API keys. Saved audio is retained.")
        return JSONObject().put("provider", provider).put("credential_id", id).put("transport_version", 1)
    }
    internal fun key(snapshot: JSONObject): String {
        require(snapshot.getString("provider") == provider && snapshot.getInt("transport_version") == 1)
        try {
            val id = snapshot.getString("credential_id"); val bytes = file(id).readFully()
            require(bytes.size > 28)
            val cipher = Cipher.getInstance("AES/GCM/NoPadding").apply {
                init(Cipher.DECRYPT_MODE, secret(), GCMParameterSpec(128, bytes.copyOfRange(0, 12)))
                updateAAD((CloudProviders.get(this@DirectCloud.provider).origin + id).toByteArray())
            }
            return String(cipher.doFinal(bytes.copyOfRange(12, bytes.size)), Charsets.UTF_8)
        } catch (_: Exception) { throw CloudFailure("This request's ${CloudProviders.get(provider).name} key is unavailable. Add a key and start a new saved-audio retry; existing audio and text remain.") }
    }
    fun forget() {
        for (entry in context.noBackupFilesDir.listFiles().orEmpty()) if (entry.name.matches(Regex("$provider-[0-9a-f-]+\\.credential(?:\\.bak|\\.new)?"))) check(entry.delete())
        check(prefs.edit().remove(pref("credential")).remove(pref("check")).commit())
    }
    fun captureChoice(): JSONObject {
        val options = CompanionPreferences(context).options(provider, model(), DictationChoice(context).language())
        return JSONObject().put("server", CloudProviders.get(provider).origin).put("cloud", snapshot().apply {
                val cleanup = options.getJSONObject("cleanup").optJSONObject("selection")
                if (cleanup != null) {
                    val selected = cleanup.getString("provider")
                    put("cleanup_cloud", runCatching { DirectCloud(context, selected).snapshot() }.getOrElse { JSONObject().put("provider", selected).put("unavailable", true) })
                }
                put("polish_policy", CompanionPreferences(context).polishPolicy(cleanup, optJSONObject("cleanup_cloud")))
            }).put("selection", options.getJSONObject("selection"))
            .put("input_language", DictationChoice(context).language()).put("options", options).put("delivery", CompanionPreferences(context).delivery())
    }
    fun prepare(store: SessionStore, item: JSONObject) {
        if (item.has("pending")) return
        prepareChoice(store, item, captureChoice())
    }
    fun replacePending(store: SessionStore, id: String) {
        val choice = captureChoice() // Validate the new selection before changing the retained request.
        val item = store.load(id)
        check(!item.has("pending_postprocess"))
        val pending = item.optJSONObject("pending") ?: return
        item.getJSONArray("attempts").put(JSONObject().put("id", pending.getString("idempotency_key"))
            .put("request", JSONObject(pending.toString())).put("execution_location", if (item.has("pending_engine")) "phone" else if (item.has("pending_cloud")) "cloud" else "server")
            .apply { item.optJSONObject("pending_engine")?.let { put("engine_snapshot", JSONObject(it.toString())) } }
            .put("status", JSONObject().put("state", "failed").put("error", JSONObject()
                .put("message", "User started a new attempt. Previous remote outcome may be unknown; it was not cancelled at the provider.").put("retryable", true))))
        for (field in listOf("pending", "pending_server", "pending_cloud", "pending_engine", "pending_delivery", "pending_timing", "cloud_upload", "pending_cloud_error")) item.remove(field)
        // Atomic replacement: old attempt is retained and the new snapshot is committed together.
        prepareChoice(store, item, choice)
    }
    enum class KeyState { Absent, Unchecked, Checked, Failed }
    fun keyState(): KeyState = when {
        !configured() -> KeyState.Absent
        else -> prefs.getString(pref("check"), null).let { when {
            it == null -> KeyState.Unchecked
            it.startsWith("Access checked") -> KeyState.Checked
            else -> KeyState.Failed
        } }
    }
    fun accessStatus(): String = if (!configured()) "No key saved" else prefs.getString(pref("check"), "Key saved · not checked")!!
    fun checkAccess(model: String) = checked(model) { snapshot -> CloudProviders.client(context, snapshot).checkModel(model) }

    /** Settings' Try polishing: one real polishing request on [text], which also counts as a key check. */
    fun tryPolishing(model: String, text: String, reasoning: String): String =
        checked(model) { snapshot -> CloudProviders.client(context, snapshot).cleanup(model, text, reasoning, CompanionPreferences(context).polishSystemPrompt()) }

    /** Runs [request] with the current key and records the result, unless the key was replaced meanwhile. */
    private fun <T> checked(model: String, request: (JSONObject) -> T): T {
        val snapshot = snapshot()
        fun record(value: String) { if (prefs.getString(pref("credential"), null) == snapshot.getString("credential_id")) prefs.edit().putString(pref("check"), value).commit() }
        try {
            return request(snapshot).also {
                record("Access checked · $model · ${java.text.DateFormat.getDateTimeInstance(java.text.DateFormat.SHORT, java.text.DateFormat.SHORT).format(java.util.Date())}")
            }
        } catch (error: Exception) {
            record("Last access check failed · $model")
            throw error
        }
    }
    fun select() {
        check(configured()) { "Save a key first." }
        check(CloudProviders.get(provider).transcribes) { "${CloudProviders.get(provider).name} does not transcribe speech. Choose a dictation engine that does." }
        check(prefs.edit().putString("provider", provider).commit())
        PortableChanges.changed(context)
    }
    companion object {
        fun selectedProvider(context: Context): String = context.getSharedPreferences("direct-cloud", Context.MODE_PRIVATE).getString("provider", "gemini")!!
        const val ORIGIN = "https://generativelanguage.googleapis.com"
        const val DEFAULT_MODEL = "gemini-3.8-flash"
        private const val ALIAS = "writer.direct.gemini.v1"
        val MODEL = Regex("[A-Za-z0-9][A-Za-z0-9._-]{0,127}")
        fun prepareChoice(store: SessionStore, item: JSONObject, choice: JSONObject) {
            if (item.has("pending")) return
            // One sidecar commit: never leave a cloud request looking like a legacy server request.
            store.prepareDirect(item, choice.getJSONObject("options"), choice.getJSONObject("cloud"), choice.getString("delivery"))
        }
    }
}
