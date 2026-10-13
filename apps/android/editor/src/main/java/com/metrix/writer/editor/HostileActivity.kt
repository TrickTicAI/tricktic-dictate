package com.metrix.writer.editor

import android.app.Activity
import android.content.*
import android.content.pm.PackageInstaller
import android.os.*

/**
 * Calls originate in this ordinary app's UID, never in Writer instrumentation or adb shell.
 * Without a reply receiver (a store-signed target has no instrumentation), results go to logcat
 * as one JSON line tagged WriterHostile.
 */
class HostileActivity : Activity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val reply = intent.getParcelableExtra("reply", ResultReceiver::class.java)
        val target = intent.getStringExtra("target")!!
        require(target in setOf("com.metrix.writer", "com.metrix.writer.debug", "com.metrix.writer.securityphone",
            "com.tricktic.dictate", "com.tricktic.dictate.debug", "com.tricktic.dictate.securityphone"))
        val result = Bundle().apply { putInt("uid", Process.myUid()) }
        fun attempt(name: String, action: () -> Unit) {
            result.putString(name, try { action(); "allowed" } catch (_: SecurityException) { "denied" }
                catch (e: Exception) { e.javaClass.simpleName })
        }
        fun component(name: String) = ComponentName(target, "com.metrix.writer.$name")
        when (intent.getStringExtra("operation")) {
            "components" -> {
                for (name in listOf("CaptureService", "PhoneInferenceService", "WriterInputService", "TranscriptionJobService")) {
                    // Resolve first: an absent component is not evidence of access control.
                    packageManager.getServiceInfo(component(name), 0)
                    val request = Intent("com.metrix.writer.CAPTURE_START").setComponent(component(name))
                    attempt("start:$name") { check(startService(request) != null) }
                    attempt("bind:$name") {
                        val connection = object : ServiceConnection {
                            override fun onServiceConnected(name: ComponentName, service: IBinder) {}
                            override fun onServiceDisconnected(name: ComponentName) {}
                        }
                        check(bindService(request, connection, BIND_AUTO_CREATE))
                        unbindService(connection)
                    }
                }
                for (name in listOf("CloudSetupActivity", "HistoryActivity", "PhoneModelsActivity", "CompanionSettingsActivity", "DictionaryActivity", "PairingActivity")) {
                    packageManager.getActivityInfo(component(name), 0)
                    attempt("activity:$name") { startActivity(Intent().setComponent(component(name))) }
                }
            }
            "launch" -> {
                val name = intent.getStringExtra("activity")!!
                attempt("launch") {
                    startActivity(Intent("com.metrix.writer.CAPTURE_START").setComponent(component(name))
                        .putExtra("record", true).putExtra("origin", "bubble")
                        .putExtra("text", "HOSTILE INSERTION").putExtra("route", "Settings/Cloud"))
                }
            }
            "export" -> {
                val uri = intent.data!!
                attempt("read") { contentResolver.openInputStream(uri)!!.use { result.putInt("bytes", it.readBytes().size) } }
                attempt("query") { contentResolver.query(uri, null, null, null, null)!!.use { check(it.moveToFirst()) } }
                attempt("write") { contentResolver.openOutputStream(uri, "w")!!.close() }
                attempt("sibling") { contentResolver.openInputStream(uri.buildUpon().path("/${uri.pathSegments.first()}/00000000-0000-0000-0000-000000000000.wav").build())!!.close() }
                attempt("traversal") { contentResolver.openInputStream(uri.buildUpon().encodedPath("/%2e%2e/files/recordings/sentinel.wav").build())!!.close() }
            }
            "callback" -> {
                // If delivered, SUCCESS changes updater state; PENDING opens our callback Activity.
                for (status in listOf(PackageInstaller.STATUS_SUCCESS, PackageInstaller.STATUS_PENDING_USER_ACTION)) {
                    sendBroadcast(Intent().setComponent(component("InstallReceiver"))
                        .putExtra(PackageInstaller.EXTRA_STATUS, status)
                        .putExtra(PackageInstaller.EXTRA_SESSION_ID, 202020)
                        .putExtra(Intent.EXTRA_INTENT, Intent(this, CallbackActivity::class.java)))
                }
                Handler(mainLooper).postDelayed({
                    result.putInt("callbacks", getSharedPreferences("attack", 0).getInt("callbacks", 0))
                    send(reply, result); finish()
                }, 1000)
                return
            }
            else -> error("Unknown fixture operation")
        }
        send(reply, result)
        finish()
    }

    @Suppress("DEPRECATION")
    private fun send(reply: ResultReceiver?, result: Bundle) {
        if (reply != null) return reply.send(0, result)
        val json = org.json.JSONObject()
        for (key in result.keySet().sorted()) json.put(key, result.get(key))
        android.util.Log.i("WriterHostile", json.toString())
    }
}

class CallbackActivity : Activity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val prefs = getSharedPreferences("attack", 0)
        prefs.edit().putInt("callbacks", prefs.getInt("callbacks", 0) + 1).commit()
        finish()
    }
}
