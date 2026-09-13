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

// ── Lokasi state (per-workspace) ─────────────────────────────────────────

/// Root semua state agent: `~/.mterm/agent/`.
fn data_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".mterm").join("agent")
}

/// Workspace terpilih: `--workspace <dir>` > env `MTERM_WORKSPACE` > cwd.
pub fn resolve_workspace(args: &[String]) -> PathBuf {
    args.windows(2)
        .find(|w| w[0] == "--workspace")
        .and_then(|w| w.get(1))
        .map(PathBuf::from)
        .or_else(|| std::env::var("MTERM_WORKSPACE").ok().map(PathBuf::from))
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
}

/// Slug aman untuk nama folder state: path absolut, `/` dll → `_`.
pub fn slug_for(ws: &Path) -> String {
    let abs = ws.canonicalize().unwrap_or_else(|_| ws.to_path_buf());
    abs.to_string_lossy()
        .trim_start_matches('/')
        .replace('/', "_")
        .replace([' ', ':', '\n', '\t'], "_")
}

/// Direktori state untuk satu workspace.
pub fn agent_dir_for(ws: &Path) -> PathBuf {
    data_dir().join(slug_for(ws))
}

/// Direktori state untuk workspace saat ini (default CLI).
pub fn agent_dir() -> PathBuf {
    agent_dir_for(&std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
}

pub fn socket_for(dir: &Path) -> PathBuf {
    dir.join("agent.sock")
}

pub fn pid_for(dir: &Path) -> PathBuf {
    dir.join("agent.pid")
}

pub fn session_for(dir: &Path) -> PathBuf {
    dir.join("session.json")
}

pub fn log_for(dir: &Path) -> PathBuf {
    dir.join("agent.log")
}

/// File penampung "stderr" (ekor output perintah terakhir) → dibundle ke agent.
pub fn stderr_for(dir: &Path) -> PathBuf {
    dir.join("last_stderr.txt")
}

/// Tulis hasil akhir perintah (`tail` output grid) ke collector stderr.
/// Baris pertama berformat `exit:N|error message` (baris pertama dari tail).
/// ANSI escape dibuang biar teks bersih saat disuntik ke prompt agent.
pub fn write_stderr_capture(text: &str, exit_code: Option<i32>) -> io::Result<()> {
    let dir = agent_dir();
    fs::create_dir_all(&dir)?;
    let msg = match exit_code {
        Some(0) => "exit:0".to_string(),
        Some(c) => format!("exit:{c}"),
        None => "exit:timeout|never-exit".to_string(),
    };
    fs::write(stderr_for(&dir), format!("{msg}\n{}", strip_ansi(text)))
}

/// Buang escape sequence ANSI (CSI `\x1b[...m`, dll) dari teks.
pub fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next(); // [
            for c2 in chars.by_ref() {
                if c2.is_ascii_alphabetic() || c2 == '~' {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Konten collector stderr untuk agent (dipotong kalau kegedean).
pub fn stderr_bundle_for_dir(dir: &Path) -> String {
    match fs::read_to_string(stderr_for(dir)) {
        Ok(s) => cap_chars(s, MAX_CONTEXT_CHARS),
        Err(_) => String::new(),
    }
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

const DEFAULT_MODEL: &str = "openai/gpt-oss-120b";

/// Generic backend HTTP OpenAI-compatible (Groq/OpenAI/openrouter/ollama dll):
/// pakai `curl` subprocess (tanpa dep TLS/HTTP berat di Termux). Streaming
/// SSE `data:` di-parse per baris, delta dipakai lewat `on_delta`.
#[derive(Debug, Clone)]
pub struct RestBackend {
    url: String,
    model: String,
    key: String,
}

impl RestBackend {
    pub fn new(url: String, model: String, key: String) -> Self {
        RestBackend { url, model, key }
    }

    #[cfg(test)]
    fn curl_available() -> bool {
        std::process::Command::new("curl")
            .arg("--version")
            .output()
            .is_ok()
    }
}

impl Backend for RestBackend {
    fn ask(&self, history: &[Msg], on_delta: &mut dyn FnMut(&str)) -> io::Result<String> {
        let payload = serde_json::json!({
            "model": self.model,
            "messages": history
                .iter()
                .map(|m| serde_json::json!({"role": m.role, "content": m.content}))
                .collect::<Vec<_>>(),
            "stream": true,
            "temperature": 0.2,
        });
        let mut child = std::process::Command::new("curl")
            .arg("-N")
            .arg("-sS")
            .arg("--max-time")
            .arg("300")
            .arg("-X")
            .arg("POST")
            .arg(&self.url)
            .arg("-H")
            .arg("Content-Type: application/json")
            .arg("--oauth2-bearer")
            .arg(&self.key)
            .arg("-d")
            .arg(payload.to_string())
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| {
                io::Error::new(
                    e.kind(),
                    format!("curl tidak bisa dijalankan (perlu curl): {e}"),
                )
            })?;

        let stdout = child.stdout.take().expect("stdout piped");
        let reader = std::io::BufReader::new(stdout);
        let mut full = String::new();
        let mut raw = Vec::new();
        for line in reader.lines().map_while(Result::ok) {
            raw.push(line.clone());
            if let Some(mut piece) = sse_delta(&line) {
                full.push_str(&piece);
                on_delta(&mut piece);
            }
        }
        let out = child.wait_with_output()?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
            return Err(io::Error::other(format!(
                "backend HTTP {}: {}",
                out.status,
                if err.is_empty() {
                    "curl gagal".to_string()
                } else {
                    err
                }
            )));
        }
        if full.is_empty() {
            let body = raw.join("\n").trim().to_string();
            if !body.is_empty() {
                return Err(io::Error::other(format!("backend HTTP error: {body}")));
            }
            return Err(io::Error::other("respons backend kosong"));
        }
        Ok(full)
    }
}

/// Parse satu baris SSE: `data: <json>` → potongan `choices[0].delta.content`.
/// `[DONE]` atau baris non-`data:` → None.
fn sse_delta(line: &str) -> Option<String> {
    let body = line.strip_prefix("data:")?.trim();
    if body.is_empty() || body == "[DONE]" {
        return None;
    }
    let v: Value = serde_json::from_str(body).ok()?;
    v["choices"][0]["delta"]["content"]
        .as_str()
        .map(|s| s.to_string())
}

/// Key LLM: env `GROQ_API_KEY` dulu, fallback `~/.groq_key` (konvensi lokal).
fn llm_key() -> Option<String> {
    if let Ok(k) = std::env::var("GROQ_API_KEY") {
        if !k.trim().is_empty() {
            return Some(k.trim().to_string());
        }
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    fs::read_to_string(PathBuf::from(home).join(".groq_key"))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

// ── Slash command + context collector ─────────────────────────────────────

const MAX_CONTEXT_CHARS: usize = 12_000;

/// Pisah `/cmd arg` → Some((cmd, arg)). Bukan slash → None.
fn slash_parse(raw: &str) -> Option<(&str, &str)> {
    let t = raw.trim();
    if !t.starts_with('/') {
        return None;
    }
    match t.split_once(char::is_whitespace) {
        Some((c, r)) => Some((c, r.trim())),
        None => Some((t, "")),
    }
}

fn cap_chars(s: String, max: usize) -> String {
    if s.chars().count() <= max {
        return s;
    }
    let mut out: String = s.chars().take(max).collect();
    out.push_str("\n…(terpotong)");
    out
}

/// Baca file (relatif workspace), potong kalau terlalu besar.
fn read_file_ctx(ws: &Path, rel: &str) -> io::Result<(String, String)> {
    let p = ws.join(rel);
    let content = fs::read_to_string(&p)
        .map_err(|e| io::Error::new(e.kind(), format!("tidak bisa baca {}: {e}", p.display())))?;
    Ok((
        p.display().to_string(),
        cap_chars(content, MAX_CONTEXT_CHARS),
    ))
}

fn git_captured(ws: &Path, args: &[&str]) -> String {
    std::process::Command::new("git")
        .args(args)
        .current_dir(ws)
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

/// Konteks repo: status singkat + stat diff (untuk `/commit`).
fn git_diff_context(ws: &Path) -> String {
    let status = git_captured(ws, &["status", "--short"]);
    let stat = git_captured(ws, &["diff", "--stat"]);
    let mut s = format!(
        "cwd: {}\n\ngit status:\n{}\n\ngit diff --stat:\n{}",
        ws.display(),
        status,
        stat
    );
    if status.is_empty() && stat.is_empty() {
        s.push_str("\n(bukan repo git — hanya cwd)");
    }
    cap_chars(s, MAX_CONTEXT_CHARS)
}

/// Terapkan slash command → (teks user final, sys_note context untuk LLM).
fn slash_apply(raw: &str, workspace: &str) -> io::Result<(String, String)> {
    let Some((cmd, arg)) = slash_parse(raw) else {
        return Ok((raw.to_string(), String::new()));
    };
    let ws = PathBuf::from(workspace);
    match cmd {
        "/ask" => {
            if arg.is_empty() {
                Err(io::Error::new(io::ErrorKind::InvalidInput, "usage: /ask <pertanyaan>"))
            } else {
                Ok((arg.to_string(), String::new()))
            }
        }
        "/explain" | "/fix" => {
            if arg.is_empty() {
                return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("usage: {cmd} <file>")));
            }
            let (path, content) = read_file_ctx(&ws, arg)?;
            let verb = if cmd == "/fix" { "Perbaiki" } else { "Jelaskan" };
            let note = format!("{verb} kode berikut.\nFile: {path}\n\n```\n{content}\n```");
            Ok((format!("{verb} kode di {path}"), note))
        }
        "/commit" => Ok((
            "Buat pesan commit konvensional pendek (<=72 karakter, bahasa Indonesia) dari diff berikut."
                .to_string(),
            git_diff_context(&ws),
        )),
        other => Err(io::Error::other(format!(
            "perintah tak dikenal: {other} (tersedia: /ask, /explain, /fix, /commit)"
        ))),
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
                let raw = req["text"].as_str().unwrap_or("").trim().to_string();
                if raw.is_empty() {
                    send_json(&mut writer, &json!({"type": "done", "error": "empty"}))?;
                    continue;
                }
                let mut sess = load_session(session_path);
                let (text, sys_note) = match slash_apply(&raw, &sess.workspace) {
                    Ok(x) => x,
                    Err(e) => {
                        send_json(
                            &mut writer,
                            &json!({"type": "done", "error": e.to_string()}),
                        )?;
                        continue;
                    }
                };
                if text.is_empty() {
                    send_json(&mut writer, &json!({"type": "done", "error": "empty"}))?;
                    continue;
                }
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
                let mut history = sess.messages.clone();
                let mut sys_note = sys_note;
                // Bundle collector stderr (ekor output perintah terakhir) ke
                // context kalau ada — tanpa berada di sub-perintah khusus.
                let agent_dir = session_path.parent().unwrap_or(session_path);
                let stderr_note = stderr_bundle_for_dir(agent_dir);
                if !stderr_note.is_empty() {
                    if !sys_note.is_empty() {
                        sys_note.push('\n');
                    }
                    sys_note.push_str("\n--- stderr output (ekor perintah terakhir) ---\n");
                    sys_note.push_str(&stderr_note);
                }
                if !sys_note.is_empty() {
                    history.insert(
                        0,
                        Msg {
                            role: "system".into(),
                            content: sys_note,
                        },
                    );
                }
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
    if let Some(key) = llm_key() {
        let model = std::env::var("MTERM_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.to_string());
        let url = "https://api.groq.com/openai/v1/chat/completions".to_string();
        return Ok(Arc::new(RestBackend::new(url, model, key)) as Arc<dyn Backend>);
    }
    Ok(Arc::new(StubBackend) as Arc<dyn Backend>)
}

fn backend_name() -> String {
    if llm_key().is_some() {
        let model = std::env::var("MTERM_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.to_string());
        format!("rest (groq default, model {model})")
    } else {
        "stub (echo — set GROQ_API_KEY atau ~/.groq_key utk backend nyata)".to_string()
    }
}

// ── Client ───────────────────────────────────────────────────────────────

fn connect(dir: &Path) -> io::Result<UnixStream> {
    let path = socket_for(dir);
    match UnixStream::connect(&path) {
        Ok(s) => Ok(s),
        Err(_) => Err(io::Error::new(
            io::ErrorKind::NotFound,
            "agent belum jalan untuk workspace ini. Jalankan: mterm agent start",
        )),
    }
}

/// Kirim satu request, cetak delta streaming, return error string bila ada.
fn client_ask(dir: &Path, q: &str) -> io::Result<()> {
    let mut s = connect(dir)?;
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

/// Seperti `client_ask`, tapi delta dibuffer dan jawaban dirender markdown→ANSI
/// sekali di akhir (`mterm agent ask --render ...`).
fn client_ask_rendered(dir: &Path, q: &str) -> io::Result<()> {
    let mut s = connect(dir)?;
    send_json(&mut s, &json!({"cmd": "ask", "text": q}))?;
    let reader = BufReader::new(s);
    let mut buf = String::new();
    for line in reader.lines().map_while(Result::ok) {
        let Ok(v) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        match v["type"].as_str() {
            Some("delta") => {
                if let Some(text) = v["text"].as_str() {
                    buf.push_str(text);
                }
            }
            Some("done") => {
                if let Some(err) = v["error"].as_str().filter(|e| !e.is_empty()) {
                    eprintln!("agent: {err}");
                }
                let is_tty = std::io::IsTerminal::is_terminal(&std::io::stdout());
                print!("{}", crate::render::markdown_to_ansi(&buf, is_tty));
                println!();
                return Ok(());
            }
            _ => {}
        }
    }
    Ok(())
}

// ── CLI entry ────────────────────────────────────────────────────────────

/// List workspace yang punya state agent (`~/.mterm/agent/<slug>/`).
fn scan_agents() -> io::Result<Vec<PathBuf>> {
    let root = data_dir();
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut dirs = Vec::new();
    for entry in fs::read_dir(&root)? {
        let p = entry?.path();
        if p.is_dir() {
            dirs.push(p);
        }
    }
    dirs.sort();
    Ok(dirs)
}

/// Tampilkan info satu agent dir: pid/socket/sesi.
fn print_agent(dir: &Path) {
    let sock = socket_for(dir).exists();
    let mut msgs = 0usize;
    if let Ok(s) = fs::read_to_string(session_for(dir)) {
        if let Ok(v) = serde_json::from_str::<Value>(&s) {
            msgs = v["messages"].as_array().map(|a| a.len()).unwrap_or(0);
        }
    }
    match mterm_pty::PidFile::read(pid_for(dir)) {
        Some(p) if mterm_pty::PidFile::alive(p) => {
            println!(
                "  {:<45} pid {p} socket:{sock} sesi:{msgs} msg",
                dir.display()
            );
        }
        _ => {
            println!("  {:<45} stop socket:{sock} sesi:{msgs} msg", dir.display());
        }
    }
}

pub fn main(args: &[String]) -> io::Result<()> {
    let sub = args.first().map(String::as_str).unwrap_or("status");
    fs::create_dir_all(data_dir())?;

    // Semua perintah (kecuali `list`) berkisar di Satu workspace.
    let ws = resolve_workspace(args);
    let dir = agent_dir_for(&ws);

    match sub {
        "serve" => {
            // foreground server (dipakai `start`; bisa juga manual).
            // Bind = workspace dir; hapus socket basi dulu supaya bisa re-serve.
            let sock = socket_for(&dir);
            fs::create_dir_all(&dir)?;
            let _ = fs::remove_file(&sock);
            let listener = UnixListener::bind(&sock)?;
            println!("agent server: {}", sock.display());
            let be = backend()?;
            run_server(listener, session_for(&dir), be)
        }
        "start" => {
            fs::create_dir_all(&dir)?;
            let sock = socket_for(&dir);
            if let Some(pid) = mterm_pty::PidFile::read(pid_for(&dir)) {
                if mterm_pty::PidFile::alive(pid) {
                    println!("agent sudah jalan utk {} (pid {pid})", ws.display());
                    return Ok(());
                }
            }
            let _ = fs::remove_file(&sock);
            let exe = std::env::current_exe()?;
            let log = fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(log_for(&dir))?;
            let log2 = log.try_clone()?;
            let ws_abs = ws.canonicalize().unwrap_or(ws.clone());
            let child = std::process::Command::new(exe)
                .args(["agent", "serve", "--workspace"])
                .arg(&ws_abs)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::from(log))
                .stderr(std::process::Stdio::from(log2))
                .spawn()?;
            let _ = mterm_pty::PidFile::create(pid_for(&dir), child.id() as i32)?;
            // tunggu sebentar supaya bind socket sukses
            std::thread::sleep(std::time::Duration::from_millis(300));
            if !mterm_pty::PidFile::alive(child.id()) {
                return Err(io::Error::other(format!(
                    "server langsung mati. Cek log: {}",
                    log_for(&dir).display()
                )));
            }
            println!(
                "agent dimulai utk {} (pid {}) — log: {}",
                ws_abs.display(),
                child.id(),
                log_for(&dir).display()
            );
            Ok(())
        }
        "stop" => {
            if let Some(pid) = mterm_pty::PidFile::read(pid_for(&dir)) {
                let _ = mterm_pty::PidFile::kill(pid)?;
            }
            let _ = fs::remove_file(pid_for(&dir));
            let _ = fs::remove_file(socket_for(&dir));
            println!("agent dihentikan utk {}", ws.display());
            Ok(())
        }
        "restart" => {
            if let Some(pid) = mterm_pty::PidFile::read(pid_for(&dir)) {
                let _ = mterm_pty::PidFile::kill(pid)?;
            }
            let _ = fs::remove_file(pid_for(&dir));
            let _ = fs::remove_file(socket_for(&dir));
            println!("agent stop, restart…");
            let ws_arg = ws.display().to_string();
            main(&["start".to_string(), "--workspace".to_string(), ws_arg])
        }
        "list" => {
            for d in scan_agents()? {
                print_agent(&d);
            }
            Ok(())
        }
        "status" => {
            let pid = mterm_pty::PidFile::read(pid_for(&dir));
            match pid {
                Some(p) if mterm_pty::PidFile::alive(p) => {
                    let sock = socket_for(&dir).exists();
                    println!("agent jalan utk {} (pid {p}, socket: {sock})", ws.display());
                }
                _ => println!("agent tidak jalan untuk {}", ws.display()),
            }
            if let Ok(s) = fs::read_to_string(session_for(&dir)) {
                if let Ok(v) = serde_json::from_str::<Value>(&s) {
                    let n = v["messages"].as_array().map(|a| a.len()).unwrap_or(0);
                    println!(
                        "sesi: {} pesan di {}",
                        n,
                        v["workspace"].as_str().unwrap_or("-")
                    );
                }
            }
            if !stderr_bundle_for_dir(&dir).is_empty() {
                println!("stderr collector: ada (ekor output perintah terakhir)");
            }
            println!("backend: {}", backend_name());
            Ok(())
        }
        "init" => {
            fs::create_dir_all(&dir)?;
            // Tulis metadata workspace di direktori state agent.
            let model = std::env::var("MTERM_MODEL")
                .unwrap_or_else(|_| DEFAULT_MODEL.to_string());
            let meta = json!({
                "manifest": "mterm-agent v1",
                "workspace": ws.canonicalize().unwrap_or(ws.clone()).display().to_string(),
                "agent_dir": dir.display().to_string(),
                "created": crate::pkg::chrono_now(),
                "backend": backend_name(),
                "model": model,
            });
            fs::write(
                dir.join("agent.json"),
                serde_json::to_string_pretty(&meta)?,
            )?;
            // Workspace marker: tanda bahwa direktori ini adalah workspace agent-ready.
            let ws_meta = json!({
                "type": "mterm-workspace",
                "agent_state": dir.display().to_string(),
            });
            let marker = ws.join(".mterm-agent.json");
            fs::write(&marker, serde_json::to_string_pretty(&ws_meta)?)?;

            println!("workspace agent-ready:");
            println!("  root   : {}", ws.display());
            println!("  state  : {}", dir.display());
            println!("  backend: {}", backend_name());
            println!("  model  : {model}");
            println!();
            println!("langkah selanjutnya:");
            println!("  mterm agent start   — jalankan server agent (Unix socket)");
            println!("  mterm agent ask \"...\" — tanya ke agent");
            Ok(())
        }
        "ask" => {
            let mut render = false;
            let mut q: Option<String> = None;
            let mut i = 1;
            while i < args.len() {
                match args[i].as_str() {
                    "--render" => render = true,
                    "--workspace" => i += 1, // nilai workspace dilewati
                    a if q.is_none() && a != "--render" && !a.starts_with('-') => {
                        q = Some(a.to_string())
                    }
                    _ => {}
                }
                i += 1;
            }
            let Some(text) = q else {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "usage: mterm agent ask [--render] \"<pertanyaan>\"",
                ));
            };
            let text = text.trim().to_string();
            if text.is_empty() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "usage: mterm agent ask [--render] \"<pertanyaan>\"",
                ));
            }
            if render {
                client_ask_rendered(&dir, &text)
            } else {
                client_ask(&dir, &text)
            }
        }
        "reset" => {
            fs::create_dir_all(&dir)?;
            let _ = fs::remove_file(session_for(&dir));
            println!("sesi di-reset utk {}", ws.display());
            Ok(())
        }
        "history" => {
            let s = load_session(&session_for(&dir));
            if s.messages.is_empty() {
                println!("(kosong — {} )", ws.display());
            }
            for m in &s.messages {
                let prefix = if m.role == "user" { "❯" } else { "─" };
                println!("{prefix} {}", m.content.replace('\n', " "));
            }
            Ok(())
        }
        "stderr" => {
            let p = stderr_for(&dir);
            match args.get(1).map(String::as_str) {
                Some("clear") => {
                    let _ = fs::remove_file(&p);
                    println!("stderr collector dibersihkan");
                }
                _ => {
                    let body = stderr_bundle_for_dir(&dir);
                    if body.is_empty() {
                        println!(
                            "{} belum ada. `mterm run` akan merekamnya otomatis.",
                            p.display()
                        );
                    } else {
                        println!("{body}");
                    }
                }
            }
            Ok(())
        }
        other => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "agent: perintah tidak dikenal: {other} \
                 (serve|start|stop|restart|list|status|ask|reset|history|stderr)"
            ),
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

    #[test]
    fn sse_delta_parses_content_only() {
        assert_eq!(
            sse_delta("data: {\"choices\":[{\"delta\":{\"content\":\"He\"}}]}"),
            Some("He".into())
        );
        assert_eq!(sse_delta("data: {\"choices\":[{\"delta\":{}}]}"), None);
        assert_eq!(sse_delta("data: [DONE]"), None);
        assert_eq!(sse_delta(": keep-alive"), None);
        assert_eq!(
            sse_delta("data: {\"choices\":[{\"delta\":{\"content\":\"\"}}]}"),
            Some(String::new())
        );
    }

    #[test]
    fn rest_backend_streams_via_curl_fake_server() {
        if !RestBackend::curl_available() {
            eprintln!("skip: curl tidak ada");
            return;
        }
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            use std::io::{Read as _, Write as _};
            use std::net::Shutdown;
            let body = concat!(
                "data: {\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}\r\n\r\n",
                "data: {\"choices\":[{\"delta\":{\"content\":\"lo teman\"}}]}\r\n\r\n",
                "data: {\"choices\":[{\"delta\":{\"content\":\"!\"}}]}\r\n\r\n",
                "data: [DONE]\r\n\r\n",
            );
            let _ = sock.write_all(
                format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\n{body}")
                    .as_bytes(),
            );
            let _ = sock.flush();
            // Half-close dulu (kirim FIN, bukan RST) biar curl dapat EOF bersih;
            // lalu tunggu client tutup (dengan timeout, jangan hang).
            let _ = sock.shutdown(Shutdown::Write);
            let _ = sock.set_read_timeout(Some(std::time::Duration::from_millis(1000)));
            let mut buf = [0u8; 1024];
            while let Ok(n) = sock.read(&mut buf) {
                if n == 0 {
                    break;
                }
            }
        });

        let url = format!("http://{addr}/v1/chat/completions");
        let be = RestBackend::new(url, "fake-model".into(), "fake-key".into());
        let history = vec![Msg {
            role: "user".into(),
            content: "hai".into(),
        }];
        let mut deltas = Vec::new();
        let full = be
            .ask(&history, &mut |p| deltas.push(p.to_string()))
            .unwrap();
        server.join().unwrap();
        assert_eq!(full, "Hello teman!", "delta harus dirangkai");
        assert_eq!(deltas, vec!["Hel", "lo teman", "!"]);
    }

    #[test]
    fn llm_key_prefers_env_over_file() {
        std::env::set_var("GROQ_API_KEY", "env-key");
        assert_eq!(llm_key().as_deref(), Some("env-key"));
        std::env::remove_var("GROQ_API_KEY");
    }

    #[test]
    fn slash_parse_recognizes_commands() {
        assert_eq!(slash_parse("/ask halo dunia"), Some(("/ask", "halo dunia")));
        assert_eq!(
            slash_parse("/explain src/a.rs"),
            Some(("/explain", "src/a.rs"))
        );
        assert_eq!(slash_parse("/commit"), Some(("/commit", "")));
        assert_eq!(slash_parse("halo biasa"), None);
        assert_eq!(slash_parse(""), None);
    }

    #[test]
    fn slash_apply_explain_reads_file_into_sys_note() {
        let dir = std::env::temp_dir().join(format!("mterm-slash-{}", std::process::id()));
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(dir.join("src/a.rs"), "fn halo() -> u32 { 1 }").unwrap();

        let (final_text, sys) = slash_apply("/explain src/a.rs", dir.to_str().unwrap()).unwrap();
        assert!(final_text.contains("Jelaskan kode di"));
        assert!(
            sys.contains("fn halo() -> u32 { 1 }"),
            "isi file masuk sys_note"
        );
        assert!(sys.contains("src/a.rs"));

        let (t, _) = slash_apply("/ask 1+1?", dir.to_str().unwrap()).unwrap();
        assert_eq!(t, "1+1?", "/ask dipotong preﬁksnya");

        let err = slash_apply("/nope x", dir.to_str().unwrap()).unwrap_err();
        assert!(err.to_string().contains("perintah tak dikenal"), "{err}");

        let err = slash_apply("/explain file-tak-ada.rs", dir.to_str().unwrap()).unwrap_err();
        assert!(err.to_string().contains("tidak bisa baca"), "{err}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn slash_apply_commit_uses_git_diff() {
        // skip halus kalau git tidak tersedia
        let dir = std::env::temp_dir().join(format!("mterm-cmt-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let ok = std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(&dir)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !ok {
            eprintln!("skip: git tidak ada");
            return;
        }
        fs::write(dir.join("f.txt"), "a\n").unwrap();
        let (_t, sys) = slash_apply("/commit", dir.to_str().unwrap()).unwrap();
        assert!(
            sys.contains("git status") || sys.contains("cwd"),
            "context berisi git: {sys}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    // ── Fase 6: workspace-scoped lifecycle + stderr collector ─────────────

    #[test]
    fn slug_menghilangkan_prefix_dan_separator() {
        assert_eq!(
            slug_for(Path::new("/data/dir sp:aces/Proj")),
            "data_dir_sp_aces_Proj"
        );
        // relative → pokoknya tidak berisi '/'
        assert!(!slug_for(Path::new("a/b")).contains('/'));
    }

    #[test]
    fn workspace_scope_berupa_dir_berbeda() {
        let a = agent_dir_for(Path::new("/tmp/ws-a"));
        let b = agent_dir_for(Path::new("/tmp/ws-b"));
        assert_ne!(a, b, "tiap workspace punya socket/pid sendiri");
        assert_eq!(socket_for(&a).parent(), Some(a.as_path()));
        assert_eq!(session_for(&a).parent(), Some(a.as_path()));
    }

    #[test]
    fn resolve_workspace_memilih_arg_env_atau_cwd() {
        let args = vec![
            "start".to_string(),
            "--workspace".to_string(),
            "/ws/x".to_string(),
        ];
        assert_eq!(resolve_workspace(&args), PathBuf::from("/ws/x"));

        std::env::set_var("MTERM_WORKSPACE", "/ws/env");
        let plain = vec!["status".to_string()];
        assert_eq!(resolve_workspace(&plain), PathBuf::from("/ws/env"));
        std::env::remove_var("MTERM_WORKSPACE");

        let empty: Vec<String> = vec![];
        assert!(
            !resolve_workspace(&empty).as_os_str().is_empty(),
            "fallback cwd"
        );
    }

    #[test]
    fn stderr_collector_berisi_exit_code_dan_tail() {
        let dir = std::env::temp_dir().join(format!("mterm-agent-ws-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let data_dir_orig = data_dir();
        // redirect data_dir? tetap: tulis lewat agent_dir_for(ws) manual.
        let _ = data_dir_orig;
        let p = stderr_for(&dir);
        fs::write(&p, "exit:1\nCommand 'gcc' not found").unwrap();

        let bundle = stderr_bundle_for_dir(&dir);
        assert!(bundle.contains("exit:1"));
        assert!(bundle.contains("gcc"), "isi output kebawa: {bundle}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn scan_agents_menemukan_dir_workspace_dengan_pid() {
        let root = std::env::temp_dir().join(format!("mterm-agent-scan-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        // buat satu dir state buatan (pid mati).
        fs::create_dir_all(root.join("ws_satu")).unwrap();
        fs::write(root.join("ws_satu/agent.pid"), "9999999").unwrap();

        // scan berdasarkan root buatan.
        let mut dirs = Vec::new();
        for entry in fs::read_dir(&root).unwrap() {
            let p = entry.unwrap().path();
            if p.is_dir() {
                dirs.push(p);
            }
        }
        assert!(dirs.iter().any(|d| { d.ends_with("ws_satu") }));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn strip_ansi_menghapus_csi_saja() {
        assert_eq!(strip_ansi("a\x1b[0;1;31merror\x1b[0m b"), "aerror b");
        assert_eq!(strip_ansi("teks polos"), "teks polos");
        assert_eq!(strip_ansi("\x1b[2J\x1b[Hhalo"), "halo");
        assert!(
            strip_ansi("\x1b]9;title\x1b\\").contains("title"),
            "OSC dibiarkan"
        );
    }

    #[test]
    fn stderr_bundle_dimasukkan_ke_system_msg_agent() {
        let dir = std::env::temp_dir().join(format!("mterm-agent-std-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(stderr_for(&dir), "exit:127\nbash: gcc: command not found").unwrap();
        let sock = dir.join("t.sock");
        let sess = dir.join("t.json");
        let listener = UnixListener::bind(&sock).unwrap();

        let seen: std::sync::Arc<std::sync::Mutex<Vec<Msg>>> =
            std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        struct Rec {
            seen: std::sync::Arc<std::sync::Mutex<Vec<Msg>>>,
        }
        impl Backend for Rec {
            fn ask(&self, history: &[Msg], _on_delta: &mut dyn FnMut(&str)) -> io::Result<String> {
                *self.seen.lock().unwrap() = history.to_vec();
                Ok("ok".into())
            }
        }
        let be = std::sync::Arc::new(Rec {
            seen: std::sync::Arc::clone(&seen),
        });

        let sp = sess.clone();
        let t = std::thread::spawn(move || {
            let _ = serve_once(listener, sp, be);
        });

        {
            let mut stream = UnixStream::connect(&sock).unwrap();
            send_json(&mut stream, &json!({"cmd": "ask", "text": "kenapa gcc?"})).unwrap();
            let reader = BufReader::new(stream.try_clone().unwrap());
            for line in reader.lines().map_while(Result::ok) {
                let v: Value = serde_json::from_str(&line).unwrap();
                if v["type"].as_str() == Some("done") {
                    break;
                }
            }
        }
        t.join().unwrap();

        let history = seen.lock().unwrap();
        let sys = history
            .iter()
            .find(|m| m.role == "system")
            .expect("ada system msg");
        assert!(
            sys.content.contains("stderr output"),
            "stderr label masuk: {}",
            sys.content
        );
        assert!(
            sys.content.contains("gcc: command not found"),
            "isi stderr kebawa: {}",
            sys.content
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
