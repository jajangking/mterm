//! E2E thread-model Fase 2: Rust emu thread feed engine, caller (main thread)
//! poll. Ini kontrak yang sama dengan loop polling Kotlin nanti:
//!
//! - thread `mterm-emu` membaca PTY → `Terminal::feed_bytes`
//! - main thread poll `exit_code()` + baca grid
//! - tanpa blokir: baca grid tidak menunggu I/O PTY

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mterm_core::terminal::{Terminal, TerminalConfig};
use mterm_pty::runner::EmuRunner;
use mterm_pty::Session;

fn grid_text(term: &Terminal) -> String {
    let mut s = String::new();
    for y in 0..term.rows() {
        let line: String = term.grid.line(y).cells.iter().map(|c| c.ch).collect();
        s.push_str(&line);
        s.push('\n');
    }
    s
}

#[test]
fn emu_thread_feeds_engine_main_thread_polls() {
    let term = Arc::new(Mutex::new(Terminal::new(TerminalConfig {
        cols: 40,
        rows: 10,
        scrollback_cap: 1000,
    })));

    let session = Session::spawn("sh", &["-c".into(), "printf 'thread-model-ok\n'; exit 0".into()], 40, 10)
        .expect("spawn sh");

    let feed = {
        let term = Arc::clone(&term);
        move |data: &[u8]| {
            term.lock().unwrap().feed_bytes(data);
        }
    };
    let runner = EmuRunner::spawn(session, feed).expect("spawn emu runner");

    // ── main thread: poll persis seperti render loop Kotlin ──
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut text = String::new();
    while runner.is_running() && Instant::now() < deadline {
        let t = {
            let g = term.lock().unwrap();
            grid_text(&g)
        };
        if t.contains("thread-model-ok") {
            text = t;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    // tunggu reap (exit-code tercatat), max 5s
    while runner.is_running() && Instant::now() < deadline + Duration::from_millis(100) {
        std::thread::sleep(Duration::from_millis(10));
    }

    assert_eq!(runner.exit_code(), 0, "exit 0, running={}", runner.is_running());
    let t = if text.is_empty() {
        grid_text(&term.lock().unwrap())
    } else {
        text
    };
    assert!(t.contains("thread-model-ok"), "grid berisi output: {t:?}");
    runner.shutdown();
}