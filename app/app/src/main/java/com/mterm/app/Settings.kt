package com.mterm.app

import android.content.Context
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Slider
import androidx.compose.material3.Surface
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.window.Dialog

/** Preferensi mterm yang persist lewat SharedPreferences. */
data class MtermSettings(
    val fontSp: Float = 13f,
    val dark: Boolean = true,
    val accent: Int = DEFAULT_ACCENT,
    val profile: String = "default",
)

const val DEFAULT_ACCENT = 0xFF00E5A0.toInt()

/** Swatch warna aksen pilihan. */
val ACCENT_PRESETS: List<Pair<String, Int>> = listOf(
    "hijau" to 0xFF00E5A0.toInt(),
    "cyan" to 0xFF00B0E0.toInt(),
    "amber" to 0xFFFFB300.toInt(),
    "merah" to 0xFFE5503A.toInt(),
    "ungu" to 0xFFB060D0.toInt(),
)

class SettingsHolder(initial: MtermSettings) {
    var value by mutableStateOf(initial)
    lateinit var onSave: (MtermSettings) -> Unit
}

@Composable
fun rememberSettings(): SettingsHolder {
    val ctx = LocalContext.current
    val prefs = remember {
        ctx.getSharedPreferences("mterm_settings", Context.MODE_PRIVATE)
    }
    val holder = remember {
        SettingsHolder(
            MtermSettings(
                fontSp = prefs.getFloat("font_sp", 13f),
                dark = prefs.getBoolean("dark", true),
                accent = prefs.getInt("accent", DEFAULT_ACCENT),
                profile = prefs.getString("profile", "default") ?: "default",
            )
        )
    }
    holder.onSave = { s ->
        holder.value = s
        prefs.edit()
            .putFloat("font_sp", s.fontSp)
            .putBoolean("dark", s.dark)
            .putInt("accent", s.accent)
            .putString("profile", s.profile)
            .apply()
    }
    return holder
}

/**
 * Panel settings: font, tema gelap/terang, warna aksen, nama profil.
 * Ditampilkan sebagai dialog di atas terminal.
 */
@Composable
fun SettingsPanel(
    settings: MtermSettings,
    onChange: (MtermSettings) -> Unit,
    onClose: () -> Unit,
) {
    Dialog(onDismissRequest = onClose) {
        Surface(
            shape = RoundedCornerShape(16.dp),
            color = if (settings.dark) Color(0xFF15151A) else Color(0xFFFFFFFF),
        ) {
            Column(Modifier.padding(20.dp).fillMaxWidth()) {
                Text(
                    "Settings",
                    fontFamily = FontFamily.Monospace,
                    fontSize = 18.sp,
                    color = MaterialTheme.colorScheme.onSurface,
                )
                Spacer(Modifier.height(12.dp))

                Text(
                    "Ukuran font",
                    fontFamily = FontFamily.Monospace,
                    fontSize = 12.sp,
                    color = MaterialTheme.colorScheme.onSurface.copy(alpha = 0.7f),
                )
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Slider(
                        value = settings.fontSp,
                        onValueChange = {
                            onChange(settings.copy(fontSp = (it * 2f).toInt() / 2f))
                        },
                        valueRange = 9f..20f,
                        steps = 21,
                        modifier = Modifier.weight(1f),
                    )
                    Text(
                        "${settings.fontSp.toInt()}",
                        fontFamily = FontFamily.Monospace,
                        fontSize = 14.sp,
                        color = Color(settings.accent),
                        modifier = Modifier.padding(start = 8.dp),
                    )
                }

                Row(
                    verticalAlignment = Alignment.CenterVertically,
                    modifier = Modifier.fillMaxWidth(),
                ) {
                    Text(
                        "Tema gelap",
                        fontFamily = FontFamily.Monospace,
                        fontSize = 14.sp,
                        modifier = Modifier.weight(1f),
                        color = MaterialTheme.colorScheme.onSurface,
                    )
                    Switch(
                        checked = settings.dark,
                        onCheckedChange = { onChange(settings.copy(dark = it)) },
                    )
                }

                Spacer(Modifier.height(6.dp))
                Text(
                    "Warna aksen",
                    fontFamily = FontFamily.Monospace,
                    fontSize = 12.sp,
                    color = MaterialTheme.colorScheme.onSurface.copy(alpha = 0.7f),
                )
                Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                    ACCENT_PRESETS.forEach { (name, rgb) ->
                        val selected = rgb == settings.accent
                        val borderColor = if (selected) Color(settings.accent) else Color.Transparent
                        Box(
                            Modifier
                                .size(28.dp)
                                .background(Color(rgb), CircleShape)
                                .border(2.dp, borderColor, CircleShape)
                                .clickable {
                                    onChange(settings.copy(accent = rgb))
                                },
                            contentAlignment = Alignment.Center,
                        )
                    }
                }

                Spacer(Modifier.height(10.dp))
                OutlinedTextField(
                    value = settings.profile,
                    onValueChange = { onChange(settings.copy(profile = it)) },
                    label = { Text("Nama profil", fontFamily = FontFamily.Monospace) },
                    singleLine = true,
                    modifier = Modifier.fillMaxWidth(),
                )

                Spacer(Modifier.height(14.dp))
                Row(
                    horizontalArrangement = Arrangement.End,
                    modifier = Modifier.fillMaxWidth(),
                ) {
                    Text(
                        "Tutup",
                        fontFamily = FontFamily.Monospace,
                        fontSize = 14.sp,
                        color = Color(settings.accent),
                        modifier = Modifier
                            .padding(4.dp)
                            .clickable(onClick = onClose),
                    )
                    Spacer(Modifier.width(16.dp))
                    Text(
                        "Reset",
                        fontFamily = FontFamily.Monospace,
                        fontSize = 14.sp,
                        color = MaterialTheme.colorScheme.onSurface.copy(alpha = 0.6f),
                        modifier = Modifier
                            .padding(4.dp)
                            .clickable {
                                onChange(
                                    MtermSettings(
                                        fontSp = 13f,
                                        dark = true,
                                        accent = DEFAULT_ACCENT,
                                        profile = "default",
                                    )
                                )
                            },
                    )
                }
            }
        }
    }
}