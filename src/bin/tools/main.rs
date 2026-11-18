//! One binary for the built-in apps: clock calendar calc contacts notes settings help beep bell. It starts the app named by how it was called (symlinks in /bin).
use std::process::exit;

fn main() {
    let mut args: Vec<String> = std::env::args().collect();
    let mut name = args.remove(0).rsplit('/').next().unwrap_or("").to_string();
    if name == "tools" { if args.is_empty() { eprintln!("usage: tools APP [args]  (APP: {})", thir::apps::NAMES.join(" ")); exit(2) } name = args.remove(0); }
    exit(thir::apps::run(&name, &args));
}
