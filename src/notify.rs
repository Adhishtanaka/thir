//! ~/.notify: the notices the background helper leaves for den's bar and nsh. Up to 5 lines `UNIX-TIME TEXT`, strictly increasing times,
//! text at most 80 printable characters. The helper writes it as the user; den (root) and nsh read it with files::read_small (1 KiB, no links, no FIFOs).
//! The text is cleaned when it is written and again when it is read, so a hand-made file cannot drive a terminal or the bar.
use crate::files::{printable, read_small, save};
use std::{fs::OpenOptions, io, os::{fd::AsRawFd, unix::fs::OpenOptionsExt}, path::Path};

pub const MAX_LINES: usize = 5;
pub const MAX_TEXT: usize = 80;
const MAX_FILE: usize = 1024;
const SHOW_SECS: i64 = 600;

/// One line of text: unsafe characters become '?', cut at 80 characters.
pub fn clean(t: &str) -> String { printable(t.trim()).chars().take(MAX_TEXT).collect() }

/// The lines that are valid: a number, a space, non-empty text, and a time later than the line before. Only the last 5 are kept.
pub fn parse(text: &str) -> Vec<(i64, String)> {
    let mut v: Vec<(i64, String)> = vec![];
    for l in text.lines() {
        let Some((t, msg)) = l.split_once(' ') else { continue };
        let (Ok(t), msg) = (t.parse::<i64>(), clean(msg)) else { continue };
        if msg.is_empty() || v.last().is_some_and(|p| p.0 >= t) { continue }
        v.push((t, msg));
    }
    v.split_off(v.len().saturating_sub(MAX_LINES))
}

/// The file text after adding `msg` at time `ts` (moved after the newest line when it is not later).
pub fn add(old: &str, ts: i64, msg: &str) -> String {
    let mut v = parse(old);
    let ts = v.last().map_or(ts, |l| ts.max(l.0 + 1));
    v.push((ts, clean(msg)));
    v.split_off(v.len().saturating_sub(MAX_LINES)).iter().map(|(t, m)| format!("{t} {m}\n")).collect()
}

pub fn read(path: &str, owner: u32) -> Vec<(i64, String)> { read_small(path, owner, MAX_FILE).map(|t| parse(&t)).unwrap_or_default() }

/// The newest notice that is at most 10 minutes old (not from the future).
pub fn latest(path: &str, owner: u32, now: i64) -> Option<String> { latest_at(path, owner, now).map(|l| l.1) }
/// The same, with the time it was left (den flashes a new one).
pub fn latest_at(path: &str, owner: u32, now: i64) -> Option<(i64, String)> {
    read(path, owner).into_iter().rev().find(|(t, _)| (0..=SHOW_SECS).contains(&(now - t)))
}

/// Adds a notice to `home`/.notify as this user. A lock file keeps two helpers (bell and the AI helper) from losing each other's line.
pub fn push(home: &str, ts: i64, msg: &str) -> io::Result<()> {
    let lock = OpenOptions::new().write(true).create(true).mode(0o600).custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC).open(Path::new(home).join(".config/thir/notify.lock"))?;
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) } != 0 { return Err(io::Error::last_os_error()) }   // released when `lock` closes
    let old = read_small(&format!("{home}/.notify"), unsafe { libc::getuid() }, MAX_FILE).unwrap_or_default();
    save(&Path::new(home).join(".notify"), add(&old, ts, msg).as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn parse_keeps_good_lines_only() {
        let v = parse("100 a\nx b\n99 older\n101\n102 \n103 c\n103 same\n");
        assert_eq!(v, [(100, "a".to_string()), (103, "c".to_string())]);
    }
    #[test] fn add_caps_at_five_and_times_increase() {
        let mut t = String::new();
        for i in 0..7 { t = add(&t, 100, &format!("m{i}")); }
        let v = parse(&t);
        assert_eq!(v.len(), 5); assert_eq!(v[0], (102, "m2".to_string())); assert_eq!(v[4].0, 106);
        assert!(v.windows(2).all(|w| w[0].0 < w[1].0));
    }
    #[test] fn text_is_cleaned_and_cut() {
        let t = add("", 5, "a\x1b[2Jb\u{202e}c");
        assert_eq!(t, "5 a?[2Jb?c\n");
        assert_eq!(parse("5 x\x1b[2Jy\n")[0].1, "x?[2Jy");
        assert_eq!(parse(&add("", 5, &"x".repeat(200)))[0].1.len(), MAX_TEXT);
        assert!(parse(&add("", 5, "a\nb")).len() == 1);   // a newline cannot start a second notice
    }
    #[test] fn push_keeps_every_line_under_the_lock() {
        let d = std::env::temp_dir().join(format!("thir-push-{}", std::process::id())); let _ = std::fs::remove_dir_all(&d); std::fs::create_dir_all(d.join(".config/thir")).unwrap();
        let h = d.to_str().unwrap();
        push(h, 100, "one").unwrap(); push(h, 100, "two").unwrap();
        assert_eq!(read(&format!("{h}/.notify"), unsafe { libc::getuid() }), [(100, "one".to_string()), (101, "two".to_string())]);
        assert_eq!(latest_at(&format!("{h}/.notify"), unsafe { libc::getuid() }, 105), Some((101, "two".to_string())));
        let _ = std::fs::remove_dir_all(d);
    }
    #[test] fn latest_is_recent_only() {
        let d = std::env::temp_dir().join(format!("thir-notify-{}", std::process::id())); let _ = std::fs::remove_dir_all(&d); std::fs::create_dir_all(&d).unwrap();
        let (f, me) = (d.join(".notify"), unsafe { libc::getuid() });
        std::fs::write(&f, "100 old\n900 new\n").unwrap();
        let p = f.to_str().unwrap();
        assert_eq!(latest(p, me, 950).as_deref(), Some("new")); assert_eq!(latest(p, me, 1600), None); assert_eq!(latest(p, me, 650).as_deref(), Some("old"));
        assert_eq!(latest(p, me + 1, 950), None);
        std::fs::write(&f, "1".repeat(2000)).unwrap(); assert_eq!(latest(p, me, 950), None);
        let _ = std::fs::remove_dir_all(d);
    }
}
