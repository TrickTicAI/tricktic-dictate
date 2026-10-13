package com.metrix.writer

import android.app.Service
import android.content.Intent
import android.os.*
import java.io.File
import java.util.UUID
import java.util.concurrent.Executors

class PhoneNative {
    external fun create(): Long
    external fun cancel(handle: Long)
    external fun destroy(handle: Long)
    external fun unload()
    external fun transcribe(handle: Long, model: String, audio: String, language: String): ByteArray
    companion object { init { System.loadLibrary("writer-phone") } }
}

/** A native crash/OOM cannot take the recording service or editor connection with it. */
class PhoneInferenceService : Service() {
    private val executor = Executors.newSingleThreadExecutor()
    private var handle = 0L
    private var generation = 0L
    @Volatile private var preparationCancelled = false
    private val guard = Any()
    private lateinit var native: PhoneNative
    override fun onCreate() { super.onCreate(); native = PhoneNative() }
    private fun cancelCurrent() {
        synchronized(guard) {
            if (handle == 0L) return
            preparationCancelled = true
            native.cancel(handle)
            val cancelledGeneration = generation
            // Loading has no upstream abort callback. Bound explicit cancellation only, never dictation duration.
            Handler(Looper.getMainLooper()).postDelayed({ synchronized(guard) {
                if (handle != 0L && generation == cancelledGeneration) Process.killProcess(Process.myPid())
            } }, 5000)
        }
    }
    private val receiver = Messenger(Handler(Looper.getMainLooper()) { message ->
        if (message.what == 2) { cancelCurrent(); true }
        else if (message.what == 1) {
            val reply = message.replyTo; val args = message.data
            synchronized(guard) {
                if (handle != 0L) { reply.send(Message.obtain(null, 3).apply { data = Bundle().apply { putString("error", "Phone worker already active") } }); return@Handler true }
                preparationCancelled = false
                try { handle = native.create(); generation++ }
                catch (error: Exception) {
                    reply.send(Message.obtain(null, 3).apply { data = Bundle().apply { putString("error", error.message ?: "Phone worker allocation failed; saved audio retained") } })
                    return@Handler true
                }
            }
            executor.execute {
                val response = Bundle()
                try {
                    val lease = checkNotNull(PhoneResultCache.lease(this, true)) { "Temporary results are being cleaned. Retry saved audio." }
                    lease.use {
                        val token = UUID.fromString(args.getString("attempt")!!).toString()
                        val models = PhoneModels(this)
                        val model = models.file(args.getString("model")!!)
                        val id = UUID.fromString(args.getString("session")!!).toString()
                        val store = SessionStore(File(filesDir, "recordings"))
                        val item = store.load(id)
                        check(item.optJSONObject("pending")?.optString("idempotency_key") == token &&
                            item.optString("capture_state") in setOf("complete", "interrupted")) { "Phone request expired; saved audio retained" }
                        val audio = store.audio(item)
                        SilencePreparation.prepare(this, audio) { preparationCancelled }.use { prepared ->
                            response.putString("audio_preparation", prepared.metrics.toString())
                            val timing = org.json.JSONObject().put("asr_started_elapsed_ms", SystemClock.elapsedRealtime()).put("warm", "cold")
                            // This revision destroys its native handle after every request.
                            // Loading and decoding are one JNI call; cold includes both.
                            val output = native.transcribe(handle, model.path, prepared.file.path, args.getString("language")!!)
                            timing.put("asr_finished_elapsed_ms", SystemClock.elapsedRealtime())
                            response.putString("inference_timing", timing.toString())
                            // File avoids Binder's transaction-size limit; caller commits to session metadata.
                            val result = PhoneResultCache.file(this, token)
                            result.outputStream().use { it.write(output); it.fd.sync() }
                            response.putString("file", result.name)
                        }
                    }
                } catch (_: OutOfMemoryError) { response.putString("error", "Phone model exceeded available memory; choose a smaller model") }
                catch (error: Exception) { response.putString("error", error.message ?: "Phone inference failed") }
                finally {
                    if (response.containsKey("error")) native.unload()
                    synchronized(guard) { native.destroy(handle); handle = 0 }
                }
                try { reply.send(Message.obtain(null, 3).apply { data = response }) } catch (_: RemoteException) { }
            }
            true
        } else false
    })
    override fun onBind(intent: Intent): IBinder = receiver.binder
    override fun onUnbind(intent: Intent): Boolean { cancelCurrent(); return false }
    @Suppress("DEPRECATION")
    override fun onTrimMemory(level: Int) {
        super.onTrimMemory(level)
        if (level >= TRIM_MEMORY_BACKGROUND || level in TRIM_MEMORY_RUNNING_LOW..TRIM_MEMORY_RUNNING_CRITICAL) {
            executor.execute { native.unload() }
        }
    }
    @Suppress("DEPRECATION")
    override fun onLowMemory() { super.onLowMemory(); executor.execute { native.unload() } }
    override fun onDestroy() {
        cancelCurrent()
        executor.execute { native.unload() }
        executor.shutdown(); super.onDestroy()
    }
}
