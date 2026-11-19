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
    ("apps", &[("clock", "big time, stopwatch, timer, alarms"), ("calendar", "month view, events, reminders"), ("calc", "calculator with history"), ("contacts", "names, phones, emails"),
        ("notes", "notes with tags and todos"), ("settings", "time zone, wallpaper, sound"), ("beep [hz ms]", "play the alarm chime or a note")]),
];

/// Names of the builtins: every section except "programs" and "apps", first word of the usage.
fn builtins() -> impl Iterator<Item = &'static str> {
    HELP.iter().filter(|s| s.0 != "programs" && s.0 != "apps").flat_map(|s| s.1.iter()).map(|u| u.0.split(' ').next().unwrap_or(""))
}

/// Programs in /bin that are not for typing: never offered as "did you mean".
const HIDDEN: &[&str] = &["agent", "login", "den", "init", "nsh"];

/// What `!` alone prints: the only way to the AI is `!` and your words.
const ASK_HELP: &str = "Type ! and your words to ask the AI. It shows one thing and asks first.\n  ! move notes into docs          ! what apps do I have\n  ! make me a timer app           ! remind me at 5pm to call mum\n  ! add Kasun to my contacts, 0771234567\n  ! change my AI provider         ! delete the app weather\nPlain lines without ! are a normal shell.\n";

fn help_text() -> String {
    let mut o = String::from("nsh, the thir shell\n");
    for (sec, rows) in HELP {
        o.push('\n');
        for (i, (u, d)) in rows.iter().enumerate() { o += &format!("  {:<8} {:<20} {}\n", if i == 0 { sec } else { "" }, u, d); }
    }
    o.push('\n');
    o += "  !        your words          ask the AI (type ! alone for examples)\n";
    o + "\nApps the AI builds run by their name. Plain lines are a normal shell.\nRedirect output with > file or >> file.\nKeys: Tab completes, Up/Down history, Ctrl-A/E start/end, Ctrl-U/K cut to\nstart/end, Ctrl-W cut word, Ctrl-L clear screen, Ctrl-C cancel, Ctrl-D exit.\n"
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
/// Names with control characters are never offered (see `printable`).
fn candidates(before: &str, word: &str) -> Vec<String> {
    let first = before.trim().is_empty();
    let mut v: Vec<String> = vec![];
    if first && !word.contains('/') {
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

/// `name` is an app in ~/apps (it has a main.lua) and no program of that name is on PATH. Apps run through /bin/agent run: no launcher files, because /data is noexec.
fn is_app(name: &str) -> bool {
    app_name_ok(name) && !std::env::var("PATH").unwrap_or_default().split(':').any(|d| Path::new(d).join(name).exists()) && Path::new(&format!("{}{name}/main.lua", app_dir())).is_file()
}

/// What a typed line is.
#[derive(Debug, PartialEq)]
enum Line { Shell, Ask(String), Typo(String, String) }

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

/// Decides on the raw line, before parse_cmd (so apostrophes in plain words work). `!` and the words after it (space or not) go to the AI; `!` alone shows help.
/// A first word that is a builtin, program or app (or has a '/') is a normal command. A near miss of a command name is a typo (no AI call). Everything else is a normal shell line.
fn classify(line: &str, is_cmd: &dyn Fn(&str) -> bool, names: &dyn Fn() -> Vec<String>) -> Line {
    let t = line.trim();
    if let Some(r) = t.strip_prefix('!') { return Line::Ask(r.trim().to_string()); }
    let words: Vec<&str> = t.split_whitespace().collect();
    let Some(&first) = words.first() else { return Line::Shell };
    if first.contains('/') || is_cmd(first) { return Line::Shell }
    if first.chars().count() >= 2 && (words.len() <= 2 || words[1..].iter().any(|w| pathlike(w))) {
        if let Some(c) = names().into_iter().find(|n| near(first, n)) { return Line::Typo(first.to_string(), c); }
    }
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
            None if color && Path::new("/bin/help").exists() => { let _ = Command::new("/bin/help").status(); }   // the full-screen guide
            None => { let _ = out.write_all(help_text().as_bytes()); }
            Some(c) => match help_cmd(c) {
                Some(t) => { let _ = out.write_all(t.as_bytes()); }
                None => eprintln!("nsh: help: no such command: {}{}", printable(c), if args.len() >= 2 { " (to ask the AI, type ! and your words)" } else { "" }),
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

/// One command line from the AI, or why not. The line is untrusted: one line, printable, no shell tricks (nsh has no sh, pipes or `;`), and it must start with a
/// command nsh knows. It is parsed by nsh's own parse_cmd, so quoting works and redirects do not.
fn check_line(line: &str, is_cmd: &dyn Fn(&str) -> bool) -> Result<Cmd, String> {
    let bad = || "could not turn that into one command".to_string();
    if line.chars().count() > 300 || printable(line) != line || line.contains([';', '|', '&', '`', '$', '>', '<', '\n', '\r']) { return Err(bad()); }
    let c = parse_cmd(line).map_err(|_| bad())?;
    match c.argv.first() { Some(f) if c.redirect.is_none() && !f.contains('/') && is_cmd(f) => Ok(c), _ => Err(bad()) }
}

/// Why a command needs a second look: it destroys, powers off, or replaces a file that exists.
fn dangerous(argv: &[String]) -> Option<&'static str> {
    let rest = &argv[1..];
    match argv[0].as_str() {
        "rm" | "rmdir" => Some("this deletes files or folders"),
        "poweroff" | "reboot" => Some("this shuts down or restarts thir"),
        "passwd" => Some("this changes your password"),
        "mv" | "cp" => { let o: Vec<&String> = rest.iter().filter(|a| !a.starts_with('-')).collect(); if let [s, d] = o[..] { if Path::new(&target(s, d)).exists() { return Some("this replaces a file that exists"); } } None }
        "curl" => rest.windows(2).any(|w| w[0] == "-o" && Path::new(&w[1]).exists()).then_some("this replaces a file that exists"),
        _ => None,
    }
}

/// `! words`: asks /bin/agent, which sends the line back on fd 3 (a pipe), or prints an answer or its own action. Some(line) when it proposed a command.
fn ask_ai(words: &str, names: &[String]) -> Option<String> {
    let mut fds = [0i32; 2];
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } != 0 { eprintln!("nsh: cannot make a pipe"); return None; }
    let w = fds[1];
    let mut c = Command::new("/bin/agent");
    c.args(["ask", "--", words]).env("THIR_CMDS", names.join(" ")).env("THIR_PICK_FD", "3");
    let r = unsafe { c.pre_exec(move || {
        for s in [libc::SIGINT, libc::SIGQUIT] { libc::signal(s, libc::SIG_DFL); }
        if w == 3 { libc::fcntl(3, libc::F_SETFD, 0); } else if libc::dup2(w, 3) < 0 { return Err(io::Error::last_os_error()); }
        Ok(())
    }) }.status();
    unsafe { libc::close(w); }
    let mut got = String::new();
    { use std::os::fd::FromRawFd; let mut f = unsafe { File::from_raw_fd(fds[0]) }; let _ = io::Read::read_to_string(&mut io::Read::take(&mut f, 2000), &mut got); }
    if let Err(e) = r { eprintln!("nsh: /bin/agent: {e}"); return None; }
    (!got.trim().is_empty()).then(|| got.trim().to_string())
}

/// Shows the proposed command exactly and asks. Enter is yes, or no when it is dangerous.
fn confirm_cmd(line: &str, warn: Option<&str>) -> bool {
    if let Some(w) = warn { println!("  \x1b[33m! {w}\x1b[0m"); }
    let q = format!("  \x1b[36m\u{2192}\x1b[0m {}   run? \x1b[2m{}\x1b[0m ", line, if warn.is_some() { "[y/N]" } else { "[Y/n]" });
    let Some(a) = thir::term::read_line(&mut Editor::new(vec![]), &q, &|_, _| vec![]) else { return false };
    let a = a.trim().to_lowercase();
    if warn.is_some() { matches!(a.as_str(), "y" | "yes") } else { matches!(a.as_str(), "" | "y" | "yes") }
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
            Line::Ask(w) if w.is_empty() => { print!("{ASK_HELP}"); continue; }
            Line::Ask(w) => {
                let Some(l) = ask_ai(&w, &names()) else { continue };
                let c = match check_line(&l, &is_cmd) { Ok(c) => c, Err(e) => { eprintln!("nsh: {e}"); continue; } };
                if !confirm_cmd(&l, dangerous(&c.argv)) { println!("  nothing done."); continue; }
                c
            }
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
            Err(e) if e.kind() == io::ErrorKind::NotFound && !name.contains('/') => eprintln!("nsh: {}: command not found (type ! and your words to ask the AI)", printable(name)),
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
        assert!(HELP.iter().filter(|s| s.0 == "apps").flat_map(|s| s.1.iter()).all(|u| thir::apps::NAMES.contains(&u.0.split(' ').next().unwrap())));
        assert!(builtins().all(|b| !progs.contains(&b)) && builtins().any(|b| b == "date") && help_cmd("find").unwrap().starts_with("find [dir]"));
    }
    fn cls(l: &str) -> Line {
        let cmds = ["ls", "cat", "mkdir", "weather", "echo", "help"];
        classify(l, &|w| cmds.contains(&w), &|| cmds.iter().map(|c| c.to_string()).collect())
    }
    fn typo(a: &str, b: &str) -> Line { Line::Typo(a.into(), b.into()) }
    #[test] fn bang_lines_go_to_the_ai_and_plain_lines_never_do() {
        assert_eq!(cls("! move notes into docs"), Line::Ask("move notes into docs".into())); assert_eq!(cls("!make me a timer"), Line::Ask("make me a timer".into()));
        assert_eq!(cls("  !  "), Line::Ask("".into())); assert_eq!(cls("!"), Line::Ask("".into())); assert_eq!(cls("!fix weather"), Line::Ask("fix weather".into()));
        for l in ["make me a weather app", "what's the time now", "I live in Colombo", "can you make a timer"] { assert_eq!(cls(l), Line::Shell, "{l}"); }
    }
    fn ok(l: &str) -> Result<Vec<String>, String> { check_line(l, &|w| ["ls", "mv", "cp", "rm", "mkdir", "curl", "echo", "clock"].contains(&w)).map(|c| c.argv) }
    #[test] fn model_lines_are_checked() {
        assert_eq!(ok("mv notes.txt docs/").unwrap(), ["mv", "notes.txt", "docs/"]); assert_eq!(ok("mkdir \"my docs\"").unwrap(), ["mkdir", "my docs"]); assert_eq!(ok("clock").unwrap(), ["clock"]);
        for l in ["ls; rm -rf ~", "ls | cat", "ls && rm x", "echo `id`", "echo $(id)", "echo $HOME", "ls > x", "ls < x", "ls\nrm x", "ls\rrm x", "", "sh -c ls", "!build x", "/bin/sh", "./x", "../../bin/sh", "ls \x1b[2J", &format!("echo {}", "a".repeat(300)), "echo 'unclosed"] {
            assert_eq!(ok(l), Err("could not turn that into one command".into()), "{l:?}");
        }
    }
    #[test] fn destroying_commands_get_a_warning() {
        let v = |l: &str| dangerous(&l.split(' ').map(String::from).collect::<Vec<_>>());
        for l in ["rm x", "rm -r docs", "rmdir d", "poweroff", "reboot", "passwd"] { assert!(v(l).is_some(), "{l}"); }
        let f = std::env::temp_dir().join(format!("thir-nsh-{}", std::process::id())); fs::write(&f, "x").unwrap(); let f = f.to_string_lossy().into_owned();
        assert!(v(&format!("mv a {f}")).is_some() && v(&format!("cp a {f}")).is_some() && v(&format!("curl -o {f} http://x")).is_some());
        fs::remove_file(&f).unwrap();
        for l in [format!("mv a {f}"), "mv a b".into(), "ls".into(), "mkdir d".into(), "clock".into(), "curl -o nosuchfile http://x".into()] { assert!(v(&l).is_none(), "{l}"); }
    }
    #[test] fn typos_get_a_hint_not_an_api_call() {
        assert_eq!(cls("mkdri docs"), typo("mkdri", "mkdir")); assert_eq!(cls("cta notes.txt"), typo("cta", "cat")); assert_eq!(cls("lss -la"), typo("lss", "ls"));
        assert_eq!(cls("can you open notes.txt"), typo("can", "cat")); assert_eq!(cls("cat/x"), Line::Shell);
    }
    #[test] fn commands_and_odd_lines_stay_in_the_shell() {
        for l in ["foo bar", "frobnicate -x a b", "tell me a > b", "tell me a | b", "./x a b c", "weather", "ls -la", "weather a b c d", "echo hello there you", "", "  ", "foo"] { assert_eq!(cls(l), Line::Shell, "{l:?}"); }
    }
    #[test] fn near_means_one_edit() {
        for (a, b) in [("cat", "cta"), ("ls", "lss"), ("lss", "ls"), ("mkdir", "mkdr"), ("cat", "cut"), ("echo", "ecoh")] { assert!(near(a, b), "{a} {b}"); }
        for (a, b) in [("cat", "cat"), ("cat", "dog"), ("ls", "lsss"), ("abc", "bca"), ("ab", "cd")] { assert!(!near(a, b), "{a} {b}"); }
    }
    #[test] fn help_has_no_bang_verbs() {
        let t = help_text(); assert!(t.contains("ask the AI") && !t.contains("agent") && !t.contains("!build") && !ASK_HELP.contains("!build") && ASK_HELP.lines().all(|l| l.chars().count() <= 80));
        assert!(candidates("", "!").is_empty());
    }
}
