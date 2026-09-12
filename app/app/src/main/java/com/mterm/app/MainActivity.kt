package com.mterm.app

import android.graphics.Bitmap
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.asImageBitmap

class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContent {
            val rows = 24
            val cols = 80
            val session = rememberTermSession(cols, rows)
            MaterialTheme {
                Surface(modifier = Modifier.fillMaxSize()) {
                    TermView(session)
                }
            }
        }
    }

    @Composable
    fun rememberTermSession(startCols: Int, startRows: Int): TermSession {
        val handle = remember { NativeTerm.nativeInit(startCols, startRows) }
        val session = remember { TermSession(handle) }
        DisposableEffect(Unit) {
            onDispose {
                NativeTerm.nativeDestroy(handle)
            }
        }
        return session
    }
}

@Composable
fun TermView(session: TermSession) {
    val cols = 80
    val rows = 24
    var frame by remember { mutableStateOf(0) }

    DisposableEffect(session) {
        // polling loop sederhana: re-render saat buffer dirty
        val timer = kotlin.concurrent.timer(period = 100) {
            if (session.dirty()) frame++
        }
        onDispose { timer.cancel() }
    }

    Box(Modifier.fillMaxSize()) {
        val bmp = remember(frame) {
            val pixels = session.snapshot(cols, rows)
            Bitmap.createBitmap(cols, rows, Bitmap.Config.ARGB_8888).apply {
                setPixels(pixels, 0, cols, 0, 0, cols, rows)
            }
        }
        Canvas(Modifier.fillMaxSize()) {
            drawImage(bmp.asImageBitmap())
        }
        Text("mterm — workspace-first terminal", color = Color.Gray)
    }
}