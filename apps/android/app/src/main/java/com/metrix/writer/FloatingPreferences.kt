package com.metrix.writer

import android.content.Context

/** Idle-only snooze; no capture or delivery state lives in these presentation preferences. */
class FloatingPreferences(context: Context) {
    private val prefs = context.getSharedPreferences("floating-control", Context.MODE_PRIVATE)
    var enabled: Boolean
        get() = prefs.getBoolean("enabled",true)
        set(value) { prefs.edit().putBoolean("enabled",value).apply() }
    var size: Int
        get() = prefs.getInt("size",40).coerceIn(32,56)
        set(value) { prefs.edit().putInt("size",value.coerceIn(32,56)).apply() }
    var dock: Boolean
        get() = prefs.getBoolean("dock",true)
        set(value) { prefs.edit().putBoolean("dock",value).apply() }
    var minutes: Int
        get() = prefs.getInt("snooze-minutes",10).takeIf { it in listOf(10,30,60) } ?: 10
        set(value) { require(value in listOf(10,30,60));prefs.edit().putInt("snooze-minutes",value).apply() }
    val until: Long get() = prefs.getLong("snooze-until",0)
    fun snoozed(now: Long = System.currentTimeMillis()) = now < until
    fun snooze(now: Long = System.currentTimeMillis()) { prefs.edit().putLong("snooze-until",now + minutes * 60000L).apply() }
    fun showNow() { prefs.edit().putLong("snooze-until",0).putBoolean("enabled",true).apply() }
    fun resetPosition() { prefs.edit().putFloat("x",1f).putFloat("y",.6f).apply() }
    val x get() = prefs.getFloat("x",1f).coerceIn(0f,1f)
    val y get() = prefs.getFloat("y",.6f).coerceIn(0f,1f)
    fun position(x: Float, y: Float) { prefs.edit().putFloat("x",x.coerceIn(0f,1f)).putFloat("y",y.coerceIn(0f,1f)).apply() }
}

/** Geometry is independent of View/Compose, so drag, resizing and IME boundaries share one rule. */
object FloatingGeometry {
    fun clamp(x: Int, y: Int, width: Int, height: Int, availableWidth: Int, availableHeight: Int): Pair<Int,Int> =
        x.coerceIn(0,(availableWidth-width).coerceAtLeast(0)) to y.coerceIn(0,(availableHeight-height).coerceAtLeast(0))
    fun dock(x: Int, width: Int, available: Int): Int = if(x + width / 2 < available / 2) 0 else (available-width).coerceAtLeast(0)
    fun dropped(x: Int, y: Int, width: Int, height: Int, targetX: Int, targetY: Int, targetSize: Int, targetHeight: Int = targetSize): Boolean =
        x + width / 2 in targetX..targetX+targetSize && y + height / 2 in targetY..targetY+targetHeight
}
