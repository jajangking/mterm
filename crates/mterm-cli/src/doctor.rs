//! `mterm doctor`: cek sistem (distro/libc/pakman) + tool penting untuk agent.

use std::io::{self, Write};

use crate::runtime::{version_of, RUNTIMES};
use crate::tool;

pub fn doctor() -> io::Result<()> {
    println!("mterm doctor");
    println!("{}", "-".repeat(40));

    println!("  os: {}", tool::distro_info());
    println!(
        "  pakman: {}",
        tool::detect_pkg_manager().unwrap_or("tidak dikenal (paket manual)")
    );
    println!("{}", "-".repeat(40));

    let mut ok = 0;
    let mut missing = 0;
    for r in RUNTIMES {
        match version_of(r.bins, r.version_flag) {
            Some((_, v)) => {
                let name = if r.optional {
                    format!("{} (opsional)", r.name)
                } else {
                    r.name.to_string()
                };
                println!("  ✓ {:<20} {v}", name);
                ok += 1;
            }
            None => {
                println!("  ✗ {:<20} tidak ditemukan", r.name);
                missing += 1;
            }
        }
    }

    println!("{}", "-".repeat(40));
    println!("{ok} tersedia, {missing} belum ter-install");
    if missing > 0 {
        println!("Hint: mterm tool install git node python go ripgrep fzf bun");
    }

    std::io::stdout().flush()?;
    Ok(())
}
