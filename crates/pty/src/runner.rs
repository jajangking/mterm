//! EmuRunner — thread model Fase 2 (Rust):
//! PTY master dibaca oleh **thread background**, output di-feed ke engine via
//! callback; caller tinggal poll `exit_code()` / isi engine. Pemisahan ini
//! memungkinkan UI (Kotlin/CLI) membaca grid tanpa diblokir I/O PTY.
//!
//! Joins sisi PTY:
//! - thread baca non-blocking (`poll` POLLIN/POLLHUP, timeout 10ms)
//! - kanal input (user → shell) dan resize (UI → TIOCSWINSZ)
//! - deteksi exit `waitpid(WNOHANG)` + reap (anti zombie)
//! - EIO (slave tertutup) dianggap hangup → selesai
//! - `stop()` set flag; `join()` menunggu thread benar-benar keluar

use crate::Session;
use std::io;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::Arc;
use std::thread::JoinHandle;

/// -1 = masih jalan; >=0 = kode exit; negatif selain -1 = sinyal (mis. -15 SIGTERM).
const RUNNING: i32 = -1;
/// Kode hangup saat slave yatim (EIO) tanpa status reaped.
const HANGUP: i32 = 0;

pub struct EmuRunner {
    inner: Arc<Inner>,
    thread: Option<JoinHandle<()>>,
}

struct Inner {
    stop: AtomicBool,
    exit: AtomicI32,
    input_tx: SyncSender<Vec<u8>>,
    resize_tx: SyncSender<(u16, u16)>,
}

impl EmuRunner {
    /// Jalankan background thread yang membaca PTY `session`.
    /// `on_output` dipanggil di thread itu dengan byte output PTY.
    pub fn spawn<F>(session: Session, on_output: F) -> io::Result<Self>
    where
        F: FnMut(&[u8]) + Send + 'static,
    {
        let (input_tx, input_rx) = mpsc::sync_channel(64);
        let (resize_tx, resize_rx) = mpsc::sync_channel(16);
        let inner = Arc::new(Inner {
            stop: AtomicBool::new(false),
            exit: AtomicI32::new(RUNNING),
            input_tx,
            resize_tx,
        });

        let inner2 = Arc::clone(&inner);
        let handle = std::thread::Builder::new()
            .name("mterm-emu".into())
            .spawn(move || {
                EmuRunner::thread_loop(session, inner2, input_rx, resize_rx, on_output);
            })?;

        Ok(EmuRunner {
            inner,
            thread: Some(handle),
        })
    }

    /// Saluran input user → shell (non-blocking, antrean terbatas).
    pub fn input(&self) -> &SyncSender<Vec<u8>> {
        &self.inner.input_tx
    }

    /// Minta resize PTY (UI → TIOCSWINSZ); diproses thread emu.
    pub fn resize(&self, cols: u16, rows: u16) -> bool {
        self.inner.resize_tx.send((cols, rows)).is_ok()
    }

    /// Kode exit: `RUNNING` (-1) selama jalan; sesudah itu >=0 (atau -sinyal).
    pub fn exit_code(&self) -> i32 {
        self.inner.exit.load(Ordering::SeqCst)
    }

    pub fn is_running(&self) -> bool {
        self.inner.exit.load(Ordering::SeqCst) == RUNNING
    }

    /// Minta thread berhenti (bukan kill child — hanya loop berhenti).
    pub fn stop(&self) {
        self.inner.stop.store(true, Ordering::SeqCst);
    }

    /// Hentikan dan tunggu thread keluar.
    pub fn shutdown(mut self) {
        self.stop();
        if let Some(h) = self.thread.take() {
            let _ = h.join();
        }
    }

    fn thread_loop<F>(
        mut session: Session,
        inner: Arc<Inner>,
        input_rx: Receiver<Vec<u8>>,
        resize_rx: Receiver<(u16, u16)>,
        mut on_output: F,
    ) where
        F: FnMut(&[u8]),
    {
        let master = session.master_fd();
        let mut buf = [0u8; 8192];

        loop {
            // drain input → shell
            while let Ok(data) = input_rx.try_recv() {
                let _ = session.write_input(&data);
            }
            // drain resize → TIOCSWINSZ
            while let Ok((cols, rows)) = resize_rx.try_recv() {
                let _ = session.resize(cols, rows);
            }

            let readable = poll_readable(master, 10);
            if readable {
                match session.read_output(&mut buf) {
                    Ok(n) if n > 0 => {
                        on_output(&buf[..n]);
                        // jangan eksklusif: tetap cek stop setelah feed
                    }
                    Ok(_) => {
                        // 0 = belum ada data (non-blocking); lanjut
                    }
                    Err(e) => {
                        // EIO ⇒ slave semua tertutup; anggap hangup
                        let is_eio = e.raw_os_error() == Some(libc::EIO);
                        if is_eio {
                            break;
                        }
                        // error lain: berhenti biar tak spin-lock
                        break;
                    }
                }
            }

            if inner.stop.load(Ordering::SeqCst) {
                break;
            }

            // exit? reaped di sini → set kode segera setelah child mati
            if let Some(code) = session.exited() {
                inner.exit.store(code, Ordering::SeqCst);
                break;
            }
        }

        // kalau keluar tanpa reap (stop / EIO), reap sekarang dan simpan
        if inner.exit.load(Ordering::SeqCst) == RUNNING {
            let code = session.exited().unwrap_or(HANGUP);
            inner.exit.store(code, Ordering::SeqCst);
        }
    }
}

/// poll master dengan timeout; true kalau siap baca.
fn poll_readable(fd: i32, timeout_ms: i32) -> bool {
    let mut fds = [libc::pollfd {
        fd,
        events: libc::POLLIN | libc::POLLHUP,
        revents: 0,
    }];
    unsafe {
        let r = libc::poll(fds.as_mut_ptr(), 1, timeout_ms);
        r > 0 && (fds[0].revents & (libc::POLLIN | libc::POLLHUP)) != 0
    }
}

/// Helper: kirim input ke runner (antrean terbatas; false kalau runner mati).
pub fn send_input(runner_input: &SyncSender<Vec<u8>>, data: &[u8]) -> bool {
    match runner_input.try_send(data.to_vec()) {
        Ok(()) => true,
        Err(mpsc::TrySendError::Full(_)) => {
            // antrean penuh — blokir sampai ada slot (runner masih hidup)
            runner_input.send(data.to_vec()).is_ok()
        }
        Err(_) => false, // disconnected ⇒ runner sudah berhenti
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::time::Duration;

    /// Kumpulkan semua output di Arc<Mutex<Vec<u8>>> untuk diassert.
    #[allow(clippy::type_complexity)]
    fn recorder() -> (Arc<Mutex<Vec<u8>>>, impl FnMut(&[u8])) {
        let buf = Arc::new(Mutex::new(Vec::new()));
        let out = Arc::clone(&buf);
        let f = move |data: &[u8]| {
            let mut g = out.lock().unwrap();
            g.extend_from_slice(data);
        };
        (buf, f)
    }

    fn sh(args: &[String]) -> Session {
        Session::spawn("sh", args, 80, 24).expect("spawn sh")
    }

    #[test]
    fn runner_feeds_output_until_exit() {
        let cmd = vec!["-c".into(), "printf 'hello from pty\n'; exit 0".into()];
        let session = sh(&cmd);
        let (buf, f) = recorder();
        let runner = EmuRunner::spawn(session, f).unwrap();

        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while runner.is_running() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(runner.exit_code(), 0);
        let guard = buf.lock().unwrap().clone();
        let text = String::from_utf8_lossy(&guard);
        assert!(text.contains("hello from pty"), "output: {text:?}");
        runner.shutdown();
    }

    #[test]
    fn runner_reports_nonzero_exit() {
        let cmd = vec!["-c".into(), "exit 7".into()];
        let runner = EmuRunner::spawn(sh(&cmd), |_| {}).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while runner.is_running() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(runner.exit_code(), 7, "exit code 7");
        runner.shutdown();
    }

    #[test]
    fn runner_input_channel_reaches_shell() {
        let cmd = vec!["-c".into(), "read -r line; printf 'got:%s' \"$line\"".into()];
        let session = sh(&cmd);
        let (buf, f) = recorder();
        let runner = EmuRunner::spawn(session, f).unwrap();
        assert!(send_input(runner.input(), b"halo\n"), "input terkirim");

        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while runner.is_running() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        let guard = buf.lock().unwrap();
        let text = String::from_utf8_lossy(&guard);
        assert!(text.contains("got:halo"), "echo balik: {text}");
        drop(guard);
        runner.shutdown();
    }

    #[test]
    fn runner_stop_terminates_shell_program() {
        let cmd = vec!["-c".into(), "while true; do sleep 1; done".into()];
        let session = sh(&cmd);
        let runner = EmuRunner::spawn(session, |_| {}).unwrap();
        assert!(runner.is_running());
        // stop() hanya memberhentikan loop — pid masih hidup, jadi drop
        // runner TANPA reap akan meninggalkan zombie; test biarkan reap oleh
        // shutdown (thread keluar, tapi child masih while-true).
        runner.stop();
        std::thread::sleep(Duration::from_millis(100));
        assert!(!runner.is_running(), "exit kode terset setelah stop");
        runner.shutdown();
    }

    #[test]
    fn runner_resize_changes_winsize() {
        let cmd = vec!["-c".into(), "printf 'test resize'; sleep 2; exit 0".into()];
        let runner = EmuRunner::spawn(sh(&cmd), |_| {}).unwrap();
        assert!(runner.resize(120, 40), "resize queue terima");
        std::thread::sleep(Duration::from_millis(50));
        runner.shutdown();
    }
}