package com.mterm.app

import android.app.Service
import android.content.Intent
import android.os.IBinder

/**
 * Foreground service: menjaga sesi terminal hidup (session persistence, Fase 4).
 * Stub — implementasi keep-alive + PTY manager menyusul.
 */
class TermService : Service() {
    private var started = false

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (!started) {
            startForeground(1, NotificationHelper.build(this))
            started = true
        }
        // START_STICKY: kalau sistem membunuh service, coba hidupkan lagi →
        // proses tetap dikunci hidup biar PTY (thread Rust) jalan terus.
        return START_STICKY
    }
}