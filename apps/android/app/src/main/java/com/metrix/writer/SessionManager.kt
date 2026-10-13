package com.metrix.writer

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.ComponentName
import android.content.Intent
import android.app.job.JobInfo
import android.app.job.JobScheduler
import android.net.ConnectivityManager
import android.net.Network
import android.net.NetworkCapabilities
import android.os.Handler
import android.os.Looper
import android.os.PersistableBundle
import android.os.SystemClock
import android.provider.Settings
import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.Executors

/** Engines use saved files; delivery is independent of model execution and Activity lifetime. */
fun interface TranscriptionEngine { fun execute(session: JSONObject, server: String): String }
interface TextDelivery {
    fun deliver(sessionId: String, text: String, originalTarget: String): DeliveryResult
}
enum class DeliveryResult { DELIVERED, PENDING_ORIGINAL_TARGET, UNSUPPORTED }

/** Process scope. Process death leaves the exact request available for explicit retry. */
class SessionManager private constructor(private val context: Context) {
    val store = SessionStore(File(context.filesDir, "recordings"), context)
    init { store.recover() }
    val capture = AudioCapture(store)
    @Synchronized internal fun acquireHistorical() {
        check(!isBusy() && !capture.isActive && !CaptureService.isActive()) { "Finish recording or processing before starting a historical batch." }
        busy = true; state = "processing"; message = "Historical batch · no automatic insertion"
    }
    @Synchronized internal fun releaseHistorical() { busy = false; state = "idle"; message = "Historical results saved · open History" }
    private val executor = Executors.newSingleThreadExecutor()
    private val main = Handler(Looper.getMainLooper())
    private val automatic = ConcurrentHashMap<String, String>()
    /** Interrupted or recovered recordings transcribed for History, announced by notification. */
    private val recovered = ConcurrentHashMap.newKeySet<String>()
    private val network = context.getSystemService(ConnectivityManager::class.java)
    init {
        network.registerDefaultNetworkCallback(object : ConnectivityManager.NetworkCallback() {
            override fun onLost(network: Network) { expireNetworkDelivery() }
            override fun onCapabilitiesChanged(network: Network, capabilities: NetworkCapabilities) {
                if (!capabilities.hasCapability(NetworkCapabilities.NET_CAPABILITY_VALIDATED)) expireNetworkDelivery()
            }
        })
    }
    @Volatile var state = "idle"
        private set
    @Volatile var message = "Ready"
        private set
    @Volatile private var displayedSession: String? = null
    @Volatile private var busy = false
    private val scheduler = context.getSystemService(JobScheduler::class.java)
    @Volatile private var jobStopped = false
    @Volatile private var activeDirect: DirectCloudEngine? = null
    @Volatile private var activeDirectProcessing: DirectCloudProcessing? = null
    private fun directProcessing(): DirectCloudProcessing = DirectCloudProcessing(context, store).also {
        activeDirectProcessing = it; if (jobStopped) it.cancel()
    }
    @Volatile private var activePhone: PhoneEngine? = null
    @Volatile private var activeEngine: ServerEngine? = null
    @Volatile private var activeProcessing: CompanionProcessing? = null
    private fun processing(): CompanionProcessing = CompanionProcessing(store, MobileConnection(context)).also {
        activeProcessing = it; if (jobStopped) it.cancel()
    }
    private fun expireNetworkDelivery() {
        for (id in automatic.keys) try { if (store.load(id).optJSONObject("capture_choice")?.has("engine") != true) automatic.remove(id) } catch (_: Exception) { automatic.remove(id) }
    }
    fun isBusy(): Boolean {
        if (busy) return true
        val queued = scheduler.getPendingJob(JOB_ID) ?: return false
        // An offline cloud queue must not prevent an installed phone model from running.
        return !(DictationChoice(context).isPhone() && queued.networkType != JobInfo.NETWORK_TYPE_NONE)
    }
    private fun boot() = Settings.Global.getInt(context.contentResolver, Settings.Global.BOOT_COUNT, -1)

    /** Only a live normal bubble Stop may grant automatic delivery. Interrupted captures are transcribed for History only. */
    fun completeBubble(id: String, stoppedAt: Long, live: Boolean = true) {
        displayedSession = id
        try {
            val item = store.load(id)
            check((item.optString("capture_state") == "complete" || !live && item.optString("capture_state") == "interrupted") && item.has("capture_completion"))
            val choice = item.optJSONObject("capture_choice")
                ?: throw ConnectionFailure("selection", item.optString("last_status", "Connect and choose a cloud model; retry saved audio in $PRODUCT_NAME."))
            val selection = choice.getJSONObject("selection")
            if (choice.has("engine")) store.preparePhone(item, choice.getJSONObject("options"), choice.getJSONObject("engine"), choice.optString("delivery", "raw"))
            else if (choice.has("cloud")) DirectCloud.prepareChoice(store, item, choice)
            else if (choice.has("options")) store.prepareOptions(item, choice.getString("server"), choice.getJSONObject("options"), choice.optString("delivery", "raw"))
            else store.prepare(item, choice.getString("server"), selection.getString("provider"), selection.getString("model"), choice.getString("input_language"))
            if (!live) recovered.add(id)
            submit(id, stoppedAt, live)
        } catch (error: Exception) {
            if (!live) notifyRecovered(id, false)
            state = "recovery"; message = if (error is ConnectionFailure) error.message!! else "Audio saved. Open $PRODUCT_NAME to retry this session."
            try { store.load(id).also { it.put("last_status", message); store.save(it) } } catch (_: Exception) { }
        }
    }

    @JvmOverloads @Synchronized fun submit(id: String, stoppedAt: Long, autoDelivery: Boolean = false) {
        val item = store.load(id)
        val queued = scheduler.getPendingJob(JOB_ID)
        val mayReplaceCloudQueue = item.has("pending_engine") && queued != null && queued.networkType != JobInfo.NETWORK_TYPE_NONE
        check(!busy && (queued == null || mayReplaceCloudQueue) && !capture.isActive) { "Session work already active" }
        if (mayReplaceCloudQueue) { scheduler.cancel(JOB_ID); automatic.clear() } // Keep the old saved request for explicit resume.
        check(item.has("pending") || item.has("pending_postprocess")) { "No pending work for this session" }
        val pending = item.optJSONObject("pending") ?: JSONObject().put("idempotency_key", "text-processing")
        val timing = item.optJSONObject("pending_timing") ?: JSONObject()
            .put("session_id", id).put("attempt_id", pending.getString("idempotency_key"))
            .put("boot_count", boot()).also { if (stoppedAt > 0) it.put("capture_stop_elapsed_ms", stoppedAt) }
        timing.put("queued_elapsed_ms", SystemClock.elapsedRealtime())
        item.put("pending_timing", timing)
        item.put("last_status", if (item.has("pending")) "Pending / processing saved audio. Open $PRODUCT_NAME to resume if interrupted."
            else "Text processing pending. Original text and audio are saved; resume in History if interrupted.")
        store.save(item)
        // A queue that started offline, was retried, or crossed process death cannot gain a delivery lease.
        automatic.remove(id)
        if (autoDelivery && (item.has("pending_engine") || network.getNetworkCapabilities(network.activeNetwork)?.hasCapability(NetworkCapabilities.NET_CAPABILITY_VALIDATED) == true) && item.has("original_target"))
            automatic[id] = pending.getString("idempotency_key")
        val extras = PersistableBundle().apply { putString("session_id", id) }
        val job = JobInfo.Builder(JOB_ID, ComponentName(context, TranscriptionJobService::class.java))
            .setExtras(extras).setRequiredNetworkType(if (item.has("pending_engine")) JobInfo.NETWORK_TYPE_NONE else JobInfo.NETWORK_TYPE_ANY).setExpedited(true).build()
        if (scheduler.schedule(job) != JobScheduler.RESULT_SUCCESS) {
            automatic.remove(id)
            error("Android could not schedule this request; retry saved audio")
        }
        displayedSession = id
        state = "processing"; message = "Processing · audio saved"
    }

    @Synchronized fun runJob(id: String, finished: Runnable) {
        if (busy) { finished.run(); return }
        busy = true; jobStopped = false
        displayedSession = id; state = "processing"; message = "Processing · audio saved"
        executor.execute {
            try {
                var item = store.load(id)
                if (!item.has("pending")) {
                    if (item.has("pending_postprocess")) {
                        if (item.getJSONObject("pending_postprocess").has("cloud")) directProcessing().run(id) else processing().run(id)
                    }
                    val updated = store.load(id)
                    updated.put("last_status", if (updated.has("pending_postprocess")) "Text processing incomplete. Original text and audio retained; resume in History."
                        else "Text processing finished. History shows each outcome; original text and audio retained.")
                    updated.remove("pending_timing"); store.save(updated)
                    state = "recovery"; message = "Text processing saved · open History"
                    return@execute
                }
                val pending = item.getJSONObject("pending")
                val attempt = pending.getString("idempotency_key")
                val timing = item.optJSONObject("pending_timing") ?: JSONObject()
                    .put("session_id", id).put("attempt_id", attempt).put("boot_count", boot())
                // Boot-relative timestamps must never be subtracted across a device reboot.
                if (timing.optInt("boot_count", -1) != boot()) {
                    for (key in listOf("capture_stop_elapsed_ms", "queued_elapsed_ms", "upload_started_elapsed_ms", "upload_finished_elapsed_ms", "asr_started_elapsed_ms", "asr_finished_elapsed_ms", "cloud_request_started_elapsed_ms", "cloud_request_finished_elapsed_ms")) timing.remove(key)
                    timing.put("boot_count", boot())
                }
                timing.put("provider_attempt", timing.optLong("provider_attempt", 0) + 1)
                for (key in listOf("upload_started_elapsed_ms", "upload_finished_elapsed_ms", "asr_started_elapsed_ms", "asr_finished_elapsed_ms", "cloud_request_started_elapsed_ms", "cloud_request_finished_elapsed_ms", "silence_prepare_ms", "result_elapsed_ms", "stop_to_result_ms")) timing.remove(key)
                timing.put("request_elapsed_ms", SystemClock.elapsedRealtime())
                item.remove("pending_latency_failed")
                item.put("pending_timing", timing); store.save(item)
                val local = item.has("pending_engine")
                val direct = item.has("pending_cloud")
                val resultMessage = if (local) {
                    val engine = PhoneEngine(context, store); activePhone = engine
                    if (jobStopped) engine.cancel()
                    engine.execute(item, "phone")
                } else if (direct) {
                    val engine = DirectCloudEngine(context, store); activeDirect = engine
                    if (jobStopped) engine.cancel()
                    engine.execute(item, item.getString("pending_server"))
                } else {
                    val engine = ServerEngine(store, MobileConnection(context)); activeEngine = engine
                    if (jobStopped) engine.cancel()
                    engine.execute(item, item.getString("pending_server"))
                }
                var phonePolish: PhonePolish? = null
                var cloudPolishError: Exception? = null
                var cloudReceipts = -1
                if (!store.load(id).has("pending")) {
                    if (!local) {
                        cloudReceipts = lastReceipts(id)
                        try { if (direct) directProcessing().afterTranscription(id) else processing().afterTranscription(id) }
                        catch (error: Exception) { cloudPolishError = error /* Raw transcript is durable; history offers explicit cleanup. */ }
                    } else {
                        val jobs = store.load(id).getJSONArray("attempts")
                        if (jobs.length() > 0 && jobs.getJSONObject(jobs.length()-1).getJSONObject("status").getString("state") == "succeeded") {
                            message = "Polishing · audio saved"
                            phonePolish = try { directProcessing().polishPhoneLive(id) }
                            catch (_: Exception) { PhonePolish.FellBack("the polisher could not start") }
                        }
                    }
                }
                item = store.load(id)
                item.put("last_status", resultMessage + polishSuffix(item, local, phonePolish, cloudReceipts, cloudPolishError)); store.save(item)
                if (item.has("pending")) {
                    automatic.remove(id); state = "recovery"; message = "Request pending · open $PRODUCT_NAME to retry"
                    notifyRecovered(id, false)
                } else {
                    val jobs = item.getJSONArray("attempts")
                    val job = jobs.getJSONObject(jobs.length() - 1)
                    val status = job.getJSONObject("status")
                    if (status.getString("state") == "succeeded") {
                        val text = CompanionProcessing.deliveryText(job)
                        main.post { completeResult(id, attempt, job.getString("id"), text, item) }
                        notifyRecovered(id, true)
                    } else {
                        automatic.remove(id); state = "failure"; message = "Transcription failed · audio saved"
                        notifyRecovered(id, false)
                    }
                }
            } catch (_: Exception) {
                automatic.remove(id); state = "recovery"; message = "Audio and pending request retained · open $PRODUCT_NAME to retry"
                notifyRecovered(id, false)
            } finally { activePhone = null; activeDirect = null; activeDirectProcessing = null; activeEngine = null; activeProcessing = null; busy = false; finished.run() }
        }
    }
    /** Receipt count of the last attempt, so a polish suffix never cites an older History run. */
    private fun lastReceipts(id: String): Int = try {
        val jobs = store.load(id).getJSONArray("attempts")
        jobs.getJSONObject(jobs.length()-1).optJSONArray("processing_attempts")?.length() ?: 0
    } catch (_: Exception) { -1 }
    /** What insertion says about live polishing: the model on success, the fallback on failure. */
    private fun polishSuffix(item: JSONObject, local: Boolean, phonePolish: PhonePolish?, cloudReceipts: Int, cloudError: Exception?): String {
        if (item.has("pending")) return ""
        if (local) return when (val outcome = phonePolish) {
            is PhonePolish.Polished -> " Polished with ${outcome.label}."
            is PhonePolish.FellBack -> " Polishing unavailable; the corrected original is ready. Retry polishing in History."
            PhonePolish.TimedOut -> " Polishing timed out; the corrected original is ready. Retry polishing in History."
            PhonePolish.Skipped, null -> ""
        }
        val jobs = item.optJSONArray("attempts") ?: return ""
        if (jobs.length() == 0) return ""
        val job = jobs.getJSONObject(jobs.length()-1)
        if (job.getJSONObject("status").optString("state") != "succeeded") return ""
        val selection = job.getJSONObject("request").getJSONObject("options").getJSONObject("cleanup").optJSONObject("selection") ?: return ""
        val receipts = job.optJSONArray("processing_attempts") ?: JSONArray()
        if (cloudReceipts >= 0 && receipts.length() > cloudReceipts) {
            val last = receipts.getJSONObject(receipts.length()-1)
            val selected = last.optJSONObject("outcome")?.optJSONObject("selected_model") ?: selection
            val label = CloudProviders.modelName(selected.getString("provider"), selected.getString("model"))
            return when {
                last.optJSONObject("cleanup")?.optString("state") == "succeeded" -> " Polished with $label."
                last.optJSONObject("outcome")?.optBoolean("original_used") == true || last.optString("state") == "incomplete" -> " Polishing unavailable; the corrected original is ready. Retry polishing in History."
                else -> ""
            }
        }
        if (cloudError != null) return " Polishing unavailable; the corrected original is ready. Retry polishing in History."
        return ""
    }
    fun deliveryVerified(id: String) {
        if (displayedSession == id && !CaptureService.isActive()) { state = "idle"; message = verifiedNotice(id) }
    }
    private fun verifiedNotice(id: String): String = try {
        val item = store.load(id)
        val ledger = DeliveryStore(File(context.filesDir, "deliveries")).load(item.getString("original_target"))
        if (ledger.optString("state") == "delivered") {
            val receipt = JSONObject().put("attempt_id", ledger.getString("attempt_id")).put("pasted", true)
                .put("original_used", ledger.optBoolean("original_used")).put("selected_model", ledger.opt("selected_model"))
                .put("at_ms", System.currentTimeMillis()).put("timing", ledger.opt("timing"))
            ledger.optJSONObject("timing")?.let { timing ->
                val stopped = timing.optLong("capture_stop_elapsed_ms", -1)
                if (timing.optInt("boot_count", -1) == boot() && stopped >= 0)
                    receipt.put("stop_to_delivery_ms", (SystemClock.elapsedRealtime() - stopped).coerceAtLeast(0L))
            }
            CompanionProcessing.find(item, ledger.getString("job_id")).put("polish_delivery", receipt); store.save(item)
        }
        if (ledger.optBoolean("original_used") && ledger.optString("state") == "delivered") {
            val notice = "Original inserted · polishing unavailable"
            main.postDelayed({ if (displayedSession == id && state == "idle" && message == notice) message = "Ready" }, 2600)
            notice
        } else "Ready"
    } catch (_: Exception) { "Ready" }
    private fun completeResult(id: String, attempt: String, job: String, text: String, item: JSONObject) {
        if (displayedSession == id && !CaptureService.isActive()) { state = "recovery"; message = "Saved · open History to copy" }
        val mayInsert = automatic.remove(id) == attempt
        if (!item.has("original_target")) return
        try {
            val token = item.getString("original_target")
            val deliveries = DeliveryStore(File(context.filesDir, "deliveries"))
            val ledger = deliveries.load(token)
            // A retry must not replace the prior attempt's delivery ledger or reissue an uncertain edit.
            if (ledger.has("text")) return
            ledger.put("job_id", job).put("attempt_id", attempt).put("timing", item.getJSONObject("last_timing"))
                .put("original_used", CompanionProcessing.find(item, job).optJSONObject("polish_outcome")?.optBoolean("original_used") == true)
                .put("selected_model", CompanionProcessing.find(item, job).optJSONObject("polish_outcome")?.opt("selected_model"))
            if (!mayInsert) ledger.put("text", text).put("state", "recovery")
                .put("reason", "Automatic delivery expired; copy explicitly after checking the field you were in")
            deliveries.save(ledger)
            if (!mayInsert) return
            AndroidTextDelivery(context).deliver(id, text, token)
            if (displayedSession == id && !CaptureService.isActive() && deliveries.load(token).optString("state") == "unconfirmed") state = "verifying"
            main.postDelayed({
                try {
                    if (displayedSession == id && !isBusy() && !CaptureService.isActive()) {
                        val delivered = deliveries.load(token).optString("state") == "delivered"
                        state = if (delivered) "idle" else "uncertain"
                        message = if (delivered) verifiedNotice(id) else "Check the field you were in before copying from History."
                    }
                } catch (_: Exception) { }
            }, 1100)
        } catch (_: Exception) { /* The raw success is already durable in the session. */ }
    }
    /** Recordings recovered after process or service loss are transcribed once, for History only. */
    @Synchronized fun submitRecovered(items: List<JSONObject>) {
        for (item in items) {
            if (!item.optBoolean("auto_submit")) continue
            // Busy work keeps the mark; the next restore submits it.
            if (busy || capture.isActive || scheduler.getPendingJob(JOB_ID) != null) return
            val id = item.getString("id")
            // A stale list must not submit twice: the saved mark is cleared before submission.
            val fresh = try { store.load(id) } catch (_: Exception) { continue }
            if (fresh.remove("auto_submit") == null) continue
            try { store.save(fresh) } catch (_: Exception) { continue }
            if (fresh.optLong("duration_ms") > 0) completeBubble(id, 0L, live = false)
        }
    }
    private fun notifyRecovered(id: String, ready: Boolean) {
        if (!recovered.remove(id)) return
        try {
            val notifications = context.getSystemService(NotificationManager::class.java)
            notifications.createNotificationChannel(NotificationChannel(RESULTS, "Interrupted recordings", NotificationManager.IMPORTANCE_DEFAULT))
            val open = PendingIntent.getActivity(context, 0, Intent(context, HistoryActivity::class.java), PendingIntent.FLAG_IMMUTABLE)
            notifications.notify(id.hashCode(), Notification.Builder(context, RESULTS).setSmallIcon(R.drawable.ic_writer_wave)
                .setContentTitle(if (ready) "Interrupted recording transcribed" else "Interrupted recording saved")
                .setContentText(if (ready) "The text is ready to copy in History." else "Transcription did not finish. Retry it in History; the audio is kept.")
                .setContentIntent(open).setAutoCancel(true).build())
        } catch (_: Exception) { /* The result is durable in History either way. */ }
    }
    fun stopJob() { jobStopped = true; automatic.clear(); activeDirect?.cancel(); activeDirectProcessing?.cancel(); activeEngine?.cancel(); activePhone?.cancel(); activeProcessing?.cancel() }

    @Synchronized fun cancelWork() {
        val queuedId = scheduler.getPendingJob(JOB_ID)?.extras?.getString("session_id")
        stopJob(); scheduler.cancel(JOB_ID)
        if (!busy && queuedId != null && store.load(queuedId).has("pending_engine")) {
            busy = true
            executor.execute {
                try { PhoneEngine(context, store).also { it.cancel() }.execute(store.load(queuedId), "phone") }
                finally { busy = false }
            }
        }
        state = "recovery"; message = "Processing stopped. Saved audio and earlier text retained; resume in History."
    }

    init { main.post { try { submitRecovered(store.recover()) } catch (_: Exception) { } } }

    companion object {
        private const val JOB_ID = 56
        private const val RESULTS = "writer-recovered-results"
        @Volatile private var instance: SessionManager? = null
        @JvmStatic @Synchronized fun get(context: Context): SessionManager =
            instance ?: SessionManager(context.applicationContext).also { instance = it }
    }
}
