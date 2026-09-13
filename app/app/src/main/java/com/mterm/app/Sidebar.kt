package com.mterm.app

import android.util.Log
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import java.io.File

/** Sisa file untuk preview (batasi biar nggak baca file raksasa). */
private const val PREVIEW_MAX_BYTES = 256_000
private const val PREVIEW_MAX_LINES = 300

/**
 * Sidebar drawer (dari kiri): header cwd, status git (kalau git ada di
 * device), dan browser file read-only + preview teks ringan.
 */
@Composable
fun SidebarDrawer(
    cwd: String,
    accent: Int,
    dark: Boolean,
    fontSp: Float,
    onClose: () -> Unit,
) {
    val pageColor = if (dark) Color(0xFF15151A) else Color.White
    val textColor = MaterialTheme.colorScheme.onSurface

    Box(
        Modifier
            .fillMaxSize()
            .background(Color.Black.copy(alpha = 0.45f))
            .clickable(onClick = onClose)
    ) {
        Surface(
            modifier = Modifier
                .fillMaxHeight()
                .width(320.dp),
            color = pageColor,
            shape = RoundedCornerShape(topEnd = 14.dp, bottomEnd = 14.dp),
        ) {
            var previewFile by remember { mutableStateOf<File?>(null) }
            var previewText by remember { mutableStateOf<String?>(null) }
            val gitStatus = rememberGitStatus(cwd)

            if (previewFile == null || previewText == null) {
                Column(Modifier.padding(14.dp)) {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Text(
                            "☰",
                            fontSize = 18.sp,
                            color = Color(accent),
                            modifier = Modifier.padding(end = 8.dp),
                        )
                        Text(
                            "Sidebar",
                            fontFamily = FontFamily.Monospace,
                            fontSize = 15.sp,
                            color = textColor,
                            modifier = Modifier.weight(1f),
                        )
                        Text(
                            "✕",
                            fontSize = 16.sp,
                            color = textColor.copy(alpha = 0.6f),
                            modifier = Modifier.clickable(onClick = onClose),
                        )
                    }
                    Spacer(Modifier.height(8.dp))
                    Text(
                        "cwd: $cwd",
                        fontFamily = FontFamily.Monospace,
                        fontSize = 10.sp,
                        color = textColor.copy(alpha = 0.55f),
                    )
                    Spacer(Modifier.height(10.dp))

                    if (gitStatus != null) {
                        for (line in gitStatus) {
                            Text(
                                line,
                                fontFamily = FontFamily.Monospace,
                                fontSize = 10.sp,
                                color = Color(accent),
                            )
                        }
                    } else {
                        Text(
                            "git: tidak tersedia di device ini",
                            fontFamily = FontFamily.Monospace,
                            fontSize = 10.sp,
                            color = textColor.copy(alpha = 0.5f),
                        )
                    }
                    Spacer(Modifier.height(10.dp))

                    val root = File(cwd)
                    val entries = remember(root) {
                        listFilesDeep(root, depth = 0, maxDepth = 2)
                    }
                    LazyColumn {
                        items(entries) { (indent, f) ->
                            val prefix = "  ".repeat(indent)
                            val icon = if (f.isDirectory) "▸ " else "  "
                            Text(
                                "$prefix$icon${f.name}${if (f.isDirectory) "/" else ""}",
                                fontFamily = FontFamily.Monospace,
                                fontSize = 11.sp,
                                color = if (f.isDirectory) Color(accent) else textColor,
                                modifier = Modifier
                                    .fillMaxWidth()
                                    .clickable {
                                        if (f.isFile) {
                                            previewFile = f
                                            previewText = readPreview(f)
                                        }
                                    }
                                    .padding(vertical = 3.dp),
                            )
                        }
                    }
                }
            } else {
                Column(Modifier.padding(14.dp)) {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Text(
                            "‹",
                            fontSize = 20.sp,
                            color = Color(accent),
                            modifier = Modifier
                                .clickable { previewFile = null; previewText = null }
                                .padding(end = 8.dp),
                        )
                        Text(
                            previewFile?.name ?: "",
                            fontFamily = FontFamily.Monospace,
                            fontSize = 13.sp,
                            color = textColor,
                            modifier = Modifier.weight(1f),
                            maxLines = 1,
                        )
                        Text(
                            "✕",
                            fontSize = 16.sp,
                            color = textColor.copy(alpha = 0.6f),
                            modifier = Modifier.clickable(onClick = onClose),
                        )
                    }
                    Spacer(Modifier.height(8.dp))
                    Text(
                        previewText!!,
                        fontFamily = FontFamily.Monospace,
                        fontSize = fontSp,
                        lineHeight = fontSp * 1.25f,
                        color = if (dark) Color(0xFFD6D6D6) else Color(0xFF222222),
                        modifier = Modifier.fillMaxSize(),
                    )
                }
            }
        }
    }
}

/** Coba jalan `git -C <cwd> status --porcelain`; null kalau git absen. */
@Composable
fun rememberGitStatus(cwd: String): List<String>? {
    return remember(cwd) {
        runCatching {
            val p = ProcessBuilder("git", "-C", cwd, "status", "--porcelain")
                .redirectErrorStream(true)
                .start()
            val out = p.inputStream.readBytes().toString(Charsets.UTF_8)
            p.waitFor()
            val lines = out.lines().filter { it.isNotBlank() }
            if (lines.isEmpty()) listOf("git: bersih (no changes)") else lines
        }.getOrElse { e ->
            Log.i("mterm", "git gagal: ${e.message}")
            null
        }
    }
}

/** Daftarkan pohon folder (dir dibuka, file didaftar) sampai maxDepth. */
private fun listFilesDeep(
    dir: File,
    depth: Int,
    maxDepth: Int,
): List<Pair<Int, File>> {
    val out = mutableListOf<Pair<Int, File>>()
    val children = runCatching { dir.listFiles()?.sortedBy { it.name } }.getOrNull() ?: return out
    for (c in children) {
        out.add(depth to c)
        if (c.isDirectory && depth < maxDepth) {
            out += listFilesDeep(c, depth + 1, maxDepth)
        }
    }
    return out
}

/** Baca file untuk preview: batas byte & banyak baris. */
private fun readPreview(f: File): String {
    val bytes = runCatching { f.inputStream().use { it.readBytes() } }
        .getOrNull() ?: return "(gagal baca file)"
    val text = bytes.copyOf(minOf(bytes.size, PREVIEW_MAX_BYTES))
        .toString(Charsets.UTF_8)
    val lines = text.lines().take(PREVIEW_MAX_LINES)
    return lines.joinToString("\n") +
        if (bytes.size > PREVIEW_MAX_BYTES || lines.size == PREVIEW_MAX_LINES) {
            "\n…(terpotong)"
        } else {
            ""
        }
}