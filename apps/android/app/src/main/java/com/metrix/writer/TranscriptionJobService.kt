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
        SessionManager.get(this).runJob(id) {
            Handler(Looper.getMainLooper()).post {
                if (active === params) { active = null; jobFinished(params, false) }
            }
        }
        return true
    }
    override fun onStopJob(params: JobParameters): Boolean {
        active = null
        SessionManager.get(this).stopJob()
        // Keep the exact pending request for explicit retry; never deliver to a new editor.
        return false
    }
}
