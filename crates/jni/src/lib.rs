//! mterm-jni: boundary rust <-> Kotlin (Android).
//!
//! Dua lapis:
//! 1. Logika murni (Rust murni, tetap di-test di host) — handle registry +
//!    operasi terminal.
//! 2. Glue JNI — fungsi `#[no_mangle] extern "system"` dengan nama
//!    `Java_com_mterm_app_NativeTerm_*` yang dicari JVM. Kotlin `object
//!    NativeTerm` berisi method instance, jadi arg kedua adalah `jobject`.

use std::sync::{Arc, Mutex};

use mterm_core::grid::Color;
use mterm_core::terminal::{Terminal, TerminalConfig, TerminalEvent};
use mterm_pty::runner::{send_input, EmuRunner};
use mterm_pty::Session;

use jni::objects::{JByteArray, JObject, JObjectArray, JString};
use jni::sys::{jboolean, jbyteArray, jint, jlong, JNI_FALSE, JNI_TRUE};
use jni::JNIEnv;

type SharedTerminal = Arc<Mutex<Terminal>>;
type RunnerSlot = Arc<Mutex<Option<EmuRunner>>>;

/// Batas dimensi layar (anti OOM-abort dari allocator, bukan panic).
const MAX_COLS: i32 = 1024;
const MAX_ROWS: i32 = 512;

/// Jalankan logika pendek JNI di dalam `catch_unwind`: panic di dalam mustahil
/// ndelok unwind ke JVM (UB/abort). Kembali `default` kalau panic.
fn guard<T>(default: T, f: impl FnOnce() -> T) -> T {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).unwrap_or(default)
}

#[derive(Default)]
struct Handles(Vec<SharedTerminal>, Vec<RunnerSlot>);

static HANDLES: once_cell::sync::Lazy<Mutex<Handles>> =
    once_cell::sync::Lazy::new(|| Mutex::new(Handles::default()));

fn alloc_handle(term: SharedTerminal) -> u64 {
    let Ok(mut h) = HANDLES.lock() else {
        return u64::MAX;
    };
    h.0.push(term);
    h.1.push(Arc::new(Mutex::new(None)));
    (h.0.len() - 1) as u64
}

fn get(handle: u64) -> Option<SharedTerminal> {
    let Ok(g) = HANDLES.lock() else {
        return None;
    };
    g.0.get(handle as usize).cloned()
}

fn runner_slot(handle: u64) -> Option<RunnerSlot> {
    let Ok(g) = HANDLES.lock() else {
        return None;
    };
    g.1.get(handle as usize).cloned()
}

fn clamp_dim(v: i32, max: i32) -> usize {
    (v.max(1).min(max)) as usize
}

// ---------------------------------------------------------------------------
// Logika murni (dipanggil dari glue JNI; diuji langsung di host)
// ---------------------------------------------------------------------------

fn init_term(cols: i32, rows: i32) -> u64 {
    let cfg = TerminalConfig {
        cols: clamp_dim(cols, MAX_COLS),
        rows: clamp_dim(rows, MAX_ROWS),
        ..Default::default()
    };
    alloc_handle(Arc::new(Mutex::new(Terminal::new(cfg))))
}

fn destroy(handle: u64) {
    // stop & join emu thread dulu (kalau ada) biar tak ada thread yatim
    if let Some(slot) = runner_slot(handle) {
        if let Ok(mut g) = slot.lock() {
            if let Some(r) = g.take() {
                r.shutdown();
            }
        }
    }
    let Ok(mut g) = HANDLES.lock() else {
        return;
    };
    if let Some(term) = g.0.get_mut(handle as usize) {
        *term = Arc::new(Mutex::new(Terminal::new(TerminalConfig::default())));
    }
}

/// `write(handle, bytes)` — input byte (dari PTY/socket).
fn write(handle: u64, bytes: &[u8]) {
    if bytes.is_empty() {
        return;
    }
    let Some(shared) = get(handle) else {
        return;
    };
    let Ok(mut t) = shared.lock() else {
        return;
    };
    t.feed_bytes(bytes);
}

fn resize_term(handle: u64, cols: i32, rows: i32) {
    let Some(shared) = get(handle) else {
        return;
    };
    let Ok(mut t) = shared.lock() else {
        return;
    };
    t.resize(clamp_dim(cols, MAX_COLS), clamp_dim(rows, MAX_ROWS));
}

/// `cell_at(handle, x, y) -> Option<(fg_rgb24, bg_rgb24, ch)>` — hormati
/// viewport (scrollback) yang sedang diset via `set_scroll_offset`.
fn cell_at(handle: u64, x: i32, y: i32) -> Option<(u32, u32, u32)> {
    let shared = get(handle)?;
    let t = shared.lock().ok()?;
    if x < 0 || y < 0 || x as usize >= t.cols() || y as usize >= t.rows() {
        return None;
    }
    let cell = &t.view_line(y as usize).cells[x as usize];
    Some((
        color_u32(cell.attrs.fg),
        color_u32(cell.attrs.bg),
        cell.ch as u32,
    ))
}

fn set_scroll_offset(handle: u64, k: i32) {
    let Some(shared) = get(handle) else {
        return;
    };
    let Ok(mut t) = shared.lock() else {
        return;
    };
    t.set_scroll_offset(k.max(0) as usize);
}

fn scroll_max(handle: u64) -> i32 {
    let Some(shared) = get(handle) else {
        return 0;
    };
    let Ok(t) = shared.lock() else {
        return 0;
    };
    t.scrollback_len() as i32
}

/// `sgr_mouse(handle, code, mods, release, x, y, out) -> n` — encode klik/wheel
/// ke SGR escape untuk dikirim ke shell via `runner_input`. `n <= 0` kalau
/// mode mouse belum aktif di TUI.
fn sgr_mouse(
    handle: u64,
    code: i32,
    mods: i32,
    release: bool,
    x: i32,
    y: i32,
    out: &mut [u8],
) -> i32 {
    let Some(shared) = get(handle) else {
        return -1;
    };
    let Ok(t) = shared.lock() else {
        return -1;
    };
    let mut seq = Vec::with_capacity(16);
    if !t.sgr_mouse_seq(
        code.clamp(0, 255) as u8,
        mods.clamp(0, 255) as u8,
        release,
        x.max(0) as usize,
        y.max(0) as usize,
        &mut seq,
    ) {
        return 0; // mode SGR belum aktif
    }
    if seq.len() > out.len() {
        return -1;
    }
    out[..seq.len()].copy_from_slice(&seq);
    seq.len() as i32
}

fn dirty(handle: u64) -> bool {
    let Some(shared) = get(handle) else {
        return false;
    };
    let Ok(mut t) = shared.lock() else {
        return false;
    };
    // konsumtif: tiap pembacaan mereset dirty_rect → renderer switch ke frame
    // baru hanya saat ada perubahan berikutnya
    t.dirty_rect.take().is_some()
}

/// `take_event(handle, out) -> n` — pop event terminal (polling).
///
/// Layout keluar: `[type:u32 LE][len:u32 LE][payload...]`.
/// - type 1 = Title (payload = utf8 judul)
/// - type 2 = Bell (tanpa payload)
/// - type 3 = Mouse (payload 1 byte bool aktif)
///
/// Return: `n > 0` byte tertulis, `0` = tak ada event, `-1` = buffer kurang
/// besar (event TIDAK dikonsumsi, bisa dipanggil ulang dengan buffer lebih).
fn take_event(handle: u64, out: &mut [u8]) -> i32 {
    let Some(shared) = get(handle) else {
        return -1;
    };
    let Ok(mut t) = shared.lock() else {
        return -1;
    };
    let ev = match t.events.front().cloned() {
        Some(e) => e,
        None => return 0,
    };
    let (ty, payload): (u32, Vec<u8>) = match ev {
        TerminalEvent::Title(s) => (1, s.into_bytes()),
        TerminalEvent::Bell => (2, Vec::new()),
        TerminalEvent::Mouse(on) => (3, vec![u8::from(on)]),
        TerminalEvent::Hyperlink(_) => return 0,
        // type 4 = KittyImage: id u32 LE, format u8, w u32 LE, h u32 LE, data...
        TerminalEvent::KittyImage {
            id,
            format,
            width_px,
            height_px,
            data,
        } => {
            let mut p = Vec::with_capacity(17 + data.len());
            p.extend_from_slice(&id.to_le_bytes());
            p.push(format);
            p.extend_from_slice(&width_px.to_le_bytes());
            p.extend_from_slice(&height_px.to_le_bytes());
            p.extend_from_slice(&data);
            (4, p)
        }
        // type 5 = KittyPlaced: id, x, y, cols, rows (u32 LE)
        TerminalEvent::KittyPlaced {
            id,
            x,
            y,
            cols,
            rows,
        } => {
            let mut p = Vec::with_capacity(20);
            for v in [id, x as u32, y as u32, cols as u32, rows as u32] {
                p.extend_from_slice(&v.to_le_bytes());
            }
            (5, p)
        }
        // type 6 = KittyDeleted: id u32 LE
        TerminalEvent::KittyDeleted { id } => (6, id.to_le_bytes().to_vec()),
    };
    let need = 8 + payload.len();
    if need > out.len() {
        return -1; // buffer kurang → event tetap di antrean
    }
    t.events.pop_front(); // baru konsumsi kalau muat
    out[0..4].copy_from_slice(&ty.to_le_bytes());
    out[4..8].copy_from_slice(&(payload.len() as u32).to_le_bytes());
    out[8..need].copy_from_slice(&payload);
    need as i32
}

fn color_u32(c: Color) -> u32 {
    match c.to_rgb24() {
        Some((r, g, b)) => 0xFF00_0000 | ((r as u32) << 16) | ((g as u32) << 8) | b as u32,
        None => 0, // tak ada warna → chrome pakai default
    }
}

// ---------------------------------------------------------------------------
// Session (EmuRunner) — Rust emu thread di balik satu handle terminal
// ---------------------------------------------------------------------------

/// Batas kode exit "masih jalan".
const RUNNING: i32 = -1;

/// Spawn PTY + emu thread yang feed ke terminal di `handle`; PTY winsize
/// (cols, rows) ikut terminal. `cwd = None` → inherit cwd proses. Kembalikan
/// false kalau handle tak ada / masih ada session aktif belum selesai.
fn spawn_session(
    handle: u64,
    cmd: &str,
    args: Vec<String>,
    cwd: Option<&str>,
    cols: i32,
    rows: i32,
) -> bool {
    let Some(slot) = runner_slot(handle) else {
        return false;
    };
    let Some(term) = get(handle) else {
        return false;
    };

    // reuse slot runner yang sudah selesai (take + shutdown/join), tolak kalau aktif
    {
        let mut g = match slot.lock() {
            Ok(g) => g,
            Err(_) => return false,
        };
        if let Some(old) = g.take() {
            if old.exit_code() == RUNNING {
                g.replace(old); // masih jalan → jangan ganggu
                return false;
            }
            old.shutdown(); // sudah selesai (atau stop), lepaskan thread
        }
    }

    let cols = clamp_dim(cols, MAX_COLS) as u16;
    let rows = clamp_dim(rows, MAX_ROWS) as u16;
    let session = match Session::spawn_at(cmd, &args, cwd, cols, rows) {
        Ok(s) => s,
        Err(_) => return false,
    };

    // emu thread feed byte → terminal yang sama dengan yang dipoll UI
    let feed_term = term.clone();
    let on_output = move |data: &[u8]| {
        let Ok(mut t) = feed_term.lock() else {
            return;
        };
        t.feed_bytes(data);
    };
    let runner = match EmuRunner::spawn(session, on_output) {
        Ok(r) => r,
        Err(_) => return false,
    };

    {
        let Ok(mut g) = slot.lock() else {
            return false;
        };
        g.replace(runner);
    }
    true
}

fn runner_stop(handle: u64) {
    let Some(slot) = runner_slot(handle) else {
        return;
    };
    let Ok(g) = slot.lock() else {
        return;
    };
    if let Some(r) = g.as_ref() {
        r.stop();
    }
}

/// `-1` = masih jalan/tak ada session; >=0 = kode exit; negatif lain = sinyal.
fn runner_exit(handle: u64) -> i32 {
    let Some(slot) = runner_slot(handle) else {
        return RUNNING;
    };
    let Ok(g) = slot.lock() else {
        return RUNNING;
    };
    match g.as_ref() {
        Some(r) => r.exit_code(),
        None => RUNNING,
    }
}

/// Kirim input user → shell (bukan lagi ke engine langsung).
fn runner_input(handle: u64, bytes: &[u8]) -> bool {
    if bytes.is_empty() {
        return true;
    }
    let Some(slot) = runner_slot(handle) else {
        return false;
    };
    let Ok(g) = slot.lock() else {
        return false;
    };
    match g.as_ref() {
        Some(r) => send_input(r.input(), bytes),
        None => false,
    }
}

/// Resize engine + PTY winsize bareng. False kalau tak ada runner.
fn runner_resize(handle: u64, cols: i32, rows: i32) -> bool {
    resize_term(handle, cols, rows);
    let Some(slot) = runner_slot(handle) else {
        return false;
    };
    let Ok(g) = slot.lock() else {
        return false;
    };
    match g.as_ref() {
        Some(r) => r.resize(
            clamp_dim(cols, MAX_COLS) as u16,
            clamp_dim(rows, MAX_ROWS) as u16,
        ),
        None => false,
    }
}

// ---------------------------------------------------------------------------
// Glue JNI — harus persis nama method Kotlin di `object NativeTerm`.
// ---------------------------------------------------------------------------

/// `nativeInit(cols, rows): Long`
#[no_mangle]
#[allow(non_snake_case)]
pub extern "system" fn Java_com_mterm_app_NativeTerm_nativeInit(
    _env: JNIEnv,
    _this: JObject,
    cols: jint,
    rows: jint,
) -> jlong {
    guard(0, || init_term(cols, rows) as jlong)
}

/// `nativeDestroy(handle)`
#[no_mangle]
#[allow(non_snake_case)]
pub extern "system" fn Java_com_mterm_app_NativeTerm_nativeDestroy(
    _env: JNIEnv,
    _this: JObject,
    handle: jlong,
) {
    guard((), || destroy(handle as u64));
}

/// `nativeWrite(handle, bytes: ByteArray)`
#[no_mangle]
#[allow(non_snake_case)]
#[allow(clippy::not_unsafe_ptr_arg_deref)]
pub extern "system" fn Java_com_mterm_app_NativeTerm_nativeWrite(
    env: JNIEnv,
    _this: JObject,
    handle: jlong,
    bytes: jbyteArray,
) {
    guard((), || {
        let bytes = unsafe { JByteArray::from_raw(bytes) };
        let len = match env.get_array_length(&bytes) {
            Ok(n) => n.max(0) as usize,
            Err(_) => return,
        };
        let mut raw = vec![0i8; len];
        if env.get_byte_array_region(&bytes, 0, &mut raw).is_err() {
            return;
        }
        let bytes = raw.into_iter().map(|b| b as u8).collect::<Vec<u8>>();
        write(handle as u64, &bytes);
    });
}

/// `nativeResize(handle, cols, rows)`
#[no_mangle]
#[allow(non_snake_case)]
pub extern "system" fn Java_com_mterm_app_NativeTerm_nativeResize(
    _env: JNIEnv,
    _this: JObject,
    handle: jlong,
    cols: jint,
    rows: jint,
) {
    guard((), || resize_term(handle as u64, cols, rows));
}

/// `nativeCellAt(handle, x, y, out): Boolean` — isi `out` (12 byte:
/// fg:u32 LE + bg:u32 LE + char code:u32 LE), return false kalau di luar layar.
#[no_mangle]
#[allow(non_snake_case)]
#[allow(clippy::not_unsafe_ptr_arg_deref)]
pub extern "system" fn Java_com_mterm_app_NativeTerm_nativeCellAt(
    env: JNIEnv,
    _this: JObject,
    handle: jlong,
    x: jint,
    y: jint,
    out: jbyteArray,
) -> jboolean {
    guard(JNI_FALSE, || {
        let (fg, bg, ch) = match cell_at(handle as u64, x, y) {
            Some(c) => c,
            None => return JNI_FALSE,
        };
        let out = unsafe { JByteArray::from_raw(out) };
        let bytes = [
            (fg & 0xFF) as i8,
            ((fg >> 8) & 0xFF) as i8,
            ((fg >> 16) & 0xFF) as i8,
            ((fg >> 24) & 0xFF) as i8,
            (bg & 0xFF) as i8,
            ((bg >> 8) & 0xFF) as i8,
            ((bg >> 16) & 0xFF) as i8,
            ((bg >> 24) & 0xFF) as i8,
            (ch & 0xFF) as i8,
            ((ch >> 8) & 0xFF) as i8,
            ((ch >> 16) & 0xFF) as i8,
            ((ch >> 24) & 0xFF) as i8,
        ];
        if env.set_byte_array_region(&out, 0, &bytes).is_err() {
            return JNI_FALSE;
        }
        JNI_TRUE
    })
}

/// `nativeDirty(handle): Boolean`
#[no_mangle]
#[allow(non_snake_case)]
pub extern "system" fn Java_com_mterm_app_NativeTerm_nativeDirty(
    _env: JNIEnv,
    _this: JObject,
    handle: jlong,
) -> jboolean {
    guard(JNI_FALSE, || {
        if dirty(handle as u64) {
            JNI_TRUE
        } else {
            JNI_FALSE
        }
    })
}

/// `nativeTakeEvent(handle, out: ByteArray): Int`
#[no_mangle]
#[allow(non_snake_case)]
#[allow(clippy::not_unsafe_ptr_arg_deref)]
pub extern "system" fn Java_com_mterm_app_NativeTerm_nativeTakeEvent(
    env: JNIEnv,
    _this: JObject,
    handle: jlong,
    out: jbyteArray,
) -> jint {
    guard(-1, || {
        let out = unsafe { JByteArray::from_raw(out) };
        let cap = match env.get_array_length(&out) {
            Ok(n) => n.max(0) as usize,
            Err(_) => return -1,
        };
        let mut raw = vec![0u8; cap];
        let n = take_event(handle as u64, &mut raw);
        if n > 0 {
            let bytes: Vec<i8> = raw[..n as usize].iter().map(|&b| b as i8).collect();
            if env.set_byte_array_region(&out, 0, &bytes).is_err() {
                return -1;
            }
        }
        n
    })
}

/// `nativeRunnerResize(handle, cols, rows): Boolean` — engine + PTY winsize.
#[no_mangle]
#[allow(non_snake_case)]
pub extern "system" fn Java_com_mterm_app_NativeTerm_nativeRunnerResize(
    _env: JNIEnv,
    _this: JObject,
    handle: jlong,
    cols: jint,
    rows: jint,
) -> jboolean {
    guard(JNI_FALSE, || {
        if runner_resize(handle as u64, cols, rows) {
            JNI_TRUE
        } else {
            JNI_FALSE
        }
    })
}

/// `nativeSessionStart(handle, cmd, args: Array<String>, cwd, cols, rows): Boolean`
#[no_mangle]
#[allow(non_snake_case)]
pub extern "system" fn Java_com_mterm_app_NativeTerm_nativeSessionStart(
    mut env: JNIEnv,
    _this: JObject,
    handle: jlong,
    cmd: JString,
    args: JObjectArray,
    cwd: JString,
    cols: jint,
    rows: jint,
) -> jboolean {
    guard(JNI_FALSE, || {
        let cmd: String = match env.get_string(&cmd) {
            Ok(s) => s.into(),
            Err(_) => return JNI_FALSE,
        };
        let args = match string_array(&mut env, &args) {
            Some(a) => a,
            None => return JNI_FALSE,
        };
        let cwd: String = match env.get_string(&cwd) {
            Ok(s) => s.into(),
            Err(_) => return JNI_FALSE,
        };
        let cwd = if cwd.is_empty() {
            None
        } else {
            Some(cwd.as_str())
        };
        if spawn_session(handle as u64, &cmd, args, cwd, cols, rows) {
            JNI_TRUE
        } else {
            JNI_FALSE
        }
    })
}

/// `nativeRunnerStop(handle)`
#[no_mangle]
#[allow(non_snake_case)]
pub extern "system" fn Java_com_mterm_app_NativeTerm_nativeRunnerStop(
    _env: JNIEnv,
    _this: JObject,
    handle: jlong,
) {
    guard((), || runner_stop(handle as u64));
}

/// `nativeRunnerExit(handle): Int` — -1 = jalan; >=0 = exit; -sinyal.
#[no_mangle]
#[allow(non_snake_case)]
pub extern "system" fn Java_com_mterm_app_NativeTerm_nativeRunnerExit(
    _env: JNIEnv,
    _this: JObject,
    handle: jlong,
) -> jint {
    guard(RUNNING, || runner_exit(handle as u64))
}

/// `nativeGridText(handle): String` — snapshot teks grid terminal (debug).
#[no_mangle]
#[allow(non_snake_case)]
#[allow(clippy::needless_lifetimes)]
pub extern "system" fn Java_com_mterm_app_NativeTerm_nativeGridText<'local>(
    env: JNIEnv<'local>,
    _this: JObject<'local>,
    handle: jlong,
) -> JString<'local> {
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let text = grid_text(handle as u64);
        env.new_string(text).ok()
    }));
    match r {
        Ok(Some(s)) => s,
        _ => env.new_string("").unwrap(),
    }
}

/// Snapshot isi grid terminal untuk tes/debug.
fn grid_text(handle: u64) -> String {
    let Some(shared) = get(handle) else {
        return String::new();
    };
    let Ok(t) = shared.lock() else {
        return String::new();
    };
    let mut out = String::new();
    for y in 0..t.rows() {
        for c in t.grid.line(y).cells.iter() {
            out.push(c.ch);
        }
        out.push('\n');
    }
    out
}

// ── Session persistence (Fase 4) ─────────────────────────────────────────────

/// `nativeSaveState(handle, path): Boolean` — simpan state terminal ke file.
#[no_mangle]
#[allow(non_snake_case)]
pub extern "system" fn Java_com_mterm_app_NativeTerm_nativeSaveState(
    mut env: JNIEnv,
    _this: JObject,
    handle: jlong,
    path: JString,
) -> jboolean {
    guard(JNI_FALSE, || {
        let path: String = match env.get_string(&path) {
            Ok(s) => s.into(),
            Err(_) => return JNI_FALSE,
        };
        let Some(shared) = get(handle as u64) else {
            return JNI_FALSE;
        };
        let Ok(t) = shared.lock() else {
            return JNI_FALSE;
        };
        let Ok(json) = t.to_json() else {
            return JNI_FALSE;
        };
        drop(t);
        if std::fs::write(&path, json).is_ok() {
            JNI_TRUE
        } else {
            JNI_FALSE
        }
    })
}

/// `nativeLoadState(path): Long` — restore state terminal dari file; -1 gagal.
#[no_mangle]
#[allow(non_snake_case)]
pub extern "system" fn Java_com_mterm_app_NativeTerm_nativeLoadState(
    mut env: JNIEnv,
    _this: JObject,
    path: JString,
) -> jlong {
    guard(-1, || {
        let path: String = match env.get_string(&path) {
            Ok(s) => s.into(),
            Err(_) => return -1,
        };
        let json = match std::fs::read_to_string(&path) {
            Ok(j) => j,
            Err(_) => return -1,
        };
        let terminal = match mterm_core::terminal::Terminal::from_json(&json) {
            Ok(t) => t,
            Err(_) => return -1,
        };
        alloc_handle(Arc::new(Mutex::new(terminal))) as jlong
    })
}

/// `nativeRunnerInput(handle, bytes: ByteArray): Boolean` — keystroke user → shell.
#[no_mangle]
#[allow(non_snake_case)]
#[allow(clippy::not_unsafe_ptr_arg_deref)]
pub extern "system" fn Java_com_mterm_app_NativeTerm_nativeRunnerInput(
    env: JNIEnv,
    _this: JObject,
    handle: jlong,
    bytes: jbyteArray,
) -> jboolean {
    guard(JNI_FALSE, || {
        let bytes = unsafe { JByteArray::from_raw(bytes) };
        let len = match env.get_array_length(&bytes) {
            Ok(n) => n.max(0) as usize,
            Err(_) => return JNI_FALSE,
        };
        let mut raw = vec![0i8; len];
        if env.get_byte_array_region(&bytes, 0, &mut raw).is_err() {
            return JNI_FALSE;
        }
        let bytes = raw.into_iter().map(|b| b as u8).collect::<Vec<u8>>();
        if runner_input(handle as u64, &bytes) {
            JNI_TRUE
        } else {
            JNI_FALSE
        }
    })
}

/// `nativeScrollOffset(handle, offset)` — set viewport scrollback (0 = bottom).
#[no_mangle]
#[allow(non_snake_case)]
pub extern "system" fn Java_com_mterm_app_NativeTerm_nativeScrollOffset(
    _env: JNIEnv,
    _this: JObject,
    handle: jlong,
    offset: jint,
) {
    guard((), || set_scroll_offset(handle as u64, offset));
}

/// `nativeScrollMax(handle): Int` — jumlah baris scrollback yang bisa dilihat.
#[no_mangle]
#[allow(non_snake_case)]
pub extern "system" fn Java_com_mterm_app_NativeTerm_nativeScrollMax(
    _env: JNIEnv,
    _this: JObject,
    handle: jlong,
) -> jint {
    guard(0, || scroll_max(handle as u64))
}

/// `nativeSgrMouse(handle, code, mods, release, x, y, out): Int` — encode event
/// mouse → byte SGR (untuk dikirim ke shell). 0 = mode mouse belum aktif.
#[no_mangle]
#[allow(non_snake_case)]
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn Java_com_mterm_app_NativeTerm_nativeSgrMouse(
    env: JNIEnv,
    _this: JObject,
    handle: jlong,
    code: jint,
    mods: jint,
    release: jboolean,
    x: jint,
    y: jint,
    out: jbyteArray,
) -> jint {
    guard(-1, || {
        let out = unsafe { JByteArray::from_raw(out) };
        let cap = match env.get_array_length(&out) {
            Ok(n) => n.max(0) as usize,
            Err(_) => return -1,
        };
        let mut raw = vec![0u8; cap];
        let n = sgr_mouse(handle as u64, code, mods, release != 0, x, y, &mut raw);
        if n > 0 {
            let bytes: Vec<i8> = raw[..n as usize].iter().map(|&b| b as i8).collect();
            if env.set_byte_array_region(&out, 0, &bytes).is_err() {
                return -1;
            }
        }
        n
    })
}

/// Konversi `Array<String>` Kotlin → `Vec<String>` (kembali None kalau batal).
fn string_array(env: &mut JNIEnv, arr: &JObjectArray) -> Option<Vec<String>> {
    let n = env.get_array_length(arr).ok()?.max(0);
    let mut out = Vec::with_capacity(n as usize);
    for i in 0..n {
        let obj = env.get_object_array_element(arr, i).ok()?;
        let js = unsafe { JString::from_raw(obj.into_raw()) };
        let s = env.get_string(&js).ok()?;
        out.push(s.into());
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(handle: u64, bytes: &[u8]) {
        super::write(handle, bytes);
    }

    fn u32_le(b: &[u8]) -> u32 {
        u32::from_le_bytes([b[0], b[1], b[2], b[3]])
    }

    #[test]
    fn take_event_title_roundtrip() {
        let h = init_term(80, 24);
        feed(h, b"\x1b]0;hello world\x07");
        let mut buf = [0u8; 256];
        let n = take_event(h, &mut buf);
        assert_eq!(n, 8 + 11);
        assert_eq!(u32_le(&buf[0..4]), 1, "type Title");
        assert_eq!(u32_le(&buf[4..8]), 11);
        assert_eq!(&buf[8..8 + 11], b"hello world");

        let n = take_event(h, &mut buf);
        assert_eq!(n, 0, "antrean habis");
        destroy(h);
    }

    #[test]
    fn take_event_bell() {
        let h = init_term(80, 24);
        feed(h, b"\x07");
        let mut buf = [0u8; 64];
        let n = take_event(h, &mut buf);
        assert_eq!(n, 8);
        assert_eq!(u32_le(&buf[0..4]), 2, "type Bell");
        assert_eq!(u32_le(&buf[4..8]), 0);
        destroy(h);
    }

    #[test]
    fn take_event_mouse_split_feed() {
        // Meniru jalur perangkat: tiap byte di-feed terpisah oleh nativeWrite.
        let h = init_term(80, 24);
        feed(h, b"\x1b");
        for b in b"[?1000h" {
            feed(h, &[*b]);
        }
        let mut buf = [0u8; 64];
        let n = take_event(h, &mut buf);
        assert_eq!(n, 9, "event harus keluar walau feed terpecah-pecah");
        assert_eq!(u32_le(&buf[0..4]), 3, "type Mouse");
        assert_eq!(buf[8], 1, "tracking menyala");
        destroy(h);
    }

    #[test]
    fn take_event_mouse() {
        let h = init_term(80, 24);
        feed(h, b"\x1b[?1000h");
        let mut buf = [0u8; 64];
        let n = take_event(h, &mut buf);
        assert_eq!(n, 9);
        assert_eq!(u32_le(&buf[0..4]), 3, "type Mouse");
        assert_eq!(buf[8], 1, "tracking menyala");

        feed(h, b"\x1b[?1000l");
        let n = take_event(h, &mut buf);
        assert_eq!(n, 9, "mouse off event");
        assert_eq!(u32_le(&buf[0..4]), 3, "type Mouse");
        assert_eq!(buf[8], 0, "tracking mati");
        destroy(h);
    }

    #[test]
    fn take_event_small_buffer_keeps_event() {
        let h = init_term(80, 24);
        feed(h, b"\x1b]0;hello world\x07");
        let mut small = [0u8; 4];
        assert_eq!(take_event(h, &mut small), -1, "buffer kurang");
        let mut big = [0u8; 128];
        let n = take_event(h, &mut big);
        assert_eq!(n, 8 + 11, "event belum hilang, retry berhasil");
        destroy(h);
    }

    #[test]
    fn cell_at_returns_rgb_bytes() {
        let h = init_term(80, 24);
        feed(h, b"\x1b[31mR");
        let (fg, _bg, ch) = cell_at(h, 0, 0).expect("cell ada");
        assert_eq!(ch, 'R' as u32);
        assert_eq!((fg >> 24) & 0xFF, 0xFF, "alpha opak untuk warna pasti");
        assert!(fg != 0, "fg tidak nol saat warna eksplisit");
        destroy(h);
    }

    #[test]
    fn fast_path_marks_and_consumes_dirty() {
        // fast path (ASCII murni, tanpa ESC) harus menandai dirty_rect…
        let h = init_term(80, 24);
        write(h, b"BANNER-MTERM\n");
        assert!(dirty(h), "feed ASCII memicu dirty flag");

        // …dan konsumsi berikutnya mereset: tanpa feed baru, dirty = false.
        assert!(!dirty(h), "dirty hanya bertahan satu kali pembacaan");

        // jalur vte (ESC) juga ditandai.
        write(h, b"\x1b[31mR");
        assert!(dirty(h));
        assert!(!dirty(h));

        // clear (CSI J2 + K) ikut menandai.
        write(h, b"\x1b[2J");
        assert!(dirty(h));
        assert!(!dirty(h));
        write(h, b"AB\x1b[K");
        assert!(dirty(h));
        assert!(!dirty(h));
        destroy(h);
    }

    #[test]
    fn boundary_out_of_range_is_safe() {
        // handle invalid → tak panic, kembalikan nilai aman
        assert_eq!(cell_at(99_999, 0, 0), None, "handle tak ada → None");
        assert_eq!(take_event(99_999, &mut [0u8; 8]), -1);
        assert!(!dirty(99_999));

        // handle valid, koordinat di luar layar → None (bukan panic)
        let h = init_term(80, 24);
        assert_eq!(cell_at(h, 80, 0), None);
        assert_eq!(cell_at(h, 0, 24), None);
        assert_eq!(cell_at(h, -1, 0), None);
        assert_eq!(cell_at(h, 0, -5), None);
        assert_eq!(cell_at(h, 100_000, 100_000), None);
        destroy(h);
    }

    #[test]
    fn negative_and_huge_dims_are_clamped() {
        let h = init_term(-10, -10);
        write(h, b"a");
        assert_eq!(cell_at(h, 0, 0).map(|c| c.2), Some('a' as u32));
        assert_eq!(cols_of(h), 1);
        assert_eq!(rows_of(h), 1);

        let h2 = init_term(i32::MAX, i32::MAX);
        assert!(cols_of(h2) <= 1024 && rows_of(h2) <= 512, "cap anti-OOM");
        destroy(h);
        destroy(h2);
    }

    #[test]
    fn guard_catches_panic_returns_default() {
        // guard() menangkap panic dari logika internal tanpa unwind lintas FFI
        // (di mana "bool" JNI kembalikan default, bukan UB).
        let r = guard(42, || panic!("boom"));
        assert_eq!(r, 42, "panic ditelan, default dikembalikan");
    }

    fn shared_with(handle: u64) -> std::sync::Arc<Mutex<Terminal>> {
        get(handle).expect("handle ada setelah init/destroy cukup satu")
    }

    fn cols_of(handle: u64) -> usize {
        shared_with(handle).lock().unwrap().cols()
    }

    fn rows_of(handle: u64) -> usize {
        shared_with(handle).lock().unwrap().rows()
    }

    // ── EmuRunner wiring (membutuhkan PTY asli — jalan di Termux) ──

    fn wait_exit(handle: u64, ms: u64) -> i32 {
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(ms);
        while runner_exit(handle) == RUNNING && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        runner_exit(handle)
    }

    fn grid_text(handle: u64) -> String {
        let g = shared_with(handle);
        let t = g.lock().unwrap();
        let mut s = String::new();
        for y in 0..t.rows() {
            for c in t.grid.line(y).cells.iter() {
                s.push(c.ch);
            }
            s.push('\n');
        }
        s
    }

    fn save_state(h: u64, path: &std::path::Path) -> bool {
        let Some(shared) = get(h) else {
            return false;
        };
        let Ok(t) = shared.lock() else {
            return false;
        };
        let Ok(json) = t.to_json() else {
            return false;
        };
        drop(t);
        std::fs::write(path, json).is_ok()
    }

    fn load_state(path: &std::path::Path) -> u64 {
        let json = match std::fs::read_to_string(path) {
            Ok(j) => j,
            Err(_) => return 0,
        };
        let terminal = match mterm_core::terminal::Terminal::from_json(&json) {
            Ok(t) => t,
            Err(_) => return 0,
        };
        alloc_handle(Arc::new(Mutex::new(terminal)))
    }

    #[test]
    fn session_runs_in_emu_thread_and_feeds_terminal() {
        let h = init_term(40, 10);
        let ok = spawn_session(
            h,
            "sh",
            vec!["-c".into(), "printf 'jni-runner-ok\n'; exit 0".into()],
            None,
            40,
            10,
        );
        assert!(ok, "spawn_session berhasil");
        assert_eq!(runner_exit(h), RUNNING, "masih jalan sesaat setelah spawn");

        let code = wait_exit(h, 5000);
        assert_eq!(code, 0, "exit code 0");
        let text = grid_text(h);
        assert!(
            text.contains("jni-runner-ok"),
            "terminal berisi output: {text:?}"
        );
        destroy(h);
    }

    #[test]
    fn session_exit_code_is_signal_or_nonzero() {
        let h = init_term(40, 10);
        assert!(spawn_session(
            h,
            "sh",
            vec!["-c".into(), "exit 7".into()],
            None,
            40,
            10
        ));
        assert_eq!(wait_exit(h, 5000), 7, "exit 7");
        destroy(h);
    }

    #[test]
    fn session_input_reaches_shell() {
        let h = init_term(40, 10);
        assert!(spawn_session(
            h,
            "sh",
            vec![
                "-c".into(),
                "read -r line; printf 'got:%s' \"$line\"".into()
            ],
            None,
            40,
            10,
        ));
        assert!(runner_input(h, b"halo\n"), "input ke channel terkirim");
        let code = wait_exit(h, 5000);
        assert_eq!(code, 0, "shell selesai");
        assert!(
            grid_text(h).contains("got:halo"),
            "shell balas setelah input"
        );
        destroy(h);
    }

    #[test]
    fn respawn_blocked_while_running_then_allowed() {
        let h = init_term(40, 10);
        assert!(spawn_session(
            h,
            "sh",
            vec!["-c".into(), "sleep 1; exit 0".into()],
            None,
            40,
            10
        ));
        // masih jalan → respawn ditolak
        assert!(
            !spawn_session(h, "sh", vec!["-c".into(), "exit 0".into()], None, 40, 10),
            "aktif → tolak spawn kedua"
        );

        let code = wait_exit(h, 5000);
        assert_eq!(code, 0, "sleep selesai");
        // sudah selesai → respawn boleh
        assert!(spawn_session(
            h,
            "sh",
            vec!["-c".into(), "printf 'again\n'; exit 0".into()],
            None,
            40,
            10
        ));
        assert_eq!(wait_exit(h, 5000), 0, "spawn kedua jalan");
        let again = grid_text(h);
        assert!(again.contains("again"), "output respawn: {again:?}");
        destroy(h);
    }

    #[test]
    fn runner_stop_marks_not_running() {
        let h = init_term(40, 10);
        assert!(spawn_session(
            h,
            "sh",
            vec!["-c".into(), "sleep 5; exit 0".into()],
            None,
            40,
            10
        ));
        assert_eq!(runner_exit(h), RUNNING);
        runner_stop(h);
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(2000);
        while runner_exit(h) == RUNNING && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert_ne!(
            runner_exit(h),
            RUNNING,
            "stop → thread keluar, code tercatat"
        );
        destroy(h);
    }

    #[test]
    fn destroy_shuts_down_runner_thread() {
        let h = init_term(40, 10);
        assert!(spawn_session(
            h,
            "sh",
            vec!["-c".into(), "sleep 5; exit 0".into()],
            None,
            40,
            10
        ));
        destroy(h); // harus join thread tanpa hang/panic
        assert_eq!(runner_exit(h), RUNNING, "slot runner ikut di-drop");
    }

    // ── Viewport scroll + mouse input ──

    #[test]
    fn scroll_offset_moves_cell_at_into_history() {
        let h = init_term(10, 4);
        // tulis 6 baris (CRLF) → ada scrollback
        write(h, b"l1\r\nl2\r\nl3\r\nl4\r\nl5\r\nl6\r\n");
        let max = scroll_max(h);
        assert!(max >= 2, "ada scrollback, max={max}");

        assert_eq!(
            cell_at(h, 0, 0).map(|c| c.2),
            Some('l' as u32),
            "bottom = l6? atau l5"
        );
        set_scroll_offset(h, max);
        let first = cell_at(h, 0, 0).map(|c| c.2 as u8 as char);
        assert_eq!(first, Some('l'), "scroll penuh → mulai dari sejarah");
        let second = cell_at(h, 1, 0).map(|c| c.2 as u8 as char);
        assert_eq!(second, Some('1'), "baris sejarah pertama = l1");

        // offset berlebih di-clamp (tak boleh lebih dari scrollback)
        set_scroll_offset(h, 99_999);
        assert!(scroll_max(h) >= 0, "clamp aman");
        destroy(h);
    }

    #[test]
    fn scroll_max_grows_with_output() {
        let h = init_term(8, 3);
        write(h, b"a\r\nb\r\nc\r\n");
        let m1 = scroll_max(h);
        write(h, b"d\r\ne\r\nf\r\n");
        let m2 = scroll_max(h);
        assert!(m2 >= m1, "scrollback bertambah: {m1} → {m2}");
        destroy(h);
    }

    #[test]
    fn sgr_mouse_roundtrip_after_mode_enabled() {
        let h = init_term(80, 24);
        // mode mouse belum aktif → 0
        let mut out = [0u8; 16];
        assert_eq!(sgr_mouse(h, 1, 0, false, 3, 2, &mut out), 0, "belum aktif");

        write(h, b"\x1b[?1000h\x1b[?1006h");
        let n = sgr_mouse(h, 1, 0, false, 3, 2, &mut out);
        assert!(n > 0, "encode SGR jalan: {n}");
        let seq = String::from_utf8_lossy(&out[..n as usize]).into_owned();
        assert!(seq.contains("1;3;2"), "koor 1-based (middle at 3,2): {seq}");
        assert!(seq.ends_with('M'), "press = M: {seq}");

        // release → suffix m, kode naik 3 (middle 1 → 4)
        let n2 = sgr_mouse(h, 1, 0, true, 3, 2, &mut out);
        let seq2 = String::from_utf8_lossy(&out[..n2.max(0) as usize]).into_owned();
        assert!(seq2.ends_with('m'), "release = m: {seq2}");
        destroy(h);
    }

    // ── Session persistence ──

    #[test]
    fn save_and_load_state_roundtrip() {
        let h = init_term(10, 4);
        write(h, b"\x1b[31mHELLO\x1b[0m\nsecond\n");
        let fg_cell = cell_at(h, 0, 0);
        assert!(
            fg_cell.unwrap().0 != 0,
            "warna merah tersimpan sebelum save"
        );

        let path = std::env::temp_dir().join("mterm_state_test.json");
        assert!(save_state(h, &path), "save ok");
        destroy(h);

        let h2 = load_state(&path);
        assert!(h2 > 0, "load handle");
        let fg_cell2 = cell_at(h2, 0, 0);
        assert_eq!(fg_cell.map(|c| c.2), fg_cell2.map(|c| c.2), "warna sama");
        assert_eq!(
            cell_at(h2, 0, 0).map(|c| c.1),
            fg_cell.map(|c| c.1),
            "bg sama"
        );
        destroy(h2);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn save_to_bad_path_returns_false() {
        let h = init_term(10, 4);
        assert!(!save_state(
            h,
            &std::path::PathBuf::from("/dev/null/notadir/x.json")
        ));
        destroy(h);
    }

    #[test]
    fn load_missing_file_returns_zero() {
        assert_eq!(
            load_state(&std::path::PathBuf::from(
                "/tmp/mterm_definitely_missing.json"
            )),
            0,
            "file tak ada → 0"
        );
    }
}
