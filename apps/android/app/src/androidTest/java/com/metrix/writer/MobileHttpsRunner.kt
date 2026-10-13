package com.metrix.writer

import android.app.Instrumentation
import android.app.Activity
import android.content.Intent
import android.os.Bundle
import android.view.View
import android.view.ViewGroup
import android.widget.Button
import android.widget.EditText
import android.widget.Spinner
import android.widget.TextView
import org.json.JSONObject
import java.io.File
import java.io.RandomAccessFile
import javax.net.ssl.SSLException

/** Explicit isolated integration runner; config enters private storage, never command arguments. */
class MobileHttpsRunner : Instrumentation() {
    private var phase = "connect"
    private var step = "load private test configuration"
    override fun onCreate(arguments: Bundle?) { super.onCreate(arguments); phase = arguments?.getString("phase") ?: "connect"; start() }
    override fun onStart() {
        var activity: Activity? = null
        try {
            val config = JSONObject(File(targetContext.noBackupFilesDir, "mobile-proof.json").readText())
            val origin = config.getString("origin"); val token = config.getString("token")
            val client = MobileConnection(targetContext)
            if (phase == "fixture-pair") {
                step = "pair isolated fixture endpoint"
                activity = startActivitySync(Intent(targetContext, PairingActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
                val root = activity.window.decorView
                fun views(view: View): List<View> = listOf(view) + if (view is ViewGroup) (0 until view.childCount).flatMap { views(view.getChildAt(it)) } else emptyList()
                runOnMainSync {
                    views(root).filterIsInstance<EditText>().single { it.contentDescription == "Writer endpoint" }.setText(origin)
                    views(root).filterIsInstance<EditText>().single { it.contentDescription == "Device credential" }.setText(token)
                    views(root).filterIsInstance<Button>().single { it.text == "Pair / check connection" }.performClick()
                }
                val deadline = System.nanoTime() + 15_000_000_000L
                var ready = false
                while (!ready && System.nanoTime() < deadline) {
                    Thread.sleep(100)
                    runOnMainSync { ready = views(root).filterIsInstance<Spinner>().first().count > 0 }
                }
                check(ready)
                runOnMainSync {
                    val spinner = views(root).filterIsInstance<Spinner>().first()
                    val selected = (0 until spinner.count).single { spinner.getItemAtPosition(it).toString().startsWith(config.getString("selection") + " ·") }
                    spinner.setSelection(selected)
                }
                waitForIdleSync()
                runOnMainSync { views(root).filterIsInstance<Button>().single { it.text == "Use selected model" }.let { check(it.isEnabled); it.performClick() } }
                check(client.endpoint() == origin && client.selection() == config.getString("selection"))
                File(targetContext.noBackupFilesDir, "mobile-proof.json").delete()
                finish(Activity.RESULT_OK, Bundle().apply { putString("stream", "PASS: fixture pairing selected\n") }); return
            }
            if (phase == "revoked") {
                step = "revoked credential refused after process recreation"
                try { client.check(origin, ""); error("Revoked credential accepted") }
                catch (error: ConnectionFailure) { check(error.category == "authentication") }
                val store = SessionStore(File(targetContext.cacheDir, config.getString("session_directory")))
                val session = store.restore().single()
                check(session.has("pending") && session.getString("pending_server") == origin)
                check(store.audio(session).length() == session.getLong("byte_length"))
                finish(Activity.RESULT_OK, Bundle().apply { putString("stream", "PASS: revoked credential rejected after process recreation; pending request and audio retained\n") }); return
            }
            step = "valid HTTPS pairing and real adapter discovery"
            val capabilities = client.check(origin, token)
            check(capabilities.getString("service") == "metrix-writer-mobile")
            check(capabilities.getJSONArray("models").length() > 0)
            step = "Keystore credential reuse"
            check(MobileConnection(targetContext).check(origin, "").getString("device_id") == capabilities.getString("device_id"))
            step = "wrong credential refusal"
            val wrong = token.dropLast(1) + if (token.last() == 'a') "b" else "a"
            try { client.check(origin, wrong); error("Wrong credential accepted") } catch (error: ConnectionFailure) { check(error.category == "authentication") }
            // A failed replacement did not destroy the previously valid credential.
            client.check(origin, "")
            for (field in listOf("untrusted", "wrong_hostname")) {
                step = "TLS refusal: $field"
                try { client.check(config.getString(field), token); error("Invalid TLS accepted") } catch (_: SSLException) { }
            }
            step = "redirect refusal"
            try { client.check(config.getString("redirect"), token); error("Redirect followed") } catch (error: ConnectionFailure) { check(error.category == "redirect") }
            step = "compatibility refusal"
            try { client.check(config.getString("incompatible"), token); error("Unsupported API accepted") } catch (error: ConnectionFailure) { check(error.category == "compatibility") }
            step = "offline classification"
            try { client.check("https://10.0.2.2:18809", token); error("Unexpected endpoint") }
            catch (error: Exception) { check(MobileConnection.explain(error).startsWith("Offline")) }
            step = "endpoint change preserves pending payload and audio"
            val store = SessionStore(File(targetContext.cacheDir, config.getString("session_directory")))
            val item = store.create("mixed").put("capture_state", "complete")
            RandomAccessFile(store.audio(item), "rw").use { it.seek(44); it.write(ByteArray(3200)); SessionStore.header(it, 3200); it.fd.sync() }
            item.put("byte_length", 3244).put("duration_ms", 100); store.save(item)
            store.prepare(item, origin, "openai", "gpt-4o-mini-transcribe", "mixed")
            val before = store.load(item.getString("id")).toString(); val audio = store.audio(item).readBytes()
            client.select("https://different.invalid", "openai/gpt-4o-mini-transcribe", "en")
            try { store.prepare(item, "https://different.invalid", "openai", "gpt-4o-mini-transcribe", "en"); error("Pending request migrated") } catch (_: IllegalStateException) { }
            check(before == store.load(item.getString("id")).toString()); check(audio.contentEquals(store.audio(item).readBytes()))
            step = "native pairing UI and unavailable provider state"
            activity = startActivitySync(Intent(targetContext, PairingActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
            val root = activity.window.decorView
            fun views(view: View): List<View> = listOf(view) + if (view is ViewGroup) (0 until view.childCount).flatMap { views(view.getChildAt(it)) } else emptyList()
            runOnMainSync {
                views(root).filterIsInstance<EditText>().single { it.contentDescription == "Writer endpoint" }.setText(origin)
                views(root).filterIsInstance<EditText>().single { it.contentDescription == "Device credential" }.setText(token)
                views(root).filterIsInstance<Button>().single { it.text == "Pair / check connection" }.performClick()
            }
            val deadline = System.nanoTime() + 15_000_000_000L
            var ready = false
            while (!ready && System.nanoTime() < deadline) {
                Thread.sleep(100)
                runOnMainSync { ready = views(root).filterIsInstance<Spinner>().first().count > 0 }
            }
            check(ready)
            runOnMainSync {
                check(views(root).filterIsInstance<EditText>().single { it.contentDescription == "Device credential" }.text.isEmpty())
                check(!views(root).filterIsInstance<Button>().single { it.text == "Use selected model" }.isEnabled)
                check(views(root).filterIsInstance<TextView>().any { it.text.toString().contains("Provider/model unavailable") })
            }
            finish(Activity.RESULT_OK, Bundle().apply { putString("stream", "PASS: real HTTPS pairing, adapter discovery, native UI, Keystore reuse, wrong credential, TLS trust/hostname, redirect, compatibility, offline and endpoint retention\n") })
        } catch (error: Exception) {
            finish(Activity.RESULT_CANCELED, Bundle().apply { putString("stream", "FAIL at $step (${error.javaClass.simpleName})\n") })
        } finally { activity?.let { runOnMainSync { it.finish() } } }
    }
}
