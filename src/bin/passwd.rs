//! passwd: change your own password. Installed setuid root; trusts only the real uid and the tty.
use std::{process::exit, thread::sleep, time::Duration};
use thir::user;

fn die(msg: &str) -> ! { eprintln!("passwd: {msg}"); exit(1) }
fn secret(prompt: &str) -> String { user::read_secret(prompt).unwrap_or_else(|_| exit(1)) }

fn main() {
    // argv and env are ignored on purpose
    if (0..3).any(|fd| unsafe { libc::isatty(fd) } != 1) || unsafe { libc::geteuid() } != 0 { exit(1); }
    unsafe { for s in [libc::SIGINT, libc::SIGQUIT, libc::SIGTSTP] { libc::signal(s, libc::SIG_IGN); } }
    let uid = unsafe { libc::getuid() };
    let mut users = user::load().unwrap_or_else(|_| exit(1));
    let Some(i) = users.iter().position(|u| u.uid == uid) else { exit(1) };
    let mut ok = false;
    for _ in 0..3 {
        if users[i].verify(&secret("old password: ")) { ok = true; break; }
        eprintln!("passwd: wrong password");
        sleep(Duration::from_secs(3));
    }
    if !ok { exit(1); }
    let new = secret("new password: ");
    if new != secret("repeat new password: ") { die("passwords do not match"); }
    users[i] = user::create(&users[i].name, uid, &new).unwrap_or_else(|e| die(&e));
    if let Err(e) = user::save(&users) { die(&format!("cannot save: {e}")); }
    println!("passwd: password changed");
}
