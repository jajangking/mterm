package com.mterm.app

import android.os.SystemClock

object NativeTerm {
    init {
        System.loadLibrary("mterm_jni")
    }

    external fun nativeInit(cols: Int, rows: Int): Long

    external fun nativeDestroy(handle: Long)
    external fun nativeWrite(handle: Long, bytes: ByteArray)
    external fun nativeResize(handle: Long, cols: Int, rows: Int)

    external fun nativeCellAt(handle: Long, x: Int, y: Int, out: ByteArray): Boolean
    external fun nativeDirty(handle: Long): Boolean
    external fun nativeMouseEnabled(handle: Long): Boolean
    external fun nativeTakeEvent(handle: Long, out: ByteArray): Int
    external fun nativeGridText(handle: Long): String

    // EmuRunner (thread model Fase 2): spawn PTY + feed engine di thread Rust.
    external fun nativeSessionStart(
        handle: Long,
        cmd: String,
        args: Array<String>,
        cwd: String,
        cols: Int,
        rows: Int,
    ): Boolean

    external fun nativeRunnerStop(handle: Long)
    external fun nativeRunnerExit(handle: Long): Int
    external fun nativeRunnerInput(handle: Long, bytes: ByteArray): Boolean
    external fun nativeRunnerResize(handle: Long, cols: Int, rows: Int): Boolean

    // Session persistence (Fase 4)
    external fun nativeSaveState(handle: Long, path: String): Boolean
    external fun nativeLoadState(path: String): Long

    // Viewport scrollback (Fase 3 scroll/selection)
    external fun nativeScrollOffset(handle: Long, offset: Int)
    external fun nativeScrollMax(handle: Long): Int

    // Mouse input → SGR escape (dikirim ke shell via runnerInput).
    external fun nativeSgrMouse(
        handle: Long,
        code: Int,
        mods: Int,
        release: Boolean,
        x: Int,
        y: Int,
        out: ByteArray,
    ): Int
}

/** Event bukan-render dari Rust core: title, bell, mouse tracking. */
sealed class TermEvent {
    data class Title(val value: String) : TermEvent()
    data class Mouse(val enabled: Boolean) : TermEvent()
    object Bell : TermEvent()

    companion object {
        /** Decode buffer `[type:u32][len:u32][payload]` dari `nativeTakeEvent`. */
        fun decode(buf: ByteArray, n: Int): TermEvent? {
            if (n < 8) return null
            val type = buf.readLE(0).toInt()
            val len = buf.readLE(4).toInt()
            if (n < 8 + len) return null
            return when (type) {
                1 -> TermEvent.Title(String(buf, 8, len, Charsets.UTF_8))
                2 -> TermEvent.Bell
                3 -> TermEvent.Mouse(buf[8].toInt() != 0)
                else -> null
            }
        }

        private fun ByteArray.readLE(off: Int): Long {
            return (this[off].toLong() and 0xFF) or
                ((this[off + 1].toLong() and 0xFF) shl 8) or
                ((this[off + 2].toLong() and 0xFF) shl 16) or
                ((this[off + 3].toLong() and 0xFF) shl 24)
        }
    }
}

data class TermCell(
    val fg: Int,
    val bg: Int,
    val ch: Char,
)

class TermSession(private val handle: Long) {
    /** Warna polyfill default (JNI `0` = warna tak eksplisit). Variabel biar
     *  bisa ganti sesama runtime (tema gelap/terang di settings). */
    var defaultBg: Int = 0xFF1B1B1F.toInt()
    var defaultFg: Int = 0xFFE0E0E0.toInt()

    fun write(bytes: ByteArray) = NativeTerm.nativeWrite(handle, bytes)

    fun resize(cols: Int, rows: Int) = NativeTerm.nativeResize(handle, cols, rows)

    /** Debug: isi grid saat ini sebagai teks (per baris). */
    fun gridText(): String = NativeTerm.nativeGridText(handle)

    /** Spawn PTY + Rust emu thread yang feed ke terminal handle ini; shell
     *  mulai dari CWD `cwd` (mis. folder data app). */
    fun startSession(
        cmd: String,
        args: Array<String>? = null,
        cwd: String,
        cols: Int,
        rows: Int,
    ): Boolean = NativeTerm.nativeSessionStart(handle, cmd, args ?: arrayOf(), cwd, cols, rows)

    /** Keystroke/user input → shell PTY. Tampilan mengikuti echo PTY shell
     *  (default ON), jadi tidak perlu feed engine langsung lagi — feed ganda
     *  dulu bikin setiap karakter tampil 2x. Bonus: `stty -echo` (password)
     *  tidak lagi bocor ke layar. */
    fun input(bytes: ByteArray): Boolean {
        val ok = NativeTerm.nativeRunnerInput(handle, bytes)
        android.util.Log.i(
            "mterm",
            "input ${bytes.size}b ok=$ok bytes=${bytes.map { it.toInt() and 0xff }}"
        )
        return ok
    }

    /** -1 = masih jalan; >=0 = exit code; negatif lain = sinyal. */
    fun sessionExit(): Int = NativeTerm.nativeRunnerExit(handle)

    fun stopSession() = NativeTerm.nativeRunnerStop(handle)

    /** Resize engine + PTY winsize biar sinkron. */
    fun sessionResize(cols: Int, rows: Int): Boolean =
        NativeTerm.nativeRunnerResize(handle, cols, rows)

    /** Scroll viewport ke offset baris (0 = ikut bottom). */
    fun scrollTo(offset: Int) = NativeTerm.nativeScrollOffset(handle, offset)

    /** Batas scrollback yang bisa dilihat. */
    fun scrollMax(): Int = NativeTerm.nativeScrollMax(handle)

    /**
     * Encode mouse/touch → byte SGR. Kirim hasilnya ke [input]. Kembalikan
     * ByteArray kosong kalau mode mouse belum aktif di TUI.
     */
    fun sgrMouse(code: Int, mods: Int, release: Boolean, x: Int, y: Int): ByteArray {
        val out = ByteArray(32)
        val n = NativeTerm.nativeSgrMouse(handle, code, mods, release, x, y, out)
        return if (n > 0) out.copyOf(n) else ByteArray(0)
    }

    /** Simpan state terminal ke file (grid, scrollback, cursor, mode). */
    fun saveState(path: String): Boolean = NativeTerm.nativeSaveState(handle, path)

    /** Restore terminal dari file. Kembalikan handle baru atau -1 gagal. */
    fun loadState(path: String): Long = NativeTerm.nativeLoadState(path)

    /** Ambil event non-render berikutnya (title/bell); null kalau kosong. */
    fun takeEvent(): TermEvent? {
        val buf = ByteArray(4096)
        val n = NativeTerm.nativeTakeEvent(handle, buf)
        return TermEvent.decode(buf, n)
    }

    /** Ambil isi layar menjadi buffer pixel ARGB (width*height u32). */
    fun snapshot(cols: Int, rows: Int): IntArray {
        val out = ByteArray(12)
        val pixels = IntArray(cols * rows)
        for (y in 0 until rows) {
            for (x in 0 until cols) {
                val ok = NativeTerm.nativeCellAt(handle, x, y, out)
                if (!ok) continue
                val fg = readLE(out, 0)
                val bg = readLE(out, 4)
                val ch = readLE(out, 8).toChar()
                pixels[y * cols + x] = paint(fg, bg, ch)
            }
        }
        return pixels
    }

    /** Pewarna default (`0` dari JNI) → polyfill per-tema. */
    private fun color(c: Int): Int = if ((c ushr 24) == 0) {
        if (c == 0) defaultBg else c or 0xFF000000.toInt()
    } else c

    /** Ambil satu sel: (fg ARGB, bg ARGB, char). Warna tak-eksplisit
     *  (alpha 0 dari JNI) di-polyfill sesuai tema aktif. */
    fun cellAt(x: Int, y: Int): Triple<Int, Int, Char>? {
        val out = ByteArray(12)
        if (!NativeTerm.nativeCellAt(handle, x, y, out)) return null
        val fg = readLE(out, 0)
        val bg = readLE(out, 4)
        return Triple(
            if ((fg ushr 24) == 0) defaultFg else fg,
            if ((bg ushr 24) == 0) defaultBg else bg,
            readLE(out, 8).toChar(),
        )
    }

    fun dirty(): Boolean = NativeTerm.nativeDirty(handle)

    /** Truth mode mouse (SGR atau tracking lama) langsung dari engine. */
    fun mouseEnabled(): Boolean = NativeTerm.nativeMouseEnabled(handle)

    private fun readLE(b: ByteArray, off: Int): Int {
        return (b[off].toInt() and 0xFF) or
            ((b[off + 1].toInt() and 0xFF) shl 8) or
            ((b[off + 2].toInt() and 0xFF) shl 16) or
            ((b[off + 3].toInt() and 0xFF) shl 24)
    }

    /**
     * Warna default (`0` dari JNI — tak ada warna) → polyfill: bg gelap,
     * fg terang.
     */
    private fun paint(fg: Int, bg: Int, ch: Char): Int {
        val b = if ((bg ushr 24) == 0) defaultBg else bg
        val f = if ((fg ushr 24) == 0) defaultFg else fg
        return if (ch == ' ') b else f
    }
}

/**
 * Kotlin-aware JNI loader agar ekspor `extern "C"` Rust bisa di-debug
 * lewat logcat (lihat scripts/adb-wireless.sh logcat).
 */
object TermTimer {
    fun nowMs(): Long = SystemClock.elapsedRealtime()
}