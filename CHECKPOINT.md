# CHECKPOINT — mterm

> **Baca ini dulu kalau sesi terputus**. Semua state penting dan cara lanjut
> dicatat di sini agar bisa di-resume tanpa menebak-nebak.

## Status terbaru (update tiap sesi berakhir)

| Baris | Status |
|-------|--------|
| Rust core (`crates/core`) — vte + grid + ANSI | ✅ lestari, 8/8 test, 0 clippy |
| JNI bridge (`crates/jni`) | ✅ ditaruh, build OK, **belum diuji di device** |
| Android chrome (`app/`) — Compose + Gradle | ✅ scaffold, **belum di-build** (butuh SDK) |
| ADB wireless helper (`scripts/adb-wireless.sh`) | ✅ siap dipakai |
| CI workflow (`.github/workflows/ci.yml`) | ⭕ dibikin di build APK/CI |
| PTY (`crates/pty`) | ✅ dibikin + test di Termux |
| CLI (`crates/mterm-cli`) — `mterm run` / `doctor` / `profile` | ✅ dibikin |
| End-to-end engine↔PTY di Termux | ✅ diverifikasi (echo, seq 500, SGR) |

## Cara resume

```sh
cd ~/mterm
cargo test -p mterm-core 2>&1 | grep "test result"     # harus ok. 8 passed
cargo build 2>&1 | grep -E "^error|^warning"            # harus kosong
./scripts/adb-wireless.sh status                       # (opsional) cek device
```

Tiap baris selesai di **WORKMAP.md**: centang `[x]` + tulis apa yang terbukti
berjalan di **Decision Log**.

## Yang bisa dikerjakan di Termux (tanpa Android SDK)

| Item | Alasan Termux cocok |
|------|---------------------|
| PTY session manager + PID file | PTY asli ada di Termux; loop uji interaktif |
| `mterm run` demo (engine↔PTY) | validasi core depan shell asli |
| Runtime profile manager `mterm profile` | tes install/doctor node/bun lokal |
| Fuzz/benchmark core | `cargo fuzz`/`bencher` jalan di host |
| Agent IPC (#6) | socket lokal, bisa diuji lintas proses |

## Yang TIDAK bisa di Termux → biarkan ke GitHub Actions

| Item | Alasan |
|------|--------|
| Build APK (`gradlew assembleDebug`) | butuh Android SDK/NDK ~GB |
| `android-tools` release pipeline | butuh signing key + artifact host |
| Cross-arch matrix (x86_64/armeabi-v7a) | butuh target + emulator |

Bila build APK mau dilanjutkan lokal: `./scripts/build-android.sh` (butuh
`cargo ndk` + SDK; jalankan di PC/CI, bukan device-only).

## Keputusan penting (jangan diubah tanpa alasan)

1. `vte` crate dipakai untuk VT parsing — jangan rewrite.
2. Jangan pakai `pkill -f <kata>` — pakai PID file / `kill <pid>` (lihat AGENTS.md).
3. Workspace = app-private dir, tanpa proot.
4. FFI JNI cuma fungsi murni `extern "C"`; semua state `Arc<Mutex>`.
5. Renderer disuntik dari buffer Rust (bukan Spannable Java).

## Next todo yang disarankan

Run silakan lanjut sesuai WORKMAP; saran urutan setelah ini:
1. Fase 1 lanjut: alternate buffer test penuh, 256-color map, OSC8 hyperlink.
2. Fase 2 lanjut: JNI callback (`onTitleChange` dsb) dari Rust ke Kotlin.
3. Fase 6: agent IPC di `crates/mterm-cli` (ikr `/ask` dulu di CLI sebelum chrome).
4. Run `cargo clippy --all-targets` + `cargo test --workspace` sebelum commit.

## Riwayat sesi

- **2026-09-12** Scaffold awal: struktur, core engine (wrap-before-write fix),
  JNI bridge, chrome Compose, ADB wireless, CHECKPOINT ini dibuat.
- **2026-09-12** CLI + PTY crate ditambahkan, end-to-end `mterm run` diverifikasi.