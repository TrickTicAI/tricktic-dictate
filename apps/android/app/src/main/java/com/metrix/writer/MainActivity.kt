package com.metrix.writer

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.lifecycle.viewmodel.compose.viewModel
import com.metrix.writer.ui.WriterApp

open class MainActivity : ComponentActivity() {
    protected open val initialRoute = "Home"
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        window.protectTouches()
        enableEdgeToEdge()
        setContent { WriterApp(viewModel(), initialRoute) }
    }
}
