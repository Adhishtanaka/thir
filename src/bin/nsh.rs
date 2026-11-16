use thir::{files::rm_refuses, line::{Act, Editor, Key}, net, parse::parse_cmd};
use std::{fs::{self, File, OpenOptions}, io::{self, BufRead, Write}, os::unix::{fs::OpenOptionsExt, process::{CommandExt, ExitStatusExt}}, path::Path, process::{exit, Command, Stdio}};

const BUILTINS: &[&str] = &["cd", "pwd", "ls", "cat", "echo", "whoami", "mkdir", "rmdir", "touch", "rm", "mv", "cp", "write", "clear", "history", "exit", "help", "ip", "poweroff", "reboot"];

/// Raw mode only while a line is being read, so children, `write`, `passwd` and `login` always see a cooked tty. None if fd 0 is not a tty.
fn raw_on() -> Option<libc::termios> {
    let mut old: libc::termios = unsafe { std::mem::zeroed() };
    if unsafe { libc::tcgetattr(0, &mut old) } != 0 { return None; }
    let mut t = old;
    t.c_lflag &= !(libc::ICANON | libc::ECHO | libc::ISIG | libc::IEXTEN);
    t.c_iflag &= !(libc::ICRNL | libc::IXON);
    t.c_cc[libc::VMIN] = 1; t.c_cc[libc::VTIME] = 0;
    unsafe { libc::tcsetattr(0, libc::TCSANOW, &t); }
    Some(old)
}
fn raw_off(t: &libc::termios) { unsafe { libc::tcsetattr(0, libc::TCSADRAIN, t); } }

fn put(b: &[u8]) { let mut o = 0; while o < b.len() { let n = unsafe { libc::write(1, b[o..].as_ptr() as *const _, b.len() - o) }; if n <= 0 { break } o += n as usize; } }

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

/// Completion candidates for `word`; the first word of a line without '/' is a command.
/// Control bytes as '?', so a hostile file name cannot send escape sequences to the terminal.
fn printable(s: &str) -> String { s.chars().map(|c| if c.is_control() { '?' } else { c }).collect() }

/// Backslash-escape what parse_cmd treats specially, so a completed name stays one word and never becomes a redirect.
fn escape(s: &str) -> String {
    s.chars().fold(String::new(), |mut o, c| { if matches!(c, ' ' | '>' | '"' | '\'' | '\\') { o.push('\\'); } o.push(c); o })
}

/// Names with control characters are never offered (see `printable`).
fn candidates(first: bool, word: &str) -> Vec<String> {
    let mut v: Vec<String> = vec![];
    if first && !word.contains('/') {
        v.extend(BUILTINS.iter().filter(|b| b.starts_with(word)).map(|b| b.to_string()));
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

/// One line from the user with editing. None = end of input. Raw mode is always off again on return.
fn read_cmd(ed: &mut Editor, stdin: &io::Stdin) -> Option<String> {
    let p = prompt();
    let Some(old) = raw_on() else {
        print!("{p}"); let _ = io::stdout().flush();
        let mut l = String::new();
        return match stdin.read_line(&mut l) { Ok(0) | Err(_) => None, Ok(_) => Some(l) };
    };
    put(&ed.redraw(&p));
    let r = 'read: loop {
        let mut buf = [0u8; 256];
        let n = unsafe { libc::read(0, buf.as_mut_ptr() as *mut _, buf.len()) };
        if n < 0 && io::Error::last_os_error().raw_os_error() == Some(libc::EINTR) { continue }
        if n <= 0 { break None }
        for k in ed.decode(&buf[..n as usize]) {   // shortcut: keys after Enter in the same read are dropped
            if k == Key::Tab { let (first, w) = ed.word(); let o = ed.complete(&candidates(first, &w), &p); put(&o); continue }
            let (a, o) = ed.key(k, &p); put(&o);
            match a { Act::Submit(l) => break 'read Some(l), Act::Eof => break 'read None, Act::None => {} }
        }
    };
    raw_off(&old);
    r
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
        "help" => { let _ = writeln!(out, "nsh builtins: {}; redirect with > and >>; commands: passwd dhcp\ntab completes, up/down history, ctrl-a/e/u/k/w/l", BUILTINS.join(" ")); }
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
        let Some(line) = read_cmd(&mut ed, &stdin) else { exit(0) };
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn escape_keeps_one_word() {
        assert_eq!(escape("my file>x"), "my\\ file\\>x");
        assert_eq!(thir::parse::parse_line(&format!("cat {}", escape("a b'c"))).unwrap(), vec!["cat", "a b'c"]);
    }
    #[test] fn printable_hides_control_bytes() { assert_eq!(printable("a\x1b[2Jb\n"), "a?[2Jb?"); }
}
