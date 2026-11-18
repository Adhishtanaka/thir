use thir::{clock, files::{app_name_ok, printable, rm_refuses}, line::Editor, net, notify, parse::{parse_cmd, Cmd}};
use std::{fs::{self, File, OpenOptions}, io::{self, BufRead, Write}, os::unix::{fs::{FileExt, OpenOptionsExt}, process::{CommandExt, ExitStatusExt}}, path::Path, process::{exit, Command, Stdio}};

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
        ("curl [opts] URL", "download a page or file (curl -h for options)"),
        ("passwd", "change your password"), ("dhcp [iface]", "request an address (root only)")]),
];

/// Names of the builtins: every section except "programs", first word of the usage.
fn builtins() -> impl Iterator<Item = &'static str> {
    HELP.iter().filter(|s| s.0 != "programs").flat_map(|s| s.1.iter()).map(|u| u.0.split(' ').next().unwrap_or(""))
}

/// The ! commands (agent, hidden): what help shows. `!` alone lists them all.
const BANG: &[(&str, &str)] = &[("!build \"idea\"", "make an app with AI"), ("!fix APP", "fix a crashed app"), ("!remind TIME TEXT", "set a reminder"),
    ("!schedule", "your reminders"), ("!memory", "what the AI remembers"), ("!setup", "change the AI provider"), ("!", "all the ! commands")];
/// What follows `!`: the verbs of agent. The first group takes an app name next.
const BANG_VERBS: &[&str] = &["ask", "build", "chat", "context", "do", "edit", "fix", "forget", "help", "list", "login", "logout", "memory", "providers", "remember", "remind", "rm", "run", "schedule", "setup", "undo", "use"];
const BANG_APP: &[&str] = &["run", "edit", "fix", "rm", "undo"];
/// Programs in /bin that are not for typing: never offered as "did you mean".
const HIDDEN: &[&str] = &["agent", "login", "den", "init", "nsh"];

fn help_text() -> String {
    let mut o = String::from("nsh, the thir shell\n");
    for (sec, rows) in HELP {
        o.push('\n');
        for (i, (u, d)) in rows.iter().enumerate() { o += &format!("  {:<8} {:<20} {}\n", if i == 0 { sec } else { "" }, u, d); }
    }
    o.push('\n');
    for (i, (u, d)) in BANG.iter().enumerate() { o += &format!("  {:<8} {:<20} {}\n", if i == 0 { "! cmds" } else { "" }, u, d); }
    o + "\nApps you make with !build run by their name.\nPlain words (3 or more, with no command of that name) go to the AI; start a\nline with !ask to send any line there.\nRedirect output with > file or >> file.\nKeys: Tab completes, Up/Down history, Ctrl-A/E start/end, Ctrl-U/K cut to\nstart/end, Ctrl-W cut word, Ctrl-L clear screen, Ctrl-C cancel, Ctrl-D exit.\n"
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

/// Completion candidates for `word`, given the text `before` it; the first word of a line without '/' is a command.
/// After `!`: the ! verbs, then app names after !run, !edit, !fix, !rm and !undo. Names with control characters are never offered (see `printable`).
fn candidates(before: &str, word: &str) -> Vec<String> {
    let first = before.trim().is_empty();
    let mut v: Vec<String> = vec![];
    if first && word.starts_with('!') { v.extend(BANG_VERBS.iter().map(|b| format!("!{b}")).filter(|b| b.starts_with(word))); }
    else if before.split_whitespace().count() == 1 && before.trim_start().strip_prefix('!').is_some_and(|b| BANG_APP.contains(&b.trim())) {
        if let Ok(rd) = fs::read_dir(app_dir()) { v.extend(rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.starts_with(word) && app_name_ok(n))); }
    }
    else if first && !word.contains('/') {
        v.extend(builtins().filter(|b| b.starts_with(word)).map(|b| b.to_string()));
        if let Ok(rd) = fs::read_dir(app_dir()) { v.extend(rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.starts_with(word) && is_app(n))); }
        for dir in std::env::var("PATH").unwrap_or_default().split(':').filter(|d| !d.is_empty()) {
            if let Ok(rd) = fs::read_dir(dir) { v.extend(rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.starts_with(word) && printable(n) == *n && !HIDDEN.contains(&n.as_str())).map(|n| escape(&n))); }
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

/// `name` is an app in ~/apps (it has a main.lua) and no program of that name is on PATH. Apps run through `!run`: no launcher files, because /data is noexec.
fn is_app(name: &str) -> bool {
    app_name_ok(name) && !std::env::var("PATH").unwrap_or_default().split(':').any(|d| Path::new(d).join(name).exists()) && Path::new(&format!("{}{name}/main.lua", app_dir())).is_file()
}

/// What a typed line is.
#[derive(Debug, PartialEq)]
enum Line { Shell, Bang(String), Agent(String), Typo(String, String) }

/// One edit away: a letter added, dropped or changed, or two neighbours swapped. Equal words are not near.
fn near(a: &str, b: &str) -> bool {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    match a.len() as i64 - b.len() as i64 {
        0 => {
            let d: Vec<usize> = (0..a.len()).filter(|&i| a[i] != b[i]).collect();
            d.len() == 1 || (d.len() == 2 && d[1] == d[0] + 1 && a[d[0]] == b[d[1]] && a[d[1]] == b[d[0]])
        }
        1 | -1 => { let (l, s) = if a.len() > b.len() { (&a, &b) } else { (&b, &a) }; (0..l.len()).any(|i| { let mut v = l.clone(); v.remove(i); v == *s }) }
        _ => false,
    }
}

/// A word that looks like a file: a '/' or a '.' inside it (a full stop at the end of a sentence does not count).
fn pathlike(w: &str) -> bool { w.contains('/') || w.trim_end_matches(['.', ',', '!', '?']).contains('.') }

/// Decides on the raw line, before parse_cmd (so apostrophes in plain words work). `!X ...` is an agent command. A first word that is a
/// builtin, program or app (or has a '/') is a normal command. A near miss of a command name is a typo (no AI call). 3 or more plain words go to the AI.
fn classify(line: &str, is_cmd: &dyn Fn(&str) -> bool, names: &dyn Fn() -> Vec<String>) -> Line {
    let t = line.trim();
    if let Some(r) = t.strip_prefix('!') { return Line::Bang(r.trim().to_string()); }
    let words: Vec<&str> = t.split_whitespace().collect();
    let Some(&first) = words.first() else { return Line::Shell };
    if first.contains('/') || is_cmd(first) { return Line::Shell }
    if first.chars().count() >= 2 && (words.len() <= 2 || words[1..].iter().any(|w| pathlike(w))) {
        if let Some(c) = names().into_iter().find(|n| near(first, n)) { return Line::Typo(first.to_string(), c); }
    }
    if words.len() >= 3 && !words.iter().any(|w| w.len() > 1 && w.starts_with('-')) && !t.contains(['>', '|']) { return Line::Agent(t.to_string()); }
    Line::Shell
}

fn is_cmd(w: &str) -> bool { builtins().any(|b| b == w) || std::env::var("PATH").unwrap_or_default().split(':').any(|d| !d.is_empty() && Path::new(d).join(w).exists()) || is_app(w) }

/// Notices the helper left in ~/.notify that are newer than `seen`, shown once each before the prompt.
/// shortcut: a notice that arrives while nsh waits at the prompt shows at the next prompt (den's bar shows it at once).
fn show_notices(seen: &mut i64, uid: u32) {
    let Ok(h) = std::env::var("HOME") else { return };
    let off = clock::home_tz().unwrap_or(0);
    for (t, m) in notify::read(&format!("{h}/.notify"), uid) {
        if t <= *seen { continue }
        let l = clock::local(t, off);
        println!("  \x1b[33m\u{2192}\x1b[0m {:02}:{:02}  {}", l.hour, l.min, printable(&m));
        *seen = t;
    }
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
            Some(c) => match help_cmd(c) {
                Some(t) => { let _ = out.write_all(t.as_bytes()); }
                None => eprintln!("nsh: help: no such command: {}{}", printable(c), if args.len() >= 2 { " (to ask the AI, type your words without \"help\")" } else { "" }),
            },
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
            // on a terminal, end a last line that has no newline, so the next prompt does not draw over it
            let r = File::open(a).and_then(|mut f| {
                let n = io::copy(&mut f, out)?; let mut b = [0u8];
                if color && n > 0 && f.read_at(&mut b, n - 1)? == 1 && b[0] != b'\n' { out.write_all(b"\n")?; }
                Ok(())
            });
            if let Err(e) = r { eprintln!("nsh: cat: {a}: {e}"); }
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
    let (uid, mut seen) = (unsafe { libc::getuid() }, clock::now() - 600);
    loop {
        show_notices(&mut seen, uid);
        let Some(line) = thir::term::read_line(&mut ed, &prompt(), &candidates) else { exit(0) };
        if ed.push_history(line.trim()) {
            if let Ok(mut f) = OpenOptions::new().append(true).create(true).mode(0o600).open(&path) { let _ = writeln!(f, "{}", line.trim()); }
        }
        let names = || candidates("", "").into_iter().filter(|n| !HIDDEN.contains(&n.as_str())).collect::<Vec<_>>();
        let cmd = match classify(&line, &is_cmd, &names) {
            Line::Bang(r) => match parse_cmd(&r) {   // `!fix weather` is /bin/agent fix weather; `!` alone lists the ! commands
                Ok(mut c) => { c.argv.insert(0, "/bin/agent".into()); if c.argv.len() == 1 { c.argv.push("help".into()); } c }
                Err(e) => { eprintln!("nsh: {e}"); continue; }
            },
            Line::Agent(t) => Cmd { argv: ["/bin/agent", "ask", "--", t.as_str()].map(String::from).to_vec(), redirect: None },
            Line::Typo(w, c) => { eprintln!("nsh: {}: command not found. Did you mean {c}?", printable(&w)); continue; }
            Line::Shell => match parse_cmd(&line) { Ok(c) => c, Err(e) => { eprintln!("nsh: {e}"); continue; } },
        };
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
            Err(e) if e.kind() == io::ErrorKind::NotFound && !name.contains('/') => eprintln!("nsh: {}: command not found (3 or more words ask the AI; ! shows the commands)", printable(name)),
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
        assert_eq!(progs, ["edit", "find", "top", "fetch", "curl", "passwd", "dhcp"]);
        assert!(builtins().all(|b| !progs.contains(&b)) && builtins().any(|b| b == "date") && help_cmd("find").unwrap().starts_with("find [dir]"));
    }
    fn cls(l: &str) -> Line {
        let cmds = ["ls", "cat", "mkdir", "weather", "echo", "help"];
        classify(l, &|w| cmds.contains(&w), &|| cmds.iter().map(|c| c.to_string()).collect())
    }
    fn typo(a: &str, b: &str) -> Line { Line::Typo(a.into(), b.into()) }
    #[test] fn sentences_go_to_the_ai() {
        for l in ["make me a weather app", "what's the time now", "can you make a timer", "can you open it.", "I live in Colombo"] { assert_eq!(cls(l), Line::Agent(l.into()), "{l}"); }
    }
    #[test] fn typos_get_a_hint_not_an_api_call() {
        assert_eq!(cls("mkdri docs"), typo("mkdri", "mkdir")); assert_eq!(cls("cta notes.txt"), typo("cta", "cat")); assert_eq!(cls("lss -la"), typo("lss", "ls"));
        assert_eq!(cls("can you open notes.txt"), typo("can", "cat")); assert_eq!(cls("cat/x"), Line::Shell);
    }
    #[test] fn commands_and_odd_lines_stay_in_the_shell() {
        for l in ["foo bar", "frobnicate -x a b", "tell me a > b", "tell me a | b", "./x a b c", "weather", "ls -la", "weather a b c d", "echo hello there you", "", "  ", "foo"] { assert_eq!(cls(l), Line::Shell, "{l:?}"); }
    }
    #[test] fn bang_lines_are_agent_commands() {
        assert_eq!(cls("!fix weather"), Line::Bang("fix weather".into())); assert_eq!(cls("  !  "), Line::Bang("".into())); assert_eq!(cls("!ask echo hi there"), Line::Bang("ask echo hi there".into()));
    }
    #[test] fn near_means_one_edit() {
        for (a, b) in [("cat", "cta"), ("ls", "lss"), ("lss", "ls"), ("mkdir", "mkdr"), ("cat", "cut"), ("echo", "ecoh")] { assert!(near(a, b), "{a} {b}"); }
        for (a, b) in [("cat", "cat"), ("cat", "dog"), ("ls", "lsss"), ("abc", "bca"), ("ab", "cd")] { assert!(!near(a, b), "{a} {b}"); }
    }
    #[test] fn bang_verbs_cover_the_help_and_complete() {
        for v in ["build", "fix", "remind", "schedule", "memory", "setup"] { assert!(BANG_VERBS.contains(&v), "{v}"); }
        assert!(BANG.iter().all(|(u, _)| u.len() < 20) && BANG_APP.iter().all(|a| BANG_VERBS.contains(a)));
        assert_eq!(candidates("", "!bu"), ["!build"]); assert!(candidates("", "!").len() == BANG_VERBS.len());
        assert!(help_text().contains("! cmds") && !help_text().contains("agent"));
    }
}
