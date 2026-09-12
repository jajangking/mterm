//! `mterm doctor`: cek tool penting untuk agent/runtime.

use std::io::{self, Write};
use std::process::Command;

struct Check {
    name: &'static str,
    bin: &'static str,
    version_flag: &'static str,
}

const CHECKS: &[Check] = &[
    Check {
        name: "git",
        bin: "git",
        version_flag: "--version",
    },
    Check {
        name: "node",
        bin: "node",
        version_flag: "--version",
    },
    Check {
        name: "bun (opsional)",
        bin: "bun",
        version_flag: "--version",
    },
    Check {
        name: "ripgrep",
        bin: "rg",
        version_flag: "--version",
    },
    Check {
        name: "fzf",
        bin: "fzf",
        version_flag: "--version",
    },
];

pub fn doctor() -> io::Result<()> {
    println!("mterm doctor");
    println!("{}", "-".repeat(40));

    let mut ok = 0;
    let mut missing = 0;
    for c in CHECKS {
        match version_of(c.bin, c.version_flag) {
            Some(v) => {
                println!("  ✓ {:<20} {v}", c.name);
                ok += 1;
            }
            None => {
                println!("  ✗ {:<20} tidak ditemukan", c.name);
                missing += 1;
            }
        }
    }

    println!("{}", "-".repeat(40));
    println!("{ok} tersedia, {missing} belum ter-install");
    if missing > 0 {
        println!("Hint: pkg install git nodejs ripgrep fzf (Termux)");
    }

    std::io::stdout().flush()?;
    Ok(())
}

/// Cek path binary di $PATH pakai `command -v` (coreutils/termux shell).
fn version_of(bin: &str, flag: &str) -> Option<String> {
    let out = Command::new("sh")
        .arg("-c")
        .arg(format!(
            "command -v {bin} 2>/dev/null && {bin} {flag} 2>/dev/null"
        ))
        .output()
        .ok()?;
    if !out.status.success() || out.stdout.is_empty() {
        return None;
    }
    String::from_utf8(out.stdout)
        .ok()
        .map(|s| s.trim().to_string())
}
