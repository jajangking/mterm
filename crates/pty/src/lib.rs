//! PTY session manager + PID file safety.
//!
//! Buka PTY via /dev/ptmx, spawn shell, kembalikan master fd untuk dibaca.
//! JANGAN pakai `pkill -f` untuk stop — pakai `PidFile::kill`.

use std::ffi::CString;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::os::unix::io::{AsRawFd, FromRawFd, RawFd};
use std::path::{Path, PathBuf};

pub mod runner;

use libc::c_ushort;

// ── Winsize ─────────────────────────────────────────────────────────────

#[repr(C)]
struct WinSize {
    ws_row: c_ushort,
    ws_col: c_ushort,
    _ws_xpixel: c_ushort,
    _ws_ypixel: c_ushort,
}

fn set_winsize(fd: RawFd, cols: u16, rows: u16) -> io::Result<()> {
    let ws = WinSize {
        ws_row: rows,
        ws_col: cols,
        _ws_xpixel: 0,
        _ws_ypixel: 0,
    };
    let ret = unsafe { libc::ioctl(fd, libc::TIOCSWINSZ as _, &ws) };
    if ret == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

// ── PTY open ────────────────────────────────────────────────────────────

struct RawFdPair(RawFd, RawFd);

fn open_pty(cols: u16, rows: u16) -> io::Result<RawFdPair> {
    let master = unsafe {
        libc::open(
            c"/dev/ptmx".as_ptr(),
            libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC,
        )
    };
    if master < 0 {
        return Err(io::Error::last_os_error());
    }

    // grantpt / unlockpt
    let _ = unsafe { libc::grantpt(master) };
    let _ = unsafe { libc::unlockpt(master) };

    // slave path
    let mut buf = [0u8; 256];
    let ret = unsafe { libc::ptsname_r(master, buf.as_mut_ptr().cast(), buf.len()) };
    if ret != 0 {
        return Err(io::Error::last_os_error());
    }

    let slave = unsafe {
        libc::open(
            buf.as_ptr().cast(),
            libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC,
        )
    };
    if slave < 0 {
        return Err(io::Error::last_os_error());
    }

    set_winsize(slave, cols, rows)?;
    Ok(RawFdPair(master, slave))
}

// ── Spawn ───────────────────────────────────────────────────────────────

pub struct Session {
    pub pid: libc::pid_t,
    master: File,
    _slave: File,
    last_code: Option<i32>,
}

impl Session {
    /// Spawn command di PTY baru. Kembalikan session dengan master fd untuk dibaca.
    /// `cmd` dieksekusi via PATH (`execvp`).
    pub fn spawn(cmd: &str, args: &[String], cols: u16, rows: u16) -> io::Result<Self> {
        Self::spawn_at(cmd, args, None, cols, rows)
    }

    /// Sama dengan [Self::spawn], tapi child `chdir` ke `cwd` sebelum exec
    /// (misal folder data app Android biar shell mulai di situ).
    pub fn spawn_at(
        cmd: &str,
        args: &[String],
        cwd: Option<&str>,
        cols: u16,
        rows: u16,
    ) -> io::Result<Self> {
        let RawFdPair(master_fd, slave_fd) = open_pty(cols, rows)?;

        let pid = unsafe { libc::fork() };
        if pid < 0 {
            return Err(io::Error::last_os_error());
        }

        if pid == 0 {
            // ── child ──
            let mut argv: Vec<CString> = Vec::with_capacity(args.len() + 2);
            if let Ok(c) = CString::new(cmd) {
                argv.push(c);
            }
            for a in args {
                if let Ok(c) = CString::new(a.as_str()) {
                    argv.push(c);
                }
            }
            let mut raw_argv: Vec<*const libc::c_char> = argv.iter().map(|c| c.as_ptr()).collect();
            raw_argv.push(std::ptr::null());

            unsafe {
                libc::setsid();
                libc::ioctl(slave_fd, libc::TIOCSCTTY as _, 0);
                if let Some(dir) = cwd {
                    let cdir = match CString::new(dir.as_bytes()) {
                        Ok(c) => c,
                        Err(_) => libc::_exit(1),
                    };
                    if libc::chdir(cdir.as_ptr()) != 0 {
                        libc::_exit(1);
                    }
                }
                libc::dup2(slave_fd, 0);
                libc::dup2(slave_fd, 1);
                libc::dup2(slave_fd, 2);
                if slave_fd > 2 {
                    libc::close(slave_fd);
                }
                libc::close(master_fd);
                libc::execvp(raw_argv[0], raw_argv.as_ptr());
                libc::_exit(1);
            }
        }

        // ── parent ──
        // SLAVE SENGJA DIBIARKAN TERBUKA di parent: kalau ditutup, read master
        // langsung EIO dan buffer output bisa hilang. Ditutup saat Session drop.
        unsafe {
            // set master non-blocking
            let flags = libc::fcntl(master_fd, libc::F_GETFL);
            libc::fcntl(master_fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
        }

        let master = unsafe { File::from_raw_fd(master_fd) };
        let _slave = unsafe { File::from_raw_fd(slave_fd) };
        Ok(Session {
            pid,
            master,
            _slave,
            last_code: None,
        })
    }

    /// Baca output dari master (non-blocking). Kembalikan byte yang terbaca,
    /// 0 = belum ada data / ujung output.
    pub fn read_output(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        use std::io::ErrorKind;
        match self.master.read(buf) {
            Ok(n) => Ok(n),
            Err(e) if e.kind() == ErrorKind::WouldBlock => Ok(0),
            Err(e) if e.raw_os_error() == Some(libc::EIO) => Ok(0), // slave semua tertutup
            Err(e) => Err(e),
        }
    }

    /// Tulis input user ke master (perintah ke shell).
    pub fn write_input(&mut self, data: &[u8]) -> io::Result<usize> {
        self.master.write(data)
    }

    /// Ubah ukuran PTY (TIOCSWINSZ). Shell dapat SIGWINCH.
    pub fn resize(&mut self, cols: u16, rows: u16) -> io::Result<()> {
        set_winsize(self.master.as_raw_fd(), cols, rows)
    }

    /// Deteksi child exit real (waitpid WNOHANG) + reap supaya tidak zombie.
    /// Setelah reap, subsequent call mengembalikan cached code.
    pub fn exited(&mut self) -> Option<i32> {
        if let Some(code) = self.last_code {
            return Some(code);
        }
        let mut status = 0;
        let r = unsafe { libc::waitpid(self.pid, &mut status, libc::WNOHANG) };
        if r == self.pid {
            let code = if libc::WIFEXITED(status) {
                libc::WEXITSTATUS(status) as i32
            } else if libc::WIFSIGNALED(status) {
                -(libc::WTERMSIG(status) as i32)
            } else {
                0
            };
            self.last_code = Some(code);
            Some(code)
        } else {
            None
        }
    }

    /// Cek apakah child masih hidup (zombie dianggap hidup — hati-hati).
    pub fn alive(&self) -> bool {
        unsafe { libc::kill(self.pid, 0) == 0 }
    }

    /// Raw fd dari ujung master PTY.
    pub fn master_fd(&self) -> RawFd {
        self.master.as_raw_fd()
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if self.last_code.is_none() {
            let mut status = 0;
            unsafe {
                libc::waitpid(self.pid, &mut status, libc::WNOHANG);
            }
        }
    }
}

// ── PidFile (anti footgun pkill -f) ─────────────────────────────────────

pub struct PidFile {
    path: PathBuf,
}

impl PidFile {
    /// Tulis PID ke file. Panggil awal sesi.
    pub fn create(path: impl AsRef<Path>, pid: libc::pid_t) -> io::Result<Self> {
        let p = path.as_ref();
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(p, pid.to_string())?;
        Ok(PidFile {
            path: p.to_path_buf(),
        })
    }

    /// Baca PID dari file. Return None kalau file tidak ada / corrupt.
    pub fn read(path: impl AsRef<Path>) -> Option<u32> {
        fs::read_to_string(path.as_ref()).ok()?.trim().parse().ok()
    }

    /// Cek apakah proses dengan pid masih berjalan.
    pub fn alive(pid: u32) -> bool {
        unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
    }

    /// Kirim SIGTERM ke pid. Return true jika kill berhasil.
    pub fn kill(pid: u32) -> io::Result<bool> {
        let ret = unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
        if ret == 0 {
            Ok(true)
        } else {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::NotFound || err.raw_os_error() == Some(libc::ESRCH) {
                Ok(false)
            } else {
                Err(err)
            }
        }
    }

    /// Hapus file pid. Panggil saat shutdown bersih.
    pub fn remove(&self) -> io::Result<()> {
        if self.path.exists() {
            fs::remove_file(&self.path)?;
        }
        Ok(())
    }

    /// Stop proses berdasarkan pid file. Return true jika proses dihentikan.
    pub fn stop_from_file(path: impl AsRef<Path>) -> io::Result<bool> {
        match Self::read(&path) {
            Some(pid) => Self::kill(pid),
            None => Ok(false),
        }
    }
}

// ── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawn_shell_echo() {
        let mut sess =
            Session::spawn("sh", &["-c".into(), "echo hello_pty".into()], 80, 24).expect("spawn");
        let mut buf = [0u8; 4096];
        let mut output = Vec::new();
        // tunggu child selesai (max ~50ms)
        for _ in 0..50 {
            match sess.read_output(&mut buf) {
                Ok(0) => std::thread::sleep(std::time::Duration::from_millis(5)),
                Ok(n) => output.extend_from_slice(&buf[..n]),
                Err(_) => break,
            }
            if !sess.alive() {
                break;
            }
        }
        let out = String::from_utf8_lossy(&output);
        assert!(out.contains("hello_pty"), "output: {out}");
    }

    #[test]
    fn pid_file_roundtrip() {
        let path = std::env::temp_dir().join("mterm_pid_test");
        let pf = PidFile::create(&path, 42).unwrap();
        assert_eq!(PidFile::read(&path), Some(42));
        pf.remove().unwrap();
        assert!(PidFile::read(&path).is_none());
    }

    #[test]
    fn pty_multiline() {
        let mut sess = Session::spawn(
            "sh",
            &["-c".into(), "printf 'line1\\nline2\\nline3\\n'".into()],
            80,
            24,
        )
        .unwrap();
        let mut out = Vec::new();
        let mut buf = [0u8; 4096];
        for _ in 0..60 {
            match sess.read_output(&mut buf) {
                Ok(0) => std::thread::sleep(std::time::Duration::from_millis(5)),
                Ok(n) => out.extend_from_slice(&buf[..n]),
                Err(_) => break,
            }
            if !sess.alive() {
                break;
            }
        }
        let s = String::from_utf8_lossy(&out);
        assert!(s.contains("line1"), "got: {s}");
        assert!(s.contains("line3"), "got: {s}");
    }
}
