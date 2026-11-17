use thir::{clock, files::{app_name_ok, printable, rm_refuses}, line::Editor, net, parse::parse_cmd};
use std::{fs::{self, File, OpenOptions}, io::{self, BufRead, Write}, os::unix::{fs::OpenOptionsExt, process::{CommandExt, ExitStatusExt}}, path::Path, process::{exit, Command, Stdio}};

/// Every command nsh offers, by section: (usage, description). Builtins and completion come from here, so they cannot drift.
const HELP: &[(&str, &[(&str, &str)])] = &[
    ("files", &[("ls [dir]", "list a directory"), ("cat file...", "print files"), ("cd [dir]", "change directory (default: home)"), ("pwd", "print the working directory"),
        ("mkdir [-p] dir...", "make directories"), ("rmdir dir...", "remove empty directories"), ("touch file...", "create files"), ("rm [-r] path...", "remove files (-r: directories too)"),
        ("mv src dest", "move or rename"), ("cp src dest", "copy a file")]),
    ("text", &[("echo text...", "print text"), ("write file", "store lines until a line with only ."), ("clear", "clear the screen")]),
    ("system", &[("date", "local date and time"), ("tz [+HH:MM]", "show or set the UTC offset (~/.tz)"), ("ip", "interfaces and addresses"), ("poweroff", "shut down"), ("reboot", "reboot")]),
    ("session", &[("whoami", "your user name"), ("history", "command history"), ("help [cmd]", "this text, or one command"), ("exit", "log out")]),
    ("programs", &[("edit [file]", "full-screen editor (also: nano)"), ("find [dir] [-name P]", "find files; P uses * and ?; -type f|d"), ("top", "CPU, memory and processes"),
        ("fetch", "system summary with a bear (also: neofetch)"),
        ("curl [opts] URL", "download a page or file (curl -h for options)"), ("agent [question]", "chat with an AI; will build apps (agent -h)"),
        ("passwd", "change your password"), ("dhcp [iface]", "request an address (root only)")]),
];

/// Names of the builtins: every section except "programs", first word of the usage.
fn builtins() -> impl Iterator<Item = &'static str> {
    HELP.iter().filter(|s| s.0 != "programs").flat_map(|s| s.1.iter()).map(|u| u.0.split(' ').next().unwrap_or(""))
}

fn help_text() -> String {
    let mut o = String::from("nsh, the thir shell\n");
    for (sec, rows) in HELP {
        o.push('\n');
        for (i, (u, d)) in rows.iter().enumerate() { o += &format!("  {:<8} {:<20} {}\n", if i == 0 { sec } else { "" }, u, d); }
    }
    o + "\nApps in ~/apps run by their name; agent list shows them.\nRedirect output with > file or >> file.\nKeys: Tab completes, Up/Down history, Ctrl-A/E start/end, Ctrl-U/K cut to\nstart/end, Ctrl-W cut word, Ctrl-L clear screen, Ctrl-C cancel, Ctrl-D exit.\n"
}

fn help_cmd(name: &str) -> Option<String> {
    HELP.iter().flat_map(|s| s.1.iter()).find(|u| u.0.split(' ').next() == Some(name)).map(|(u, d)| format!("{u}  {d}\n"))
}

/// `tz`: show the offset, or set it (`~/.tz`, mode 0600).
fn tz(args: &[String], out: &mut dyn Write) {
    let Some(a) = args.first() else { let _ = writeln!(out, "{}", clock::format_offset(clock::home_tz().unwrap_or(0))); return };
    let Some(off) = clock::parse_offset(a) else { eprintln!("nsh: tz: use +HH:MM or -HH:MM"); return };
    let Ok(home) = std::env::var("HOME") else { eprintln!("nsh: tz: no home directory"); return };
    let r = OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).custom_flags(libc::O_NOFOLLOW).open(format!("{home}/.tz"))
        .and_then(|mut f| writeln!(f, "{}", clock::format_offset(off)));
    if let Err(e) = r { eprintln!("nsh: tz: {e}"); }
}

fn prompt() -> String {
    let user = std::env::var("USER").unwrap_or_else(|_| "?".into());
    let host = fs::read_to_string("/proc/sys/kernel/hostname").map(|h| h.trim().to_string()).unwrap_or_else(|_| "thir".into());
    let cwd = std::env::current_dir().map(|d| d.display().to_string()).unwrap_or_else(|_| "?".into());
    let cwd = match std::env::var("HOME") {
        Ok(h) if !h.is_empty() && h != "/" && (cwd == h || cwd.starts_with(&format!("{h}/"))) => format!("~{}", &cwd[h.len()..]),
        _ => cwd,
    };
    format!("\x1b[1;32m{}@{}\x1b[0m:\x1b[1;34m{}\x1b[0m$ ", printable(&user), printable(&host), printable(&cwd))
}

/// Backslash-escape what parse_cmd treats specially, so a completed name stays one word and never becomes a redirect.
fn escape(s: &str) -> String {
    // the same set parse_cmd treats specially: all whitespace, '>', quotes, backslash
    s.chars().fold(String::new(), |mut o, c| { if c.is_whitespace() || matches!(c, '>' | '"' | '\'' | '\\') { o.push('\\'); } o.push(c); o })
}

/// Completion candidates for `word`; the first word of a line without '/' is a command.
/// Names with control characters are never offered (see `printable`).
fn candidates(first: bool, word: &str) -> Vec<String> {
    let mut v: Vec<String> = vec![];
    if first && !word.contains('/') {
        v.extend(builtins().filter(|b| b.starts_with(word)).map(|b| b.to_string()));
        if let Ok(rd) = fs::read_dir(app_dir()) { v.extend(rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.starts_with(word) && is_app(n))); }
        for dir in std::env::var("PATH").unwrap_or_default().split(':').filter(|d| !d.is_empty()) {
            if let Ok(rd) = fs::read_dir(dir) { v.extend(rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.starts_with(word) && printable(n) == *n).map(|n| escape(&n))); }
        }
    } else {
        let (dir, pre) = word.rsplit_once('/').map_or(("", word), |(d, p)| (&word[..d.len() + 1], p));
        if let Ok(rd) = fs::read_dir(if dir.is_empty() { "." } else { dir }) {
            for e in rd.flatten() {
                let n = e.file_name().to_string_lossy().into_owned();
                if !n.starts_with(pre) || (n.starts_with('.') && !pre.starts_with('.')) || printable(&n) != n { continue }
                v.push(format!("{dir}{}{}", escape(&n), if e.path().is_dir() { "/" } else { "" }));
            }
        }
    }
    v.sort(); v.dedup(); v
}

fn app_dir() -> String { format!("{}/apps/", std::env::var("HOME").unwrap_or_default()) }

/// `name` is an app in ~/apps (it has a main.lua) and no program of that name is on PATH. Apps run through `agent run`: no launcher files, because /data is noexec.
fn is_app(name: &str) -> bool {
    app_name_ok(name) && !std::env::var("PATH").unwrap_or_default().split(':').any(|d| Path::new(d).join(name).exists()) && Path::new(&format!("{}{name}/main.lua", app_dir())).is_file()
}

fn hist_path() -> String { format!("{}/.nsh_history", std::env::var("HOME").unwrap_or_else(|_| "/".into())) }

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
fn builtin(argv: &[String], out: &mut dyn Write, stdin: &io::Stdin, hist: &[String], color: bool) -> Option<()> {
    let args = &argv[1..];
    let each = |allowed: &str, f: &mut dyn FnMut(&str, &str) -> io::Result<()>| match opts(args, allowed) {
        Ok((fl, rest)) => for a in rest { if let Err(e) = f(&fl, a) { eprintln!("nsh: {}: {a}: {e}", argv[0]); } },
        Err(e) => eprintln!("nsh: {}: {e}", argv[0]),
    };
    match argv[0].as_str() {
        "cd" => { let d = args.first().cloned().or_else(|| std::env::var("HOME").ok()).unwrap_or("/".into()); if let Err(e) = std::env::set_current_dir(&d) { eprintln!("nsh: cd: {d}: {e}"); } }
        "exit" => exit(args.first().and_then(|s| s.parse().ok()).unwrap_or(0)),
        "help" => match args.first() {
            None => { let _ = out.write_all(help_text().as_bytes()); }
            Some(c) => match help_cmd(c) { Some(t) => { let _ = out.write_all(t.as_bytes()); } None => eprintln!("nsh: help: no such command: {}", printable(c)) },
        },
        "date" => { let _ = writeln!(out, "{}", clock::date_line(clock::now(), clock::home_tz().unwrap_or(0))); }
        "tz" => tz(args, out),
        "clear" => { let _ = out.write_all(b"\x1b[H\x1b[2J"); }
        "history" => for (i, l) in hist.iter().enumerate() { let _ = writeln!(out, "{:>5}  {}", i + 1, printable(l)); },
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
                        .map(|e| { let n = printable(&e.file_name().to_string_lossy()); if e.path().is_dir() { if color { format!("\x1b[1;34m{n}/\x1b[0m") } else { n + "/" } } else { n } }).collect();
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
    let path = hist_path();
    let old = fs::read_to_string(&path).unwrap_or_default();
    let mut ed = Editor::new(old.lines().rev().take(1000).map(String::from).collect::<Vec<_>>().into_iter().rev().collect());
    loop {
        let Some(line) = thir::term::read_line(&mut ed, &prompt(), &candidates) else { exit(0) };
        if ed.push_history(line.trim()) {
            if let Ok(mut f) = OpenOptions::new().append(true).create(true).mode(0o600).open(&path) { let _ = writeln!(f, "{}", line.trim()); }
        }
        let cmd = match parse_cmd(&line) { Ok(c) => c, Err(e) => { eprintln!("nsh: {e}"); continue; } };
        let Some(name) = cmd.argv.first() else { continue };
        let file = match &cmd.redirect {   // opened first; umask makes it 0600
            Some((p, append)) => match OpenOptions::new().write(true).create(true).append(*append).truncate(!*append).open(p) {
                Ok(f) => Some(f), Err(e) => { eprintln!("nsh: {p}: {e}"); continue; }
            },
            None => None,
        };
        let mut out: Box<dyn Write> = match file.as_ref().map(File::try_clone) { Some(Ok(f)) => Box::new(f), Some(Err(e)) => { eprintln!("nsh: {e}"); continue }, None => Box::new(io::stdout()) };
        let color = cmd.redirect.is_none() && unsafe { libc::isatty(1) } == 1;
        if builtin(&cmd.argv, &mut out, &stdin, ed.history(), color).is_some() { continue }
        let app = is_app(name);
        let mut c = Command::new(if app { "/bin/agent" } else { name });
        if app { c.args(["run", name]); }
        c.args(&cmd.argv[1..]);
        if let Some(f) = file { c.stdout(Stdio::from(f)); }
        match unsafe { c.pre_exec(|| { for s in [libc::SIGINT, libc::SIGQUIT] { libc::signal(s, libc::SIG_DFL); } Ok(()) }) }.status() {
            Err(e) => eprintln!("nsh: {name}: {e}"),
            Ok(s) if !s.success() => match s.signal() { Some(n) => println!("[signal {n}]"), None => println!("[exit {}]", s.code().unwrap_or(-1)) },
            Ok(_) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn escape_keeps_one_word() {
        assert_eq!(escape("my file>x"), "my\\ file\\>x");
        assert_eq!(thir::parse::parse_line(&format!("cat {}", escape("a b'c"))).unwrap(), vec!["cat", "a b'c"]);
    }
    #[test] fn escape_covers_all_whitespace() {
        for n in ["a\u{a0}b", "a\u{3000}b", "x\ty", "-n > \\q"] {
            assert_eq!(thir::parse::parse_line(&format!("cat {}", escape(n))).unwrap(), vec!["cat", n]);
        }
    }
    #[test] fn reserved_covers_builtins_and_programs() {
        for b in builtins() { assert!(thir::files::RESERVED.contains(&b), "{b}"); }
        for (_, rows) in HELP.iter().filter(|s| s.0 == "programs") { for r in *rows { assert!(thir::files::RESERVED.contains(&r.0.split(' ').next().unwrap()), "{}", r.0); } }
    }
    #[test] fn help_lines_fit_80_cols() { for l in help_text().lines() { assert!(l.chars().count() <= 80, "{l}"); } }
    #[test] fn help_names_unique_and_programs_known() {
        let mut all: Vec<&str> = HELP.iter().flat_map(|s| s.1.iter()).map(|u| u.0.split(' ').next().unwrap()).collect();
        let n = all.len(); all.sort(); all.dedup(); assert_eq!(all.len(), n);
        let progs: Vec<&str> = HELP.iter().filter(|s| s.0 == "programs").flat_map(|s| s.1.iter()).map(|u| u.0.split(' ').next().unwrap()).collect();
        assert_eq!(progs, ["edit", "find", "top", "fetch", "curl", "agent", "passwd", "dhcp"]);
        assert!(builtins().all(|b| !progs.contains(&b)) && builtins().any(|b| b == "date") && help_cmd("find").unwrap().starts_with("find [dir]"));
    }
}
