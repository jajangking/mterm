package com.mterm.app

import android.content.Context
import android.util.Log
import java.io.File

/**
 * Fase B: siapkan biner mterm + proot di filesDir app.
 *
 * Assets `assets/mterm/bin|lib` disalin ke `filesDir/mterm`, agar `mterm`
 * (distro install/login) dan `proot` tersedia dari shell app. PATH + 
 * LD_LIBRARY_PATH diberikan lewat wrapper `sh` yang dipakai MainActivity.
 */
object MtermBootstrap {
    private const val TAG = "mterm"

    /** Jalur `sh` wrapper yang export PATH bundle lalu exec /system/bin/sh. */
    fun shellWrapper(ctx: Context): String {
        val base = File(ctx.filesDir, "mterm")
        val bin = File(base, "bin")
        val lib = File(base, "lib")

        if (!File(bin, "mterm").exists() || !File(bin, "proot").exists()) {
            bin.mkdirs()
            lib.mkdirs()
            copyAssetDir(ctx, "mterm/bin", bin)
            copyAssetDir(ctx, "mterm/lib", lib)
            bin.listFiles()?.forEach { it.setExecutable(true) }
            Log.i(TAG, "bundled mterm+proot siap di ${bin.path}")
        }

        val sh = File(bin, "sh")
        val home = File(base, "home")
        val tmp = File(home, "tmp")
        if (!sh.exists()) {
            sh.writeText(
                "#!/system/bin/sh\n" +
                    "export PATH=$bin:/system/bin:/system/xbin:/sbin:\$PATH\n" +
                    "export LD_LIBRARY_PATH=$lib:\$LD_LIBRARY_PATH\n" +
                    "export HOME=$home\n" +
                    "export TMPDIR=$tmp\n" +
                    "mkdir -p \"$home\" \"$tmp\"\n" +
                    "exec /system/bin/sh \"\$@\"\n"
            )
            sh.setExecutable(true)
        }
        home.mkdirs()
        return sh.path
    }

    private fun copyAssetDir(ctx: Context, assetPath: String, dest: File) {
        try {
            // Ambil daftar file asset via AssetManager (daftar langsung,
            // bukan rekursif — struktur kita flat: bin/*, lib/*).
            val names = ctx.assets.list(assetPath) ?: emptyArray()
            for (name in names) {
                ctx.assets.open("$assetPath/$name").use { input ->
                    val out = File(dest, name)
                    out.outputStream().use { input.copyTo(it) }
                    out.setExecutable(true)
                }
            }
        } catch (e: Exception) {
            Log.e(TAG, "gagal salin asset $assetPath: ${e.message}")
        }
    }
}