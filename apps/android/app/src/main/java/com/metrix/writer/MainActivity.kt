package com.metrix.writer

import android.content.Intent
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.runtime.mutableStateOf
import androidx.lifecycle.viewmodel.compose.viewModel
import com.metrix.writer.ui.AppRequest
import com.metrix.writer.ui.WriterApp

open class MainActivity : ComponentActivity() {
    protected open val initialRoute = "Home"
    private val request = mutableStateOf<AppRequest?>(null)
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        window.protectTouches()
        enableEdgeToEdge()
        // A restored activity already handled its launching intent before recreation.
        if (savedInstanceState == null) request.value = requestOf(intent)
        setContent { WriterApp(viewModel(), initialRoute, request.value) }
    }
    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        requestOf(intent)?.let { request.value = it }
    }
    /** Recovery notification extras: open a dictation, or copy its saved result. Only Writer's own routes are accepted. */
    private fun requestOf(intent: Intent?): AppRequest? {
        val route = intent?.getStringExtra(ROUTE)?.takeIf { it.startsWith("History/") || it.startsWith("Settings/") || it == "History" }
        val copy = intent?.getStringExtra(COPY_SESSION)?.takeIf { runCatching { java.util.UUID.fromString(it) }.isSuccess }
        return if (route == null && copy == null) null else AppRequest(route, copy, android.os.SystemClock.elapsedRealtime())
    }
    companion object {
        const val ROUTE = "writer_route"
        const val COPY_SESSION = "writer_copy_session"
    }
}
