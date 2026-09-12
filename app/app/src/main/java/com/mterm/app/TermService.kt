package com.mterm.app

import android.app.Service
import android.content.Intent
import android.os.IBinder

/**
 * Foreground service: menjaga sesi terminal hidup (session persistence, Fase 4).
 * Stub — implementasi keep-alive + PTY manager menyusul.
 */
class TermService : Service() {
    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        startForeground(1, NotificationHelper.build(this))
        return START_STICKY
    }
}