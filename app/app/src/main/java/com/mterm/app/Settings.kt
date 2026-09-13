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
    val theme: String = "system", // "system" | "light" | "dark"
    val accent: Int = DEFAULT_ACCENT,
    val profile: String = "default",
)

const val THEME_SYSTEM = "system"
const val THEME_LIGHT = "light"
const val THEME_DARK = "dark"

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
                theme = prefs.getString("theme", null)
                    // migrasi dari field lama `dark` (Boolean)
                    ?: if (prefs.contains("dark")) {
                        if (prefs.getBoolean("dark", true)) THEME_DARK else THEME_LIGHT
                    } else {
                        THEME_SYSTEM
                    },
                accent = prefs.getInt("accent", DEFAULT_ACCENT),
                profile = prefs.getString("profile", "default") ?: "default",
            )
        )
    }
    holder.onSave = { s ->
        holder.value = s
        prefs.edit()
            .putFloat("font_sp", s.fontSp)
            .putString("theme", s.theme)
            .putInt("accent", s.accent)
            .putString("profile", s.profile)
            .remove("dark")
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
    dark: Boolean,
    onChange: (MtermSettings) -> Unit,
    onClose: () -> Unit,
) {
    val res = LocalContext.current.resources
    Dialog(onDismissRequest = onClose) {
        Surface(
            shape = RoundedCornerShape(16.dp),
            color = if (dark) Color(0xFF15151A) else Color(0xFFFFFFFF),
        ) {
            Column(Modifier.padding(20.dp).fillMaxWidth()) {
                Text(
                    res.getString(R.string.settings_title),
                    fontFamily = FontFamily.Monospace,
                    fontSize = 18.sp,
                    color = MaterialTheme.colorScheme.onSurface,
                )
                Spacer(Modifier.height(12.dp))

                Text(
                    res.getString(R.string.settings_font),
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
                        res.getString(R.string.settings_theme),
                        fontFamily = FontFamily.Monospace,
                        fontSize = 14.sp,
                        modifier = Modifier.weight(1f),
                        color = MaterialTheme.colorScheme.onSurface,
                    )
                }
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    val opts = listOf(
                        THEME_SYSTEM to res.getString(R.string.settings_theme_system),
                        THEME_LIGHT to res.getString(R.string.settings_theme_light),
                        THEME_DARK to res.getString(R.string.settings_theme_dark),
                    )
                    opts.forEach { (key, label) ->
                        val selected = settings.theme == key
                        val chipColor =
                            if (selected) Color(settings.accent).copy(alpha = 0.18f)
                            else MaterialTheme.colorScheme.surface.copy(alpha = 0.6f)
                        val textColor =
                            if (selected) Color(settings.accent)
                            else MaterialTheme.colorScheme.onSurface.copy(alpha = 0.7f)
                        Surface(
                            shape = RoundedCornerShape(8.dp),
                            color = chipColor,
                            modifier = Modifier.clickable {
                                onChange(settings.copy(theme = key))
                            },
                        ) {
                            Text(
                                label,
                                fontFamily = FontFamily.Monospace,
                                fontSize = 12.sp,
                                color = textColor,
                                modifier = Modifier.padding(horizontal = 10.dp, vertical = 5.dp),
                            )
                        }
                    }
                }

                Spacer(Modifier.height(6.dp))
                Text(
                    res.getString(R.string.settings_accent),
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
                        ) {}
                    }
                }

                Spacer(Modifier.height(10.dp))
                OutlinedTextField(
                    value = settings.profile,
                    onValueChange = { onChange(settings.copy(profile = it)) },
                    label = { Text(res.getString(R.string.settings_profile), fontFamily = FontFamily.Monospace) },
                    singleLine = true,
                    modifier = Modifier.fillMaxWidth(),
                )

                Spacer(Modifier.height(14.dp))
                Row(
                    horizontalArrangement = Arrangement.End,
                    modifier = Modifier.fillMaxWidth(),
                ) {
                    Text(
                        res.getString(R.string.settings_close),
                        fontFamily = FontFamily.Monospace,
                        fontSize = 14.sp,
                        color = Color(settings.accent),
                        modifier = Modifier
                            .padding(4.dp)
                            .clickable(onClick = onClose),
                    )
                    Spacer(Modifier.width(16.dp))
                    Text(
                        res.getString(R.string.settings_reset),
                        fontFamily = FontFamily.Monospace,
                        fontSize = 14.sp,
                        color = MaterialTheme.colorScheme.onSurface.copy(alpha = 0.6f),
                        modifier = Modifier
                            .padding(4.dp)
                            .clickable {
                                onChange(
                                    MtermSettings(
                                        fontSp = 13f,
                                        theme = THEME_SYSTEM,
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