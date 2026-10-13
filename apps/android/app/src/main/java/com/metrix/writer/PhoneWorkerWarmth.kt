@file:Suppress("DEPRECATION", "OVERRIDE_DEPRECATION")
package com.metrix.writer

import android.content.ComponentCallbacks2
import android.content.Context
import android.content.ServiceConnection
import android.content.res.Configuration
import android.os.Handler
import android.os.Looper

/** Retain at most one idle binding for a short burst of dictations. No started
 * service, wake lock, audio, result or activity is retained. Android may reclaim
 * the worker; the next request still uses the ordinary checked binding path.
 */
internal object PhoneWorkerWarmth {
    const val IDLE_MS = 60_000L
    private val main = Handler(Looper.getMainLooper())
    private var release: (() -> Unit)? = null
    private var registered = false
    private val expiry = Runnable { clear() }

    fun retain(context: Context, connection: ServiceConnection, idleMs: Long = IDLE_MS) {
        check(Looper.myLooper() == Looper.getMainLooper())
        val application = context.applicationContext
        if (!registered) {
            application.registerComponentCallbacks(object : ComponentCallbacks2 {
                override fun onConfigurationChanged(config: Configuration) {}
                override fun onLowMemory() { main.post { clear() } }
                override fun onTrimMemory(level: Int) {
                    if (level >= ComponentCallbacks2.TRIM_MEMORY_BACKGROUND ||
                        level in ComponentCallbacks2.TRIM_MEMORY_RUNNING_LOW..ComponentCallbacks2.TRIM_MEMORY_RUNNING_CRITICAL) main.post { clear() }
                }
            })
            registered = true
        }
        clear() // The new connection is already bound, so the service stays alive.
        release = { application.unbindService(connection) }
        main.postDelayed(expiry, idleMs)
    }
    fun clear() {
        check(Looper.myLooper() == Looper.getMainLooper())
        main.removeCallbacks(expiry)
        release?.invoke(); release = null
    }
}
