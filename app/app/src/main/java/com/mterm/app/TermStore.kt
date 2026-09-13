package com.mterm.app

import android.content.Context
import android.util.Log
import androidx.compose.runtime.mutableStateListOf
import java.io.File

/**
 * Registri sesi proses-global (Fase 4). Tab disimpan di sini (bukan di
 * Activity) supaya PTY/jaringan Rust terus hidup walau Activity di-destroy.
 *
 * Persistensi: auto-save ke `files/.sessions/session_<id>.json` saat app
 * di-background, auto-restore saat proses start berikutnya (cold launch).
 */
object TermStore {
    val tabs = mutableStateListOf<TabState>()

    private fun sessionDir(ctx: Context): File = File(ctx.filesDir, ".sessions")

    /** Load session tersimpan (kalau ada) → isi [tabs]. Kembalikan true kalau
     *  isi sesi dari file, false kalau kosong/tidak ada file. Idempoten: saat
     *  [tabs] sudah berisi, tidak melakukan apa-apa. */
    fun init(ctx: Context): Boolean {
        if (tabs.isNotEmpty()) return false
        val dir = sessionDir(ctx)
        val restored = dir.listFiles { f ->
            f.name.startsWith("session_") && f.name.endsWith(".json")
        }?.mapNotNull { f ->
            val id = f.name.removePrefix("session_").removeSuffix(".json")
                .toIntOrNull() ?: return@mapNotNull null
            val h = NativeTerm.nativeLoadState(f.absolutePath)
            if (h >= 0) TabState(id, h) else null
        }?.sortedBy { it.id }
            ?: emptyList()
        if (restored.isNotEmpty()) {
            tabs.addAll(restored)
            Log.i("mterm", "auto-restore ${restored.size} session(s) dari .sessions")
            return true
        }
        return false
    }

    fun newTab(): TabState {
        val id = (tabs.maxOfOrNull { it.id } ?: 0) + 1
        val t = TabState(id)
        tabs += t
        return t
    }

    /** Tutup tab (destroy handle Rust). Minimal 1 tab selalu dipertahankan. */
    fun closeTab(t: TabState): Boolean {
        if (tabs.size <= 1) return false
        tabs.remove(t)
        NativeTerm.nativeDestroy(t.handle)
        return true
    }

    /** Simpan seluruh tab ke files/.sessions. Dipanggil di [MainActivity.onStop]. */
    fun saveAll(ctx: Context) {
        val dir = sessionDir(ctx)
        dir.mkdirs()
        tabs.forEach { t ->
            t.session.saveState(File(dir, "session_${t.id}.json").absolutePath)
        }
        Log.i("mterm", "auto-save ${tabs.size} session(s)")
    }

    /** Destroy semua handle Rust (dipanggil saat Activity benar-benar finish). */
    fun destroyAll() {
        tabs.forEach { NativeTerm.nativeDestroy(it.handle) }
        tabs.clear()
    }
}