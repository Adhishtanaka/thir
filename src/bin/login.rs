//! login: the only way into a session. No args: serial console (init). `--as <uid>`: extra den panes.
use std::{os::unix::{fs::chown, process::CommandExt}, path::Path, process::{exit, Command, Stdio}, thread::sleep, time::Duration};
use thir::{clock, user::{self, User}};

fn die(msg: &str) -> ! { eprintln!("login: {msg}"); exit(1) }
fn die_slow(msg: &str) -> ! { eprintln!("login: {msg}"); sleep(Duration::from_secs(5)); exit(1) }   // slow: init restarts us

/// bell, the background helper (alarms, timers, reminders), as the user: a double fork, so init adopts and reaps it.
/// login is single-threaded here, so fork in pre_exec is safe. A second copy for the same user exits at once (lock file).
fn bell(home: &str, user: &str) {
    let mut d = Command::new("/bin/bell");
    d.env_clear().env("PATH", "/bin").env("HOME", home).env("USER", user).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    unsafe { d.pre_exec(|| { if libc::fork() != 0 { libc::_exit(0); } libc::setsid(); Ok(()) }); }
    if let Ok(mut c) = d.spawn() { let _ = c.wait(); }
}

/// Never returns: becomes the user's shell.
fn start(u: &User, tz: Option<i32>) -> ! {
    let home = u.home();
    unsafe { libc::mkdir(format!("{home}\0").as_ptr() as *const _, 0o700); }   // EEXIST is fine
    if let Err(e) = chown(&home, Some(u.uid), Some(u.uid)) { die(&format!("{home}: {e}")); }
    unsafe { libc::umask(0o077); }
    if let Err(e) = user::drop_to(u.uid, u.uid) { die(&format!("cannot drop privileges: {e}")); }
    if std::env::set_current_dir(&home).is_err() { let _ = std::env::set_current_dir("/"); }
    if let Some(m) = tz { let _ = std::fs::write(format!("{home}/.tz"), format!("{}\n", clock::format_offset(m))); }   // umask 077 makes it 0600
    bell(&home, &u.name);
    let e = Command::new("/bin/nsh").env_clear().env("PATH", "/bin").env("HOME", &home).env("USER", &u.name).exec();
    die(&format!("/bin/nsh: {e}"))
}

fn load_or_die() -> Vec<User> {
    match user::load() {
        Ok(v) => v,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => vec![],
        Err(_) => die_slow("account file corrupt"),
    }
}

fn secret(prompt: &str) -> String { user::read_secret(prompt).unwrap_or_else(|_| exit(1)) }
fn line(prompt: &str) -> String { user::read_line(prompt).unwrap_or_else(|_| exit(1)) }

/// First boot: make the first user. None = another console did it first.
fn setup() -> Option<(User, Option<i32>)> {
    println!("thir: first boot: create the first user");
    loop {
        let name = line("username: ");
        if !user::valid_name(&name) { println!("invalid name: letters, digits, _ or -, max 32, starting with a letter or _"); continue; }
        let pw = secret("password: ");
        if pw != secret("repeat password: ") { println!("passwords do not match"); continue; }
        if pw.len() < user::MIN_PASSWORD { println!("password too short (min {})", user::MIN_PASSWORD); continue; }
        if !load_or_die().is_empty() { println!("a user was already created on another console"); return None; }
        let tz = loop {
            let def = clock::default_offset();
            let o = match def {
                Some(d) => line(&format!("utc offset [{}]: ", clock::format_offset(d))),
                None => line("utc offset (e.g. +05:30, Enter for UTC): "),
            };
            if o.is_empty() { break def }
            match clock::parse_offset(&o) { Some(m) => break Some(m), None => println!("use +HH:MM or -HH:MM") }
        };
        match user::create(&name, user::FIRST_UID, &pw) {
            Ok(u) => { if let Err(e) = user::save(std::slice::from_ref(&u)) { die(&format!("cannot save: {e}")); } println!("user {name} created"); return Some((u, tz)); }
            Err(e) => println!("{e}"),
        }
    }
}

fn main() {
    if unsafe { libc::getuid() } != 0 { die("must be run by init or den"); }
    unsafe {   // a shell that died in raw mode must not leave the tty raw
        let mut t: libc::termios = std::mem::zeroed();
        if libc::tcgetattr(0, &mut t) == 0 {
            t.c_lflag |= libc::ICANON | libc::ECHO | libc::ECHOE | libc::ISIG | libc::IEXTEN; t.c_iflag |= libc::ICRNL; t.c_oflag |= libc::OPOST | libc::ONLCR;
            libc::tcsetattr(0, libc::TCSANOW, &t);
        }
    }
    unsafe { for s in [libc::SIGINT, libc::SIGQUIT, libc::SIGTSTP] { libc::signal(s, libc::SIG_IGN); } }
    if !Path::new("/data/etc").exists() { die_slow("/data is not mounted"); }
    let args: Vec<String> = std::env::args().collect();
    if args.len() == 3 && args[1] == "--as" {
        let uid: u32 = args[2].parse().unwrap_or(0);
        match load_or_die().iter().find(|u| u.uid == uid && uid != 0) { Some(u) => start(u, None), None => exit(1) }
    }
    let mut users = load_or_die();
    if users.is_empty() { match setup() { Some((u, tz)) => start(&u, tz), None => users = load_or_die() } }
    for _ in 0..5 {
        let name = line("thir login: ");
        let pw = secret("password: ");
        match users.iter().find(|u| u.name == name) {
            Some(u) if u.verify(&pw) => start(u, None),
            Some(_) => {}
            None => user::dummy_verify(&pw),
        }
        println!("login incorrect");
        sleep(Duration::from_secs(3));
    }
    die("too many failures");
}
