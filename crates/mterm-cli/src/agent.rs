//! `mterm agent`: agent IPC workspace-first (Fase 6).
//!
//! Server `agent serve`: Unix socket NDJSON, simpan sesi per-workspace.
//! Backend LLM bisa diganti (trait `Backend`); sekarang `StubBackend`
//! (echo) supaya IPC bisa dipakai & diuji tanpa API key/dep berat.
//! Integrasi Groq beneran = implement `Backend` sekali.

use std::fs;
use std::io::{self, BufRead, BufReader, Write};
use std::net::Shutdown;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

// ── Lokasi state ─────────────────────────────────────────────────────────

fn data_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".mterm").join("agent")
}

pub fn socket_path() -> PathBuf {
    data_dir().join("agent.sock")
}

pub fn pid_path() -> PathBuf {
    data_dir().join("agent.pid")
}

fn session_path() -> PathBuf {
    data_dir().join("session.json")
}

fn log_path() -> PathBuf {
    data_dir().join("agent.log")
}

// ── Sesi percakapan ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Msg {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Session {
    pub workspace: String,
    pub messages: Vec<Msg>,
}

fn load_session(path: &Path) -> Session {
    let mut s: Session = fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    if s.workspace.is_empty() {
        s.workspace = std::env::current_dir()
            .map(|d| d.display().to_string())
            .unwrap_or_default();
    }
    s
}

fn save_session(path: &Path, s: &Session) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(
        path,
        serde_json::to_string_pretty(s).unwrap_or_else(|_| "{}".into()),
    )
}

// ── Backend (abstraksi LLM) ──────────────────────────────────────────────

pub trait Backend: Send + Sync {
    /// Dapatkan jawaban penuh untuk `history` (user turn terbaru sudah
    /// masuk). `on_delta` dipanggil tiap ada potongan saat streaming.
    fn ask(&self, history: &[Msg], on_delta: &mut dyn FnMut(&str)) -> io::Result<String>;
}

/// Backend placeholder: echo dari user turn terakhir.
/// Integrasi Groq nanti = ganti ini dengan implementasi `Backend` yang
/// memanggil API (SSE streaming), tanpa menyentuh server/client.
pub struct StubBackend;

impl Backend for StubBackend {
    fn ask(&self, history: &[Msg], on_delta: &mut dyn FnMut(&str)) -> io::Result<String> {
        let last = history.last().map(|m| m.content.as_str()).unwrap_or("");
        let reply = format!("[stub] kamu bilang: {last}");
        on_delta(&reply);
        Ok(reply)
    }
}

// ── Server (daemon) ──────────────────────────────────────────────────────

fn send_json<W: Write>(w: &mut W, v: &Value) -> io::Result<()> {
    writeln!(w, "{}", serde_json::to_string(v).unwrap())?;
    w.flush()
}

/// Layani satu koneksi client: baca request → jawab (streaming).
fn handle_conn(
    stream: UnixStream,
    session_path: &Path,
    backend: Arc<dyn Backend>,
) -> io::Result<()> {
    stream.set_read_timeout(Some(std::time::Duration::from_secs(600)))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut writer = stream;

    let mut line = String::new();
    loop {
        line.clear();
        let n = reader.read_line(&mut line)?;
        if n == 0 {
            break;
        }
        let Ok(req) = serde_json::from_str::<Value>(&line) else {
            send_json(
                &mut writer,
                &json!({"type": "done", "error": "bad request"}),
            )?;
            break;
        };
        match req["cmd"].as_str() {
            Some("ping") => send_json(&mut writer, &json!({"type": "pong"}))?,
            Some("reset") => {
                if session_path.exists() {
                    fs::remove_file(session_path)?;
                }
                send_json(&mut writer, &json!({"type": "done", "reset": true}))?;
            }
            Some("ask") => {
                let text = req["text"].as_str().unwrap_or("").trim().to_string();
                if text.is_empty() {
                    send_json(&mut writer, &json!({"type": "done", "error": "empty"}))?;
                    continue;
                }
                let mut sess = load_session(session_path);
                sess.messages.push(Msg {
                    role: "user".into(),
                    content: text.clone(),
                });
                send_json(
                    &mut writer,
                    &json!({
                        "type": "meta",
                        "workspace": sess.workspace,
                        "history": sess.messages.len(),
                    }),
                )?;

                let backend = backend.clone();
                let history = sess.messages.clone();
                let result = backend.ask(&history, &mut |piece| {
                    let _ = send_json(&mut writer, &json!({"type": "delta", "text": piece}));
                });
                match result {
                    Ok(full) => {
                        sess.messages.push(Msg {
                            role: "assistant".into(),
                            content: full,
                        });
                        let _ = save_session(session_path, &sess);
                        send_json(&mut writer, &json!({"type": "done", "error": null}))?;
                    }
                    Err(e) => {
                        // rollback user msg agar tidak menyisakan percakapan hang
                        sess.messages.pop();
                        let _ = save_session(session_path, &sess);
                        send_json(
                            &mut writer,
                            &json!({"type": "done", "error": e.to_string()}),
                        )?;
                    }
                }
            }
            _ => send_json(
                &mut writer,
                &json!({"type": "done", "error": "unknown cmd"}),
            )?,
        }
    }
    let _ = writer.shutdown(Shutdown::Both);
    Ok(())
}

/// Accept loop tak terbatas (daemon).
fn run_server(
    listener: UnixListener,
    session_path: PathBuf,
    backend: Arc<dyn Backend>,
) -> io::Result<()> {
    for conn in listener.incoming() {
        match conn {
            Ok(stream) => {
                let sp = session_path.clone();
                let be = backend.clone();
                std::thread::spawn(move || {
                    let _ = handle_conn(stream, &sp, be);
                });
            }
            Err(e) => eprintln!("agent: koneksi gagal: {e}"),
        }
    }
    Ok(())
}

/// Layani SATU koneksi lalu keluar (dipakai test).
#[cfg(test)]
fn serve_once(
    listener: UnixListener,
    session_path: PathBuf,
    backend: Arc<dyn Backend>,
) -> io::Result<()> {
    let (stream, _) = listener.accept()?;
    handle_conn(stream, &session_path, backend)
}

fn backend() -> io::Result<Arc<dyn Backend>> {
    Ok(Arc::new(StubBackend) as Arc<dyn Backend>)
}

// ── Client ───────────────────────────────────────────────────────────────

fn connect() -> io::Result<UnixStream> {
    let path = socket_path();
    match UnixStream::connect(&path) {
        Ok(s) => Ok(s),
        Err(_) => Err(io::Error::new(
            io::ErrorKind::NotFound,
            "agent belum jalan. Jalankan: mterm agent start",
        )),
    }
}

/// Kirim satu request, cetak delta streaming, return error string bila ada.
fn client_ask(q: &str) -> io::Result<()> {
    let mut s = connect()?;
    send_json(&mut s, &json!({"cmd": "ask", "text": q}))?;
    let reader = BufReader::new(s);
    let mut first = true;
    for line in reader.lines().map_while(Result::ok) {
        let Ok(v) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        match v["type"].as_str() {
            Some("delta") => {
                if first {
                    if let Some(text) = v["text"].as_str() {
                        print!("{text}");
                    }
                    let _ = io::stdout().flush();
                    first = false;
                } else if let Some(text) = v["text"].as_str() {
                    print!("{text}");
                    let _ = io::stdout().flush();
                }
            }
            Some("done") => {
                println!();
                if let Some(err) = v["error"].as_str().filter(|e| !e.is_empty()) {
                    eprintln!("agent: {err}");
                }
                return Ok(());
            }
            _ => {}
        }
    }
    Ok(())
}

// ── CLI entry ────────────────────────────────────────────────────────────

pub fn main(args: &[String]) -> io::Result<()> {
    let sub = args.first().map(String::as_str).unwrap_or("status");
    fs::create_dir_all(data_dir())?;

    match sub {
        "serve" => {
            // foreground server (dipakai `start`; bisa juga manual)
            let listener = UnixListener::bind(socket_path())?;
            println!("agent server: {}", socket_path().display());
            let be = backend()?;
            run_server(listener, session_path(), be)
        }
        "start" => {
            if let Some(pid) = mterm_pty::PidFile::read(pid_path()) {
                if mterm_pty::PidFile::alive(pid) {
                    println!("agent sudah jalan (pid {pid})");
                    return Ok(());
                }
            }
            let _ = fs::remove_file(socket_path());
            let exe = std::env::current_exe()?;
            let log = fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(log_path())?;
            let log2 = log.try_clone()?;
            let child = std::process::Command::new(exe)
                .args(["agent", "serve"])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::from(log))
                .stderr(std::process::Stdio::from(log2))
                .spawn()?;
            let _ = mterm_pty::PidFile::create(pid_path(), child.id() as i32)?;
            // tunggu sebentar supaya bind socket sukses
            std::thread::sleep(std::time::Duration::from_millis(300));
            if !mterm_pty::PidFile::alive(child.id()) {
                return Err(io::Error::other(
                    "server langsung mati. Cek log: ~/.mterm/agent/agent.log",
                ));
            }
            println!("agent dimulai (pid {}) — log: {}", child.id(), log_path().display());
            Ok(())
        }
        "stop" => {
            let mut removed = false;
            if let Some(pid) = mterm_pty::PidFile::read(pid_path()) {
                removed = mterm_pty::PidFile::kill(pid)?;
            }
            let _ = fs::remove_file(pid_path());
            let _ = fs::remove_file(socket_path());
            println!("agent dihentikan");
            let _ = removed;
            Ok(())
        }
        "status" => {
            let pid = mterm_pty::PidFile::read(pid_path());
            match pid {
                Some(p) if mterm_pty::PidFile::alive(p) => {
                    let sock = socket_path().exists();
                    println!("agent jalan (pid {p}, socket: {sock})");
                }
                _ => println!("agent tidak jalan"),
            }
            if let Ok(s) = fs::read_to_string(session_path()) {
                if let Ok(v) = serde_json::from_str::<Value>(&s) {
                    let n = v["messages"].as_array().map(|a| a.len()).unwrap_or(0);
                    println!("sesi: {} pesan di {}", n, v["workspace"].as_str().unwrap_or("-"));
                }
            }
            Ok(())
        }
        "ask" => {
            let q = args
                .get(1)
                .map(String::as_str)
                .unwrap_or("")
                .trim()
                .to_string();
            if q.is_empty() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "usage: mterm agent ask \"<pertanyaan>\"",
                ));
            }
            client_ask(&q)
        }
        "reset" => {
            let _ = fs::remove_file(session_path());
            println!("sesi di-reset");
            Ok(())
        }
        "history" => {
            let s = load_session(&session_path());
            if s.messages.is_empty() {
                println!("(kosong)");
            }
            for m in &s.messages {
                let prefix = if m.role == "user" { "❯" } else { "─" };
                println!("{prefix} {}", m.content.replace('\n', " "));
            }
            Ok(())
        }
        other => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("agent: perintah tidak dikenal: {other} (serve|start|stop|status|ask|reset|history)"),
        )),
    }
}

// ── Tes ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeBackend {
        parts: Vec<String>,
    }
    impl Backend for FakeBackend {
        fn ask(&self, _history: &[Msg], on_delta: &mut dyn FnMut(&str)) -> io::Result<String> {
            let mut full = String::new();
            for p in &self.parts {
                full.push_str(p);
                on_delta(p);
            }
            Ok(full)
        }
    }

    #[test]
    fn ask_streams_and_persists() {
        let dir = std::env::temp_dir().join(format!("mterm-agent-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let sock = dir.join("t.sock");
        let sess_path = dir.join("t.json");
        let listener = UnixListener::bind(&sock).unwrap();
        let backend = FakeBackend {
            parts: vec!["halo ".into(), "dunia".into()],
        };

        let sp = sess_path.clone();
        let t = std::thread::spawn(move || {
            let _ = serve_once(listener, sp, Arc::new(backend));
        });

        // client dalam blok; koneksi ditutup sebelum join supaya server
        // melihat EOF (read_line) dan thread server bisa selesai.
        let (result, s) = {
            let mut stream = UnixStream::connect(&sock).unwrap();
            send_json(&mut stream, &json!({"cmd": "ask", "text": "tes dong"})).unwrap();

            let mut result = String::new();
            let reader = BufReader::new(stream.try_clone().unwrap());
            for line in reader.lines().map_while(Result::ok) {
                let v: Value = serde_json::from_str(&line).unwrap();
                match v["type"].as_str().unwrap_or_default() {
                    "delta" => result.push_str(v["text"].as_str().unwrap_or("")),
                    "done" => break,
                    _ => {}
                }
            }
            let s: Session =
                serde_json::from_str(&fs::read_to_string(&sess_path).unwrap()).unwrap();
            (result, s)
        };
        assert_eq!(result, "halo dunia", "delta harus terkumpul jadi satu");
        assert_eq!(s.messages.len(), 2, "user + assistant");
        assert_eq!(s.messages[0].role, "user");
        assert_eq!(s.messages[0].content, "tes dong");
        assert_eq!(s.messages[1].role, "assistant");
        assert_eq!(s.messages[1].content, "halo dunia");

        t.join().unwrap();
        let _ = fs::remove_dir_all(&dir);
    }
}
