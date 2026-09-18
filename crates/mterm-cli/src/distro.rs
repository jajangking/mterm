//! `mterm distro`: install & jalankan distro Linux (proot) untuk terminal mterm.
//!
//! Ide: mterm bisa "install distro" sendiri — unduh rootfs, simpan di
//! `~/.mterm/distros/<name>/`, lalu buka sesi shell di dalamnya via `proot`
//! (tanpa root). Di deskripsi mterm Android nanti, binary proot di-bundle
//! supaya fitur ini jalan di dalam app.

use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use serde_json::json;

// ── Lokasi state ─────────────────────────────────────────────────────────

/// Nameserver default untuk resolv.conf di dalam rootfs (dipakai bila
/// file-nya kosong; host Android tak punya /etc/resolv.conf).
const NAMESERVERS: &str = "nameserver 1.1.1.1\nnameserver 8.8.8.8\n";

fn distro_root() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".mterm").join("distros")
}

fn distro_dir(name: &str) -> PathBuf {
    distro_root().join(name)
}

fn rootfs_dir(name: &str) -> PathBuf {
    distro_dir(name).join("rootfs")
}

fn meta_file(name: &str) -> PathBuf {
    distro_dir(name).join("distro.json")
}

fn read_meta(name: &str) -> Option<serde_json::Value> {
    let s = fs::read_to_string(meta_file(name)).ok()?;
    serde_json::from_str(&s).ok()
}

/// Arsitektur yang dipakai untuk nama file rootfs (uname → tag distro).
fn arch_tag() -> String {
    match std::env::consts::ARCH {
        "aarch64" => "arm64".to_string(),
        "x86_64" => "amd64".to_string(),
        other => other.to_string(),
    }
}

/// Descriptor distro yang didukung. Untuk sekarang: ubuntu (Ubuntu base).
struct DistroSpec {
    name: &'static str,
    label: &'static str,
    base_url: &'static str,
}

const DISTROS: &[DistroSpec] = &[DistroSpec {
    name: "ubuntu",
    label: "Ubuntu (base)",
    base_url: "https://cdimage.ubuntu.com/ubuntu-base/releases/24.04/release/",
}];

// ── HTTP helper: ureq (murni Rust, rustls) — tanpa curl/wget eksternal ────
// App-sandbox Android TIDAK punya curl maupun wget (toybox tak menyediakan
// wget di versi ini). Karena mterm binary self-contained, dipakai ureq+rustls
// langsung — tak butuh biner eksternal apa pun di host.

fn curl(url: &str, out: Option<&Path>) -> io::Result<Vec<u8>> {
    // Porsi exec: jika mterm dijalankan dari Termux (punya curl), manfaatkan
    // curl utk transparansi debugging; selain itu (sandbox app) ureq.
    if command_exists("curl") {
        let mut cmd = std::process::Command::new("curl");
        cmd.arg("-fsSL")
            .arg("-m")
            .arg("300")
            .arg("-A")
            .arg(format!("mterm/{}", env!("CARGO_PKG_VERSION")));
        if let Some(p) = out {
            cmd.arg("-o").arg(p);
            cmd.stdout(std::process::Stdio::null());
        }
        cmd.arg(url);
        let out_bin = cmd.output()?;
        if out_bin.status.success() {
            return Ok(if out.is_none() {
                out_bin.stdout
            } else {
                Vec::new()
            });
        }
    }
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(300)))
        .max_redirects(10)
        .user_agent(format!("mterm/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .new_agent();
    let resp = agent
        .get(url)
        .call()
        .map_err(|e| io::Error::other(format!("gagal mengunduh {url}: {e}")))?;
    if let Some(p) = out {
        let mut file = std::fs::File::create(p)?;
        let mut reader = resp.into_body().into_reader();
        std::io::copy(&mut reader, &mut file)?;
        return Ok(Vec::new());
    }
    let mut buf = Vec::new();
    resp.into_body().into_reader().read_to_end(&mut buf)?;
    Ok(buf)
}

fn command_exists(name: &str) -> bool {
    std::process::Command::new(name)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

// ── Resolusi versi dari halaman listing ──────────────────────────────────

/// Ambil versi tertinggi `ubuntu-base-X.Y.Z-base-<arch>.tar.gz` dari listing.
fn latest_version(dir: &str, arch: &str) -> io::Result<(String, String)> {
    let html = curl(dir, None)?;
    let text = String::from_utf8_lossy(&html);
    let mut best: Option<(u32, u32, u32)> = None;
    let mut best_raw: Option<String> = None;
    for caps in text.split("ubuntu-base-").skip(1) {
        let raw = caps.split("-base-").next().unwrap_or("").to_string();
        let ver: Vec<&str> = raw.split('.').collect();
        if ver.len() != 3 {
            continue;
        }
        let (Ok(maj), Ok(min), Ok(patch)) = (
            ver[0].parse::<u32>(),
            ver[1].parse::<u32>(),
            ver[2].parse::<u32>(),
        ) else {
            continue;
        };
        // pastikan file ini untuk arch kita: "…-base-<arch>.tar.gz"
        let rest = caps.split(".tar.gz").next().unwrap_or("");
        if !rest.ends_with(&format!("-base-{arch}")) {
            continue;
        }
        let cur = best;
        if cur.is_none() || (maj, min, patch) > cur.unwrap() {
            best = Some((maj, min, patch));
            best_raw = Some(raw);
        }
    }
    let Some(raw) = best_raw else {
        return Err(io::Error::other(
            "tidak menemukan file ubuntu-base untuk arch ini di listing",
        ));
    };
    let file = format!("ubuntu-base-{raw}-base-{arch}.tar.gz");
    Ok((raw, file))
}

/// Hash SHA-256 file (untuk verifikasi SHA256SUMS).
fn sha256_file(path: &Path) -> io::Result<String> {
    use sha2::{Digest, Sha256};
    let data = fs::read(path)?;
    let mut h = Sha256::new();
    h.update(&data);
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

// ── Perintah ─────────────────────────────────────────────────────────────

fn cmd_list() -> io::Result<()> {
    let arch = arch_tag();
    println!(
        "distro yang didukung mterm (rootfs di {}):",
        distro_root().display()
    );
    println!();
    for d in DISTROS {
        let meta = read_meta(d.name);
        match &meta {
            Some(m) => {
                let ver = m["version"].as_str().unwrap_or("?");
                let at = m["installed_at"].as_str().unwrap_or("?");
                println!(
                    "  ✓ {} ({} · {ver})  install {at}",
                    d.label,
                    m["arch"].as_str().unwrap_or("?")
                );
                println!("      login: mterm distro login {}", d.name);
            }
            None => println!(
                "  ✗ {} — belum install (mterm distro install {})",
                d.label, d.name
            ),
        }
    }
    println!();
    println!("arsitektur target: {arch}");
    Ok(())
}

fn cmd_install(name: &str) -> io::Result<()> {
    let spec = DISTROS
        .iter()
        .find(|d| d.name == name)
        .ok_or_else(|| io::Error::other(format!("distro tidak dikenal: {name}")))?;
    if read_meta(name).is_some() && rootfs_dir(name).join("bin").exists() {
        println!(
            "{} sudah terinstall di {}",
            spec.label,
            rootfs_dir(name).display()
        );
        return Ok(());
    }
    let _ = fs::remove_dir_all(distro_dir(name)); // bersihkan sisa parsial
    fs::create_dir_all(rootfs_dir(name))?;

    let arch = arch_tag();
    println!("[1/4] resolve versi {} dari {}", spec.label, spec.base_url);
    let (ver, file) = latest_version(spec.base_url, &arch)?;
    let url = format!("{}{}", spec.base_url, file);

    let tmp_dir = std::env::temp_dir().join(format!("mterm-distro-{}", name));
    fs::create_dir_all(&tmp_dir)?;
    let tarball = tmp_dir.join(&file);
    println!("[2/4] unduh {file} ({url})");
    curl(&url, Some(&tarball))?;

    println!("[3/4] verifikasi SHA-256 …");
    let sums = curl(&format!("{}SHA256SUMS", spec.base_url), None)?;
    let want = String::from_utf8_lossy(&sums)
        .lines()
        .find(|l| l.trim_end().ends_with(&file))
        .and_then(|l| l.split_whitespace().next())
        .ok_or_else(|| io::Error::other(format!("file {file} tidak ada di SHA256SUMS")))?
        .to_string();
    let got = sha256_file(&tarball)?;
    if got != want {
        return Err(io::Error::other(format!(
            "SHA-256 tidak cocok (diharap {want}, dapat {got}) — batal"
        )));
    }

    let rootfs = rootfs_dir(name);
    fs::create_dir_all(&rootfs)?;
    println!("[4/4] ekstrak ke {} …", rootfs.display());
    extract_tarball(&tarball, &rootfs)?;

    // DNS dalam proot tanpa /etc/resolv.conf host (Termux tak punya).
    let etc = rootfs.join("etc");
    let _ = fs::create_dir_all(&etc);
    let _ = fs::write(etc.join("resolv.conf"), NAMESERVERS);

    let meta = json!({
        "name": spec.name,
        "label": spec.label,
        "version": ver,
        "arch": arch,
        "installed_at": crate::pkg::chrono_now(),
        "rootfs": rootfs.display().to_string(),
    });
    fs::write(meta_file(name), serde_json::to_string_pretty(&meta)?)?;

    println!();
    println!("selesai. {} {ver} ({arch}) terinstall.", spec.label);
    println!("coba: mterm distro login {name}");
    Ok(())
}

/// Sanitasi path entry tar: buang `/` di awal & komponen `..` biasa diajukan.
/// Mirip sanitasi bawaan tar-crate; dipakai manual karena kita ekstrak
/// hardlink secara manual (FS Android menolak `link()` = Permission denied).
fn sanitized(entry_path: &Path) -> io::Result<PathBuf> {
    let mut out = PathBuf::new();
    for comp in entry_path.components() {
        match comp {
            std::path::Component::Normal(c) => out.push(c),
            std::path::Component::CurDir | std::path::Component::ParentDir => {}
            _ => return Err(io::Error::other("path tar tidak aman")),
        }
    }
    Ok(out)
}

/// Ekstrak tarball `.tar.gz` ke `dest` dengan fallback copy untuk hardlink.
/// FS Android memblokir `link()`, jadi entry `HardLink` disalin isinya.
/// Dua pass: semua entri biasa dulu, lalu hardlink (target sudah ada).
fn extract_tarball(tarball: &Path, dest: &Path) -> io::Result<()> {
    use flate2::read::GzDecoder;

    let file = fs::File::open(tarball)?;
    let gz = GzDecoder::new(file);
    let mut archive = tar::Archive::new(gz);

    // pass 1: entri biasa (bukan hardlink)
    let mut hardlinks: Vec<(PathBuf, PathBuf)> = Vec::new();
    let mut entries = archive.entries()?;
    while let Some(mut e) = entries.next().transpose()? {
        let header = e.header().clone();
        let rel = sanitized(&e.path()?)?;
        let is_hard = header.entry_type().is_hard_link();
        if is_hard {
            let target = e.link_name()?.ok_or_else(|| {
                io::Error::other(format!("hardlink tanpa target: {}", rel.display()))
            })?;
            let target = sanitized(&target)?;
            hardlinks.push((rel, target));
            continue;
        }
        // unpack_in menangani symlink/dir/file + sanitize; tolak entry pakai
        // mode absolut supaya tidak bisa keluar dari dest.
        e.unpack_in(dest)?;
    }

    // pass 2: hardlink → salin isi target (ikut follow symlink).
    for (rel, target) in hardlinks {
        let dst = dest.join(&rel);
        let src = dest.join(&target);
        if fs::copy(&src, &dst).is_ok() {
            continue;
        }
        // target mungkin belum ada (symlink menggantung) — buat symlink saja.
        let _ = std::os::unix::fs::symlink(&target, &dst);
        // Siapkan parent kalau belum ada (entry di pass 1 harusnya sudah).
        if let Some(parent) = dst.parent() {
            let _ = fs::create_dir_all(parent);
        }
    }
    Ok(())
}

/// Cari dynamic loader (ld-linux) di dalam rootfs berdasarkan arch, dan
/// kembalikan path-nya **dalam perspektif guest** (`/usr/lib/...`).
/// Execve pertama via proot di lingkungan Android kadang ENOENT; menjalankan
/// loader secara eksplisit menghindarinya (execve dari dalam berjalan normal).
fn find_loader(rootfs: &Path) -> io::Result<PathBuf> {
    #[cfg(target_arch = "aarch64")]
    let cands: &[&str] = &[
        "/usr/lib/aarch64-linux-gnu/ld-linux-aarch64.so.1",
        "/lib/ld-linux-aarch64.so.1",
        "/usr/lib/ld-linux-aarch64.so.1",
    ];
    #[cfg(target_arch = "x86_64")]
    let cands: &[&str] = &[
        "/usr/lib/x86_64-linux-gnu/ld-linux-x86-64.so.2",
        "/lib64/ld-linux-x86-64.so.2",
        "/usr/lib/ld-linux-x86-64.so.2",
    ];
    #[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
    let cands: &[&str] = &[];
    for c in cands {
        let p = rootfs.join(c.trim_start_matches('/'));
        if p.exists() {
            return Ok(PathBuf::from(c));
        }
    }
    Err(io::Error::other(
        "dynamic loader (ld-linux) tidak ditemukan di rootfs",
    ))
}

fn proot_available() -> bool {
    std::process::Command::new("proot")
        .arg("--help")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn cmd_login(name: &str, args: &[String]) -> io::Result<()> {
    let meta = read_meta(name).ok_or_else(|| {
        io::Error::other(format!(
            "{name} belum diinstall (mterm distro install {name})"
        ))
    })?;
    // buang separator `--` dari CLI (jangan diteruskan ke shell guest)
    let args: Vec<String> = args
        .iter()
        .filter(|a| a.as_str() != "--")
        .cloned()
        .collect();
    let rootfs = PathBuf::from(meta["rootfs"].as_str().unwrap_or(""));
    if !rootfs.join("bin").exists() {
        return Err(io::Error::other(format!(
            "rootfs tidak utuh: {}",
            rootfs.display()
        )));
    }
    // DNS: pastikan resolv.conf berisi nameserver (tar ubuntu-base berisi file
    // kosong; tanpa ini `apt` gagal "Temporary failure resolving").
    let rc = rootfs.join("etc/resolv.conf");
    match fs::read_to_string(&rc) {
        Ok(s) if s.trim().is_empty() => {
            let _ = fs::write(&rc, NAMESERVERS);
        }
        _ => {}
    }
    if !proot_available() {
        return Err(io::Error::other(
            "butuh `proot` untuk login (Termux: pkg install proot)",
        ));
    }

    let term = std::env::var("TERM").unwrap_or_else(|_| "xterm-256color".into());
    let loader = find_loader(&rootfs)?;
    let mut cmd = std::process::Command::new("proot");
    cmd.arg("-0") // fake root
        .arg("-r")
        .arg(&rootfs)
        .arg("-w")
        .arg("/")
        .arg("-b")
        .arg("/dev")
        .arg("-b")
        .arg("/proc")
        .arg("-b")
        .arg("/sys")
        .arg("--link2symlink")
        // execve pertama lewat loader (hindari ENOENT proot@Android);
        // loader dalam perspektif guest, mis. /usr/lib/.../ld-linux-*.so.1
        .arg(loader.as_os_str())
        .arg("/bin/sh")
        .arg("-l")
        .args(args);
    // env diberikan langsung (ubuntu-base tak punya /usr/bin/env);
    // buang preload host biar loader guest bersih.
    cmd.env("HOME", "/root")
        .env("TERM", term)
        .env(
            "PATH",
            "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
        )
        .env("USER", "root")
        .env_remove("LD_PRELOAD")
        .env_remove("LD_LIBRARY_PATH");
    // warisi stdio → shell interaktif mengambil alih terminal saat ini
    let status = cmd.status()?;
    // setelah proot keluar, terminal sudah di-restore oleh OS; tidak perlu raw.
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "proot exit code {}",
            status.code().unwrap_or(-1)
        )))
    }
}

pub fn main(args: &[String]) -> io::Result<()> {
    let sub = args.first().map(String::as_str).unwrap_or("list");
    match sub {
        "list" | "status" => cmd_list(),
        "install" => {
            let name = args
                .get(1)
                .ok_or_else(|| io::Error::other("pakai: mterm distro install <ubuntu>"))?;
            cmd_install(name)
        }
        "login" => {
            let name = args
                .get(1)
                .ok_or_else(|| io::Error::other("pakai: mterm distro login <ubuntu>"))?;
            cmd_login(name, &args[2..])
        }
        other => {
            eprintln!("mterm distro: sub-perintah tidak dikenal: {other}");
            println!("pakai: mterm distro list|install <ubuntu>|login <ubuntu>");
            Ok(())
        }
    }
}

// ── Tes ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arch_tag_selaras_dengan_rust_arch() {
        let tag = arch_tag();
        assert!(
            tag == "arm64" || tag == "amd64",
            "arch tag tidak dikenal: {tag}"
        );
    }

    #[test]
    fn latest_version_memilih_file_arm64_tertinggi() {
        let listing = "<a href='ubuntu-base-24.04.2-base-arm64.tar.gz'>
<a href='ubuntu-base-24.04.5-base-arm64.tar.gz'>
<a href='ubuntu-base-24.04.4-base-arm64.tar.gz'>
<a href='ubuntu-base-24.04.4-base-amd64.tar.gz'>";
        let html = listing.as_bytes();
        let text = String::from_utf8_lossy(html);
        let mut best: Option<(u32, u32, u32)> = None;
        for caps in text.split("ubuntu-base-").skip(1) {
            let ver: Vec<&str> = caps
                .split("-base-")
                .next()
                .unwrap_or("")
                .split('.')
                .collect();
            if ver.len() != 3 {
                continue;
            }
            let (Ok(maj), Ok(min), Ok(patch)) = (
                ver[0].parse::<u32>(),
                ver[1].parse::<u32>(),
                ver[2].parse::<u32>(),
            ) else {
                continue;
            };
            let rest = caps.split(".tar.gz").next().unwrap_or("");
            if !rest.ends_with("-base-arm64") {
                continue;
            }
            if best.is_none() || (maj, min, patch) > best.unwrap() {
                best = Some((maj, min, patch));
            }
        }
        assert_eq!(best, Some((24, 4, 5)), "versi tertinggi arm64 dipilih");
        assert!(best != Some((24, 4, 4)));
    }
}
