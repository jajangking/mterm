//! mterm-jni: boundary rust <-> Kotlin (Android).
//!
//! Semua state dibungkus `Arc<Mutex<>>`; fn dialihkan murni agar aman
//! dipanggil dari thread mana pun (MainThread, EMuThread, IO thread).

use std::sync::{Arc, Mutex};

use mterm_core::terminal::{Terminal, TerminalConfig};

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

fn color_u32(c: mterm_core::grid::Color) -> u32 {
    use mterm_core::grid::Color;
    match c {
        Color::Default => 0x0000_0000, // chrome memberi default fg/bg
        Color::Indexed(i) => 0xFF00_0000 | (i as u32) << 8, // placeholder palette
        Color::Rgb(r, g, b) => 0xFF00_0000 | ((r as u32) << 16) | ((g as u32) << 8) | b as u32,
    }
}
