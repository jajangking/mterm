# mterm — Work Map

**Project**: Modern Android terminal, workspace-first + agent-ready  
**Stack**: Rust core (terminal) + Kotlin/Compose (chrome) + JNI bridge  
**Status**: F0–F2 core selesai; F3 chrome scaffold; F4/F5/Termux-only work jalan

---

## Fase 0 — Foundation
- [x] Struktur folder
- [x] Cargo workspace + crate skeleton
- [x] `.gitignore`, `README.md`, `ARCHITECTURE.md`
- [x] CI skeleton (GitHub Actions) — `.github/workflows/ci.yml`

### Dev workflow (integrasi ADB wireless)
Iterasi APK tanpa kabel — pairing sekali per day via Wireless debugging (Android 11+).

- [x] `scripts/adb-wireless.sh`: pair / connect / install / logcat
- [x] `.mterm.env` — simpan alamat device + port biar `connect` jadi satu perintah (save/show; connect/pair auto-baca; lokasi repo root, gitignore)
- [x] `scripts/build-android.sh` → build APK → install ke device via adb-wireless (modes: local / `--ci` dispatch / `--pull` artifact CI / `--install`; install ALWAYS hash-verify)
- [x] CI build artifact → pull pakai `adb-sync` (= `build-android.sh --pull`)

## Fase 1 — Rust Core (terminal engine)
**Tujuan**: Terminal emulator yang benar — VT parsing, cell buffer, ANSI rendering.

- [x] Integrasikan `vte` crate untuk VT parsing (via `Terminal::feed_bytes`)
- [x] Cell grid model (`Cell`, `Grid`, `Line`, `Cursor`) + scrollback ring
- [x] ANSI state machine: bold, italic, underline, 256-color, truecolor (`38;2;r;g;b`)
- [x] Alternate screen buffer (swap primary ↔ alternate, CSI 1049)
- [x] Scrollback buffer (ring buffer, cap configurable)
- [x] Title setter (`ESC ] 0;title BEL` → `TerminalEvent::Title`)
- [x] OSC 8 hyperlink support (untuk opencode) — registry `u32→uri`, `%XX` unescape, cell ber-link
- [x] Kitty graphics protocol (minimal: transmit, transmit+place `a=T`, place,
      delete; base64 decoder tanpa dep, chunked `m=1..m=0`, APC terpotong
      di-buffer lintas feed, registry image + event `KittyImage/Placed/Deleted`,
      placeholder cell `attrs.image`) — 10 test integrasi
- [x] Unit test: parser + grid rendering (40 test)
- [x] Benchmark: 10k lines scroll throughput (`seq_10k_benchmark`, ignored;
      release ≈6µs/baris) + `seq` real diverifikasi via `mterm run`

## Fase 2 — JNI Bridge
**Tujuan**: Rust core bisa dipanggil dari Kotlin/Compose via JNI.

- [x] FFI boundary: `nativeInit`, `nativeWrite`, `nativeResize`, `nativeCellAt`, `nativeDirty`
- [x] Callback → Kotlin: `onCellUpdate` (snapshot poll), `onTitleChange`, `onBell`,
      `onMouse` via `nativeTakeEvent` (encoding `[type][len][payload]`)
- [x] Thread model: Rust emu thread + polling di Kotlin
      (`mterm-pty::runner::EmuRunner` — thread `mterm-emu` baca PTY →
      `Terminal::feed_bytes` via callback; caller poll `exit_code()`,
      input/resize via sync_channel; RuntimeException-safe drop. E2E diuji
      `mterm-cli/tests/thread_model.rs`: main thread poll grid, controller
      thread feed, sama dengan kontrak loop Kotlin)
- [x] EmuRunner → JNI wiring: `nativeSessionStart` (spawn PTY + emu thread,
      respawn kalau sudah selesai), `nativeRunnerStop`/`nativeRunnerExit`/
      `nativeRunnerInput`/`nativeRunnerResize`; Kotlin `TermSession`:
      `startSession`/`input`/`sessionExit`/`stopSession`/`sessionResize`.
      6 test integrasi PTY asli di host (exit code, signal, input→shell,
      respawn block, stop, destroy join)
- [x] Memory safety: bounds check, leak audit (guard `catch_unwind` FFI, handle
      OOB/poison→None, clamp dimensi, fuzz no-panic + invariant test)

## Fase 2.5 — Termux-first work (selesai)
Kerja yang bisa/tidak bisa dikerjakan di Termux — lihat CHECKPOINT.md.

- [x] `crates/pty` — openpt + spawn (argv benar), slave dipertahankan di parent
  (jangkar EIO), deteksi exit pakai waitpid WNOHANG (bukan kill(0) — zombie).
- [x] `PidFile` — anti footgun `pkill -f` (create/read/alive/kill/stop_from_file).
- [x] `crates/mterm-cli` — `mterm run` (PTY→engine→text dump; interactive via
  poll stdin+master), `mterm doctor`, `mterm profile list/use/show/add`.
- [x] End-to-end diverifikasi di Termux: `echo`, `seq 1 500` (scroll tail),
  SGR warna (`\033[31m..\033[0m` — catatan: dash `printf` tak dukung `\x1b`).
- [x] `.github/workflows/ci.yml` — build APK otomatis (tak bisa di Termux).

## Fase 3 — Android Chrome (Compose)
**Tujuan**: GUI yang tipis tapi fungsional — tab, sidebar, command palette.

- [ ] Gradle project skeleton (AGP 8.x, Kotlin 2.x)
- [ ] Compose theme (dark terminal, accent color configurable)
- [ ] Terminal view: Rust core render → `SurfaceView` / `TextureView`
- [ ] Tab management: add/remove/switch (session = PTY + Rust core)
- [ ] Sidebar drawer: git status, project files (read-only preview)
- [ ] Keyboard handling: IME, Ctrl/Alt/Meta shortcuts, Escape passthrough
- [ ] Scroll/selection: touch handling → mouse events ke Rust core
  - [x] Rust side: viewport scrollback (`set_scroll_offset`/`scrollback_len`/
        `view_line` di core + `nativeScrollOffset`/`nativeScrollMax`/JNI cell_at
        hormati viewport) + mouse→SGR (`nativeSgrMouse`; 4 test core + 3 JNI)
  - [ ] Chrome side: gesture → `scrollTo` + `sgrMouse` → `input` (butuh CI)
- [ ] Settings page: font, theme, profile selection

## Fase 4 — Session Persistence + Process Management
**Tujuan**: Proses nggak mati ditiban Android; session bisa resume.

- [ ] Foreground service (notification persistent) — `TermService` stub sudah ada
- [x] Session state serializer: cursor pos, scrollback, env vars
      (`Terminal::to_json`/`from_json` — grid+scrollback+cursor+mode+hyperlink,
      transient di-skip; 4 test core + 3 JNI `nativeSaveState`/`nativeLoadState`
      + e2e `session_persist.rs` PTY→save→restore→continue)
- [ ] Auto-save on app background, auto-restore on foreground
      (chrome lifecycle → `saveState`/`loadState`, butuh CI)
- [x] PTY manager: fork + setsid + setpgid (pkill-safe) → `crates/pty`
- [x] PID file per session (anti footgun `pkill -f`) → `PidFile`

## Fase 5 — Runtime Profile Manager
**Tujuan**: Bun/node/git/ripgrep terinstall per-workspace, bukan global bootstrap.

- [x] `mterm profile list/use/show/add` — manifest `~/.mterm/profiles.json` + marker `.mterm.profile` per-workspace
- [x] Profiles default: `dev` (git+node+ripgrep), `web` (node+npm) — extendible via `mterm profile add`
- [x] Profiles lengkap: `full` (python+go), rilis terverifikasi
      (`mterm profile verify <name>` cek tiap runtime via `command -v`+`--version`;
       `full` ditambahkan default + migrasi ke manifest lama)
- [x] Binary download + cache di app-private dir (`~/.mterm/cache`): `mterm tool
      install node` — .deb aarch64 dari repo Termux resmi (nodejs.org tidak
      menyediakan android-arm64 lagi sejak v18+), SHA256 diverifikasi vs
      `Packages.gz`, ekstrak `data.tar.xz` (strip-components), symlink ke
      `cache/bin/`; `tool status` lihat instalasi; pin versi via `--version` /
      `MTERM_NODE_VERSION`; 6 test unit (parser index, filter, deb URL,
      marker)
- [x] `mterm doctor` — cek runtime ready (git/node/rg present, bun/fzf optional)

## Fase 6 — Agent IPC + Command Palette
**Tujuan**: Agent (opencode/hermes) hidup di workspace dan bisa dipanggil dari chrome.

- [x] Unix socket IPC + protokol NDJSON (`mterm agent serve/start/ask/stop/reset/history`)
- [x] Backend LLM beneran: `RestBackend` (curl streaming SSE, OpenAI-compatible:
      Groq/OpenAI/openrouter/ollama) — default model `openai/gpt-oss-120b` via
      env `MTERM_MODEL`; key dari `GROQ_API_KEY`/`~/.groq_key`; fallback `StubBackend`
- [x] Slash command: `/ask`, `/explain`, `/fix`, `/commit` + context collector
      (file read cap 12k char; `git status`+`git diff --stat` untuk `/commit`)
- [x] Agent output renderer: markdown → ANSI (tanpa dep, subset: heading/bold/italic/
      inline-code/fence/list/blockquote/hr/link) — `mterm agent ask --render`
- [x] Workspace-scoped agent lifecycle: state per-workspace di
      `~/.mterm/agent/<slug>/` (socket+pid+session+log); `start/stop/status/ask/
      reset/history` hormati `--workspace`/`MTERM_WORKSPACE`/cwd; tambah
      `restart` & `list` (semua workspace); 5 test unit (slug, scope, resolve,
      scan)
- [x] Stderr + context collector → bundle ke agent: `mterm run` merekam ekor
      output PTY + exit code ke `last_stderr.txt` (ANSI dibuang); `agent ask`
      mengemasnya jadi system msg otomatis; `agent stderr [clear]` lihat/hapus;
      3 test unit (collector, strip_ansi, bundling ke system msg)

## Fase 7 — Package Manager (Ringan)
**Tujuan**: Ganti apt/dpkg dengan yang lebih cepat dan kecil.

- [x] Minimal pkg manager: tar.zst + metadata JSON
      (`mterm pkg make-repo <dir>`: index.json untuk *.tar.zst di <dir>/pkgs/;
      install ekstrak via zstd+tar ke `~/.mterm/pkg/prefix/` app-private)
- [x] Repository mirrors (standalone, bukan Debian)
      (url `https://…` atau `file://…`; `pkg mirrors add|list|remove`;
      index.json+parts di root repo; indepes dari struktur Debian)
- [x] `pkg install <name>`, `pkg update`, `pkg remove`, `pkg list`
      (+ `search`, `info`, `verify`, `install <nama>@<versi>`, tarball lokal)
- [x] Delta updates (rsync-style)
      (arsip dipecah jadi part 1 MiB content-addressed `parts/<sha256>`;
      install hanya mengunduh part yang belum ada di cache — versi baru dengan
      konten sebagian sama → sebagian besar dipakai ulang; teruji e2e)
- [x] Signing: GPG atau signature file
      (tanda tangan Ed25519 murni Rust `index.json.sig` via `pkg keygen` +
      `repo-sign`; `pkg update` menolak index yang tidak cocok tanda tangan
      saat kunci publik tersedia)

## Fase 8 — Polish + Release
- [ ] APK signing + Play Store / F-Droid metadata
- [ ] Onboarding flow
- [ ] Dark/light theme
- [ ] Localization skeleton
- [ ] Documentation site

---

## Decisions Log

| Date | Decision | Rationale |
|------|----------|-----------|
| 2026-09-12 | Rust core + JNI (bukan fork Termux Java) | Terminal emulator Java rakus RAM, spannable overhead |
| 2026-09-12 | Pakai `vte` crate (bukan bangun VT sendiri) | VT emulation proyek 6+ bulan kalau dari nol |
| 2026-09-12 | Workspace di app-private (tanpa proot) | scoped storage sulit; proot overhead |
| 2026-09-12 | mksh default, bash optional | hemat memori, startup cepat |
| 2026-09-12 | Parent mempertahankan slave fd PTY | kalau slave ditutup, master EIO dan buffer output hilang |
| 2026-09-12 | Deteksi exit vs zombie pakai `waitpid(WNOHANG)` | `kill(pid,0)` tetap bilang hidup untuk zombie |
| 2026-09-12 | Eksekusi command lewat `execvp` + argv | `execv(whole-string)` gagal → "echo hallo" ≠ file path
| 2026-09-12 | PTY/CLI kerja di Termux; APK di GitHub Actions | SDK ~GB nggak feasible di device-only |
| 2026-09-12 | Indexed warna solve via `Color::to_rgb24()` di core | CLI & JNI renderer pakai peta xterm 256, bukan dummy |
| 2026-09-12 | Hyperlink OSC 8: id `u32` di cell + registry `HashMap` | CellAttrs tetap `Copy`; uri tak ter-embed di tiap cell |
| 2026-09-12 | CSI 1049 simpan/restore kursor + reset ke home | sesuai perilaku xterm; swap grid saja ternyata kurang |
| 2026-09-12 | Event delivery via polling `nativeTakeEvent`, bukan callback push | tak perlu `jni` crate/thread attach; aman & tesable (~Termux) |
| 2026-09-12 | Event queue FIFO di core (`VecDeque`) + Bell produksi | take_event order deterministik, JNI tinggal baca |
| 2026-09-12 | Dukung intermediate `?` (DEC private mode) di CSI | vim/less kirim `CSI ? 1049 h` — sempat di-ignore total |
| 2026-09-12 | Mouse tracking DEC 1000/1002/1003 → `Mouse(bool)` event | chrome tau kapan harus tangkap touch → encode SGR |
| 2026-09-12 | Agent IPC: Unix socket NDJSON + sesi JSON per-workspace, backend pluggable | integrasi LLM via trait `Backend`; sekarang `StubBackend` echo |
| 2026-09-12 | Skip reqwest/Groq HTTP (kompilasi berat di Termux) | CI (GH Actions) yang jadi gate tes; backend LLM menyusul |
| 2026-09-12 | Kitty graphics: APC dicegat di `feed_bytes` sebelum vte (vte 0.11 tak punya hook APC) + base64 decoder ditulis sendiri | hindari dep `base64`; state kitty tetap di core (registry `images`, `pending_apc` buffer lintas feed) |
| 2026-09-12 | Image ditransfer sebagai `Arc<Vec<u8>>` di event `KittyImage` (refcount clone, bukan copy) | renderer dapat bytes; core tetapkan registry sebagai sumber kebenaran |
