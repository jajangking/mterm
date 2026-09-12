//! mterm CLI: `mterm run` (PTY↔engine), `mterm doctor`, `mterm profile`, `mterm agent`.

mod agent;
mod doctor;
mod profile;
mod render;
mod run;

use std::io;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let cmd = args.get(1).map(String::as_str).unwrap_or("run");

    let result = match cmd {
        "run" => run::run(&args[2..]),
        "doctor" => doctor::doctor(),
        "profile" => profile::main(&args[2..]),
        "agent" => agent::main(&args[2..]),
        "help" | "--help" | "-h" => {
            print_usage();
            Ok(())
        }
        other => {
            eprintln!("mterm: perintah tidak dikenal: {other}");
            print_usage();
            std::process::exit(2);
        }
    };

    if let Err(e) = result {
        eprintln!("mterm: {e}");
        std::process::exit(1);
    }
    let _ = io::Write::flush(&mut io::stdout());
}

fn print_usage() {
    println!(
        "mterm {} — terminal workspace-first

usage:
  mterm run [args...]        jalankan command di PTY + engine
  mterm doctor               cek ketersediaan tool (git, node, dll)
  mterm profile list|use|show
  mterm agent start|ask|stop
",
        env!("CARGO_PKG_VERSION")
    );
}
