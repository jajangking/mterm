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
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
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
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.drawText
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.rememberTextMeasurer
import androidx.compose.ui.unit.TextUnit
import androidx.compose.ui.unit.TextUnitType
import androidx.compose.ui.unit.dp
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
        val timer = kotlin.concurrent.timer(period = 100) {
            if (session.dirty()) frame++
        }
        onDispose { timer.cancel() }
    }

    Box(Modifier.fillMaxSize()) {
        val textMeasurer = rememberTextMeasurer()
        var diag by remember { mutableStateOf(0) }
        LaunchedEffect(frame) {
            if (diag < 3) {
                diag++
                val c0 = session.cellAt(0, 0)
                android.util.Log.i(
                    "mterm",
                    "diag f=$frame c00=${c0?.let { String.format("%08x/%08x %c", it.first, it.second, it.third) }}"
                )
            }
        }
        key(frame) {
            Canvas(Modifier.fillMaxSize()) {
                val cellW = size.width / cols
                val cellH = size.height / rows
                // lebar glyph monospace ≈ 0.55×fontSize → isi lebar sel
                val fontSize = cellW * 1.7f
                val glyphH = fontSize * 1.25f
                try {
                    for (y in 0 until rows) {
                        val top = y * cellH
                        val glyphTop = top + (cellH - glyphH) / 2f
                        for (x in 0 until cols) {
                            val cell = session.cellAt(x, y) ?: continue
                            drawRect(
                                color = Color(cell.second),
                                topLeft = Offset(x * cellW, top),
                                size = Size(cellW, cellH + 1f),
                            )
                            val ch = cell.third
                            if (ch != ' ' && ch != '\u0000') {
                                drawText(
                                    textMeasurer = textMeasurer,
                                    text = ch.toString(),
                                    topLeft = Offset(x * cellW, glyphTop),
                                    style = TextStyle(
                                        color = Color(cell.first),
                                        fontSize = TextUnit(fontSize, TextUnitType.Sp),
                                        fontFamily = FontFamily.Monospace,
                                    ),
                                )
                            }
                        }
                    }
                } catch (t: Throwable) {
                    android.util.Log.e("mterm", "draw error: ${t}")
                }
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