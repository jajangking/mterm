package com.mterm.app

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent

/** Helper notifikasi foreground service (satu channel, tidak mencolok). */
object NotificationHelper {
    private const val CHANNEL_ID = "mterm_session"
    private const val NOTIF_ID = 1

    fun ensureChannel(ctx: Context) {
        val nm = ctx.getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
        if (nm.getNotificationChannel(CHANNEL_ID) == null) {
            nm.createNotificationChannel(
                NotificationChannel(
                    CHANNEL_ID,
                    "mterm sessions",
                    NotificationManager.IMPORTANCE_LOW,
                )
            )
        }
    }

    fun build(ctx: Context): Notification {
        ensureChannel(ctx)
        val open = PendingIntent.getActivity(
            ctx, 0, Intent(ctx, MainActivity::class.java),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        return Notification.Builder(ctx, CHANNEL_ID)
            .setContentTitle("mterm")
            .setContentText("Sesi terminal aktif")
            .setOngoing(true)
            .setContentIntent(open)
            .build()
    }
}