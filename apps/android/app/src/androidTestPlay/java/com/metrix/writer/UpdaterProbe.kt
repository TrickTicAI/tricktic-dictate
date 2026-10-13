package com.metrix.writer

import android.content.Context

/** Play has no self-updater: there is no install receiver or updater state to forge. */
object UpdaterProbe {
    const val PRESENT = false
    fun prime(context: Context) {}
    fun receiverExported(context: Context): Boolean = error("The Play build has no install receiver")
    fun state(context: Context): Any? = null
}
