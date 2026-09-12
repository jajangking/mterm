# mterm — Arsitektur

## Ringkasan

```
┌─────────────────────────────────────────────────┐
│  Android Chrome (Kotlin/Compose)                │
│  Tab | Sidebar | Command Palette | Settings     │
└──────────────┬──────────────────────────────────┘
               │ JNI (ffi)
┌──────────────▼──────────────────────────────────┐
│  Rust Core (cdylib)                             │
│  ┌────────────┐  ┌────────────┐  ┌───────────┐  │
│  │ vte parser │→ │ Cell grid  │→ │ Renderer  │  │
│  └────────────┘  └────────────┘  └─────┬─────┘  │
│         ▲                                │       │
│         │ PTY                             │      │
│  ┌──────┴────────────────────────────────┴───┐  │
│  │ Workspace runtime (bun/node/git/ripgrep)  │  │
│  │ + agents (opencode/hermes) via IPC        │  │
│  └───────────────────────────────────────────┘  │
└─────────────────────────────────────────────────┘
```

## Komponen

### 0. Dev loop (ADB wireless)
Iterasi tanpa kabel: `scripts/adb-wireless.sh` menangani pairing + connect +
install + logcat (filtered). Build daily via `scripts/build-android.sh`:
`cargo ndk build` → assemble APK → `adb install -r` → `am start`.
Alamat device disimpan di `.mterm.env` biar udah nggak perlu pair ulang.

### 1. Rust Core `crates/core` (lib)
Input: byte stream (dari PTY atau socket). Output: model terminal berubah
(cursor, cell grid, scrollback, mode).

- `vte` — parser VT (ECMA-48/ANSI, CSI/OSC/DCS). **Jangan rewrite**.
- `Grid` — memory-efficient cell buffer: `Cols × Rows`, scrollback ring.
- `Terminal` — state machine: modes (insert, wraparound, alternate screen),
  attributes (bold/italic/underline/colors), title/hyperlink hooks.
- Rendering: generate u32 pixel buffer / VBO untuk GPU (Skia/GL via app layer).

### 2. JNI Bridge `crates/jni` (cdylib)
Boundary monokotomatik, semua objek lintas batas dibungkus `Arc<Mutex<>>`.

```
nativeInit(width, height) -> handle
nativeWrite(handle, bytes)       // input user dari IME/kb
nativeResize(handle, w, h)
nativeNextEvent(handle) -> Event  // poll Rust → Kotlin callback
nativeDestroy(handle)
```

Callback ke Kotlin:
- `onCellRender(handle, x, y, fg, bg, ch, attrs)`
- `onTitleChange(handle, title)`
- `onBell(handle)`
- `onMouseMode(handle, enabled)`
- `onAgentReady(handle, agentId)`

### 3. Android Chrome `app/`
- Terminal view: `SurfaceView`/`TextureView` yang di-render dari buffer Rust.
- Tab ≈ { PTY session, Rust core handle, scrollback }.
- Command palette: global shortcut memanggil `/ask`, `/explain`, `/fix`, `/commit`.
- Sidebar: baca `git status`, `package.json` scripts — preview read-only.
- Settings: font, theme, profile runtime.

## Thread Model

```
┌─ Main/UI thread ── Compose, input IME
┌─ Emu thread ────── Rust core poll loop (digerakkan callback Kotlin)
┌─ PTY/IO thread ── baca output proses → tulis ke Rust core
┌─ Foreground svc ─ keep-alive sesi saat app di background
```

## Keamanan & Kebersihan Proses

- PTY manager fork dengan `setsid` + `setpgid`; **jangan pernah** bunuh lewat
  pattern match string cmdline (`pkill -f` = risiko bunuh shell sendiri).
- Setiap session menulis PID file sendiri → stop pakai `kill $(cat pidfile)`.
- Rust core wajib thread-safe; FFI expose cuma fn-fn murni berdampingan Mutex.

## Keputusan (log)

- `vte` crate dipakai untuk parse — bangun sendiri tidak ekonomis.
- Workspace app-private tanpa proot.
- mksh default shell (RAM kecil), bash opsional.
- PID file sebagai contract anti bunuh diri proses.

## Alur agent (Fase 6)

1. User ketik `/ask "<pertanyaan>"`.
2. Chrome kumpulin konteks: stderr terakhir, file terpilih, git status.
3. Kirim ke agent (opencode/hermes) via IPC socket dalam workspace.
4. Output dicapture ke alternate buffer, render markdown (kode/table/link).
5. Aksi disepakati: apply diff → tulis file → render ulang.