# CHECKPOINT — mterm

> **Baca ini dulu kalau sesi terputus**. Semua state penting dan cara lanjut
> dicatat di sini agar bisa di-resume tanpa menebak-nebak.

## Status terbaru (update tiap sesi berakhir)

| Baris | Status |
|-------|--------|
| Rust core (`crates/core`) — vte + grid + ANSI | ✅ lestari, **40/40** test, 0 clippy; OSC8 + 256-color + `?` private mode + mouse tracking + **kitty graphics minimal** (transmit/place/delete, chunked, base64 internal) |
| JNI bridge (`crates/jni`) | ✅ `nativeTakeEvent` (title/bell/mouse) + 4 test lokal; **belum diuji di device** |
| Android chrome (`app/`) — Compose + Gradle | ✅ build + artifact APK 16 MB (`mterm-debug-apk`) — belum diinstall di device |
| ADB wireless helper (`scripts/adb-wireless.sh`) | ✅ siap dipakai (mode executable sudah di-`chmod +x`) |
| CI workflow (`.github/workflows/ci.yml`) | ✅ **GREEN**: rust + android semua ok; failure log dipublish ke cabang `ci-logs` |
| PTY (`crates/pty`) | ✅ dibikin + test di Termux |
| CLI (`crates/mterm-cli`) — `mterm run` / `doctor` / `profile` / `agent` | ✅ dibikin |
| **Thread model** (`crates/pty::runner::EmuRunner`) | ✅ thread `mterm-emu` baca PTY → feed engine via callback; input/resize lewat sync_channel; exit code atomic; e2e `thread_model.rs` pass + 8 test runner |
| **EmuRunner→JNI** (`crates/jni`) | ✅ `nativeSessionStart`/`nativeRunnerStop`/`nativeRunnerExit`/`nativeRunnerInput`/`nativeRunnerResize` + `TermSession` Kt; 6 test PTY asli di host (respawn, stop, destroy join) — kotlin belum di-compile (butuh CI) |
| **Viewport scroll + mouse** (Fase 3 Rust-side) | ✅ `Terminal::set_scroll_offset`/`view_line` (#44 core) + JNI `nativeScrollOffset`/`nativeScrollMax`/`nativeSgrMouse` (20 jni); `cell_at` hormati scrollback; chrome gesture tinggal panggil |
| **Session persistence** (Fase 4) | ✅ `Terminal::to_json`/`from_json` (serde, transient di-skip) + JNI `nativeSaveState`/`nativeLoadState` + e2e PTY save→restore→continue (#48 core, 20 jni, 1 cli) |
| **Profile full + rilises terverifikasi** (Fase 5) | ✅ profile `full` (python+go+git+node) + `mterm profile verify <name>` (command -v + versi nyata) |
| **Tool download + cache** (Fase 5) | ✅ `mterm tool install node`: .deb aarch64 repo Termux resmi → SHA256 vs `Packages.gz` → ekstrak `data.tar.xz` → `~/.mterm/cache/bin`; `tool status`; pin `--version`/`MTERM_NODE_VERSION` (mterm-cli bin 20 test; total 98) |
| **Agent WS lifecycle** (Fase 6) | ✅ state per-workspace `~/.mterm/agent/<slug>/`; `start/stop/restart/status/ask/list` hormati `--workspace`/`MTERM_WORKSPACE`/cwd; teruji 2 agent paralel (mterm + /tmp) |
| **Stderr collector** (Fase 6) | ✅ `mterm run` → `last_stderr.txt` (exit code + tail output, ANSI dibuang) → otomatis dibundle jadi system msg di `agent ask`; live-proven: Groq tahu error `gcc` terakhir (mterm-cli bin 27 test; total 104) |
| `Session` exit semantics | ✅ `exited()` sekarang cache kode (dulu setelah reap selalu kasih `Some(0)`); drop reap anti-zombie tetap |
| End-to-end engine↔PTY di Termux | ✅ diverifikasi (echo, seq 500, SGR) |
| **Android renderer (Chrome)** | ✅ **MILESTONE: teks tampil di device** (Transsion) — render per-baris via `Text` composable (Canvas draw TIDAK tampil di device itu); faktor advance mono 0.62 supaya tiap kolom pas 1 sel; grid **gelap penuh** via `SpanStyle(color, background)` per-sel, spasi/NUL dijadikan `' '` TIDAK di-skip (commit `acdde40`); keyboard tersembunyi `EditText` + input PTY via `session.input`, d-pad/tab → escape sequence |

## Cara resume

```sh
cd ~/mterm
git status                       # kerjaan terbaru: renderer Android (lihat Riwayat)
cargo test --workspace 2>&1 | grep "test result"   # 113 passed (core 48 + jni 21 + pty 8 + cli 34 + e2e 2)
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

1. **Fase 3 (Android Chrome)**: renderer sudah MILESTONE di device — lanjut:
   input IME (backspace, autocorrect, superscript ⚠) + scroll gesture →
   `scrollTo`/`scrollMax`, tap → `sgrMouse`→`input`; ganti hardcode `cols=80
   rows=24` dengan ukuran dinamis dari `BoxWithConstraints`.
   EmuRunner + viewport + mouse semua sudah siap di Rust (113 test hijau, 0 clippy).
2. **Fase 4 lifecycle (CI)**: auto-save term ke file app-private via
   `saveState`/`loadState` saat background/foreground; `TermService`
   foreground service + notification persistent.
3. **Fase 6 (agent): ganti `StubBackend`** dengan backend HTTP nyata (Bun/Node
   standalone dulu; Groq HTTP di-skip untuk sekarang).
4. Bersihkan: hapus step `Publish failure log for diagnosis` dari ci.yml + branch
   `ci-logs` kalau sudah tidak dibutuhkan; kembalikan repo ke private.

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
- **2026-09-12** Core: **SGR mouse 1006** — `crates/core/src/mouse.rs`
  (`sgr_mouse_seq`/`encode`: bit modif xterm Shift=4/Alt=8/Ctrl=16/Motion=32,
  release `m` vs press `M`, roda 64/65, koordinat 1-based clamp) + parsing
  `CSI ?1006h/l` jadi `sgr_mouse` flag di `Terminal`. 9 test baru → core 27/27,
  workspace green, 0 clippy. Commit `d9b4ddb`.
- **2026-09-12** CI: `cargo install --version "^0.9"` tidak valid (caret);
  pakai `cargo install cargo-ndk` (latest) + key cache `cargo-ndk-latest`;
  step failure-log di-hardening (`cp` pakai `|| true`). Green lagi, kini jobs
  paralel → ~2,5 mnt. Commit `f1ec39c`. Contoh ID run hijau: lihat
  `https://github.com/jajangking/mterm/actions`.
- **2026-09-12** Core: **fast-path ASCII feed** — `feed_bytes` menulis run
  printable/CR/LF langsung tanpa `vte` (ESC/control/utf8 → vte untuk sisa
  chunk). `put_ascii`/`put_glyph_wide` (wide = lebar 1 sementara). Benchmark
  `seq_10k_benchmark` (#[ignore]): **10k baris 80x24 = 60ms release (~6µs/baris,
  ±166k baris/s)**; wrap 700k = 57ms. Core 27/27, clippy 0. Commit `da46c65`.
- **2026-09-12** Fase 2: **memory safety audit** — JNI boundary di-bungkus
  `guard(catch_unwind)` (panic tak lintas FFI), handle OOB/poison → `None`/
  default (bukan `.unwrap()`), dimensi di-clamp (>=1, cap 1024x512 anti-OOM),
  `Terminal::new`/`Grid::new` clamp cols/rows, grid pakai `saturating_sub`.
  Test baru: fuzz byte acak + sekuens ESC terpotong (no-panic + invariant),
  OOB handle/koordinat → aman, clamp dimensi negatif/raksasa. Core 28/28, jni
  8/8.
- **2026-09-12** Fase 6: **backend agent nyata** — `RestBackend` (OpenAI-
  compatible, curl streaming SSE tanpa dep TLS/HTTP). Default Groq, model
  `openai/gpt-oss-120b` (bisa `MTERM_MODEL` override), key `GROQ_API_KEY`/
  `~/.groq_key`, fallback `StubBackend`. `mterm agent status` menampilkan
  backend aktif. Error HTTP body di-surface. Terverifikasi LIVE di Termux:
  `mterm agent ask "1+1?"` → "dua". Test: SSE parse unit + end-to-end curl
  → fake HTTP server (skip kalau curl tak ada). mterm-cli 4/4, clippy 0.
- **2026-09-12** CI rusak sesaat (3 run gagal) karena `cargo install
  cargo-ndk --version "^0.9"`; sudah diperbaiki di atas.

- **2026-09-12** Fase 7: **Package manager ringan** (`mterm pkg`) — mandiri,
  bukan wrapper apt. Ed25519 murni Rust (`ed25519-dalek` + `sha2`): `keygen`,
  `repo-index`, `repo-sign`; `update` verifikasi `index.json.sig` (tolak index
  tampered). Repo standalone: `pkgs/*.tar.zst` + `parts/<sha256>` (1 MiB
  content-addressed) + `index.json` — support `file://` & `https://`, mirror
  config `pkg mirrors add/list/remove`. Delta rsync-style: hanya part baru
  diunduh (lama di-cache dipakai ulang), teruji e2e dengan `prng filler` > 1 MiB.
  Full flow: `keygen → make-repo → update → install → verify → remove` semuanya
  jalan; 112 test total, 0 clippy; sedang dipush.

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
- **Editor vs device**: di device Transsion, `Canvas.drawText` (Compose DrawScope)
  tidak menampilkan apa pun — pakai `Text` composable per-baris (commit `d79a184`).
- **Sel kosong jangan di-skip**: spasi/NUL tetap di-render sebagai `' '` dengan
  `SpanStyle(background)` — kalau di-`continue`, latar gelap "bolong" (commit `acdde40`).
- **Advance mono**: `FontFamily.Monospace` di device bukan 1:1 per sel —
  scale `fontSize = (cellW / 0.62).sp` agar tiap kolom pas 1 sel (commit `c155b49`).

## Riwayat sesi (2026-09-13: renderer Android selesai di device)

- **2026-09-13** Renderer Chrome: MILESTONE teks tampil di device Transsion.
  Lesit: Canvas draw tak tampil → `Text` per-baris; advance mono 0.62; grid
  gelap penuh span per-sel (spasi/NUL → `' '`, tidak di-skip).
  Commit: `d79a184` (Text), `c155b49` (mono 0.62), `c368c70` (MILESTONE + bg per-sel),
  `2499a40` (fallback Column bg), `387ff02` (import background), `12c0e69`,
  `048443e` (pola append per-sel), `acdde40` (grid gelap penuh, final).
  Rust: 113 test pass (core 48 + jni 21 + pty 8 + cli 34 + e2e 2),
  0 clippy. Semua di-push ke `origin/main`.