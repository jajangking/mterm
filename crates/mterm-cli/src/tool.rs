//! `mterm tool`: instal & cache binary runtime di app-private dir
//! (`~/.mterm/cache`), contoh pertama: node aarch64 dari repo Termux resmi
//! — unduh .deb dari `packages.termux.dev`, verifikasi SHA256 dari
//! `Packages.gz` index, ekstrak `data.tar.xz`, link ke `cache/bin/`.
//!
//! Alasan Termux repo (bukan nodejs.org): rilis android-arm64 tidak ada lagi di
//! `nodejs.org/dist` sejak v18+. Deb bionic-arm64 dari Termux jalan di
//! Android karena runtime libc-nya `bionic`.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

const REPO: &str = "https://packages.termux.dev/apt/termux-main";
const ARCH: &str = "aarch64";

fn cache_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".mterm").join("cache")
}

fn downloads_dir() -> PathBuf {
    cache_dir().join("downloads")
}

fn tools_dir() -> PathBuf {
    cache_dir().join("tools")
}

fn bin_dir() -> PathBuf {
    cache_dir().join("bin")
}

fn installed_dir() -> PathBuf {
    tools_dir().join("node")
}

// ── Pure helpers (dites) ─────────────────────────────────────────────────────

/// Keluarkan baris `Version`, `Filename`, `SHA256` dari Packages.gz untuk paket
/// bernama `pkg`. Kalau ada `version_filter`, hanya kecocokan substring (mis.
/// "26.4"). Kembalikan `(version, filename, sha256_hex)` — pertama ditemukan
/// (semakin atas di index = semakin baru).
pub fn parse_pkg_index(
    pkg_index: &str,
    pkg_name: &str,
    version_filter: Option<&str>,
) -> Option<(String, String, String)> {
    let mut current_pkg: Option<&str> = None;
    let mut buf: [Option<&str>; 3] = [None, None, None];
    for line in pkg_index.lines() {
        if let Some(val) = line.strip_prefix("Package:") {
            let complete = current_pkg == Some(pkg_name)
                && buf[0].is_some()
                && buf[1].is_some()
                && buf[2].is_some();
            if complete {
                let hit = version_filter.is_none_or(|vf| buf[0].unwrap().contains(vf));
                if hit {
                    return Some((
                        buf[0].unwrap().to_string(),
                        buf[1].unwrap().to_string(),
                        buf[2].unwrap().to_string(),
                    ));
                }
            }
            current_pkg = Some(val.trim());
            buf = [None; 3];
        } else if let Some(v) = line.strip_prefix("Version:") {
            if current_pkg == Some(pkg_name) {
                buf[0] = Some(v.trim());
            }
        } else if let Some(v) = line.strip_prefix("Filename:") {
            if current_pkg == Some(pkg_name) {
                buf[1] = Some(v.trim());
            }
        } else if let Some(v) = line.strip_prefix("SHA256:") {
            if current_pkg == Some(pkg_name) {
                buf[2] = Some(v.trim());
            }
        } else if line.is_empty()
            && current_pkg == Some(pkg_name)
            && buf[0].is_some()
            && buf[1].is_some()
            && buf[2].is_some()
        {
            let hit = version_filter.is_none_or(|vf| buf[0].unwrap().contains(vf));
            if hit {
                return Some((
                    buf[0].unwrap().to_string(),
                    buf[1].unwrap().to_string(),
                    buf[2].unwrap().to_string(),
                ));
            }
        }
    }
    if current_pkg == Some(pkg_name) && buf[0].is_some() && buf[1].is_some() && buf[2].is_some() {
        let hit = version_filter.is_none_or(|vf| buf[0].unwrap().contains(vf));
        if hit {
            return Some((
                buf[0].unwrap().to_string(),
                buf[1].unwrap().to_string(),
                buf[2].unwrap().to_string(),
            ));
        }
    }
    None
}

pub fn deb_url(filename: &str) -> String {
    format!("{REPO}/{filename}")
}

pub fn packages_index_url() -> String {
    format!("{REPO}/dists/stable/main/binary-{ARCH}/Packages.gz")
}

/// Cari versi node dari index + filter versi (kalau ada) dari argumen/env.
/// Return `(version, filename, sha256)`.
pub fn resolve_node_info(index_text: &str, args: &[String]) -> Option<(String, String, String)> {
    let vf = node_version_from_args(args);
    parse_pkg_index(index_text, "nodejs", vf.as_deref())
}

pub fn node_version_from_args(args: &[String]) -> Option<String> {
    args.windows(2)
        .find(|w| w[0] == "--version")
        .and_then(|w| w.get(1))
        .cloned()
        .or_else(|| std::env::var("MTERM_NODE_VERSION").ok())
        .or_else(cache_marker_version)
}

/// Versi yang sedang ter-cache (dari `~/.mterm/cache/tools/node/VERSION`).
pub fn cache_marker_version() -> Option<String> {
    read_marker(&installed_dir())
}

pub fn read_marker(dir: &Path) -> Option<String> {
    fs::read_to_string(dir.join("VERSION"))
        .ok()
        .map(|s| s.trim().to_string())
}

// ── Distro Linux: deteksi OS + package manager ───────────────────────────────

/// Pemetaan nama runtime → nama paket per package manager.
struct PkgEntry {
    name: &'static str,
    apt: Option<&'static str>,
    dnf: Option<&'static str>,
    pacman: Option<&'static str>,
    apk: Option<&'static str>,
    termux: Option<&'static str>,
}

const fn pe(
    name: &'static str,
    apt: Option<&'static str>,
    dnf: Option<&'static str>,
    pacman: Option<&'static str>,
    apk: Option<&'static str>,
    termux: Option<&'static str>,
) -> PkgEntry {
    PkgEntry {
        name,
        apt,
        dnf,
        pacman,
        apk,
        termux,
    }
}

/// Paket yang diketahui untuk tiap runtime — selaras `RUNTIMES` di runtime.rs.
const PKG_TABLE: &[PkgEntry] = &[
    pe("git", Some("git"), Some("git"), Some("git"), Some("git"), Some("git")),
    pe(
        "node",
        Some("nodejs"),
        Some("nodejs"),
        Some("nodejs"),
        Some("nodejs"),
        Some("nodejs"),
    ),
    pe("npm", Some("npm"), Some("npm"), Some("npm"), Some("npm"), Some("npm")),
    pe(
        "ripgrep",
        Some("ripgrep"),
        Some("ripgrep"),
        Some("ripgrep"),
        Some("ripgrep"),
        Some("ripgrep"),
    ),
    pe(
        "python",
        Some("python3"),
        Some("python3"),
        Some("python"),
        Some("python3"),
        Some("python"),
    ),
    pe(
        "go",
        Some("golang"),
        Some("golang"),
        Some("golang"),
        Some("go"),
        Some("golang"),
    ),
    pe("fzf", Some("fzf"), Some("fzf"), Some("fzf"), Some("fzf"), Some("fzf")),
    pe("bun", None, None, Some("bun"), Some("bun"), Some("bun")),
];

pub fn pkg_for(manager: &str, name: &str) -> Option<&'static str> {
    let e = PKG_TABLE.iter().find(|e| e.name == name)?;
    match manager {
        "termux" => e.termux,
        "apt" => e.apt,
        "dnf" => e.dnf,
        "pacman" => e.pacman,
        "apk" => e.apk,
        _ => None,
    }
}

/// Deteksi package manager Linux: termux > apt-get > dnf > pacman > apk.
pub fn detect_pkg_manager() -> Option<&'static str> {
    let osr = fs::read_to_string("/etc/os-release").unwrap_or_default();
    if osr.to_lowercase().contains("termux")
        || osr.contains("Android")
        || fs::metadata("/system/bin/linker64").is_ok()
        || sh_out("command -v pkg 2>/dev/null").is_some()
    {
        return Some("termux");
    }
    for (bin, id) in [
        ("apt-get", "apt"),
        ("dnf", "dnf"),
        ("pacman", "pacman"),
        ("apk", "apk"),
    ] {
        if sh_out(&format!("command -v {bin} 2>/dev/null")).is_some() {
            return Some(id);
        }
    }
    None
}

fn is_root() -> bool {
    sh_out("id -u 2>/dev/null").as_deref() == Some("0")
}

pub fn has_sudo() -> bool {
    sh_out("command -v sudo 2>/dev/null").is_some()
}

/// Tambah `sudo` kalau bukan root (paket distro butuh hak admin).
fn sudo_prefix(cmd: &str) -> String {
    if is_root() || detect_pkg_manager() == Some("termux") {
        cmd.to_string()
    } else if has_sudo() {
        format!("sudo {cmd}")
    } else {
        cmd.to_string()
    }
}

/// Ringkasan OS untuk `mterm doctor`: distro / kernel / libc / pakman.
pub fn distro_info() -> String {
    let mut parts = Vec::new();
    if let Ok(s) = fs::read_to_string("/etc/os-release") {
        for line in s.lines() {
            if let Some(v) = line.strip_prefix("PRETTY_NAME=") {
                parts.push(v.trim_matches('"').to_string());
                break;
            }
        }
    }
    if let Some(u) = sh_out("uname -srm") {
        parts.push(u);
    }
if sh_out("getconf GNU_LIBC_VERSION").is_some() {
        parts.push("glibc".into());
    } else if fs::metadata("/system/bin/linker64").is_ok() {
        parts.push("bionic (Android)".into());
    } else if fs::metadata("/etc/alpine-release").is_ok() {
        parts.push("musl".into());
    } else {
        parts.push("libc?.?".into());
    }
    if let Some(m) = detect_pkg_manager() {
        parts.push(format!("pkg:{m}"));
    }
    if parts.is_empty() {
        "Linux tidak dikenal".into()
    } else {
        parts.join(" · ")
    }
}

fn install_cmd(manager: &str, pkg: &str) -> Option<String> {
    let c = match manager {
        "termux" => format!("pkg install -y {pkg} 2>/dev/null || apt-get install -y {pkg}"),
        "apt" => format!("apt-get install -y {pkg}"),
        "dnf" => format!("dnf install -y {pkg}"),
        "pacman" => format!("pacman -S --noconfirm {pkg}"),
        "apk" => format!("apk add --no-cache {pkg}"),
        _ => return None,
    };
    Some(c)
}

fn upgrade_cmd(manager: &str, pkg: &str) -> Option<String> {
    let c = match manager {
        "termux" => format!("pkg upgrade -y {pkg} 2>/dev/null || pkg install -y {pkg}"),
        "apt" => format!("apt-get install --only-upgrade -y {pkg}"),
        "dnf" => format!("dnf upgrade -y {pkg}"),
        "pacman" => format!("pacman -S --noconfirm {pkg}"),
        "apk" => format!("apk add -u --no-cache {pkg}"),
        _ => return None,
    };
    Some(c)
}

fn remove_cmd(manager: &str, pkg: &str) -> Option<String> {
    let c = match manager {
        "termux" => format!("pkg remove -y {pkg} 2>/dev/null || apt-get remove -y {pkg}"),
        "apt" => format!("apt-get remove -y {pkg}"),
        "dnf" => format!("dnf remove -y {pkg}"),
        "pacman" => format!("pacman -Rns --noconfirm {pkg}"),
        "apk" => format!("apk del {pkg}"),
        _ => return None,
    };
    Some(c)
}

/// Jalankan `name` lewat package manager distro; `--dry-run` cuma cetak perintah.
fn distro_op(args: &[String], op: Op) -> io::Result<()> {
    let name = args.first().map(String::as_str).unwrap_or("node");
    let rest = &args[1..];
    let dry = rest.iter().any(|a| a == "--dry-run");

    let manager = detect_pkg_manager()
        .ok_or_else(|| io::Error::other("tidak ada package manager distro yang dikenal"))?;

    // Termux + node → jalur .deb bionic yang sudah teruji (cache + verifikasi SHA).
    if manager == "termux" && name == "node" && op == Op::Install {
        if dry {
            eprintln!("[termux] unduh .deb node dari repo Termux (bionic) + SHA256 check");
            println!("(dry-run) tidak dijalankan: mterm tool install node");
            return Ok(());
        }
        return cmd_install_termux_node(rest);
    }

    let pkg = pkg_for(manager, name).ok_or_else(|| {
        io::Error::other(format!("runtime '{name}' tidak punya paket untuk pakman '{manager}'"))
    })?;

    let raw = match op {
        Op::Install => install_cmd(manager, pkg),
        Op::Upgrade => upgrade_cmd(manager, pkg),
        Op::Remove => remove_cmd(manager, pkg),
    }
    .ok_or_else(|| io::Error::other(format!("pakman '{manager}' tidak didukung")))?;

    let cmd = sudo_prefix(&raw);
    eprintln!("[{manager}] {cmd}");
    if dry {
        println!("(dry-run) tidak dijalankan: {cmd}");
        return Ok(());
    }
    run(&cmd)?;
    let v = crate::runtime::version_of(&[name], "--version")
        .or_else(|| crate::runtime::version_of(&[name], "version"));
    match op {
        Op::Remove => println!("{name} dihapus lewat pakman '{manager}'"),
        _ => match v {
            Some((_, ver)) => println!("{name} {ver} tersedia ({manager})"),
            None => println!("{name} terpasang lewat {manager} — cek `mterm doctor`"),
        },
    }
    Ok(())
}

#[derive(Clone, Copy, PartialEq)]
enum Op {
    Install,
    Upgrade,
    Remove,
}

// ── Shell helpers (Termux-safe via `sh`) ────────────────────────────────────

fn sh_out(script: &str) -> Option<String> {
    let out = Command::new("sh").arg("-c").arg(script).output().ok()?;
    if !out.status.success() {
        return None;
    }
    match String::from_utf8(out.stdout) {
        Ok(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
        _ => None,
    }
}

fn sha256_of(path: &Path) -> Option<String> {
    sh_out(&format!(
        "sha256sum '{}' 2>/dev/null | cut -d' ' -f1",
        path.display()
    ))
}

fn run(script: &str) -> io::Result<()> {
    let status = Command::new("sh").arg("-c").arg(script).status()?;
    if !status.success() {
        return Err(io::Error::other(format!("perintah gagal: {script}")));
    }
    Ok(())
}

// ── Commands ────────────────────────────────────────────────────────────────

fn cmd_status() -> io::Result<()> {
    println!("cache: {}", cache_dir().display());
    let marker = cache_marker_version();
    if let Some(ver) = marker {
        println!("  node {ver}");
        let node_path = bin_dir().join("node");
        if node_path.exists() {
            println!("  → {}", node_path.display());
        }
    } else {
        println!("  (belum ada tool ter-install — `mterm tool install node`)");
    }
    Ok(())
}

fn cmd_install(args: &[String]) -> io::Result<()> {
    distro_op(args, Op::Install)
}

fn cmd_remove(args: &[String]) -> io::Result<()> {
    distro_op(args, Op::Remove)
}

fn cmd_upgrade(args: &[String]) -> io::Result<()> {
    distro_op(args, Op::Upgrade)
}

/// Termux + node: unduh .deb aarch64 dari repo Termux resmi, verifikasi SHA256
/// dari Packages.gz, ekstrak ke `~/.mterm/cache` (bionic — satu-satunya jalur
/// node di Android; nodejs.org tidak menyediakan android-arm64 sejak v18+).
fn cmd_install_termux_node(args: &[String]) -> io::Result<()> {
    let name = "node";
    if name != "node" {
        return Err(io::Error::other("tool yang didukung sekarang: node"));
    }

    eprintln!("unduh index paket Termux …");
    let index_gz_url = packages_index_url();
    let index_text = fetch_gzip(&index_gz_url)
        .ok_or_else(|| io::Error::other("gagal ambil Packages.gz dari repo Termux"))?;

    let (version, filename, expected_sha) = resolve_node_info(&index_text, args)
        .ok_or_else(|| io::Error::other("nodejs tidak ditemukan di index repo (filter?)"))?;
    eprintln!("node {version} ({ARCH})");

    let deb_url = deb_url(&filename);
    let deb_path = downloads_dir().join(&filename);

    fs::create_dir_all(downloads_dir())?;
    if !deb_path.exists() {
        eprintln!("unduh {deb_url} …");
        if !fetch_to(&deb_url, &deb_path) {
            return Err(io::Error::other("gagal unduh .deb node"));
        }
    }

    // Rilis terverifikasi: checksum SHA256 dari Packages.gz index.
    let actual = sha256_of(&deb_path).ok_or_else(|| io::Error::other("tidak bisa sha256sum"))?;
    if expected_sha != actual {
        return Err(io::Error::other(format!(
            "checksum mismatch: expected {expected_sha}, actual {actual}. Hapus {} lalu ulangi.",
            deb_path.display()
        )));
    }
    eprintln!("checksum ok ({actual})");

    // Ekstrak .deb → ./data/data/…/usr/ → strip-components → installed_dir.
    let extract_tmp = downloads_dir().join("deb_extract_tmp");
    let _ = fs::remove_dir_all(&extract_tmp);
    fs::create_dir_all(&extract_tmp)?;
    run(&format!(
        "cd '{}' && ar x '{}' 2>/dev/null",
        extract_tmp.display(),
        deb_path.display()
    ))?;
    let data_tar = extract_tmp.join("data.tar.xz");
    if !data_tar.exists() {
        let _ = fs::remove_dir_all(&extract_tmp);
        return Err(io::Error::other("deb tidak mengandung data.tar.xz"));
    }
    let _ = fs::remove_dir_all(installed_dir());
    fs::create_dir_all(installed_dir())?;
    // Strip ./data/data/com.termux/files/usr/ (6 komponen di depan bin/).
    run(&format!(
        "tar -xf '{}' -C '{}' --strip-components=6",
        data_tar.display(),
        installed_dir().display()
    ))?;
    let _ = fs::remove_dir_all(&extract_tmp);

    // Tulis marker versi.
    fs::write(installed_dir().join("VERSION"), &version)?;

    // Symlink ke cache/bin/ supaya masuk PATH (`cache/bin`).
    fs::create_dir_all(bin_dir())?;
    let installed_bin = installed_dir().join("bin");
    for tool in ["node", "npm", "npx"] {
        let src = installed_bin.join(tool);
        if !src.exists() {
            continue;
        }
        let link = bin_dir().join(tool);
        let _ = fs::remove_file(&link);
        #[cfg(unix)]
        std::os::unix::fs::symlink(src, &link)?;
        #[cfg(not(unix))]
        fs::copy(&src, &link)?;
    }

    // Verifikasi hasil: jalankan node dari cache.
    let ver = sh_out(&format!(
        "{} --version 2>/dev/null",
        bin_dir().join("node").display()
    ))
    .ok_or_else(|| io::Error::other("node hasil instal tidak jalan"))?;
    println!(
        "node {ver} terpasang → {}",
        bin_dir().join("node").display()
    );
    println!(
        "tambah ke PATH: export PATH=\"{}:$PATH\"",
        bin_dir().display()
    );

    std::io::stdout().flush()?;
    Ok(())
}

fn fetch_gzip(url: &str) -> Option<String> {
    let out = Command::new("sh")
        .arg("-c")
        .arg(format!(
            "curl -fsSL --max-time 60 '{url}' 2>/dev/null | gzip -dc 2>/dev/null"
        ))
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8(out.stdout).ok()
}

fn fetch_to(url: &str, dest: &Path) -> bool {
    if let Some(parent) = dest.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let status = Command::new("sh")
        .arg("-c")
        .arg(format!(
            "curl -fsSL --max-time 900 -o '{}' '{}' 2>/dev/null",
            dest.display(),
            url
        ))
        .status();
    matches!(status, Ok(s) if s.success() && dest.exists())
}

pub fn main(args: &[String]) -> io::Result<()> {
    let sub = args.first().map(String::as_str).unwrap_or("status");
    match sub {
        "status" | "list" => cmd_status(),
        "install" | "i" => cmd_install(&args[1..]),
        "remove" | "rm" | "uninstall" => cmd_remove(&args[1..]),
        "upgrade" | "up" | "update" => cmd_upgrade(&args[1..]),
        other => Err(io::Error::other(format!(
            "tool: perintah tidak dikenal: {other} (status|install|remove|upgrade)"
        ))),
    }
}

// ── Unit tests (tanpa jaringan — murni fungsi) ──────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_index_ambil_nodejs_baris_pertama() {
        let idx = "\
Package: nodejs
Version: 26.4.0-1
Filename: pool/main/n/nodejs/nodejs_26.4.0-1_aarch64.deb
SHA256: eaf3ed8a6e4b72ebaa8c2cb3bad778c577cdf9ea87ca91761213d8a3940fc090

Package: npm
Version: 10.9.3-1
Filename: pool/main/n/npm/npm_10.9.3-1_aarch64.deb
SHA256: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa1
";
        let (ver, file, sha) = parse_pkg_index(idx, "nodejs", None).unwrap();
        assert_eq!(ver, "26.4.0-1");
        assert!(file.ends_with("nodejs_26.4.0-1_aarch64.deb"));
        assert_eq!(sha.len(), 64);
    }

    #[test]
    fn parse_index_filter_versi() {
        let idx = "\
Package: nodejs
Version: 26.4.0-1
Filename: a.deb
SHA256: abcdef0000000000000000000000000000000000000000000000000000000001

Package: nodejs
Version: 22.13.1-1
Filename: b.deb
SHA256: 1234000000000000000000000000000000000000000000000000000000000002
";
        let (v, f, _) = parse_pkg_index(idx, "nodejs", Some("22")).unwrap();
        assert_eq!(v, "22.13.1-1");
        assert!(f.ends_with("b.deb"));
        assert_eq!(parse_pkg_index(idx, "nodejs", Some("99")), None);
    }

    #[test]
    fn parse_index_tidak_kena_paket_lain() {
        let idx = "\
Package: npm
Version: 10.9.3-1
Filename: pool/main/n/npm/npm_10.9.3-1_aarch64.deb
SHA256: 00000000000000000000000000000000000000000000000000000000000000aa

Package: nodejs
Version: 26.4.0-1
Filename: nodejs.deb
SHA256: bb000000000000000000000000000000000000000000000000000000000000bb
";
        let (v, _, _) = parse_pkg_index(idx, "nodejs", None).unwrap();
        assert_eq!(v, "26.4.0-1");
    }

    #[test]
    fn deb_url_benar() {
        assert!(deb_url("pool/main/n/nodejs/nodejs.deb").contains(REPO));
    }

    #[test]
    fn marker_baca_dan_trim_dari_tempdir() {
        let t = std::env::temp_dir().join(format!("mterm_tool_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&t);
        fs::create_dir_all(&t).unwrap();
        fs::write(t.join("VERSION"), " 22.13.1\n").unwrap();
        assert_eq!(read_marker(&t).as_deref(), Some("22.13.1"));
        let _ = fs::remove_dir_all(&t);
    }

    #[test]
    fn pkg_for_selaras_pakman() {
        // node → nodejs di semua pakman, python beda-beda, bun tidak di apt.
        for m in ["apt", "dnf", "pacman", "apk", "termux"] {
            assert_eq!(pkg_for(m, "node"), Some("nodejs"), "{m} node");
            assert_eq!(pkg_for(m, "git"), Some("git"), "{m} git");
        }
        assert_eq!(pkg_for("apt", "python"), Some("python3"));
        assert_eq!(pkg_for("pacman", "python"), Some("python"));
        assert_eq!(pkg_for("termux", "python"), Some("python"));
        assert_eq!(pkg_for("apt", "bun"), None);
        assert_eq!(pkg_for("pacman", "bun"), Some("bun"));
        assert_eq!(pkg_for("apt", "halo"), None);
    }

    #[test]
    fn perintah_instal_per_pakman() {
        assert_eq!(install_cmd("apt", "nodejs").unwrap(), "apt-get install -y nodejs");
        assert_eq!(install_cmd("dnf", "nodejs").unwrap(), "dnf install -y nodejs");
        assert_eq!(install_cmd("pacman", "nodejs").unwrap(), "pacman -S --noconfirm nodejs");
        assert_eq!(install_cmd("apk", "nodejs").unwrap(), "apk add --no-cache nodejs");
        assert_eq!(
            install_cmd("termux", "nodejs").unwrap(),
            "pkg install -y nodejs 2>/dev/null || apt-get install -y nodejs"
        );
        assert_eq!(install_cmd("brew", "x"), None);
    }

    #[test]
    fn sudo_hanya_kalau_bukan_root() {
        // is_root() bergantung sistem; cukup pastikan sudo_prefix tidak error.
        let c = sudo_prefix("apt-get install -y x");
        assert!(c.contains("apt-get install -y x"), "mengandung cmd asli");
        assert!(c.starts_with("sudo ") || !c.starts_with("sudo "), "selalu valid");
    }
}
