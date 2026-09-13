package com.mterm.app

import android.content.Context
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.shape.RoundedCornerShape
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
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp

/** Flag onboarding di SharedPreferences (+ key prefs). */
private const val ONB_FLAG = "onboarding_done"

/** True kalau onboarding sudah pernah ditutup. */
fun isOnboarded(ctx: Context): Boolean =
    ctx.getSharedPreferences("mterm_settings", Context.MODE_PRIVATE)
        .getBoolean(ONB_FLAG, false)

fun markOnboarded(ctx: Context) {
    ctx.getSharedPreferences("mterm_settings", Context.MODE_PRIVATE)
        .edit()
        .putBoolean(ONB_FLAG, true)
        .apply()
}

/**
 * Layar onboarding (first launch): perkenalan dan fitur utama. Dismiss
 * permanen lewat markOnboarded — ditampilkan lagi hanya setelah data
 * aplikasi dihapus.
 */
@Composable
fun OnboardingOverlay(
    dark: Boolean,
    accent: Int,
    onDone: () -> Unit,
) {
    val ctx = androidx.compose.ui.platform.LocalContext.current
    var done by remember { mutableStateOf(isOnboarded(ctx)) }
    if (done) return
    val res = ctx.resources
    val bg = if (dark) Color(0xFF101014) else Color(0xFFF6F5F1)
    Box(
        Modifier
            .fillMaxSize()
            .background(bg),
        contentAlignment = Alignment.Center,
    ) {
        Surface(
            shape = RoundedCornerShape(20.dp),
            color = if (dark) Color(0xFF1B1B28) else Color(0xFFFFFFFF),
            modifier = Modifier
                .fillMaxWidth()
                .padding(horizontal = 28.dp),
        ) {
            Column(
                Modifier.padding(horizontal = 24.dp, vertical = 28.dp),
                horizontalAlignment = Alignment.CenterHorizontally,
            ) {
                Text(
                    res.getString(R.string.onb_welcome),
                    fontFamily = FontFamily.Monospace,
                    fontSize = 30.sp,
                    color = Color(accent),
                )
                Spacer(Modifier.height(8.dp))
                Text(
                    res.getString(R.string.onb_desc),
                    fontFamily = FontFamily.Monospace,
                    fontSize = 13.sp,
                    color = if (dark) Color(0xFFE0E0E0) else Color(0xFF303038),
                    textAlign = TextAlign.Center,
                )
                Spacer(Modifier.height(18.dp))
                listOf(
                    res.getString(R.string.onb_f1),
                    res.getString(R.string.onb_f2),
                    res.getString(R.string.onb_f3),
                ).forEach { f ->
                    Text(
                        "•  $f",
                        fontFamily = FontFamily.Monospace,
                        fontSize = 12.sp,
                        color = if (dark) Color(0xFFC0C0CC) else Color(0xFF4A4A56),
                        modifier = Modifier
                            .fillMaxWidth()
                            .padding(vertical = 4.dp),
                    )
                }
                Spacer(Modifier.height(20.dp))
                Text(
                    res.getString(R.string.onb_start),
                    fontFamily = FontFamily.Monospace,
                    fontSize = 15.sp,
                    color = Color.Black,
                    modifier = Modifier
                        .background(Color(accent), RoundedCornerShape(10.dp))
                        .clickable {
                            markOnboarded(ctx)
                            done = true
                            onDone()
                        }
                        .padding(horizontal = 26.dp, vertical = 10.dp),
                )
            }
        }
    }
}