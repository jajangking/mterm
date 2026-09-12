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

use jni::objects::{JByteArray, JObject};
use jni::sys::{jboolean, jbyteArray, jint, jlong, JNI_FALSE, JNI_TRUE};
use jni::JNIEnv;

type SharedTerminal = Arc<Mutex<Terminal>>;

/// Batas dimensi layar (anti OOM-abort dari allocator, bukan panic).
const MAX_COLS: i32 = 1024;
const MAX_ROWS: i32 = 512;

/// Jalankan logika pendek JNI di dalam `catch_unwind`: panic di dalam mustahil
/// ndelok unwind ke JVM (UB/abort). Kembali `default` kalau panic.
fn guard<T>(default: T, f: impl FnOnce() -> T) -> T {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).unwrap_or(default)
}

#[derive(Default)]
struct Handles(Vec<SharedTerminal>);

static HANDLES: once_cell::sync::Lazy<Mutex<Handles>> =
    once_cell::sync::Lazy::new(|| Mutex::new(Handles::default()));

fn alloc_handle(term: SharedTerminal) -> u64 {
    let Ok(mut h) = HANDLES.lock() else {
        return u64::MAX;
    };
    h.0.push(term);
    (h.0.len() - 1) as u64
}

fn get(handle: u64) -> Option<SharedTerminal> {
    let Ok(g) = HANDLES.lock() else {
        return None;
    };
    g.0.get(handle as usize).cloned()
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

/// `cell_at(handle, x, y) -> Option<(fg_rgb24, bg_rgb24, ch)>`.
fn cell_at(handle: u64, x: i32, y: i32) -> Option<(u32, u32, u32)> {
    let shared = get(handle)?;
    let t = shared.lock().ok()?;
    if x < 0 || y < 0 || x as usize >= t.cols() || y as usize >= t.rows() {
        return None;
    }
    let cell = &t.grid.line(y as usize).cells[x as usize];
    Some((
        color_u32(cell.attrs.fg),
        color_u32(cell.attrs.bg),
        cell.ch as u32,
    ))
}

fn dirty(handle: u64) -> bool {
    let Some(shared) = get(handle) else {
        return false;
    };
    let Ok(t) = shared.lock() else {
        return false;
    };
    t.dirty_rect.is_some()
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
}
