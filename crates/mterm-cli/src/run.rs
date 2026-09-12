//! `mterm run`: pipe output command → engine, render grid.
//!
//! Mode otomatis:
//! - stdin TTY → interaktif (raw mode, kirim input user ke PTY)
//! - stdin bukan TTY → sekali jalan, tak perlu terminal asli

use std::io::{self, Read, Write};
use std::os::unix::io::AsRawFd;
use std::time::Duration;

use crossterm::cursor::MoveTo;
use crossterm::execute;
use crossterm::terminal::{Clear, ClearType};
use mterm_core::grid::Color;
use mterm_core::terminal::{Terminal, TerminalConfig};
use mterm_pty::Session;

pub fn run(args: &[String]) -> io::Result<()> {
    let cmd = args.first().map(String::as_str).unwrap_or("sh");
    let cmd_args = &args[1..];
    let cols = 80u16;
    let rows = 24u16;

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

    if std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        run_interactive(&mut session, &mut term)?;
    } else {
        run_once(&mut session, &mut term)?;
    }

    pf.remove()?;
    Ok(())
}

fn run_once(session: &mut Session, term: &mut Terminal) -> io::Result<()> {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);

    loop {
        let _ = drain(session, term);
        if let Some(code) = session.exited() {
            println!("[mterm] exit code: {code}");
            break;
        }
        if std::time::Instant::now() > deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let _ = drain(session, term);

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

/// Baca semua output yang tersedia di PTY master → engine. Return 0.
fn drain(session: &mut Session, term: &mut Terminal) -> io::Result<usize> {
    let mut buf = [0u8; 4096];
    let mut total = 0;
    loop {
        match session.read_output(&mut buf)? {
            0 => break,
            n => {
                term.feed_bytes(&buf[..n]);
                total += n;
            }
        }
    }
    Ok(total)
}

fn run_interactive(session: &mut Session, term: &mut Terminal) -> io::Result<()> {
    use crossterm::terminal::{disable_raw_mode, enable_raw_mode};

    println!("[mterm] interactive — Ctrl-D/exit untuk keluar");
    let stdout = io::stdout();
    enable_raw_mode()?;
    let mut out = stdout.lock();

    struct Restore;
    impl Drop for Restore {
        fn drop(&mut self) {
            let _ = disable_raw_mode();
        }
    }
    let _restore = Restore;

    let _master_fd = session.master_fd();
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
        let _ = drain(session, &mut *term);
        if session.exited().is_some() {
            let _ = drain(session, &mut *term);
            done = true;
        }

        // render layar
        execute!(out, MoveTo(0, 0), Clear(ClearType::All))?;
        for y in 0..term.rows() {
            for x in 0..term.cols() {
                let cell = &term.grid.line(y).cells[x];
                let (r, g, b) = map_color(cell.attrs.fg);
                write!(out, "\x1b[38;2;{r};{g};{b}m{}", cell.ch)?;
            }
            writeln!(out, "\x1b[0m")?;
        }
        out.flush()?;
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
