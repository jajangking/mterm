# CHECKPOINT — mterm

> **Baca ini dulu kalau sesi terputus**. Semua state penting dan cara lanjut
> dicatat di sini agar bisa di-resume tanpa menebak-nebak.

## Status terbaru (update tiap sesi berakhir)

| Baris | Status |
|-------|--------|
| Rust core (`crates/core`) — vte + grid + ANSI | ✅ lestari, 18/18 test, 0 clippy, OSC8 + 256-color + `?` private mode + mouse tracking |
| JNI bridge (`crates/jni`) | ✅ `nativeTakeEvent` (title/bell/mouse) + 4 test lokal; **belum diuji di device** |
| Android chrome (`app/`) — Compose + Gradle | ✅ build + artifact APK 16 MB (`mterm-debug-apk`) — belum diinstall di device |
| ADB wireless helper (`scripts/adb-wireless.sh`) | ✅ siap dipakai (mode executable sudah di-`chmod +x`) |
| CI workflow (`.github/workflows/ci.yml`) | ✅ **GREEN**: rust + android semua ok; failure log dipublish ke cabang `ci-logs` |
| PTY (`crates/pty`) | ✅ dibikin + test di Termux |
| CLI (`crates/mterm-cli`) — `mterm run` / `doctor` / `profile` / `agent` | ✅ dibikin |
| End-to-end engine↔PTY di Termux | ✅ diverifikasi (echo, seq 500, SGR) |

## Cara resume

```sh
cd ~/mterm
git status                       # sesi terakhir: CI android belum tuntas
cargo test -p mterm-core 2>&1 | grep "test result"   # 18 passed
cargo clippy --workspace --all-targets 2>&1 | tail -1   # 0 warning
curl -s "https://api.github.com/repos/jajangking/mterm/actions/runs?per_page=1" \
  | grep -E '"head_sha"|"status"|"conclusion"'
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

1. Install APK ke device via ADB wireless (unduh artifact `mterm-debug-apk` dari run `3332a12`, `adb install`), uji `nativeInit` di device + lihat logcat.
2. Fase 2 lanjut: encode mouse → SGR mode 1006 (belum didukung), kirim ke PTY.
3. Fase 6 lanjut: ganti `StubBackend` dengan implementasi nyata (Bun/Node standalone dulu; Groq HTTP di-skip).
4. Uji JNI di device: source `nativeTakeEvent` dari Kotlin (TermService) begitu APK bisa diinstall.
5. Bersihkan: hapus step `Publish failure log for diagnosis` dari ci.yml + branch `ci-logs` kalau sudah tidak dibutuhkan; kembalikan repo ke private.

## Riwayat sesi

- **2026-09-12** Scaffold awal: struktur, core engine (wrap-before-write fix),
  JNI bridge, chrome Compose, ADB wireless, CHECKPOINT ini dibuat.
- **2026-09-12** CLI + PTY crate ditambahkan, end-to-end `mterm run` diverifikasi.
- **2026-09-12** Re-verifikasi penuh: 8/8 core test, workspace 11 test pass,
  0 clippy, build clean, `mterm run echo`/`seq 500`/SGR ok, `doctor` ok,
  `adb-wireless.sh` (daemon jalan, device belum paired).
- **2026-09-12** Fase 1 lanjut: OSC 8 hyperlink (registry+unescape+cell link),
  peta 256-color xterm via `Color::to_rgb24()`, CSI 1049 simpan/restore kursor,
  test alt buffer + 256-color + OSC8 (14 core test, 0 clippy, `run` OSC8 terbukti).
- **2026-09-12** Fase 2 cicil: event queue FIFO di core (title+bell), Bell produksi,
  JNI `nativeTakeEvent` (polling, buffer-retry aman) + `color_u32` = palette asli,
  test jni 3/3; Kotlin `TermEvent.decode` siap (16 core + 3 jni test, 0 clippy).
- **2026-09-12** Fase 2 cicil: DEC private mode `?` di CSI (alternate screen
  `?1049` nyala — sebelumnya di-ignore), mouse tracking 1000/1002/1003 →
  `Mouse(bool)` event (type 3 di JNI), test jni 4/4; total 18 core + 4 jni, 0 clippy.
- **2026-09-12** Fase 6 cicil: `mterm agent` (serve/start/ask/stop/reset/history),
  Unix socket NDJSON + sesi per-workspace, backend pluggable (`StubBackend` echo;
  Groq HTTP di-skip — kompilasi berat). Test lokal diserahkan ke GH Actions.
- **2026-09-12** CI GHA diluruskan beruntun: (1) `cargo fmt --check` gagal →
  `cargo fmt` + push; (2) test `agent::ask_streams_and_persists` hang →
  client socket tidak di-`drop` sebelum `thread::join` (server stuck di
  `read_line`), fix dengan blok scope client; (3) android job: `chmod +x gradlew`
  gagal (wrapper tak pernah di-commit) → unduh wrapper gradle 8.11.1 dari repo
  gradle + tulis `gradle-wrapper.properties`; (4) `error[E0463] can't find crate
  for core` → tambah `targets: aarch64-linux-android` di `dtolnay/rust-toolchain`;
  (5) fix brace `app/app/build.gradle.kts` yang tidak ditutup (syntax error).
  Commit tag: `c5ac7bd` (hang fix), `cd7ccde` (target), `4b5681d` (wrapper+build).
- **2026-09-12** CI tuntas green: `android.useAndroidX=true` di
  `app/gradle.properties`, fix compile Kotlin (scope `@Composable`
  `rememberTermSession`, buang `@NativeMethods` unresolved, `remember(frame)`,
  `Long`/`Int` di `TermEvent.decode`), workflow publish log error ke cabang
  `ci-logs` (perlu "Workflow permissions → Read and write" di Settings).
  APK `mterm-debug-apk` 16 MB ter-upload di run `3332a12`.
- **2026-09-12** APK terinstall tapi FC → `UnsatisfiedLinkError`: Rust ekspor
  nama `nativeInit` dkk tanpa prefix JNI, padahal JVM mencari
  `Java_com_mterm_app_NativeTerm_*`; argumen `ByteArray` juga beda ABI.
  Fix: rewrite `crates/jni` dua lapis — logika murni (5 test) + glue
  `#[no_mangle] extern "system"` pakai crate `jni = 0.21` (default-features
  off), `JByteArray::from_raw`, `get_byte_array_region`/`set_byte_array_region`.
  APK baru green di run `d17f4fa`.
- **2026-09-12** CI dipercepat: `workflow_dispatch` (rebuild tanpa push), job
  android paralel (tanpa `needs: rust`), `Swatinem/rust-cache` + cache biner
  `cargo-ndk`. Target waktu ~1-2 mnt.

> **Belum dicek**: hasil run terakhir (`4b5681d`) — tunggu job android
> (`assembleDebug`) selesai + status lewat public API sebelum lanjut fitur.

## Decision Log (tambahan di luar WORKMAP)

- **Jangan pakai `sleep 120`** saat menunggu CI di Termux — user minta polling
  singkat / tanpa sleep panjang (lihat Riwayat).
- Test IPC pakai Unix socket lokal + `thread::join`: **client harus menutup
  koneksinya** (drop stream / scope) sebelum join, biar server dapat EOF.
- Gradle wrapper di-vendor langsung (tidak `gradle wrapper` di CI) — Termux tak
  punya gradle; jar diunduh dari tag `v8.11.1` repo gradle.
- `.gitignore`: `Cargo.lock` di-ignore; biarkan (keputusan lama), dev dep
  pinning timing bias.