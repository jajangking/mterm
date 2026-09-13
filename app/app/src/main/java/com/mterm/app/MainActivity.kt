package com.mterm.app

import android.content.Context
import android.os.Bundle
import android.text.Editable
import android.text.InputType
import android.text.TextWatcher
import android.view.KeyEvent
import android.view.inputmethod.InputMethodManager
import android.widget.EditText
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.viewinterop.AndroidView

class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContent {
            val cols = 80
            val rows = 24
            val session = rememberTermSession(cols, rows)
            MaterialTheme {
                Surface(modifier = Modifier.fillMaxSize()) {
                    Box(
                        Modifier
                            .fillMaxSize()
                            .navigationBarsPadding()
                            .imePadding()
                    ) {
                        TermView(session)
                        TermKeyboard(session)
                    }
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

    // start_shell: PTY emulator dijalankan (sh), output → session
    val ctx = LocalContext.current
    LaunchedEffect(session) {
        session.startSession(
            "/system/bin/sh",
            null,
            ctx.filesDir.path,
            cols,
            rows,
        )
    }

    DisposableEffect(session) {
        var lastTick = 0L
        val timer = kotlin.concurrent.timer(period = 100) {
            val d = session.dirty()
            if (d) frame++
            val now = android.os.SystemClock.elapsedRealtime()
            if (now - lastTick > 2000) {
                lastTick = now
                android.util.Log.i("mterm", "tick d=$d f=$frame")
            }
        }
        onDispose { timer.cancel() }
    }

    BoxWithConstraints(Modifier.fillMaxSize()) {
        val cellW = maxWidth / cols
        val cellH = maxHeight / rows
        val fs = (cellW.value / 0.62f).sp
        Column(Modifier.fillMaxSize()) {
            for (row in 0 until rows) {
                val line = remember(row, frame) {
                    buildAnnotatedString {
                        for (x in 0 until cols) {
                            val c = session.cellAt(x, row) ?: continue
                            val ch = c.third
                            if (ch == '\u0000' || ch == ' ') continue
                            withStyle(SpanStyle(color = Color(c.first))) { append(ch.toString()) }
                        }
                    }
                }
                Text(
                    line,
                    modifier = Modifier
                        .fillMaxWidth()
                        .height(cellH),
                    fontFamily = FontFamily.Monospace,
                    fontSize = fs,
                    maxLines = 1,
                    softWrap = false,
                    overflow = TextOverflow.Clip,
                )
            }
        }
        Text(
            "ketuk layar untuk keyboard",
            color = Color.Gray,
            modifier = Modifier
                .align(Alignment.BottomEnd)
                .padding(8.dp)
        )
    }
}

@Composable
fun TermKeyboard(session: TermSession) {
    var edit by remember { mutableStateOf<EditText?>(null) }
    Box(
        Modifier
            .fillMaxSize()
            .clickable {
                val v = edit
                if (v != null && !v.hasFocus()) {
                    v.requestFocus()
                    val ime =
                        v.context.getSystemService(Context.INPUT_METHOD_SERVICE) as InputMethodManager
                    ime.showSoftInput(v, InputMethodManager.SHOW_IMPLICIT)
                }
            }
    ) {
        AndroidView(
            modifier = Modifier.alpha(0f),
            factory = { ctx ->
                EditText(ctx).apply {
                    // multiline biar tombol enter IME menghasilkan "\n" → diterjemah jadi CR
                    isSingleLine = false
                    isFocusableInTouchMode = true
                    setSelectAllOnFocus(false)
                    setRawInputType(InputType.TYPE_CLASS_TEXT)
                    var prev = ""
                    var suppress = false
                    addTextChangedListener(object : TextWatcher {
                        override fun beforeTextChanged(s: CharSequence?, a: Int, b: Int, c: Int) {}
                        override fun onTextChanged(s: CharSequence?, a: Int, b: Int, c: Int) {}
                        override fun afterTextChanged(s: Editable?) {
                            if (suppress || s == null) return
                            val cur = s.toString()
                            when {
                                cur.length > prev.length -> {
                                    val added = cur.substring(prev.length)
                                    val bytes = added.replace("\n", "\r").toByteArray(Charsets.UTF_8)
                                    session.input(bytes)
                                    if (added.contains("\n")) {
                                        suppress = true
                                        setText("")
                                        suppress = false
                                        prev = ""
                                    } else {
                                        prev = cur
                                    }
                                }
                                cur.length < prev.length -> {
                                    val n = prev.length - cur.length
                                    repeat(n) { session.input(byteArrayOf(0x7f.toByte())) }
                                    prev = cur
                                }
                                else -> prev = cur
                            }
                        }
                    })
                    setOnKeyListener { _, keyCode, event ->
                        if (event?.action == KeyEvent.ACTION_DOWN) {
                            when (keyCode) {
                                KeyEvent.KEYCODE_TAB -> {
                                    session.input(byteArrayOf(0x09))
                                    true
                                }
                                KeyEvent.KEYCODE_DPAD_UP -> {
                                    session.input("\u001b[A".toByteArray())
                                    true
                                }
                                KeyEvent.KEYCODE_DPAD_DOWN -> {
                                    session.input("\u001b[B".toByteArray())
                                    true
                                }
                                KeyEvent.KEYCODE_DPAD_RIGHT -> {
                                    session.input("\u001b[C".toByteArray())
                                    true
                                }
                                KeyEvent.KEYCODE_DPAD_LEFT -> {
                                    session.input("\u001b[D".toByteArray())
                                    true
                                }
                                else -> false
                            }
                        } else false
                    }
                    edit = this
                    requestFocus()
                }
            }
        )
    }
}