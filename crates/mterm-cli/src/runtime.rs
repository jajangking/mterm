//! Deteksi runtime versi mterm: tabel tool + resolusi versi via `command -v`
//! (Termux-safe: lewat `sh`, bukan env lookup yang bisa kena symlink timeout).

use std::process::Command;

pub struct Runtime {
    pub name: &'static str,
    /// Nama binary yang dicoba berurutan (fallback).
    pub(crate) bins: &'static [&'static str],
    pub version_flag: &'static str,
    /// True = tidak wajib ada (hanya informasi).
    pub optional: bool,
}

const fn rt(name: &'static str, bins: &'static [&'static str], flag: &'static str, optional: bool) -> Runtime {
    Runtime {
        name,
        bins,
        version_flag: flag,
        optional,
    }
}

/// Daftar runtime yang dikenal. Urutan = urutan tampil doctor.
pub const RUNTIMES: &[Runtime] = &[
    rt("git", &["git"], "--version", false),
    rt("node", &["node"], "--version", false),
    rt("npm", &["npm"], "--version", false),
    rt("ripgrep", &["rg"], "--version", false),
    rt("python", &["python3", "python"], "--version", false),
    rt("go", &["go"], "version", false),
    rt("bun", &["bun"], "--version", true),
    rt("fzf", &["fzf"], "--version", true),
];

pub fn runtime_for(name: &str) -> Option<&'static Runtime> {
    RUNTIMES.iter().find(|r| r.name == name)
}

/// Cari binary pertama yang ada di $PATH dan minta versinya.
/// Return `(binary_yang_dipakai, output_version)`.
pub fn version_of(bins: &[&str], flag: &str) -> Option<(String, String)> {
    for bin in bins {
        let out = Command::new("sh")
            .arg("-c")
            .arg(format!(
                "command -v {bin} 2>/dev/null || true"
            ))
            .output()
            .ok()?;
        if !out.status.success() || out.stdout.is_empty() {
            continue;
        }
        let version = Command::new("sh")
            .arg("-c")
            .arg(format!("{bin} {flag} 2>/dev/null"))
            .output()
            .ok()?;
        if !version.status.success() || version.stdout.is_empty() {
            continue;
        }
        return Some((
            bin.to_string(),
            String::from_utf8_lossy(&version.stdout).trim().to_string(),
        ));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_tabel_lengkap_dan_unik() {
        // Nama unik & mencakup runtime profile: git/node/npm/rg/python/go.
        let mut names: Vec<&str> = RUNTIMES.iter().map(|r| r.name).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), RUNTIMES.len(), "nama runtime unik");
        for need in ["git", "node", "npm", "ripgrep", "python", "go"] {
            assert!(runtime_for(need).is_some(), "runtime {need} terdaftar");
        }
    }

    #[test]
    fn python_fallback_bin_ada_di_tabel() {
        let py = runtime_for("python").unwrap();
        assert!(py.bins.contains(&"python"), "python dulu, python3 fallback");
    }
}