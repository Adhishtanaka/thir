use thir::{files::rm_refuses, net, parse::parse_cmd};
use std::{fs::{self, File, OpenOptions}, io::{self, BufRead, Write}, os::unix::process::{CommandExt, ExitStatusExt}, path::Path, process::{exit, Command, Stdio}};

/// Splits `-x` flags from operands; Err(flag) for a flag not in `allowed`.
fn opts<'a>(args: &'a [String], allowed: &str) -> Result<(String, Vec<&'a String>), String> {
    let (mut flags, mut rest) = (String::new(), vec![]);
    for a in args {
        if a.len() > 1 && a.starts_with('-') {
            for c in a[1..].chars() { if !allowed.contains(c) { return Err(format!("unknown option -{c}")); } flags.push(c); }
        } else { rest.push(a); }
    }
    Ok((flags, rest))
}

/// `dest` is an existing dir => dest/basename(src).
fn target(src: &str, dest: &str) -> String {
    if Path::new(dest).is_dir() { format!("{}/{}", dest.trim_end_matches('/'), Path::new(src).file_name().map_or(src.into(), |n| n.to_string_lossy())) } else { dest.into() }
}

fn rm(path: &str, recursive: bool) -> io::Result<()> {
    if let Some(why) = rm_refuses(path) { return Err(io::Error::new(io::ErrorKind::Other, format!("refusing to remove '{path}' ({why})"))); }
    let m = fs::symlink_metadata(path)?;   // never follows symlinks
    if !m.is_dir() { fs::remove_file(path) }
    else if recursive { fs::remove_dir_all(path) }
    else { Err(io::Error::new(io::ErrorKind::Other, "is a directory (use -r)")) }
}

/// Runs a builtin; None if `argv[0]` is not one. Builtin errors go to stderr.
fn builtin(argv: &[String], out: &mut dyn Write, stdin: &io::Stdin) -> Option<()> {
    let args = &argv[1..];
    let each = |allowed: &str, f: &mut dyn FnMut(&str, &str) -> io::Result<()>| match opts(args, allowed) {
        Ok((fl, rest)) => for a in rest { if let Err(e) = f(&fl, a) { eprintln!("nsh: {}: {a}: {e}", argv[0]); } },
        Err(e) => eprintln!("nsh: {}: {e}", argv[0]),
    };
    match argv[0].as_str() {
        "cd" => { let d = args.first().cloned().or_else(|| std::env::var("HOME").ok()).unwrap_or("/".into()); if let Err(e) = std::env::set_current_dir(&d) { eprintln!("nsh: cd: {d}: {e}"); } }
        "exit" => exit(args.first().and_then(|s| s.parse().ok()).unwrap_or(0)),
        "help" => { let _ = writeln!(out, "nsh builtins: cd pwd ls cat echo whoami mkdir rmdir touch rm mv cp write exit help ip poweroff reboot; redirect with > and >>; commands: passwd dhcp"); }
        "pwd" => match std::env::current_dir() { Ok(d) => { let _ = writeln!(out, "{}", d.display()); } Err(e) => eprintln!("nsh: pwd: {e}") },
        "echo" => { let _ = writeln!(out, "{}", args.join(" ")); }
        "whoami" => { let _ = writeln!(out, "{}", std::env::var("USER").unwrap_or_else(|_| if unsafe { libc::getuid() } == 0 { "root".into() } else { unsafe { libc::getuid() }.to_string() })); }
        "mkdir" => each("p", &mut |f, a| if f.contains('p') { fs::create_dir_all(a) } else { fs::create_dir(a) }),
        "rmdir" => each("", &mut |_, a| fs::remove_dir(a)),
        "touch" => each("", &mut |_, a| OpenOptions::new().create(true).append(true).open(a).map(drop)),
        "rm" => each("r", &mut |f, a| rm(a, f.contains('r'))),
        "mv" | "cp" => {
            let cp = argv[0] == "cp";
            match opts(args, "") {
                Ok((_, r)) if r.len() == 2 => {
                    let to = target(r[0], r[1]);
                    if let Err(e) = if cp { fs::copy(r[0], &to).map(drop) } else { fs::rename(r[0], &to) } { eprintln!("nsh: {}: {}: {e}", argv[0], r[0]); }
                }
                _ => eprintln!("nsh: {}: usage: {} src dest", argv[0], argv[0]),
            }
        }
        "write" => match args {
            [f] => match File::create(f) {
                Ok(mut file) => loop {
                    let mut l = String::new();
                    if !matches!(stdin.lock().read_line(&mut l), Ok(n) if n > 0) || l.trim_end() == "." { break }
                    if let Err(e) = file.write_all(l.as_bytes()) { eprintln!("nsh: write: {f}: {e}"); break }
                },
                Err(e) => eprintln!("nsh: write: {f}: {e}"),
            },
            _ => eprintln!("nsh: write: usage: write file (lines until a line with only .)"),
        },
        "ls" => {
            let d = args.first().map_or(".", |s| s);
            match fs::read_dir(d) {
                Ok(rd) => {
                    let mut names: Vec<String> = rd.filter_map(|e| e.ok())
                        .map(|e| { let n = e.file_name().to_string_lossy().into_owned(); if e.path().is_dir() { n + "/" } else { n } }).collect();
                    names.sort();
                    for n in names { let _ = writeln!(out, "{n}"); }
                }
                Err(e) => eprintln!("nsh: ls: {d}: {e}"),
            }
        }
        "cat" => for a in args {
            if let Err(e) = File::open(a).and_then(|mut f| io::copy(&mut f, out)) { eprintln!("nsh: cat: {a}: {e}"); }
        },
        "ip" => match net::interfaces() { Ok(v) => for i in &v { let _ = writeln!(out, "{}", i.line()); }, Err(e) => eprintln!("nsh: ip: {e}") },
        "poweroff" => exit(thir::EXIT_POWEROFF),
        "reboot" => exit(thir::EXIT_REBOOT),
        _ => return None,
    }
    let _ = out.flush();
    Some(())
}

fn main() {
    // Ctrl-C / Ctrl-\ / Ctrl-Z hit the whole foreground group: the shell ignores them.
    // Children get Ctrl-C and Ctrl-\ back; Ctrl-Z stays ignored because nsh has no job control to resume a stopped child.
    unsafe { for s in [libc::SIGINT, libc::SIGQUIT, libc::SIGTSTP] { libc::signal(s, libc::SIG_IGN); } }
    let stdin = io::stdin();
    loop {
        print!("nsh> ");
        let _ = io::stdout().flush();
        let mut line = String::new();
        match stdin.read_line(&mut line) { Ok(0) | Err(_) => exit(0), Ok(_) => {} }
        let cmd = match parse_cmd(&line) { Ok(c) => c, Err(e) => { eprintln!("nsh: {e}"); continue; } };
        let Some(name) = cmd.argv.first() else { continue };
        let file = match &cmd.redirect {   // opened first; umask makes it 0600
            Some((p, append)) => match OpenOptions::new().write(true).create(true).append(*append).truncate(!*append).open(p) {
                Ok(f) => Some(f), Err(e) => { eprintln!("nsh: {p}: {e}"); continue; }
            },
            None => None,
        };
        let mut out: Box<dyn Write> = match file.as_ref().map(File::try_clone) { Some(Ok(f)) => Box::new(f), Some(Err(e)) => { eprintln!("nsh: {e}"); continue }, None => Box::new(io::stdout()) };
        if builtin(&cmd.argv, &mut out, &stdin).is_some() { continue }
        let mut c = Command::new(name);
        c.args(&cmd.argv[1..]);
        if let Some(f) = file { c.stdout(Stdio::from(f)); }
        match unsafe { c.pre_exec(|| { for s in [libc::SIGINT, libc::SIGQUIT] { libc::signal(s, libc::SIG_DFL); } Ok(()) }) }.status() {
            Err(e) => eprintln!("nsh: {name}: {e}"),
            Ok(s) if !s.success() => match s.signal() { Some(n) => println!("[signal {n}]"), None => println!("[exit {}]", s.code().unwrap_or(-1)) },
            Ok(_) => {}
        }
    }
}
