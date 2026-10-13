package com.metrix.writer

import android.Manifest
import android.app.*
import android.content.*
import android.content.pm.PackageManager
import android.content.pm.ServiceInfo
import android.os.*
import android.provider.Settings
import org.json.JSONObject

/** User-started, non-sticky microphone service. The companion is only a view/controller. */
class CaptureService : Service() {
    private val handler = Handler(Looper.getMainLooper())
    private lateinit var manager: SessionManager
    private var item: JSONObject? = null
    @Volatile private var discard = false
    private var destroyed = false
    private var latestStartId = 0
    private var stoppedAt = 0L
    override fun onCreate() {
        super.onCreate(); manager = SessionManager.get(this); instance = this
        getSystemService(NotificationManager::class.java).createNotificationChannel(
            NotificationChannel(CHANNEL, "Recording controls", NotificationManager.IMPORTANCE_LOW))
    }
    override fun onBind(intent: Intent?): IBinder? = null
    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        latestStartId = startId
        when (intent?.action) {
            START -> {
                if (item != null || manager.capture.isActive) return START_NOT_STICKY
                stoppedAt = 0L; discard = false; appVisibleAt = 0L; bubbleVisibleAt = 0L
                val serviceAt = SystemClock.elapsedRealtime()
                try {
                    check(checkSelfPermission(Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED)
                    check(getSystemService(PowerManager::class.java).isInteractive && !getSystemService(KeyguardManager::class.java).isKeyguardLocked)
                    startForeground(NOTIFICATION, notification("Starting microphone…"), ServiceInfo.FOREGROUND_SERVICE_TYPE_MICROPHONE)
                    val choice = DictationChoice(this)
                    val session = manager.store.create(choice.language(), { session ->
                        session.put("startup_timing", JSONObject().put("tap_elapsed_ms", startedAtElapsed).put("service_elapsed_ms", serviceAt))
                        session.put("capture_origin", intent.getStringExtra("origin") ?: "companion")
                        // Bind only in the real service start, on the editor service's main thread.
                        if (session.optString("capture_origin") in listOf("bubble", "app")) {
                            try { session.put("capture_choice", choice.capture()) }
                            catch (error: Exception) { session.put("last_status", MobileConnection.explain(error)) }
                            // Insert Anywhere: both origins may insert into the editor focused when the text is ready.
                            WriterInputService.instance?.bind(session.getString("id"))?.let { session.put("original_target", it) }
                        }
                        session.getJSONObject("startup_timing").put("prepared_elapsed_ms", SystemClock.elapsedRealtime())
                    }, reserveCapture = true)
                    item = session
                    state = "starting"; message = "Starting microphone…"; activeSessionId = session.getString("id")
                    getSystemService(NotificationManager::class.java).notify(NOTIFICATION, notification(message))
                    manager.capture.start(session, { saved, error ->
                        // Persist discard before publishing any completion hint. Recovery honors the tombstone.
                        var failure = error
                        if (discard) try { manager.store.discard(saved) }
                        catch (_: Exception) { failure = "Discard could not finish. Reopen $PRODUCT_NAME to retry removal." }
                        handler.post { completed(saved, failure) }
                    }, {
                        handler.post {
                            if (item?.optString("id") == session.getString("id") && state == "starting") {
                                state = "recording"; message = "Recording · tap Stop to save"
                                getSystemService(NotificationManager::class.java).notify(NOTIFICATION, notification(message))
                                WriterInputService.instance?.refreshControl()
                            }
                        }
                    })
                    WriterInputService.instance?.refreshControl()
                    handler.post(monitor)
                } catch (_: Exception) {
                    state = "error"; message = "Android could not start the microphone. Open $PRODUCT_NAME, check microphone access and start there."
                    item?.let { try { manager.store.finishCapture(it, message) } catch (_: Exception) {} }
                    if (!manager.capture.isActive) manager.store.activeCaptureId = null
                    pendingStart = false; activeSessionId = null; item = null
                    android.widget.Toast.makeText(this, message, android.widget.Toast.LENGTH_LONG).show()
                    stopForeground(STOP_FOREGROUND_REMOVE); stopSelfResult(latestStartId)
                }
            }
            STOP, DISCARD -> {
                // Old notification actions cannot stop a later session.
                if (intent.getStringExtra("session_id") == activeSessionId && item != null) {
                    if (intent.action == DISCARD && state != "saving") {
                        // Write discard intent first, before draining. Crash recovery must not resurrect it.
                        try {
                            val saved = manager.store.load(activeSessionId!!)
                            saved.put("capture_state", "discarded"); manager.store.save(saved); discard = true
                        } catch (_: Exception) { message = "Could not save discard intent. Stop to retain audio, then retry."; return START_NOT_STICKY }
                    }
                    if (stoppedAt == 0L) stoppedAt = SystemClock.elapsedRealtime()
                    state = "saving"; message = if (discard) "Discarding recording…" else "Saving recording…"
                    manager.capture.stop()
                    getSystemService(NotificationManager::class.java).notify(NOTIFICATION, notification(message))
                } else if (item == null) stopSelfResult(latestStartId)
            }
            else -> if (item == null) stopSelfResult(latestStartId)
        }
        return START_NOT_STICKY
    }
    private val monitor = object : Runnable {
        override fun run() {
            if (destroyed || item == null) return
            if (checkSelfPermission(Manifest.permission.RECORD_AUDIO) != PackageManager.PERMISSION_GRANTED)
                interrupt("Microphone permission revoked. Saved audio retained.")
            // Screen-off and lock keep recording until Stop (MW-103).
            else if (item?.optString("capture_origin") == "bubble" &&
                (!Settings.canDrawOverlays(this@CaptureService) || WriterInputService.instance == null))
                interrupt("Floating control permission or editor service lost. Saved audio retained.")
            if (state == "starting" && manager.capture.isRecording) {
                state = "recording"; message = "Recording · tap Stop to save"
                getSystemService(NotificationManager::class.java).notify(NOTIFICATION, notification(message))
            }
            handler.postDelayed(this, 200)
        }
    }
    private fun interrupt(reason: String) {
        if (item == null || state == "saving") return
        // The interruption is when capture ended, so its stop-to-result time is still measured.
        if (stoppedAt == 0L) stoppedAt = SystemClock.elapsedRealtime()
        message = reason; state = "saving"; manager.capture.interrupt(reason)
    }
    private fun completed(saved: JSONObject, error: String?) {
        // AudioCapture invokes completion just before releasing its active ownership flag.
        if (manager.capture.isActive) { handler.postDelayed({ completed(saved, error) }, 10); return }
        handler.removeCallbacks(monitor)
        try {
            val current = manager.store.load(saved.getString("id"))
            current.optJSONObject("startup_timing")?.let { timing ->
                if (appVisibleAt > 0) timing.put("app_visible_elapsed_ms", appVisibleAt)
                if (bubbleVisibleAt > 0) timing.put("bubble_visible_elapsed_ms", bubbleVisibleAt)
                manager.store.save(current)
            }
        } catch (_: Exception) { /* Saved audio does not depend on diagnostic persistence. */ }
        // AudioCapture has released AudioRecord and atomically persisted the terminal sidecar.
        pendingStart = false; activeSessionId = null; item = null
        state = if (error == null) "idle" else "error"
        message = error ?: if (discard) "Recording discarded" else "Audio saved · open $PRODUCT_NAME to play or transcribe"
        // This package-scoped hint is replayable from capture_completion in SessionStore.restore().
        val durable = try { manager.store.load(saved.getString("id")).has("capture_completion") } catch (_: Exception) { false }
        val heard = try { manager.store.load(saved.getString("id")).optLong("duration_ms") > 0 } catch (_: Exception) { false }
        // An interrupted capture with saved audio is transcribed too, but only a live Stop may insert.
        if (!discard && durable && (error == null || heard) && saved.optString("capture_origin") in listOf("bubble", "app")) {
            try {
                manager.store.load(saved.getString("id")).also {
                    it.put("capture_timing", JSONObject().put("boot_count", Settings.Global.getInt(contentResolver, Settings.Global.BOOT_COUNT, -1))
                        .put("capture_stop_elapsed_ms", stoppedAt))
                    manager.store.save(it)
                }
            } catch (_: Exception) { /* Audio remains retained even if timing persistence fails. */ }
            manager.completeBubble(saved.getString("id"), stoppedAt, live = error == null)
            message = if (error == null) manager.message
                else if (manager.state == "processing") "$error Transcribing it now; the text will be in History." else "$error ${manager.message}"
        }
        if (!discard && durable) sendBroadcast(Intent(SAVED).setPackage(packageName)
            .putExtra("session_id", saved.getString("id")))
        if (!destroyed) {
            if (error != null) android.widget.Toast.makeText(this, message, android.widget.Toast.LENGTH_LONG).show()
            stopForeground(STOP_FOREGROUND_REMOVE); stopSelfResult(latestStartId)
        }
    }
    private fun notification(text: String): Notification {
        val open = PendingIntent.getActivity(this, 0, Intent(this, MainActivity::class.java), PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
        fun action(name: String) = PendingIntent.getService(this, name.hashCode(), Intent(this, CaptureService::class.java)
            .setAction(name).setData(android.net.Uri.parse("writer-capture://control/${activeSessionId}/$name")).putExtra("session_id", activeSessionId), PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
        return Notification.Builder(this, CHANNEL).setSmallIcon(R.drawable.ic_writer_wave)
            .setContentTitle("$PRODUCT_NAME microphone").setContentText(text).setContentIntent(open)
            .setOngoing(true).setOnlyAlertOnce(true).setForegroundServiceBehavior(Notification.FOREGROUND_SERVICE_IMMEDIATE)
            .addAction(Notification.Action.Builder(null, "Stop and save", action(STOP)).build())
            .addAction(Notification.Action.Builder(null, "Discard", action(DISCARD)).build()).build()
    }
    override fun onDestroy() {
        destroyed = true; handler.removeCallbacks(monitor)
        if (item != null) {
            if (stoppedAt == 0L) stoppedAt = SystemClock.elapsedRealtime()
            manager.capture.interrupt("Recording service interrupted. Saved audio retained.")
        }
        if (instance === this) instance = null
        super.onDestroy()
    }
    companion object {
        @Volatile private var appVisibleAt = 0L
        @Volatile private var bubbleVisibleAt = 0L
        fun visibleRecording(sessionId: String?, bubble: Boolean) {
            if (sessionId == null || sessionId != activeSessionId || state != "recording") return
            if (bubble && bubbleVisibleAt == 0L) bubbleVisibleAt = SystemClock.elapsedRealtime()
            if (!bubble && appVisibleAt == 0L) appVisibleAt = SystemClock.elapsedRealtime()
        }
        const val SAVED = "com.metrix.writer.CAPTURE_SAVED"
        private const val START = "com.metrix.writer.CAPTURE_START"
        private const val STOP = "com.metrix.writer.CAPTURE_STOP"
        private const val DISCARD = "com.metrix.writer.CAPTURE_DISCARD"
        private const val CHANNEL = "writer-capture"
        private const val NOTIFICATION = 58
        @Volatile private var instance: CaptureService? = null
        @Volatile private var pendingStart = false
        @Volatile var startedAtElapsed = 0L
            private set
        @Volatile var activeSessionId: String? = null
            private set
        @Volatile var state = "idle"
            private set
        @Volatile var message = "Ready"
            private set
        @JvmStatic fun isActive() = pendingStart || activeSessionId != null
        @JvmStatic fun start(context: Context, origin: String): Boolean {
            check(Looper.myLooper() == Looper.getMainLooper())
            if (isActive() || SessionManager.get(context).capture.isActive) return false
            if (context.checkSelfPermission(Manifest.permission.RECORD_AUDIO) != PackageManager.PERMISSION_GRANTED) {
                state = "error"; message = "Allow microphone access in $PRODUCT_NAME → Settings → Permissions, then start again."; return false
            }
            startedAtElapsed = SystemClock.elapsedRealtime()
            pendingStart = true; state = "starting"
            return try { context.startForegroundService(Intent(context, CaptureService::class.java).setAction(START).putExtra("origin", origin)); true }
            catch (_: RuntimeException) { pendingStart = false; state = "error"; message = "Android blocked microphone start. Open $PRODUCT_NAME and start recording there."; false }
        }
        @JvmStatic fun stop(context: Context, discard: Boolean) {
            val id = activeSessionId ?: return
            context.startService(Intent(context, CaptureService::class.java).setAction(if (discard) DISCARD else STOP).putExtra("session_id", id))
        }
    }
}
