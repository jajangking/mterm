# mterm

Terminal Android modern — workspace-first + agent-ready. Dibangun dari nol
(bukan fork Termux): Rust core untuk terminal emulator, Kotlin/Compose untuk
chrome, JNI bridge di antaranya.

## Kenapa beda

- **Rust core** — render murah, RAM kecil (Termux pakai emulator Java + Spannable)
- **Workspace-first** — buka folder, langsung lihat git status + scripts; bukan "shell + cd"
- **Agent-ready** — opencode/hermes jalan out-of-the-box (runtime profile: bun/node/git)
- **Anti-footgun** — PID file per session, manajemen proses yang nggak bunuh shell sendiri
- **Tanpa proot** — distro run di app-private directory

## Struktur

```
mterm/
├── crates/
│   ├── core/        # Rust core: VT parsing + cell grid  (lib)
│   ├── jni/         # JNI bridge: native funcs dipanggil Kotlin (cdylib)
│   ├── pty/         # PTY session manager + PidFile anti pkill footgun
│   └── mterm-cli/   # CLI: `mterm run|doctor|profile` (dev di Termux)
├── app/             # Android chrome (Kotlin + Compose)
├── runtimes/        # profile manager: bun/node/git per-workspace
├── scripts/         # build & dev scripts (+ adb wireless)
├── docs/            # arsitektur & decision log
├── .github/workflows/ci.yml   # build APK (Rust→cargo-ndk→gradle)
├── WORKMAP.md       # pemetaan kerjaan + fase
└── CHECKPOINT.md    # resume state (BACA DULU kalau sesi terputus)
```

## Status singkat

| Fase | Status |
|------|--------|
| F0 Foundation + CI | ✅ |
| F1 Rust core | ✅ (OSC8/kitty belum) |
| F2 JNI bridge | ⚠️ FFI jalan, callback belum |
| F3 Android chrome | 🔧 scaffold, belum di-build |
| F4–F5 | ⏳ PTY/PidFile/doctor rampung, sisanya roadmap |
| F6–F8 | roadmap (lihat WORKMAP.md) |

## Build (dev)

```sh
cargo build               # seluruh workspace
cargo test --workspace    # core 8 + pty 3 test
cargo clippy --all-targets

./target/debug/mterm run sh -c 'seq 1 500'      # PTY→engine→text
./target/debug/mterm doctor
./target/debug/mterm profile use dev
```

Build APK tidak bisa di device-only (butuh Android SDK) → jalankan via
GitHub Actions (`.github/workflows/ci.yml`) atau `scripts/build-android.sh`
di PC dengan SDK/NDK + `cargo ndk`.

## Catatan Termux

Shebang absolut diperlukan (`/data/data/com.termux/files/usr/bin/...`),
`env` di Termux adalah symlink coreutils — `#!/usr/bin/env` bisa gagal.