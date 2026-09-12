//! E2E session persistence (Fase 4): PTY output di-feed → state disimpan ke
//! file JSON → terminal "di-restart" (restore) → output lanjutan tetap utuh.
//!
//! Mensimulasikan proses Android dibunuh lalu resume: grid+scrollback+cursor
//! selama ini, terus output baru ditambahkan.

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

fn wait_exit(runner: &EmuRunner, ms: u64) -> bool {
    let deadline = Instant::now() + Duration::from_millis(ms);
    while runner.is_running() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    !runner.is_running()
}

#[test]
fn pty_output_survives_save_restore() {
    let dir = std::env::temp_dir();
    let state_path = dir.join("mterm_persist_e2e.json");

    // ── Run 1: shell jalan, output "first-run", state disimpan ──
    let term = Arc::new(Mutex::new(Terminal::new(TerminalConfig {
        cols: 40,
        rows: 8,
        scrollback_cap: 200,
    })));
    let session = Session::spawn(
        "sh",
        &[
            "-c".into(),
            "printf 'first-run\n'; sleep 3; printf 'second-burst\n'; exit 0".into(),
        ],
        40,
        8,
    )
    .expect("spawn sh");

    let feed = {
        let term = Arc::clone(&term);
        move |data: &[u8]| term.lock().unwrap().feed_bytes(data)
    };
    let runner = EmuRunner::spawn(session, feed).expect("runner");
    assert!(wait_exit(&runner, 5000), "sh selesai dalam 5s");

    let grid1 = grid_text(&term.lock().unwrap());
    assert!(grid1.contains("first-run"), "run1 output ada: {grid1:?}");
    assert!(grid1.contains("second-burst"), "run1 kedua ada: {grid1:?}");

    let json = term.lock().unwrap().to_json().expect("serialize");
    std::fs::write(&state_path, json).expect("tulis state");
    runner.shutdown();

    // ── "Process mati". Restore dari file, output harus persis sama ──
    let json = std::fs::read_to_string(&state_path).expect("baca state");
    let restored = Terminal::from_json(&json).expect("restore");
    assert!(
        grid_text(&restored).contains("second-burst"),
        "restore membawa output run1"
    );

    // ── Run 2: lanjut feed output baru ke terminal restore ──
    let restored = Arc::new(Mutex::new(restored));
    let session2 = Session::spawn(
        "sh",
        &["-c".into(), "printf 'resumed\n'; exit 0".into()],
        40,
        8,
    )
    .unwrap();
    let feed2 = {
        let term = Arc::clone(&restored);
        move |data: &[u8]| term.lock().unwrap().feed_bytes(data)
    };
    let runner2 = EmuRunner::spawn(session2, feed2).unwrap();
    assert!(wait_exit(&runner2, 5000), "run2 selesai");
    let grid2 = grid_text(&restored.lock().unwrap());
    assert!(grid2.contains("resumed"), "output run2 ada: {grid2:?}");
    runner2.shutdown();

    let _ = std::fs::remove_file(&state_path);
}
