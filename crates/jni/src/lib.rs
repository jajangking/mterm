//! mterm-jni: boundary rust <-> Kotlin (Android).
//!
//! Semua state dibungkus `Arc<Mutex<>>`; fn dialihkan murni agar aman
//! dipanggil dari thread mana pun (MainThread, EMuThread, IO thread).

use std::sync::{Arc, Mutex};

use mterm_core::terminal::{Terminal, TerminalConfig, TerminalEvent};

type SharedTerminal = Arc<Mutex<Terminal>>;

#[derive(Default)]
struct Handles(Vec<SharedTerminal>);

static HANDLES: once_cell::sync::Lazy<Mutex<Handles>> =
    once_cell::sync::Lazy::new(|| Mutex::new(Handles::default()));

fn alloc_handle(term: SharedTerminal) -> u64 {
    let mut h = HANDLES.lock().unwrap();
    h.0.push(term);
    (h.0.len() - 1) as u64
}

fn get(handle: u64) -> Arc<Mutex<Terminal>> {
    HANDLES.lock().unwrap().0[handle as usize].clone()
}

// ---------------------------------------------------------------------------
// FFI surface (fungsi `#[no_mangle]` diekspor sebagai `extern "C"`)
// ---------------------------------------------------------------------------

/// `nativeInit(cols, rows) -> handle`
#[no_mangle]
pub extern "C" fn nativeInit(cols: i32, rows: i32) -> u64 {
    let cfg = TerminalConfig {
        cols: cols.max(1) as usize,
        rows: rows.max(1) as usize,
        ..Default::default()
    };
    alloc_handle(Arc::new(Mutex::new(Terminal::new(cfg))))
}

/// `nativeDestroy(handle)`
#[no_mangle]
pub extern "C" fn nativeDestroy(handle: u64) {
    let mut g = HANDLES.lock().unwrap();
    if let Some(term) = g.0.get_mut(handle as usize) {
        // drop Arong pemegang state; Arc lain akan membuat terminal
        *term = Arc::new(Mutex::new(Terminal::new(TerminalConfig::default())));
    }
}

/// `nativeWrite(handle, bytes, len)` — input byte (dari PTY/socket).
#[no_mangle]
#[allow(clippy::not_unsafe_ptr_arg_deref)]
pub extern "C" fn nativeWrite(handle: u64, ptr: *const u8, len: i32) {
    if ptr.is_null() || len <= 0 {
        return;
    }
    let bytes = unsafe { std::slice::from_raw_parts(ptr, len as usize) };
    let term = get(handle);
    {
        let mut t = term.lock().unwrap();
        let mut parser = vte::Parser::new();
        parser.advance(&mut *t, bytes);
    }
}

/// `nativeResize(handle, cols, rows)`
#[no_mangle]
pub extern "C" fn nativeResize(handle: u64, cols: i32, rows: i32) {
    let shared = get(handle);
    let mut t = shared.lock().unwrap();
    t.resize(cols.max(1) as usize, rows.max(1) as usize);
}

/// `nativeCellAt(handle, x, y, out) -> bool` — ambil satu cell untuk renderer.
/// `out` harus 12 byte: fg(u32 LE) + bg(u32 LE) + char code (u32 LE).
#[no_mangle]
#[allow(clippy::not_unsafe_ptr_arg_deref)]
pub extern "C" fn nativeCellAt(handle: u64, x: i32, y: i32, out: *mut u8) -> bool {
    if out.is_null() {
        return false;
    }
    let shared = get(handle);
    let t = shared.lock().unwrap();
    if x < 0 || y < 0 || x as usize >= t.cols() || y as usize >= t.rows() {
        return false;
    }
    let cell = &t.grid.line(y as usize).cells[x as usize];
    let fg = color_u32(cell.attrs.fg);
    let bg = color_u32(cell.attrs.bg);
    let ch = cell.ch as u32;
    let out = unsafe { std::slice::from_raw_parts_mut(out, 12) };
    out[0..4].copy_from_slice(&fg.to_le_bytes());
    out[4..8].copy_from_slice(&bg.to_le_bytes());
    out[8..12].copy_from_slice(&ch.to_le_bytes());
    true
}

/// `nativeDirty(handle) -> bool` — apakah ada cell yang perlu re-render.
#[no_mangle]
pub extern "C" fn nativeDirty(handle: u64) -> bool {
    let shared = get(handle);
    let t = shared.lock().unwrap();
    t.dirty_rect.is_some()
}

/// `nativeTakeEvent(handle, out, cap) -> n` — pop event terminal (polling).
///
/// Layout keluar: `[type:u32 LE][len:u32 LE][payload...]`.
/// - type 1 = Title (payload = utf8 judul)
/// - type 2 = Bell (tanpa payload)
///
/// Return: `n > 0` byte tertulis, `0` = tak ada event, `-1` = buffer kurang
/// besar (event TIDAK dikonsumsi, bisa dipanggil ulang dengan buffer lebih).
#[no_mangle]
#[allow(clippy::not_unsafe_ptr_arg_deref)]
pub extern "C" fn nativeTakeEvent(handle: u64, out: *mut u8, cap: i32) -> i32 {
    if out.is_null() || cap < 8 {
        return -1;
    }
    let shared = get(handle);
    let mut t = shared.lock().unwrap();
    let ev = match t.events.front().cloned() {
        Some(e) => e,
        None => return 0,
    };
    let (ty, payload): (u32, Vec<u8>) = match ev {
        TerminalEvent::Title(s) => (1, s.into_bytes()),
        TerminalEvent::Bell => (2, Vec::new()),
        TerminalEvent::Mouse(on) => (3, vec![u8::from(on)]),
        TerminalEvent::Hyperlink(_) => return 0,
    };
    let need = 8 + payload.len();
    if need > cap as usize {
        return -1; // bufer kurang → event tetap di antrean
    }
    t.events.pop_front(); // baru konsumsi kalau muat
    let out = unsafe { std::slice::from_raw_parts_mut(out, need) };
    out[0..4].copy_from_slice(&ty.to_le_bytes());
    out[4..8].copy_from_slice(&(payload.len() as u32).to_le_bytes());
    out[8..].copy_from_slice(&payload);
    need as i32
}

fn color_u32(c: mterm_core::grid::Color) -> u32 {
    match c.to_rgb24() {
        Some((r, g, b)) => {
            0xFF00_0000 | ((r as u32) << 16) | ((g as u32) << 8) | b as u32
        }
        None => 0, // tak ada warna → chrome pakai default
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(handle: u64, bytes: &[u8]) {
        nativeWrite(handle, bytes.as_ptr(), bytes.len() as i32);
    }

    fn u32_le(b: &[u8]) -> u32 {
        u32::from_le_bytes([b[0], b[1], b[2], b[3]])
    }

    #[test]
    fn take_event_title_roundtrip() {
        let h = nativeInit(80, 24);
        write(h, b"\x1b]0;hello world\x07");
        let mut buf = [0u8; 256];
        let n = nativeTakeEvent(h, buf.as_mut_ptr(), buf.len() as i32);
        assert_eq!(n, 8 + 11);
        assert_eq!(u32_le(&buf[0..4]), 1, "type Title");
        assert_eq!(u32_le(&buf[4..8]), 11);
        assert_eq!(&buf[8..8 + 11], b"hello world");

        let n = nativeTakeEvent(h, buf.as_mut_ptr(), buf.len() as i32);
        assert_eq!(n, 0, "antrean habis");
        nativeDestroy(h);
    }

    #[test]
    fn take_event_bell() {
        let h = nativeInit(80, 24);
        write(h, b"\x07");
        let mut buf = [0u8; 64];
        let n = nativeTakeEvent(h, buf.as_mut_ptr(), buf.len() as i32);
        assert_eq!(n, 8);
        assert_eq!(u32_le(&buf[0..4]), 2, "type Bell");
        assert_eq!(u32_le(&buf[4..8]), 0);
        nativeDestroy(h);
    }

    #[test]
    fn take_event_mouse() {
        let h = nativeInit(80, 24);
        write(h, b"\x1b[?1000h");
        let mut buf = [0u8; 64];
        let n = nativeTakeEvent(h, buf.as_mut_ptr(), buf.len() as i32);
        assert_eq!(n, 9);
        assert_eq!(u32_le(&buf[0..4]), 3, "type Mouse");
        assert_eq!(buf[8], 1, "tracking menyala");

        write(h, b"\x1b[?1000l");
        let n = nativeTakeEvent(h, buf.as_mut_ptr(), buf.len() as i32);
        assert_eq!(n, 9, "mouse off event");
        assert_eq!(u32_le(&buf[0..4]), 3, "type Mouse");
        assert_eq!(buf[8], 0, "tracking mati");
        nativeDestroy(h);
    }

    #[test]
    fn take_event_small_buffer_keeps_event() {
        let h = nativeInit(80, 24);
        write(h, b"\x1b]0;hello world\x07");
        let mut small = [0u8; 4];
        assert_eq!(
            nativeTakeEvent(h, small.as_mut_ptr(), small.len() as i32),
            -1,
            "buffer kurang"
        );
        let mut big = [0u8; 128];
        let n = nativeTakeEvent(h, big.as_mut_ptr(), big.len() as i32);
        assert_eq!(n, 8 + 11, "event belum hilang, retry berhasil");
        nativeDestroy(h);
    }
}
