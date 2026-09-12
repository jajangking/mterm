//! `mterm profile`: profile runtime per-workspace.
//!
//! Manifest disimpan di `~/.mterm/profiles.json` (default `dev`, `web`).
//! Pemilihan profile ditulis ke `.mterm.profile` di cwd (per-workspace).

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct Profiles {
    current: Option<String>,
    available: BTreeMap<String, Profile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Profile {
    runtimes: Vec<String>,
    theme: Option<String>,
}

fn manifest_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".mterm")
}

fn manifest_path() -> PathBuf {
    manifest_dir().join("profiles.json")
}

fn load() -> Profiles {
    let p = manifest_path();
    match fs::read_to_string(&p) {
        Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
        Err(_) => {
            let mut def = Profiles::default();
            def.available.insert(
                "dev".into(),
                Profile {
                    runtimes: vec!["git".into(), "node".into(), "ripgrep".into()],
                    theme: Some("dark".into()),
                },
            );
            def.available.insert(
                "web".into(),
                Profile {
                    runtimes: vec!["node".into(), "npm".into()],
                    theme: Some("dark".into()),
                },
            );
            def
        }
    }
}

fn save(p: &Profiles) -> io::Result<()> {
    let dir = manifest_dir();
    fs::create_dir_all(&dir)?;
    fs::write(manifest_path(), serde_json::to_string_pretty(p).unwrap())
}

fn write_marker(profile: &str) -> io::Result<()> {
    let cwd = std::env::current_dir()?;
    fs::write(cwd.join(".mterm.profile"), profile)
}

pub fn main(args: &[String]) -> io::Result<()> {
    let sub = args.first().map(String::as_str).unwrap_or("list");
    let mut profiles = load();

    match sub {
        "list" => {
            let cur = profiles.current.as_deref().unwrap_or("-");
            println!("profile aktif: {cur}");
            for (name, p) in &profiles.available {
                println!("  {name}: {}", p.runtimes.join(", "));
            }
        }
        "use" => {
            let name = args.get(1).ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "usage: mterm profile use <name>",
                )
            })?;
            if !profiles.available.contains_key(name) {
                eprintln!("profile {name} tidak ada. Lihat `mterm profile list`.");
                std::process::exit(2);
            }
            profiles.current = Some(name.clone());
            save(&profiles)?;
            write_marker(name)?;
            println!("profile {name} aktif untuk workspace ini → .mterm.profile");
        }
        "show" => {
            let marker = std::env::current_dir()?.join(".mterm.profile");
            match fs::read_to_string(&marker) {
                Ok(s) => println!("workspace profile: {}", s.trim()),
                Err(_) => println!("tidak ada .mterm.profile di workspace ini"),
            }
        }
        "add" => {
            let name = args.get(1).ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "usage: mterm profile add <name> [runtime...]",
                )
            })?;
            let runtimes = args[2..].to_vec();
            profiles.available.insert(
                name.clone(),
                Profile {
                    runtimes,
                    theme: Some("dark".into()),
                },
            );
            save(&profiles)?;
            println!("profile {name} ditambahkan");
        }
        other => {
            eprintln!("profile: perintah tidak dikenal: {other}");
            std::process::exit(2);
        }
    }

    std::io::stdout().flush()?;
    Ok(())
}
