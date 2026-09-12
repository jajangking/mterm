//! `mterm pkg`: pengelola paket ringan mandiri (bukan apt/dpkg).
//!
//! Konsep:
//! - Repo standalone (mis. https://mirror/ atau file://tmp/repo) berisi
//!   `index.json` (metadata paket), `index.json.sig` (tanda tangan Ed25519
//!   opsional), `pkgs/<name>-<ver>.tar.zst` (arsip), dan `parts/<sha256>`
//!   (potongan arsip 1 MiB, content-addressed) untuk delta rsync-style.
//! - `pkg install` mengunduh **hanya potongan yang belum punya** sha-nya di
//!   cache — versi baru yang sebagian besar sama hanya menarik bagian yg berubah.
//! - Instal ke prefix app-private `~/.mterm/pkg/prefix` (tanpa root).
//! - `pkg update` memverifikasi index terhadap tanda tangan bila kunci publik
//!   sudah disimpan (`pkg keygen`).

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const FORMAT: u8 = 1;
const PART_SIZE: u64 = 1024 * 1024; // 1 MiB — granularity delta rsync-style

// ── Layout data ──────────────────────────────────────────────────────────────

fn pkg_root() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".mterm").join("pkg")
}
fn prefix_dir() -> PathBuf {
    pkg_root().join("prefix")
}
fn cache_parts() -> PathBuf {
    pkg_root().join("cache").join("parts")
}
fn cache_pkg() -> PathBuf {
    pkg_root().join("cache").join("pkg")
}
fn indexes_dir() -> PathBuf {
    pkg_root().join("indexes")
}
fn state_file() -> PathBuf {
    pkg_root().join("state").join("installed.json")
}
fn mirrors_file() -> PathBuf {
    pkg_root().join("mirrors.json")
}
fn sign_priv() -> PathBuf {
    pkg_root().join("sign.priv")
}
fn sign_pub() -> PathBuf {
    pkg_root().join("sign.pub")
}

// ── Model data (matching index.json / mirrors.json / installed.json) ─────────

#[derive(Serialize, Deserialize)]
struct Index {
    format: u8,
    name: String,
    updated: String,
    packages: Vec<IndexPkg>,
}

#[derive(Serialize, Deserialize, Clone)]
struct IndexPkg {
    name: String,
    version: String,
    description: String,
    sha256: String,
    size: u64,
    parts: Vec<PartRef>,
    #[serde(default)]
    deps: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone)]
struct PartRef {
    sha256: String,
    size: u64,
}

#[derive(Serialize, Deserialize, Default)]
struct Mirrors {
    mirrors: Vec<Mirror>,
}

#[derive(Serialize, Deserialize, Clone)]
struct Mirror {
    name: String,
    url: String,
}

#[derive(Serialize, Deserialize, Default)]
struct Installed {
    pkgs: BTreeMap<String, InstalledPkg>,
}

#[derive(Serialize, Deserialize)]
struct InstalledPkg {
    version: String,
    repo: String,
    sha256: String,
    files: Vec<String>,
    parts: Vec<String>,
    installed_at: String,
}

// ── Util crypto & hash (pure Rust) ──────────────────────────────────────────

fn sha256_hex(data: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(data);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

fn sha256_file(path: &Path) -> io::Result<String> {
    let mut h = Sha256::new();
    let mut f = fs::File::open(path)?;
    io::copy(&mut f, &mut h)?;
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

fn random_seed() -> io::Result<[u8; 32]> {
    use std::io::Read;
    let mut seed = [0u8; 32];
    let mut f = fs::File::open("/dev/urandom")?;
    f.read_exact(&mut seed)?;
    Ok(seed)
}

fn hexv(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        _ => None,
    }
}

fn load_seed() -> Option<[u8; 32]> {
    let t = fs::read_to_string(sign_priv()).ok()?.trim().to_string();
    if t.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, c) in t.as_bytes().chunks(2).enumerate() {
        let hi = hexv(c[0])?;
        let lo = hexv(c[1])?;
        out[i] = (hi << 4) | lo;
    }
    Some(out)
}

fn load_pub() -> Option<[u8; 32]> {
    let t = fs::read_to_string(sign_pub()).ok()?.trim().to_string();
    if t.len() != 64 {
        return None;
    }
    let mut v = Vec::with_capacity(32);
    for c in t.as_bytes().chunks(2) {
        let hi = hexv(c[0])?;
        let lo = hexv(c[1])?;
        v.push((hi << 4) | lo);
    }
    <[u8; 32]>::try_from(v.as_slice()).ok()
}

fn save_priv(seed: &[u8; 32]) -> io::Result<PathBuf> {
    fs::create_dir_all(pkg_root())?;
    fs::write(
        sign_priv(),
        seed.iter().map(|b| format!("{b:02x}")).collect::<String>(),
    )?;
    Ok(sign_priv())
}

fn pub_of_priv(seed: &[u8; 32]) -> [u8; 32] {
    SigningKey::from_bytes(seed).verifying_key().to_bytes()
}

fn save_pub(pubkey: &[u8; 32]) -> io::Result<()> {
    fs::write(
        sign_pub(),
        pubkey
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>(),
    )
}

fn sign_bytes(data: &[u8]) -> io::Result<Vec<u8>> {
    let seed = load_seed()
        .ok_or_else(|| io::Error::other("kunci privat belum ada — jalankan `mterm pkg keygen`"))?;
    let key = SigningKey::from_bytes(&seed);
    let sig: Signature = key.sign(data);
    Ok(sig.to_bytes().to_vec())
}

/// Verifikasi Ed25519. `None` untuk pubkey = tanpa kunci → hasil `Err` harus
/// di-handle oleh pemanggil (index dianggap "tanpa tandatangan").
fn verify_bytes(pubkey: [u8; 32], data: &[u8], sig: &[u8]) -> bool {
    let Ok(pk) = VerifyingKey::from_bytes(&pubkey) else {
        return false;
    };
    let Ok(sb) = <[u8; 64]>::try_from(sig) else {
        return false;
    };
    pk.verify(data, &Signature::from_bytes(&sb)).is_ok()
}

// ── Fetch (file:// lokal + https via curl) ───────────────────────────────────

/// `file:///abs/path` dibaca langsung dari disk; `https://` via curl.
fn fetch_bytes(url: &str) -> io::Result<Vec<u8>> {
    if let Some(p) = url.strip_prefix("file://") {
        return fs::read(p).map_err(|e| io::Error::other(format!("baca {p}: {e}")));
    }
    let out = Command::new("sh")
        .arg("-c")
        .arg(format!("curl -fsSL --max-time 120 '{url}' 2>/dev/null"))
        .output()?;
    if !out.status.success() {
        return Err(io::Error::other(format!("unduh gagal: {url}")));
    }
    Ok(out.stdout)
}

fn fetch_to(url: &str, dest: &Path) -> io::Result<()> {
    if let Some(p) = url.strip_prefix("file://") {
        fs::copy(p, dest).map(|_| ())?;
        return Ok(());
    }
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    let status = Command::new("sh")
        .arg("-c")
        .arg(format!(
            "curl -fsSL --max-time 180 -o '{}' '{}' 2>/dev/null",
            dest.display(),
            url
        ))
        .status()?;
    if !status.success() {
        return Err(io::Error::other(format!("unduh gagal: {url}")));
    }
    Ok(())
}

fn base_url(mirror: &Mirror) -> String {
    mirror.url.trim_end_matches('/').to_string()
}

// ── Repo host: bangun index + parts + tanda tangan ───────────────────────────

/// Bangun index untuk repo di `<dir>`:
/// - pindai `<dir>/pkgs/*.tar.zst`,
/// - potong tiap arsip jadi part 1 MiB & tulis ke `<dir>/parts/<sha>`,
/// - tulis `<dir>/index.json` (+ `index.json.sig` bila kunci ada).
pub fn repo_index(repo_dir: &Path) -> io::Result<()> {
    let pkgs = repo_dir.join("pkgs");
    if !pkgs.is_dir() {
        return Err(io::Error::other(format!(
            "{} tidak ada — letakkan *.tar.zst di <repo>/pkgs/",
            pkgs.display()
        )));
    }
    let parts = repo_dir.join("parts");
    fs::create_dir_all(&parts)?;

    let mut entries: Vec<_> = fs::read_dir(&pkgs)?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().ends_with(".tar.zst"))
        .collect();
    entries.sort_by_key(|e| e.file_name());

    let mut packages = Vec::new();
    for e in entries {
        let path = e.path();
        let data = fs::read(&path)?;
        let stem = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("paket")
            .trim_end_matches(".tar.zst")
            .to_string();
        // Konvensi nama berkas: `<nama>-<versi>.tar.zst` (versi = bagian sesudah dash pertama).
        let (name, version) = match stem.split_once('-') {
            Some((n, v)) if !n.is_empty() && !v.is_empty() => (n.to_string(), v.to_string()),
            _ => (stem.clone(), "1.0.0".to_string()),
        };
        // Split → part content-addressed.
        let mut part_refs = Vec::new();
        let mut off = 0usize;
        while off < data.len() {
            let end = (off + PART_SIZE as usize).min(data.len());
            let chunk = &data[off..end];
            let sha = sha256_hex(chunk);
            fs::write(parts.join(&sha), chunk)?;
            part_refs.push(PartRef {
                sha256: sha,
                size: chunk.len() as u64,
            });
            off = end;
        }
        packages.push(IndexPkg {
            name: name.clone(),
            version: version.clone(),
            description: format!("paket {name}"),
            sha256: sha256_hex(&data),
            size: data.len() as u64,
            parts: part_refs,
            deps: Vec::new(),
        });
    }

    let index = Index {
        format: FORMAT,
        name: repo_dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "repo".into()),
        updated: chrono_now(),
        packages,
    };
    let json = serde_json::to_vec_pretty(&index)?;
    fs::write(repo_dir.join("index.json"), &json)?;
    println!("index.json: {} paket", index.packages.len());

    // Tanda tangan (2 jalur: kunci privat pakai alamat kunci; untuk repo host,
    // kunci publik adikita; sign_index menulis sig).
    if load_seed().is_some() {
        fs::write(repo_dir.join("index.json.sig"), sign_bytes(&json)?)?;
        println!("index.json.sig: ditandatangani (Ed25519)");
    } else {
        println!("index.json.sig: tanpa kunci — repo tidak ditandatangani");
    }
    Ok(())
}

/// Tanda tangani index di repo dir secara eksplisit (`pkg repo-sign <dir>`).
pub fn repo_sign(repo_dir: &Path) -> io::Result<()> {
    let idx = repo_dir.join("index.json");
    let json = fs::read(&idx)?;
    fs::write(repo_dir.join("index.json.sig"), sign_bytes(&json)?)?;
    println!("index.json.sig diperbarui untuk {}", idx.display());
    Ok(())
}

fn chrono_now() -> String {
    std::process::Command::new("date")
        .args(["+%Y-%m-%dT%H:%M:%SZ"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|_| "0".into())
}

// ── State mirrors / index / installed ────────────────────────────────────────

fn load_mirrors() -> io::Result<Mirrors> {
    if mirrors_file().exists() {
        Ok(serde_json::from_str(&fs::read_to_string(mirrors_file())?)?)
    } else {
        Ok(Mirrors::default())
    }
}
fn save_mirrors(m: &Mirrors) -> io::Result<()> {
    fs::create_dir_all(mirrors_file().parent().unwrap())?;
    fs::write(mirrors_file(), serde_json::to_vec_pretty(m)?)?;
    Ok(())
}

fn load_installed() -> io::Result<Installed> {
    if state_file().exists() {
        Ok(serde_json::from_str(&fs::read_to_string(state_file())?)?)
    } else {
        Ok(Installed::default())
    }
}
fn save_installed(i: &Installed) -> io::Result<()> {
    fs::create_dir_all(state_file().parent().unwrap())?;
    fs::write(state_file(), serde_json::to_vec_pretty(i)?)?;
    Ok(())
}

fn read_index(name: &str) -> io::Result<Index> {
    let p = indexes_dir().join(format!("{name}.json"));
    Ok(serde_json::from_str(&fs::read_to_string(p)?)?)
}

fn write_index(name: &str, json: &[u8]) -> io::Result<()> {
    fs::create_dir_all(indexes_dir())?;
    fs::write(indexes_dir().join(format!("{name}.json")), json)
}

/// Unduh + (bila pub key ada) verifikasi index dari mirror → simpan.
fn update_mirror(mirror: &Mirror) -> io::Result<Index> {
    let base = base_url(mirror);
    let json = fetch_bytes(&format!("{base}/index.json"))?;
    let mut verified = false;
    if let Some(pubk) = load_pub() {
        let sig = fetch_bytes(&format!("{base}/index.json.sig"))?;
        if !verify_bytes(pubk, &json, &sig) {
            return Err(io::Error::other(format!(
                "mirror {}: tanda tangan index TIDAK valid — tolak",
                mirror.name
            )));
        }
        verified = true;
    }
    write_index(&mirror.name, &json)?;
    let index: Index = serde_json::from_slice(&json)?;
    let status = if verified {
        "terverifikasi"
    } else {
        "tanpa kunci (tidak diverifikasi)"
    };
    println!(
        "mirror {} → {} paket ({status})",
        mirror.name,
        index.packages.len()
    );
    Ok(index)
}

// ── Daftar perintah ──────────────────────────────────────────────────────────

pub fn main(args: &[String]) -> io::Result<()> {
    let Some(cmd) = args.first().map(String::as_str) else {
        print_usage();
        return Ok(());
    };
    match cmd {
        "keygen" => cmd_keygen(&args[1..]),
        "repo-index" | "repo-index / make-repo" | "make-repo" | "repo-init" => {
            let dir = args.get(1).map(String::as_str).unwrap_or(".");
            repo_index(Path::new(dir))
        }
        "repo-sign" => repo_sign(Path::new(args.get(1).map(String::as_str).unwrap_or("."))),
        "update" | "up" => cmd_update(&args[1..]),
        "list" | "ls" => cmd_list(&args[1..]),
        "search" => cmd_search(&args[1..]),
        "info" => {
            let name = args
                .get(1)
                .map(String::as_str)
                .ok_or_else(|| io::Error::other("pkg info <nama>"))?;
            cmd_info(name)
        }
        "install" | "i" => {
            let name = args
                .get(1)
                .map(String::as_str)
                .ok_or_else(|| io::Error::other("pkg install <nama>"))?;
            cmd_install_spec(name, parse_flag(args, "--repo"))
        }
        "remove" | "rm" => {
            let name = args
                .get(1)
                .map(String::as_str)
                .ok_or_else(|| io::Error::other("pkg remove <nama>"))?;
            cmd_remove(name)
        }
        "verify" => cmd_verify(args.get(1).map(String::as_str)),
        "mirrors" | "mirror" => cmd_mirrors(&args[1..]),
        "help" | "--help" | "-h" => {
            print_usage();
            Ok(())
        }
        other => {
            eprintln!("mterm pkg: sub-perintah tak dikenal: {other}");
            print_usage();
            std::process::exit(2);
        }
    }
}

fn print_usage() {
    println!(
        "mterm pkg {} — pkg manager ringan (bukan apt)

sub-perintah:
  keygen                              buat kunci Ed25519 (untuk tanda tangan index)
  make-repo <dir>                     bangun index.json + parts dari <dir>/pkgs/*.tar.zst
  repo-sign <dir>                     tanda tangani index.json yang ada
  mirrors list|add <nama> <url>|remove <nama>
  update                              unduh + verifikasi index semua mirror
  list [--repo <nama>]                daftar paket dari index yang sudah diunduh
  search <kata> [--repo <nama>]       cari paket
  info <nama>                         detail paket
  install <nama> [--repo <nama>]      pasang (delta: hanya unduh part baru)
  remove <nama>                       hapus paket
  verify [nama]                       cek integritas paket terpasang

repo adalah direktori berisi index.json + parts/, URL mirror mis.
  file:///data/data/com.termux/files/usr/tmp/opencode/mrepo  (lokal)
  https://mirror.example/mrepo                                    (jarak jauh)
",
        env!("CARGO_PKG_VERSION")
    );
}

fn parse_flag<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    args.windows(2)
        .find(|w| w[0] == flag)
        .map(|w| w[1].as_str())
}

fn cmd_keygen(args: &[String]) -> io::Result<()> {
    let force = args.iter().any(|a| a == "--force" || a == "-f");
    if !force && (sign_priv().exists() || sign_pub().exists()) {
        return Err(io::Error::other(
            "kunci sudah ada — pakai --force untuk mengganti",
        ));
    }
    let seed = random_seed()?;
    let pubkey = pub_of_priv(&seed);
    save_priv(&seed)?;
    save_pub(&pubkey)?;
    println!("kunci dibuat:");
    println!("  privat : {}", sign_priv().display());
    println!("  publik : {}", sign_pub().display());
    println!(
        "  sidik  : {}…",
        pubkey
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
            .chars()
            .take(16)
            .collect::<String>()
    );
    Ok(())
}

/// Pilih mirror: array `[--repo <name>]`; bila ada arg nama langsung, dipakai.
fn select_mirrors(args: &[String]) -> io::Result<Vec<Mirror>> {
    let all = load_mirrors()?;
    if let Some(name) = parse_flag(args, "--repo") {
        let m = all
            .mirrors
            .iter()
            .find(|m| m.name == name)
            .ok_or_else(|| io::Error::other(format!("mirror tidak dikenal: {name}")))?;
        return Ok(vec![m.clone()]);
    }
    Ok(all.mirrors)
}

fn cmd_update(args: &[String]) -> io::Result<()> {
    let mirrors = load_mirrors()?;
    if mirrors.mirrors.is_empty() {
        println!("belum ada mirror. Tambah dulu: mterm pkg mirrors add <nama> <url>");
        return Ok(());
    }
    for m in select_mirrors(args)? {
        update_mirror(&m)?;
    }
    Ok(())
}

fn collect_pkgs(
    only_repo: Option<&str>,
    only_installed: bool,
) -> io::Result<Vec<(String, IndexPkg)>> {
    let indexes_dir = indexes_dir();
    let mut out: BTreeMap<String, IndexPkg> = BTreeMap::new();
    if indexes_dir.is_dir() {
        for e in fs::read_dir(&indexes_dir)? {
            let e = e?;
            let fname = e.file_name().to_string_lossy().into_owned();
            let Some(name) = fname.strip_suffix(".json") else {
                continue;
            };
            if let Some(r) = only_repo {
                if r != name {
                    continue;
                }
            }
            if let Ok(idx) = read_index(name) {
                for p in idx.packages {
                    // nama unik; versi terakhir menang
                    let old = out.get_mut(&p.name);
                    if old.map(|o| o.version != p.version).unwrap_or(true) {
                        out.insert(p.name.clone(), p);
                    } else {
                        let _ = old;
                    }
                }
            }
        }
    }
    if only_installed {
        let inst = load_installed()?;
        out.retain(|k, _| inst.pkgs.contains_key(k));
    }
    Ok(out.into_iter().collect())
}

fn cmd_list(args: &[String]) -> io::Result<()> {
    let only = parse_flag(args, "--repo");
    let pkgs = collect_pkgs(only, false)?;
    if pkgs.is_empty() {
        println!("(tidak ada paket — jalankan `mterm pkg update` dulu)");
        return Ok(());
    }
    for (name, p) in pkgs {
        println!(
            "{name} {}({}) — {}",
            p.version,
            fmt_size(p.size),
            p.description
        );
    }
    Ok(())
}

fn cmd_search(args: &[String]) -> io::Result<()> {
    let q = args
        .first()
        .map(String::as_str)
        .unwrap_or("")
        .to_lowercase();
    let pkgs = collect_pkgs(parse_flag(args, "--repo"), false)?;
    let hits: Vec<_> = pkgs
        .into_iter()
        .filter(|(n, p)| n.to_lowercase().contains(&q) || p.description.to_lowercase().contains(&q))
        .collect();
    if hits.is_empty() {
        println!("(tidak ditemukan untuk: {q})");
        return Ok(());
    }
    for (name, p) in hits {
        println!("{name} — {}", p.description);
    }
    Ok(())
}

fn find_pkg(name: &str, only_repo: Option<&str>) -> io::Result<IndexPkg> {
    collect_pkgs(only_repo, false)?
        .into_iter()
        .find(|(n, _)| n == name)
        .map(|(_, p)| p)
        .ok_or_else(|| io::Error::other(format!("paket tidak ditemukan: {name}")))
}

fn cmd_info(name: &str) -> io::Result<()> {
    let p = find_pkg(name, None)?;
    println!("nama        : {}", p.name);
    println!("versi       : {}", p.version);
    println!("deskripsi   : {}", p.description);
    println!("sha256      : {}", p.sha256);
    println!("ukuran      : {}", fmt_size(p.size));
    println!("jumlah part : {} (δ 1 MiB)", p.parts.len());
    if !p.deps.is_empty() {
        println!("dependensi  : {}", p.deps.join(", "));
    }
    let inst = load_installed()?;
    match inst.pkgs.get(&p.name) {
        Some(_) => println!("status      : terpasang"),
        None => println!("status      : belum dipasang"),
    }
    Ok(())
}

// ── Instal / hapus ───────────────────────────────────────────────────────────

fn repo_of_index_containing(name: &str) -> io::Result<Option<String>> {
    let indexes_dir = indexes_dir();
    if !indexes_dir.is_dir() {
        return Ok(None);
    }
    for e in fs::read_dir(&indexes_dir)? {
        let e = e?;
        let fname = e.file_name().to_string_lossy().into_owned();
        let Some(rname) = fname.strip_suffix(".json") else {
            continue;
        };
        let Ok(idx) = read_index(rname) else { continue };
        if idx.packages.iter().any(|p| p.name == name) {
            return Ok(Some(rname.to_string()));
        }
    }
    Ok(None)
}

fn cmd_install_spec(spec: &str, only_repo: Option<&str>) -> io::Result<()> {
    // spec bisa "nama" atau "nama@versi" atau "file:///path/to/x.tar.zst"
    let (name, version) = match spec.split_once('@') {
        Some((n, v)) if !n.is_empty() => (n.to_string(), Some(v.to_string())),
        _ => (spec.to_string(), None),
    };

    // Instal dari tarball lokal langsung (tanpa mirror).
    if let Some(p) = spec.strip_prefix("file://") {
        let path = Path::new(p);
        if !path.exists() {
            return Err(io::Error::other(format!("file tidak ada: {p}")));
        }
        let fname = path
            .file_name()
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_else(|| "paket".into());
        install_local_tarball(path, &fname, version)?;
        return Ok(());
    }

    let pkg = find_pkg(&name, only_repo)?;
    let version = version.as_deref().unwrap_or(&pkg.version);
    if version != pkg.version.as_str() {
        return Err(io::Error::other(format!(
            "versi {version} tidak tersedia (index punya {})",
            pkg.version
        )));
    }
    let repo = repo_of_index_containing(&name)?
        .ok_or_else(|| io::Error::other(format!("paket tidak di index mana pun: {name}")))?;
    install_from_index(&name, &pkg, &repo)?;
    Ok(())
}

/// Pasang paket dari index yang sudah diunduh. Delta: reuse part by sha.
fn install_from_index(name: &str, pkg: &IndexPkg, repo: &str) -> io::Result<()> {
    let real_base = load_mirrors()?
        .mirrors
        .iter()
        .find(|m| m.name == repo)
        .map(base_url)
        .ok_or_else(|| io::Error::other(format!("mirror {repo} tidak terdaftar")))?;

    let _ = fs::create_dir_all(cache_parts());
    let _ = fs::create_dir_all(cache_pkg());

    // 1) Pastikan semua part ada di cache (delta: hanya unduh yang belum ada).
    let mut need = Vec::new();
    for pr in &pkg.parts {
        let c = cache_parts().join(&pr.sha256);
        if !c.exists() {
            need.push((pr, c));
        }
    }
    let skip_sz = pkg.parts.len().saturating_sub(need.len());
    println!(
        "instal {} {}: {} part, {} dari cache (delta)",
        pkg.name,
        pkg.version,
        pkg.parts.len(),
        skip_sz
    );
    for (pr, c) in &need {
        print!("  part {} …", short(&pr.sha256));
        std::io::stdout().flush()?;
        let url = format!("{}/parts/{}", real_base, pr.sha256);
        fetch_to(&url, c).map_err(|_| io::Error::other(format!("gagal part: {}", pr.sha256)))?;
        if sha256_file(c)? != pr.sha256 {
            let _ = fs::remove_file(c);
            return Err(io::Error::other("part rusak (sha tidak cocok)"));
        }
        println!(" ok");
    }

    // 2) Rangkap part → tarball, verifikasi sha penuh.
    let tar_path = cache_pkg().join(format!("{}.tar.zst", pkg.sha256));
    let mut w = fs::File::create(&tar_path)?;
    for pr in &pkg.parts {
        let c = cache_parts().join(&pr.sha256);
        io::copy(&mut fs::File::open(&c)?, &mut w)?;
    }
    w.flush()?;
    drop(w);
    let actual = sha256_file(&tar_path)?;
    if actual != pkg.sha256 {
        let _ = fs::remove_file(&tar_path);
        return Err(io::Error::other(format!(
            "arSip rusak: sha {actual} ≠ {} (diharapkan)",
            pkg.sha256
        )));
    }

    // 2b) Hapus dulu entry lama (sebelum ekstrak, biar tidak timpa-menimpa).
    let mut inst = load_installed()?;
    if let Some(old) = inst.pkgs.remove(name) {
        cleanup_prefix_files(&old.files);
    }

    // 3) Ekstrak tar.zst ke prefix.
    let _ = fs::create_dir_all(prefix_dir());
    let status = Command::new("sh")
        .arg("-c")
        .arg(format!(
            "zstd -d -c '{}' 2>/dev/null | tar -x -C '{}' 2>/dev/null",
            tar_path.display(),
            prefix_dir().display()
        ))
        .status()?;
    if !status.success() {
        let _ = fs::remove_file(&tar_path);
        return Err(io::Error::other("ekstraksi tar.zst gagal (zstd/tar?)"));
    }

    // 4) Catat file yang terpasang + part cache (untuk integritas & delta).
    let files = walk_rel(&prefix_dir())?;
    inst.pkgs.insert(
        name.to_string(),
        InstalledPkg {
            version: pkg.version.clone(),
            repo: repo.to_string(),
            sha256: pkg.sha256.clone(),
            files: files.clone(),
            parts: pkg.parts.iter().map(|p| p.sha256.clone()).collect(),
            installed_at: chrono_now(),
        },
    );
    save_installed(&inst)?;
    println!("terpasang: {name} {} — {} file", pkg.version, files.len());
    Ok(())
}

fn install_local_tarball(path: &Path, fname: &str, version: Option<String>) -> io::Result<()> {
    let _ = fs::create_dir_all(prefix_dir());
    let sha = sha256_file(path)?;
    let tar_path = cache_pkg().join(format!("{fname}.{sha}.tar.zst"));
    fs::copy(path, &tar_path)?;
    let status = Command::new("sh")
        .arg("-c")
        .arg(format!(
            "zstd -d -c '{}' 2>/dev/null | tar -x -C '{}' 2>/dev/null",
            tar_path.display(),
            prefix_dir().display()
        ))
        .status()?;
    if !status.success() {
        let _ = fs::remove_file(&tar_path);
        return Err(io::Error::other("ekstraksi tar.zst gagal (zstd/tar?)"));
    }
    let name = fname.trim_end_matches(".tar.zst").to_string();
    let files = walk_rel(&prefix_dir())?;
    let mut inst = load_installed()?;
    if let Some(old) = inst.pkgs.remove(&name) {
        cleanup_prefix_files(&old.files);
    }
    inst.pkgs.insert(
        name.clone(),
        InstalledPkg {
            version: version.unwrap_or_else(|| "1.0.0".into()),
            repo: "local".into(),
            sha256: sha,
            files,
            parts: Vec::new(),
            installed_at: chrono_now(),
        },
    );
    save_installed(&inst)?;
    println!("terpasang (lokal): {name}");
    Ok(())
}

/// Daftar path relatif semua file di bawah `root` (bersih dari dir kosong utk daftar file).
fn walk_rel(root: &Path) -> io::Result<Vec<String>> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for e in fs::read_dir(&dir)? {
            let e = e?;
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                let rel = p
                    .strip_prefix(root)
                    .unwrap_or(&p)
                    .to_string_lossy()
                    .into_owned();
                out.push(rel);
            }
        }
    }
    out.sort();
    Ok(out)
}

fn cleanup_prefix_files(files: &[String]) {
    let mut dirs: Vec<PathBuf> = Vec::new();
    for f in files {
        let p = prefix_dir().join(f);
        let _ = fs::remove_file(&p);
        if let Some(d) = p.parent() {
            dirs.push(d.to_path_buf());
        }
    }
    // Bersihkan dir kosong (naik sampai prefix root).
    dirs.sort_by_key(|d| std::cmp::Reverse(d.components().count()));
    let root = prefix_dir();
    for d in dirs {
        if d == root || !d.starts_with(&root) {
            continue;
        }
        let _ = fs::remove_dir(&d);
    }
}

fn cmd_remove(name: &str) -> io::Result<()> {
    let mut inst = load_installed()?;
    let Some(info) = inst.pkgs.remove(name) else {
        return Err(io::Error::other(format!("{name} tidak terpasang")));
    };
    cleanup_prefix_files(&info.files);
    save_installed(&inst)?;
    println!(
        "dihapus: {name} {} ({} file)",
        info.version,
        info.files.len()
    );
    Ok(())
}

fn cmd_verify(name: Option<&str>) -> io::Result<()> {
    let inst = load_installed()?;
    let names: Vec<String> = match name {
        Some(n) => {
            if !inst.pkgs.contains_key(n) {
                return Err(io::Error::other(format!("{n} tidak terpasang")));
            }
            vec![n.to_string()]
        }
        None => inst.pkgs.keys().cloned().collect(),
    };
    let names = {
        let mut v = names;
        v.sort();
        v
    };
    let mut ok = true;
    for n in &names {
        let info = &inst.pkgs[n];
        // 1) Semua file ada?
        let missing: Vec<_> = info
            .files
            .iter()
            .filter(|f| !prefix_dir().join(f).exists())
            .cloned()
            .collect();
        if !missing.is_empty() {
            println!(
                "{n}: KORUP — {} file hilang: {}",
                missing.len(),
                missing.first().unwrap()
            );
            ok = false;
            continue;
        }
        // 2) Kalau paket dari index (punya part+sha) → rekomposisi + cek sha penuh.
        let mut garbage = false;
        if !info.parts.is_empty() {
            let mut hasher = Sha256::new();
            for sha in &info.parts {
                let c = cache_parts().join(sha);
                if !c.exists() {
                    println!("{n}: part {} hilang dari cache", short(sha));
                    garbage = true;
                    break;
                }
                let mut f = fs::File::open(&c)?;
                io::copy(&mut f, &mut hasher)?;
            }
            if !garbage {
                let d = hasher.finalize();
                let digest = d.iter().map(|b| format!("{b:02x}")).collect::<String>();
                if digest != info.sha256 {
                    println!("{n}: KORUP — sha penuh tidak cocok");
                    ok = false;
                    continue;
                }
            }
        }
        if missing.is_empty() && !garbage {
            println!("{n} {}: OK ({} file)", info.version, info.files.len());
        } else {
            ok = false;
        }
    }
    if !ok {
        return Err(io::Error::other("verify: ada paket yang korup/hilang"));
    }
    Ok(())
}

fn cmd_mirrors(args: &[String]) -> io::Result<()> {
    let mut m = load_mirrors()?;
    let Some(action) = args.first().map(String::as_str) else {
        for r in &m.mirrors {
            println!("{}  {}", r.name, r.url);
        }
        return Ok(());
    };
    match action {
        "list" | "ls" => {
            for r in &m.mirrors {
                println!("{}  {}", r.name, r.url);
            }
            Ok(())
        }
        "add" => {
            let name = args
                .get(1)
                .map(String::as_str)
                .ok_or_else(|| io::Error::other("mirrors add <nama> <url>"))?;
            let url = args
                .get(2)
                .map(String::as_str)
                .ok_or_else(|| io::Error::other("mirrors add <nama> <url>"))?;
            if m.mirrors.iter().any(|r| r.name == name) {
                m.mirrors.retain(|r| r.name != name);
                println!("mirror {name}: diganti");
            }
            m.mirrors.push(Mirror {
                name: name.to_string(),
                url: url.to_string(),
            });
            save_mirrors(&m)?;
            println!("mirror {name} → {url}");
            Ok(())
        }
        "remove" | "rm" => {
            let name = args
                .get(1)
                .map(String::as_str)
                .ok_or_else(|| io::Error::other("mirrors remove <nama>"))?;
            let before = m.mirrors.len();
            m.mirrors.retain(|r| r.name != name);
            save_mirrors(&m)?;
            if m.mirrors.len() == before {
                println!("mirror tidak ada: {name}");
            } else {
                println!("mirror {name}: dihapus");
            }
            Ok(())
        }
        other => Err(io::Error::other(format!(
            "perintah mirrors tak dikenal: {other}"
        ))),
    }
}

// ── Util kecil ───────────────────────────────────────────────────────────────

fn short(s: &str) -> String {
    s.chars().take(12).collect()
}

fn fmt_size(n: u64) -> String {
    if n >= 1024 * 1024 {
        format!("{:.1} MiB", n as f64 / (1024.0 * 1024.0))
    } else if n >= 1024 {
        format!("{:.1} KiB", n as f64 / 1024.0)
    } else {
        format!("{n} B")
    }
}

// ── Unit test (offline: file:// mirror + zstd/tar) ───────────────────────────

#[cfg(test)]
fn have_zstd_tar() -> bool {
    Command::new("sh")
        .arg("-c")
        .arg("command -v zstd >/dev/null && command -v tar >/dev/null")
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Bangun tarball `name-version.tar.zst` berisi beberapa file di `srcdir`.
#[cfg(test)]
fn build_tarball(srcdir: &Path, dst: &Path) -> bool {
    if !have_zstd_tar() {
        eprintln!("  (skip: zstd/tar tak ada)");
        return false;
    }
    let tmp = srcdir.with_file_name("x.tar");
    let st = Command::new("sh")
        .arg("-c")
        .arg(format!(
            "rm -f '{}'; tar -cf '{}' -C '{}' . && zstd -q -f '{}' -o '{}' && rm -f '{}'",
            tmp.display(),
            tmp.display(),
            srcdir.display(),
            tmp.display(),
            dst.display(),
            tmp.display()
        ))
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    st && dst.exists()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Test saling mengubah env `HOME` global → jalankan serial.
    fn lock() -> std::sync::MutexGuard<'static, ()> {
        static G: std::sync::Mutex<()> = std::sync::Mutex::new(());
        G.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    fn tmpdir() -> PathBuf {
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let d = std::env::temp_dir().join(format!("mterm-pkg-test-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    // sandbox HOME supaya tidak menyentuh `~/.mterm` sungguhan
    fn sandbox() -> PathBuf {
        let home = tmpdir().join("home");
        fs::create_dir_all(&home).unwrap();
        unsafe { std::env::set_var("HOME", &home) };
        home
    }

    fn make_pkg_src(dir: &Path, extra: &str, text: &str) {
        fs::create_dir_all(dir).unwrap();
        fs::write(dir.join("bin"), "shell script\n").unwrap();
        fs::write(dir.join("data.txt"), text).unwrap();
        if !extra.is_empty() {
            fs::write(dir.join("extra.txt"), extra).unwrap();
        }
    }

    fn mk_repo(home: &Path, repo_dir: &Path, with_key: bool) {
        let _ = fs::create_dir_all(repo_dir.join("pkgs"));
        make_pkg_src(&home.join("src1"), "", "halo");
        build_tarball(
            &home.join("src1"),
            &repo_dir.join("pkgs").join("demo-1.0.0.tar.zst"),
        );
        if with_key {
            let seed = random_seed().unwrap();
            save_priv(&seed).unwrap();
            save_pub(&pub_of_priv(&seed)).unwrap();
        }
        repo_index(repo_dir).unwrap();
    }

    #[test]
    fn keygen_membuat_kunci_dan_bisa_dibaca() {
        let _g = lock();
        let home = sandbox();
        cmd_keygen(&[]).unwrap();
        assert!(sign_priv().exists());
        assert!(sign_pub().exists());
        assert!(load_seed().is_some());
        assert_eq!(load_pub().unwrap().len(), 32);
        let _ = &home;
    }

    #[test]
    fn repo_index_menghasilkan_index_dan_parts() {
        let _g = lock();
        let home = sandbox();
        let repo = tmpdir().join("repo");
        let _ = fs::create_dir_all(repo.join("pkgs"));
        make_pkg_src(&home.join("src1"), "", "halo");
        build_tarball(
            &home.join("src1"),
            &repo.join("pkgs").join("demo-1.0.0.tar.zst"),
        );
        repo_index(&repo).unwrap();
        assert!(repo.join("index.json").exists());
        assert!(!repo.join("index.json.sig").exists());
        let idx: Index =
            serde_json::from_str(&fs::read_to_string(repo.join("index.json")).unwrap()).unwrap();
        assert_eq!(idx.packages.len(), 1);
        assert_eq!(idx.packages[0].name, "demo");
        assert_eq!(idx.packages[0].version, "1.0.0");
        assert!(!idx.packages[0].parts.is_empty());
        // bagian tercatat sesuai sha
        assert!(repo
            .join("parts")
            .join(&idx.packages[0].parts[0].sha256)
            .exists());
    }

    #[test]
    fn tanda_tangan_lengkap_update_tolak_index_tampered() {
        let _g = lock();
        let home = sandbox();
        let repo = tmpdir().join("repo");
        mk_repo(&home, &repo, true);
        // mirror lokal
        cmd_mirrors(&[
            "add".to_string(),
            "local".into(),
            format!("file://{}", repo.display()),
        ])
        .unwrap();
        cmd_update(&[]).unwrap();
        let st = load_installed().unwrap(); // sanity: indeks ter-simpan
        let idx = read_index("local").unwrap();
        assert_eq!(idx.packages.len(), 1);
        let _ = &st;
        // tamper index
        let data = fs::read(repo.join("index.json")).unwrap();
        fs::write(repo.join("index.json"), [&data[..5], b"TAMPER"].concat()).unwrap();
        let r = cmd_update(&[]);
        assert!(r.is_err(), "update harus menolak index yang di-tamper");
    }

    #[test]
    fn tanda_tangan_tanpa_kunci_allow_tetapi_tidak_diverifikasi() {
        let _g = lock();
        let home = sandbox();
        let repo = tmpdir().join("repo");
        mk_repo(&home, &repo, false);
        cmd_mirrors(&[
            "add".to_string(),
            "local2".into(),
            format!("file://{}", repo.display()),
        ])
        .unwrap();
        cmd_update(&[]).unwrap();
        assert!(read_index("local2").is_ok());
    }

    #[test]
    fn instal_remove_dan_delta_reuse_part() {
        let _g = lock();
        let home = sandbox();
        let repo = tmpdir().join("repo");
        let _ = fs::create_dir_all(repo.join("pkgs"));
        make_pkg_src(&home.join("src1"), "", "halo");
        build_tarball(
            &home.join("src1"),
            &repo.join("pkgs").join("demo-1.0.0.tar.zst"),
        );
        repo_index(&repo).unwrap();
        cmd_mirrors(&[
            "add".to_string(),
            "local3".into(),
            format!("file://{}", repo.display()),
        ])
        .unwrap();
        cmd_update(&[]).unwrap();
        cmd_install_spec("demo", None).unwrap();
        assert!(
            prefix_dir().join("data.txt").exists(),
            "file dari tarball terpasang"
        );
        let inst = load_installed().unwrap();
        assert!(inst.pkgs.contains_key("demo"));
        // 2nd install: reuse — jumlah part cache tidak bertambah
        let n1 = fs::read_dir(cache_parts()).unwrap().count();
        cmd_install_spec("demo", None).unwrap();
        let n2 = fs::read_dir(cache_parts()).unwrap().count();
        assert_eq!(n1, n2, "delta: tidak unduh part baru");
        // ganti versi (misal tambah file besar) → perlu part baru? pakai versi-copy
        cmd_remove("demo").unwrap();
        assert!(
            !prefix_dir().join("data.txt").exists(),
            "file ikut terhapus"
        );
        assert!(load_installed().unwrap().pkgs.is_empty());
    }

    #[test]
    fn delta_versi_baru_hanya_unduh_part_berubah() {
        let _g = lock();
        let home = sandbox();
        let repo = tmpdir().join("repo");
        let _ = fs::create_dir_all(repo.join("pkgs"));
        cmd_mirrors(&[
            "add".to_string(),
            "d".into(),
            format!("file://{}", repo.display()),
        ])
        .unwrap();
        // PRNG pseudo-random supaya zstd tidak bisa mengkompres → tarball > 1 MiB
        let mut x: u64 = 0x9e3779b97f4a7c15;
        let mut filler = Vec::with_capacity(3 * 1_100_000);
        while filler.len() < 3 * 1_100_000 {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            filler.push((x ^ (x >> 32)) as u8);
        }

        // Fase 1: versi 1 terpasang (file besar + data.txt "a").
        make_pkg_src(&home.join("v1"), "", "a");
        fs::write(home.join("v1").join("big.bin"), &filler).unwrap();
        build_tarball(
            &home.join("v1"),
            &repo.join("pkgs").join("app-1.0.0.tar.zst"),
        );
        repo_index(&repo).unwrap();
        cmd_update(&[]).unwrap();
        cmd_install_spec("app", None).unwrap();
        let v1_parts = load_installed().unwrap().pkgs["app"].parts.clone();
        let cache_after_v1 = fs::read_dir(cache_parts()).unwrap().count();

        // Fase 2: repo punya app-2.0.0 — big.bin IDENTIK, cuma data.txt beda → 1 part baru.
        let _ = fs::remove_file(repo.join("pkgs").join("app-1.0.0.tar.zst"));
        make_pkg_src(&home.join("v2"), "", "b");
        fs::write(home.join("v2").join("big.bin"), &filler).unwrap();
        build_tarball(
            &home.join("v2"),
            &repo.join("pkgs").join("app-2.0.0.tar.zst"),
        );
        repo_index(&repo).unwrap();
        cmd_update(&[]).unwrap();
        cmd_install_spec("app", None).unwrap();
        assert!(
            prefix_dir().join("data.txt").exists(),
            "file setelah upgrade ada"
        );
        let inst = load_installed().unwrap();
        assert_eq!(inst.pkgs["app"].version, "2.0.0");
        // delta nyata: ada part yang dipakai ulang antar versi
        let v2_parts = inst.pkgs["app"].parts.clone();
        assert!(
            v2_parts.iter().any(|s| v1_parts.contains(s)),
            "ada part yang dipakai ulang (delta)"
        );
        let cache_after_v2 = fs::read_dir(cache_parts()).unwrap().count();
        assert!(
            cache_after_v2 > cache_after_v1 && cache_after_v2 - cache_after_v1 < v2_parts.len(),
            "hanya sebagian kecil part baru yang diunduh"
        );
    }

    #[test]
    fn verify_mendeteksi_file_hilang() {
        let _g = lock();
        let home = sandbox();
        let repo = tmpdir().join("repo");
        let _ = fs::create_dir_all(repo.join("pkgs"));
        make_pkg_src(&home.join("s"), "", "x");
        build_tarball(&home.join("s"), &repo.join("pkgs").join("t-1.0.0.tar.zst"));
        repo_index(&repo).unwrap();
        cmd_mirrors(&[
            "add".to_string(),
            "v".into(),
            format!("file://{}", repo.display()),
        ])
        .unwrap();
        cmd_update(&[]).unwrap();
        cmd_install_spec("t", None).unwrap();
        // hapus satu file
        fs::remove_file(prefix_dir().join("data.txt")).unwrap();
        // verify harus error
        let r = cmd_verify(None);
        assert!(r.is_err());
    }
}
