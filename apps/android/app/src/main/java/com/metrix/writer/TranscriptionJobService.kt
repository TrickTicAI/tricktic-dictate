package com.metrix.writer

import android.app.job.JobParameters
import android.app.job.JobService
import android.os.Handler
import android.os.Looper

/** Android owns background execution eligibility; no Activity or microphone service is needed. */
class TranscriptionJobService : JobService() {
    private var active: JobParameters? = null
    override fun onStartJob(params: JobParameters): Boolean {
        val id = params.extras.getString("session_id") ?: return false
        active = params
        val manager = SessionManager.get(this)
        manager.runJob(id) {
            val again = manager.consumeRerun()
            Handler(Looper.getMainLooper()).post {
                // An automatic rerun (waiting for network) uses JobScheduler backoff and constraints.
                if (active === params) { active = null; jobFinished(params, again) }
            }
        }
        return true
    }
    override fun onStopJob(params: JobParameters): Boolean {
        active = null
        // Keep the exact pending request. Only checkpointed streaming work reruns, never with a delivery lease.
        return SessionManager.get(this).stopJob()
    }
}
