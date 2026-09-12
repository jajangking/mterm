package com.mterm.app

import android.os.SystemClock
import androidx.annotation.NativeMethods

object NativeTerm {
    init {
        System.loadLibrary("mterm_jni")
    }

    @NativeMethods
    external fun nativeInit(cols: Int, rows: Int): Long

    external fun nativeDestroy(handle: Long)
    external fun nativeWrite(handle: Long, bytes: ByteArray)
    external fun nativeResize(handle: Long, cols: Int, rows: Int)

    external fun nativeCellAt(handle: Long, x: Int, y: Int, out: ByteArray): Boolean
    external fun nativeDirty(handle: Long): Boolean
}

data class TermCell(
    val fg: Int,
    val bg: Int,
    val ch: Char,
)

class TermSession(private val handle: Long) {
    fun write(bytes: ByteArray) = NativeTerm.nativeWrite(handle, bytes)

    fun resize(cols: Int, rows: Int) = NativeTerm.nativeResize(handle, cols, rows)

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

    fun dirty(): Boolean = NativeTerm.nativeDirty(handle)

    private fun readLE(b: ByteArray, off: Int): Int {
        return (b[off].toInt() and 0xFF) or
            ((b[off + 1].toInt() and 0xFF) shl 8) or
            ((b[off + 2].toInt() and 0xFF) shl 16) or
            ((b[off + 3].toInt() and 0xFF) shl 24)
    }

    private fun paint(fg: Int, bg: Int, ch: Char): Int {
        return if (ch == ' ') bg else bg or 0x00 or fg // stub: polyfill warna nanti via atlas font
    }
}

/**
 * Kotlin-aware JNI loader agar ekspor `extern "C"` Rust bisa di-debug
 * lewat logcat (lihat scripts/adb-wireless.sh logcat).
 */
object TermTimer {
    fun nowMs(): Long = SystemClock.elapsedRealtime()
}