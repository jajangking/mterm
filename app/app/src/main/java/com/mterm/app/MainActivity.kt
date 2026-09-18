package com.mterm.app

import android.content.Context
import android.content.Intent
import android.os.Bundle
import android.text.Editable
import android.text.InputType
import android.text.TextWatcher
import android.view.KeyEvent
import android.view.inputmethod.InputMethodManager
import android.widget.EditText
import java.io.File
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.gestures.detectDragGestures
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.clickable
import androidx.compose.foundation.background
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import kotlinx.coroutines.delay
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalViewConfiguration
import androidx.compose.ui.res.stringResource
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
        Intent(this, TermService::class.java).let { startForegroundService(it) }
        setContent {
            val settingsState = rememberSettings()
            val s = settingsState.value
            val sysDark = isSystemInDarkTheme()
            val dark = when (s.theme) {
                THEME_LIGHT -> false
                THEME_DARK -> true
                else -> sysDark
            }
            val pageColor = if (dark) Color(0xFF0B0B0D) else Color(0xFFF2F1EE)
            val scheme = if (dark) {
                darkColorScheme(
                    background = pageColor,
                    surface = Color(0xFF15151A),
                    onBackground = Color(0xFFE0E0E0),
                )
            } else {
                lightColorScheme(
                    background = pageColor,
                    surface = Color(0xFFFFFFFF),
                    onBackground = Color(0xFF101014),
                )
            }

            val tabs = TermStore.tabs
            var activeId by remember { mutableStateOf(0) }
            var settingsOpen by remember { mutableStateOf(false) }
            var sideOpen by remember { mutableStateOf(false) }
            var tick by remember { mutableStateOf(0) }
            val ctx = LocalContext.current
            val testHook = intent.getStringExtra("xtermtest")

            val activeTab = tabs.firstOrNull { it.id == activeId }

            // Selalu ada >= 1 tab; restore kalau ada session tersimpan (F4).
            LaunchedEffect(Unit) {
                TermStore.init(ctx)
                if (TermStore.tabs.isEmpty()) {
                    TermStore.newTab()
                }
                val bg = if (dark) 0xFF1B1B1F.toInt() else 0xFFFFFFFF.toInt()
                val fg = if (dark) 0xFFE0E0E0.toInt() else 0xFF101014.toInt()
                TermStore.tabs.forEach {
                    it.session.defaultBg = bg
                    it.session.defaultFg = fg
                }
                activeId = TermStore.tabs.last().id
                // Test hook (dev): --es xtermtest mouse1006 → tulis ESC[?1006h
                // ke ENGINE (mirip aplikasi TUI yang menulis ke stdout PTY).
                // Bukan input() (PTY stdin → shell) — adb input text pun tak
                // bisa mengetik byte ESC.
                if (testHook == "mouse1006") {
                    val bytes = "\u001b[?1006h".toByteArray()
                    for (i in 1..10) {
                        val t = tabs.firstOrNull { it.id == activeId } ?: break
                        if (t.session.input(bytes)) {
                            android.util.Log.i("mterm", "test-hook: PTY stdin OK (input)")
                            break
                        }
                        kotlinx.coroutines.delay(500)
                    }
                    // tunggu PTY jalan dulu, terus feed engine langsung
                    kotlinx.coroutines.delay(800)
                    val t = tabs.firstOrNull { it.id == activeId }
                    if (t != null) {
                        t.session.write(bytes)
                        android.util.Log.i("mterm", "test-hook: ESC[?1006h ke engine (write)")
                    }
                }
                // Debug hook (dev): --es mterm_debug_cmd "<shell>"
                // jalankan command di domain app asli kemudian simpan
                // stdout+stderr ke files/mterm/debug.out (baca via run-as).
                val dbgCmd = intent.getStringExtra("mterm_debug_cmd")
                if (dbgCmd != null) {
                    val out = File(ctx.filesDir, "mterm/debug.out")
                    try {
                        val pb = ProcessBuilder("/system/bin/sh", "-c", dbgCmd)
                        val base = File(ctx.filesDir, "mterm")
                        val oldPath = pb.environment().getOrDefault("PATH", "")
                        pb.environment()["PATH"] =
                            File(base, "bin").path + ":/system/bin:/system/xbin:" +
                                (if (oldPath.isEmpty()) "" else oldPath + ":")
                        pb.environment()["LD_LIBRARY_PATH"] = File(base, "lib").path
                        pb.environment()["HOME"] = File(base, "home").path
                        pb.environment()["TMPDIR"] = File(base, "home/tmp").path
                        pb.redirectErrorStream(true)
                        val p = pb.start()
                        val txt = p.inputStream.bufferedReader().readText()
                        val code = p.waitFor()
                        File(base, "home").mkdirs()
                        File(base, "home/tmp").mkdirs()
                        out.writeText("cmd=$dbgCmd\nrc=$code\n---\n$txt")
                        android.util.Log.i("mterm", "debug-cmd rc=$code -> ${out.path}")
                    } catch (e: Exception) {
                        out.writeText("cmd=$dbgCmd\nerr=${e}\n")
                        android.util.Log.e("mterm", "debug-cmd err", e)
                    }
                }
            }

            // Warna default sel terminal ikut tema; bump tick biar sel re-render.
            LaunchedEffect(dark, tabs) {
                val bg = if (dark) 0xFF1B1B1F.toInt() else 0xFFFFFFFF.toInt()
                val fg = if (dark) 0xFFE0E0E0.toInt() else 0xFF101014.toInt()
                tabs.forEach {
                    it.session.defaultBg = bg
                    it.session.defaultFg = fg
                }
                tick++
            }

            fun addTab() {
                val t = TermStore.newTab()
                t.session.defaultBg = if (dark) 0xFF1B1B1F.toInt() else 0xFFFFFFFF.toInt()
                t.session.defaultFg = if (dark) 0xFFE0E0E0.toInt() else 0xFF101014.toInt()
                activeId = t.id
            }

            fun closeTab(t: TabState) {
                if (TermStore.closeTab(t)) {
                    if (activeId == t.id) {
                        activeId = TermStore.tabs.lastOrNull()?.id ?: 0
                    }
                }
            }

            DisposableEffect(Unit) {
                onDispose {
                    // Hanya destroy saat Activity benar-benar ditutup (back), bukan
                    // saat re-create (config change): pakai isFinishing.
                    if ((ctx as? ComponentActivity)?.isFinishing == true) {
                        TermStore.destroyAll()
                    }
                }
            }

            MaterialTheme(colorScheme = scheme) {
                Surface(
                    modifier = Modifier.fillMaxSize(),
                    color = pageColor,
                ) {
                    Box(
                        Modifier
                            .fillMaxSize()
                            .statusBarsPadding()
                            .navigationBarsPadding()
                            .imePadding()
                    ) {
                        Column(Modifier.fillMaxSize()) {
                            TabBar(
                                tabs = tabs,
                                activeId = activeId,
                                accent = s.accent,
                                dark = dark,
                                onSelect = { activeId = it },
                                onAdd = { addTab() },
                                onClose = { closeTab(it) },
                                onSidebar = { sideOpen = true },
                            )
                            Box(Modifier.fillMaxSize()) {
                                if (activeTab != null) {
                                    key(activeTab.handle) {
                                        TermView(
                                            session = activeTab.session,
                                            fontSp = s.fontSp,
                                            hint = s.accent,
                                            tick = tick,
                                            onTitle = { activeTab.updateTitle(it) },
                                        )
                                    }
                                }
                            }
                        }
                        Text(
                            "⚙",
                            color = Color(s.accent),
                            fontSize = (s.fontSp + 4).sp,
                            modifier = Modifier
                                .align(Alignment.BottomStart)
                                .padding(10.dp)
                                .background(
                                    Color.Black.copy(alpha = 0.35f),
                                    CircleShape,
                                )
                                .padding(horizontal = 10.dp, vertical = 4.dp)
                                .clickable { settingsOpen = true }
                        )
                        if (sideOpen && activeTab != null) {
                            SidebarDrawer(
                                cwd = ctx.filesDir.path,
                                accent = s.accent,
                                dark = dark,
                                fontSp = s.fontSp,
                                onClose = { sideOpen = false },
                            )
                        }
                        if (settingsOpen) {
                            SettingsPanel(
                                settings = s,
                                dark = dark,
                                onChange = settingsState.onSave,
                                onClose = { settingsOpen = false },
                            )
                        }
                        if (!isOnboarded(ctx)) {
                            OnboardingOverlay(dark = dark, accent = s.accent, onDone = {})
                        }
                    }
                }
            }
        }
    }

    override fun onStop() {
        super.onStop()
        // Auto-save sesi saat app ke background (Fase 4): file dipakai bila
        // proses mati dan di-restore pada cold launch berikutnya.
        if (!isChangingConfigurations) {
            TermStore.saveAll(this)
        }
    }
}

@Composable
fun TermView(
    session: TermSession,
    fontSp: Float = 13f,
    hint: Int = 0xFF00E5A0.toInt(),
    tick: Int = 0,
    onTitle: (String) -> Unit = {},
) {
    var frame by remember { mutableStateOf(0) }

    // start_shell: PTY emulator dijalankan (sh), output → session
    val ctx = LocalContext.current
    var started by remember { mutableStateOf(false) }
    var cols by remember { mutableStateOf(80) }
    var rows by remember { mutableStateOf(24) }
    var mouseMode by remember { mutableStateOf(false) }

    DisposableEffect(session) {
        val timer = kotlin.concurrent.timer(period = 100) {
            try {
                val eng = session.mouseEnabled()
                if (eng != mouseMode) {
                    android.util.Log.i("mterm", "poll mouse=$eng (was $mouseMode)")
                    mouseMode = eng
                }
                if (session.dirty()) frame++
                while (true) {
                    val ev = session.takeEvent() ?: break
                    when (ev) {
                        is TermEvent.Mouse -> {
                            android.util.Log.i("mterm", "drain ev=$ev")
                            mouseMode = ev.enabled
                        }
                        is TermEvent.Title -> {
                            android.util.Log.i("mterm", "drain ev=$ev")
                            onTitle(ev.value)
                        }
                        TermEvent.Bell -> android.util.Log.i("mterm", "drain ev=Bell")
                    }
                }
            } catch (e: Throwable) {
                android.util.Log.e("mterm", "drain crash", e)
            }
        }
        onDispose { timer.cancel() }
    }

    BoxWithConstraints(Modifier.fillMaxSize()) {
        // Ukuran dialog dari constrain layar: font dari settings → cols/rows
        // ikut turun/naik (auto-resize PTY via LaunchedEffect di bawah).
        val density = LocalDensity.current
        val font = fontSp.sp
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
                    MtermBootstrap.shellWrapper(ctx),
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
                val line = remember(row, frame, cols, tick) {
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
            stringResource(R.string.hint_tap_keyboard),
            color = Color(hint),
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
    fun showKeyboard() {
        val v = edit ?: return
        if (!v.hasFocus()) v.requestFocus()
        val ime =
            v.context.getSystemService(Context.INPUT_METHOD_SERVICE) as InputMethodManager
        ime.showSoftInput(v, InputMethodManager.SHOW_IMPLICIT)
    }
    Box(
        Modifier
            .fillMaxSize()
            .pointerInput(session, mouseMode, cellW, cellH, slopPx) {
                // kode SGR (sinkron dgn crates/core/src/mouse.rs): BTN_LEFT=0, MOTION=32.
                fun sendMouse(code: Int, release: Boolean, x: Int, y: Int) {
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
                        onDragStart = { totalDy = 0f; startOff = scrollOffset },
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
            .pointerInput(mouseMode) {
                // Detektor tap terpisah (mode non-mouse): gestur ringan yang
                // nggak sampai ambang drag → keyboard terbuka reliably, tanpa
                // bergantung ke onDragEnd.
                if (!mouseMode) {
                    detectTapGestures(onTap = { showKeyboard() })
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