package com.mterm.app

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp

/** Satu tab = satu handle Rust core + PTY (nativeInit). State terminal
 *  (grid/scrollback) hidup di Rust, jadi pindah tab bisa bolak-balik
 *  tanpa kehilangan isi terminal. `restoredHandle` = handle hasil
 *  `nativeLoadState` (Fase 4 auto-restore); -1 → buat baru. */
class TabState(val id: Int, restoredHandle: Long = -1) {
    val handle: Long =
        if (restoredHandle >= 0) restoredHandle else NativeTerm.nativeInit(80, 24)
    val session = TermSession(handle)
    var title by mutableStateOf("tab $id")
        private set

    fun updateTitle(v: String) {
        if (v.isNotEmpty()) title = v
    }
}

/**
 * Bar tab: tombol sidebar (☰), tab list dengan tombol tutup (×), dan
 * tambah tab (+).
 */
@Composable
fun TabBar(
    tabs: List<TabState>,
    activeId: Int,
    accent: Int,
    dark: Boolean,
    onSelect: (Int) -> Unit,
    onAdd: () -> Unit,
    onClose: (TabState) -> Unit,
    onSidebar: () -> Unit,
) {
    Row(
        Modifier
            .fillMaxWidth()
            .height(42.dp)
            .padding(horizontal = 6.dp, vertical = 5.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(4.dp),
    ) {
        Text(
            "☰",
            fontSize = 20.sp,
            color = Color(accent),
            modifier = Modifier
                .clickable(onClick = onSidebar)
                .padding(end = 6.dp),
        )
        tabs.forEach { t ->
            val selected = t.id == activeId
            val tabColor = if (selected) Color(accent).copy(alpha = 0.18f) else Color.Transparent
            val tabText = if (selected) Color(accent) else MaterialTheme.colorScheme.onSurface
            Surface(
                shape = RoundedCornerShape(6.dp),
                color = tabColor,
                modifier = Modifier.clickable { onSelect(t.id) },
            ) {
                Row(
                    verticalAlignment = Alignment.CenterVertically,
                    modifier = Modifier.padding(horizontal = 8.dp, vertical = 4.dp),
                ) {
                    Text(
                        t.title,
                        fontFamily = FontFamily.Monospace,
                        fontSize = 12.sp,
                        color = tabText,
                        maxLines = 1,
                    )
                    Spacer(Modifier.width(6.dp))
                    Text(
                        "✕",
                        fontSize = 11.sp,
                        color = MaterialTheme.colorScheme.onSurface.copy(alpha = 0.55f),
                        modifier = Modifier
                            .clickable { onClose(t) }
                            .padding(2.dp),
                    )
                }
            }
        }
        Spacer(Modifier.weight(1f))
        Text(
            "＋",
            fontSize = 20.sp,
            color = Color(accent),
            modifier = Modifier.clickable(onClick = onAdd),
        )
        Spacer(Modifier.width(6.dp))
    }
}