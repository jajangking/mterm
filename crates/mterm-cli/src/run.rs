//! `mterm run`: pipe output command → engine, render grid.
//!
//! Mode otomatis:
//! - stdin TTY → interaktif (raw mode, kirim input user ke PTY)
//! - stdin bukan TTY → sekali jalan, tak perlu terminal asli
//!
//! Render Linux-first (M2): ukuran dari terminal host via TIOCGWINSZ,
//! diteruskan ke PTY + resized otomatis; repaint hanya baris yang dirty.

use std::io::{self, Read, Write};
use std::os::unix::io::AsRawFd;
use std::time::Duration;

use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::execute;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, Clear, ClearType};
use mterm_core::grid::Color;
use mterm_core::terminal::{Terminal, TerminalConfig};
use mterm_pty::Session;

/// Batas maksimum tail yang ditulis ke collector stderr agent.
const STDERR_CAP: usize = 16 * 1024;

/// Ukuran terminal host via ioctl TIOCGWINSZ (crossterm::size hanya lihat
/// stdout; pakai stdin yang pasti TTY saat interaktif).
fn tty_size(fd: i32) -> Option<(u16, u16)> {
    unsafe {
        let mut w: libc::winsize = std::mem::zeroed();
        if libc::ioctl(fd, libc::TIOCGWINSZ, &mut w) == 0 && w.ws_col > 0 && w.ws_row > 0 {
            Some((w.ws_col, w.ws_row))
        } else {
            None
        }
    }
}

fn tty_dims() -> (u16, u16) {
    tty_size(std::io::stdin().as_raw_fd())
        .or_else(|| tty_size(std::io::stdout().as_raw_fd()))
        .unwrap_or((80, 24))
}

pub fn run(args: &[String]) -> io::Result<()> {
    let cmd = args.first().map(String::as_str).unwrap_or("sh");
    let cmd_args = &args[1..];
    let is_tty = std::io::IsTerminal::is_terminal(&std::io::stdin());

    let (cols, rows) = if is_tty { tty_dims() } else { (80, 24) };
    let (cols, rows) = (cols.max(1), rows.max(1));

    let mut session = Session::spawn(cmd, cmd_args, cols, rows)?;
    println!(
        "[mterm] pid={} pty {}x{} cmd: {}",
        session.pid,
        cols,
        rows,
        args.join(" ")
    );

    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    let pf = mterm_pty::PidFile::create(
        format!("{home}/.mterm-sessions/{}", session.pid),
        session.pid,
    )?;

    let mut term = Terminal::new(TerminalConfig {
        cols: cols as usize,
        rows: rows as usize,
        scrollback_cap: 10_000,
    });
    // Collector "stderr": ekor output PTY → dipakai agent saat /ask.
    let mut tail: Vec<u8> = Vec::with_capacity(2 * STDERR_CAP);

    if is_tty {
        run_interactive(&mut session, &mut term, &mut tail)?;
    } else {
        run_once(&mut session, &mut term, &mut tail)?;
    }

    let code = session.exited();
    let text = String::from_utf8_lossy(&tail).into_owned();
    let _ = crate::agent::write_stderr_capture(&text, code);

    pf.remove()?;
    Ok(())
}

fn run_once(session: &mut Session, term: &mut Terminal, tail: &mut Vec<u8>) -> io::Result<()> {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);

    loop {
        let _ = drain(session, term, tail);
        if let Some(code) = session.exited() {
            println!("[mterm] exit code: {code}");
            break;
        }
        if std::time::Instant::now() > deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let _ = drain(session, term, tail);

    // render hasil akhir sebagai teks polos (SGR dibersihkan)
    let stdout = io::stdout();
    let mut w = stdout.lock();
    for y in 0..term.rows() {
        for x in 0..term.cols() {
            let cell = &term.grid.line(y).cells[x];
            write!(w, "{}", cell.ch)?;
        }
        writeln!(w)?;
    }
    w.flush()?;
    Ok(())
}

/// Baca semua output yang tersedia di PTY master → engine + tail collector.
fn drain(session: &mut Session, term: &mut Terminal, tail: &mut Vec<u8>) -> io::Result<usize> {
    let mut buf = [0u8; 4096];
    let mut total = 0;
    loop {
        match session.read_output(&mut buf)? {
            0 => break,
            n => {
                term.feed_bytes(&buf[..n]);
                tail.extend_from_slice(&buf[..n]);
                if tail.len() > STDERR_CAP {
                    tail.drain(..tail.len() - STDERR_CAP);
                }
                total += n;
            }
        }
    }
    Ok(total)
}

fn render_row(out: &mut impl Write, term: &Terminal, y: usize) -> io::Result<()> {
    let line = &term.grid.line(y);
    for x in 0..term.cols() {
        let cell = &line.cells[x];
        let (r, g, b) = map_color(cell.attrs.fg);
        write!(out, "\x1b[38;2;{r};{g};{b}m{}", cell.ch)?;
    }
    write!(out, "\x1b[0m")?;
    Ok(())
}

/// Repaint penuh: clear all + tulis tiap baris (resize / startup).
fn redraw_all(out: &mut impl Write, term: &Terminal) -> io::Result<()> {
    execute!(out, MoveTo(0, 0), Clear(ClearType::All))?;
    for y in 0..term.rows() {
        execute!(out, MoveTo(0, y as u16))?;
        render_row(out, term, y)?;
    }
    Ok(())
}

/// Repaint diff: hanya baris dalam `dirty_rect` engine yang ditulis ulang.
fn redraw_diff(out: &mut impl Write, term: &mut Terminal) -> io::Result<()> {
    let Some((_x1, y1, _x2, y2)) = term.dirty_rect.take() else {
        return Ok(());
    };
    let max_y = term.rows().saturating_sub(1);
    for y in y1..=y2.min(max_y) {
        execute!(out, MoveTo(0, y as u16))?;
        write!(out, "\x1b[2K")?;
        render_row(out, term, y)?;
    }
    Ok(())
}

fn run_interactive(
    session: &mut Session,
    term: &mut Terminal,
    tail: &mut Vec<u8>,
) -> io::Result<()> {
    println!("[mterm] interactive — Ctrl-D/exit untuk keluar");
    enable_raw_mode()?;

    struct Restore;
    impl Drop for Restore {
        fn drop(&mut self) {
            let _ = disable_raw_mode();
        }
    }
    let _restore = Restore;

    let mut out = io::stdout();
    execute!(out, Hide)?;

    let mut last_dims = (term.cols() as u16, term.rows() as u16);
    apply_size(session, term, last_dims)?;
    redraw_all(&mut out, term)?;
    out.flush()?;

    let stdin_fd = std::io::stdin().as_raw_fd();
    let mut obuf = [0u8; 1024];
    let mut done = false;

    while !done {
        if wait_read(stdin_fd, 60) {
            match std::io::stdin().read(&mut obuf) {
                Ok(0) => done = true, // EOF → tutup
                Ok(n) => {
                    session.write_input(&obuf[..n])?;
                }
                Err(_) => {}
            }
        }
        let _ = drain(session, &mut *term, tail);
        if session.exited().is_some() {
            let _ = drain(session, &mut *term, tail);
            done = true;
        }

        // Ukuran host berubah? sync ke PTY + engine + repaint penuh.
        if let Some(dims) = tty_size(stdin_fd) {
            let dims = (dims.0.max(1), dims.1.max(1));
            if dims != last_dims {
                last_dims = dims;
                apply_size(session, term, dims)?;
                redraw_all(&mut out, term)?;
                out.flush()?;
                continue;
            }
        }

        redraw_diff(&mut out, term)?;
        out.flush()?;
    }

    execute!(out, Show)?;
    Ok(())
}

fn apply_size(session: &mut Session, term: &mut Terminal, dims: (u16, u16)) -> io::Result<()> {
    let (cols, rows) = dims;
    if (cols as usize) != term.cols() || (rows as usize) != term.rows() {
        session.resize(cols, rows)?;
        term.resize(cols as usize, rows as usize);
    }
    Ok(())
}

fn wait_read(fd: i32, timeout_ms: i32) -> bool {
    unsafe {
        let mut fds = [libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        }];
        let r = libc::poll(fds.as_mut_ptr(), 1, timeout_ms);
        r > 0 && (fds[0].revents & libc::POLLIN) != 0
    }
}

fn map_color(c: Color) -> (u8, u8, u8) {
    c.to_rgb24().unwrap_or((220, 220, 220))
}
