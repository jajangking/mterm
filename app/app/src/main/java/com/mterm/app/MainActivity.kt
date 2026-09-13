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
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.gestures.detectDragGestures
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
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalViewConfiguration
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.viewinterop.AndroidView

// Kode SGR xterm (crates/core/src/mouse.rs): 0=left, 1=middle, 2=right; MOTION=32.
private const val BTN_LEFT = 0
private const val MOTION = 32

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
    var frame by remember { mutableStateOf(0) }

    // start_shell: PTY emulator dijalankan (sh), output → session
    val ctx = LocalContext.current
    var started by remember { mutableStateOf(false) }
    var cols by remember { mutableStateOf(80) }
    var rows by remember { mutableStateOf(24) }
    var mouseMode by remember { mutableStateOf(false) }

    DisposableEffect(session) {
        val timer = kotlin.concurrent.timer(period = 100) {
            if (session.dirty()) frame++
            while (true) {
                when (val ev = session.takeEvent() ?: break) {
                    is TermEvent.Mouse -> {
                        android.util.Log.i("mterm", "drain ev=$ev")
                        mouseMode = ev.enabled
                    }
                    is TermEvent.Title -> android.util.Log.i("mterm", "drain ev=$ev")
                    TermEvent.Bell -> android.util.Log.i("mterm", "drain ev=Bell")
                }
            }
        }
        onDispose { timer.cancel() }
    }

    BoxWithConstraints(Modifier.fillMaxSize()) {
        // Ukuran dialog dari constrain layar: font tetap → turunkan cols/rows.
        val density = LocalDensity.current
        val font = 13.sp
        val lh = font * 1.2f
        val cellW = with(density) { font.toPx() * 0.62f }
        val cellH = with(density) { lh.toPx() }
        val wPx = with(density) { maxWidth.toPx() }
        val hPx = with(density) { maxHeight.toPx() }
        val dCols = (wPx / cellW).toInt().coerceIn(2, 512)
        val dRows = (hPx / cellH).toInt().coerceIn(2, 512)
        val rowH = with(density) { cellH.toDp() }

        LaunchedEffect(started) {
            if (!started) {
                cols = dCols
                rows = dRows
                session.startSession(
                    "/system/bin/sh",
                    null,
                    ctx.filesDir.path,
                    dCols,
                    dRows,
                )
                started = true
            }
        }

        LaunchedEffect(dCols, dRows) {
            if (started && (dCols != cols || dRows != rows)) {
                if (session.sessionResize(dCols, dRows)) {
                    cols = dCols
                    rows = dRows
                }
            }
        }

        Column(Modifier.fillMaxSize()) {
            for (row in 0 until rows) {
                val line = remember(row, frame, cols) {
                    buildAnnotatedString {
                        for (x in 0 until cols) {
                            val c = session.cellAt(x, row) ?: continue
                            val ch = c.third
                            val glyph: Char = if (ch == '\u0000') ' ' else ch
                            withStyle(
                                SpanStyle(
                                    color = Color(c.first),
                                    background = Color(c.second),
                                )
                            ) {
                                append(glyph.toString())
                            }
                        }
                    }
                }
                Text(
                    line,
                    modifier = Modifier
                        .fillMaxWidth()
                        .height(rowH),
                    fontFamily = FontFamily.Monospace,
                    fontSize = font,
                    lineHeight = lh,
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
        TermKeyboard(session, cellH, cellW, mouseMode, onScroll = { frame++ })
    }
}

@Composable
fun TermKeyboard(
    session: TermSession,
    cellH: Float,
    cellW: Float,
    mouseMode: Boolean,
    onScroll: () -> Unit,
) {
    var edit by remember { mutableStateOf<EditText?>(null) }
    var scrollOffset by remember { mutableStateOf(0) }
    val vc = LocalViewConfiguration.current
    val slopPx = vc.touchSlop
    Box(
        Modifier
            .fillMaxSize()
            .pointerInput(session, mouseMode, cellW, cellH, slopPx) {
                fun showKeyboard() {
                    val v = edit ?: return
                    if (!v.hasFocus()) v.requestFocus()
                    val ime =
                        v.context.getSystemService(Context.INPUT_METHOD_SERVICE) as InputMethodManager
                    ime.showSoftInput(v, InputMethodManager.SHOW_IMPLICIT)
                }

                // kode SGR (sinkron dgn crates/core/src/mouse.rs): BTN_LEFT=0, MOTION=32.
                fun sendMouse(code: Int, release: Boolean, x: Int, y: Int) {
                    android.util.Log.i("mterm", "sendMouse mode=$mouseMode code=$code r=$release x=$x y=$y")
                    if (!mouseMode) return
                    val bytes = session.sgrMouse(code, 0, release, x, y)
                    if (bytes.isNotEmpty()) session.input(bytes)
                }

                android.util.Log.i("mterm", "ptr-block start mode=$mouseMode")
                if (mouseMode) {
                    // TUI menyalakan SGR mouse: tap = klik (press + release), drag =
                    // press di titik awal → motion (bit 32) → release di titik akhir.
                    awaitEachGesture {
                        val down = awaitFirstDown(requireUnconsumed = false)
                        android.util.Log.i("mterm", "gest-down x=${down.position.x} y=${down.position.y}")
                        val x0 = (down.position.x / cellW).toInt() + 1
                        val y0 = (down.position.y / cellH).toInt() + 1
                        sendMouse(BTN_LEFT, false, x0, y0)
                        var lastX = x0
                        var lastY = y0
                        while (true) {
                            val ev = awaitPointerEvent()
                            val change = ev.changes.first()
                            val x = (change.position.x / cellW).toInt() + 1
                            val y = (change.position.y / cellH).toInt() + 1
                            if (change.pressed) {
                                val moved =
                                    (change.position - down.position).getDistance() > slopPx
                                if (moved && (x != lastX || y != lastY)) {
                                    sendMouse(BTN_LEFT or MOTION, false, x, y)
                                    lastX = x
                                    lastY = y
                                }
                                change.consume()
                            } else {
                                sendMouse(BTN_LEFT, true, x, y)
                                change.consume()
                                break
                            }
                        }
                    }
                } else {
                    var totalDy = 0f
                    var startOff = 0
                    detectDragGestures(
                        onDragStart = { android.util.Log.i("mterm", "drag-start"); totalDy = 0f; startOff = scrollOffset },
                        onDrag = { change, dragAmount ->
                            change.consume()
                            totalDy += dragAmount.y
                            if (kotlin.math.abs(totalDy) > slopPx) {
                                val off = (startOff - (totalDy / cellH).toInt())
                                    .coerceIn(0, session.scrollMax())
                                if (off != scrollOffset) {
                                    scrollOffset = off
                                    session.scrollTo(off)
                                    android.util.Log.i("mterm", "scroll off=$off max=${session.scrollMax()}")
                                    onScroll()
                                }
                            }
                        },
                        onDragEnd = {
                            if (kotlin.math.abs(totalDy) <= slopPx) showKeyboard()
                        },
                        onDragCancel = {
                            if (kotlin.math.abs(totalDy) <= slopPx) showKeyboard()
                        },
                    )
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
                            if (cur == prev) return
                            // Diff prefix-retype: cari common prefix, hapus sisanya,
                            // ketik ulang bagian baru. Aman untuk autocorrect/IME
                            // yang mengganti teks di tengah (bukan cuma ekor).
                            var p = 0
                            while (p < prev.length && p < cur.length && prev[p] == cur[p]) p++
                            val del = prev.length - p
                            val ins = cur.substring(p)
                            repeat(del) { session.input(byteArrayOf(0x7f.toByte())) }
                            if (ins.isNotEmpty()) {
                                val hasNl = ins.indexOf('\n') >= 0
                                session.input(ins.replace("\n", "\r").toByteArray(Charsets.UTF_8))
                                if (hasNl) {
                                    suppress = true
                                    setText("")
                                    suppress = false
                                    prev = ""
                                    return
                                }
                            }
                            prev = cur
                        }
                    })
                    setOnKeyListener { _, keyCode, event ->
                        if (event?.action == KeyEvent.ACTION_DOWN) {
                            when (keyCode) {
                                KeyEvent.KEYCODE_ESCAPE -> {
                                    session.input("\u001b".toByteArray())
                                    true
                                }
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