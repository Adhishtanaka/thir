//! Reminders: `!remind`, `!schedule`, and the background helper (`agent daemon`) that fires them. Alarms, timers and calendar reminders are `bell`'s (src/apps/bell.rs); both leave notices through notify::push.
//! The helper is started by login with a double fork, as the user. Its power is small on purpose: it reads the schedule file and the apps' last_error
//! times, and writes ~/.notify and its lock. It never reads a key or the provider config, never touches the network or the AI, and never starts a
//! program (this file and route.rs spawn nothing). PR_SET_NO_NEW_PRIVS and a 64 KiB file size limit are set before the first tick.
use crate::{agent::{private_dir, read_private, write_private}, make::{self, note_line}, memory, progress::{self, fit, CYAN, DIM, GOOD, R, WARN}};
use std::{collections::BTreeMap, fs::{self, OpenOptions}, os::{fd::AsRawFd, unix::fs::{MetadataExt, OpenOptionsExt}}, path::{Path, PathBuf}, thread::sleep, time::Duration};
use thir::{clock, files::{app_name_ok, printable}, notify, procinfo::parse_status};

const MAX_ENTRIES: usize = 50;
const MAX_TEXT: usize = 80;
const MAX_FILE: u64 = 8192;
const MAX_AHEAD: i64 = 30 * 86400;
const KEEP_PAST: i64 = 86400;
const TICK: Duration = Duration::from_secs(5);
const SLOW_EVERY: u32 = 12;   // every 12th tick (60 s): crash notices and the "is a shell still open" check
const HEADER: &str = "# thir reminders: ID WHEN TEXT; WHEN is a unix time (once) or daily HH:MM (local time)\n";
const TIME_HELP: &str = "agent: TIME is HH:MM (24-hour), daily HH:MM or +N with s, m or h, e.g. +10m";

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum When { At(i64), Daily(u32, u32) }
#[derive(Clone, PartialEq, Debug)]
pub struct Entry { pub id: u32, pub when: When, pub text: String }

fn hhmm(s: &str) -> Option<(u32, u32)> {
    let (h, m) = s.split_once(':')?;
    if !(1..=2).contains(&h.len()) || m.len() != 2 || !h.bytes().chain(m.bytes()).all(|b| b.is_ascii_digit()) { return None }
    let (h, m) = (h.parse().ok()?, m.parse().ok()?);
    (h < 24 && m < 60).then_some((h, m))
}

/// `+10m`: seconds from now, 1 s to 30 days.
fn plus(s: &str) -> Option<i64> {
    let r = s.strip_prefix('+')?;
    let (n, u) = r.split_at(r.len().checked_sub(1)?);
    if n.is_empty() || n.len() > 8 || !n.bytes().all(|b| b.is_ascii_digit()) { return None }
    let secs = n.parse::<i64>().ok()? * match u { "s" => 1, "m" => 60, "h" => 3600, _ => return None };
    (1..=MAX_AHEAD).contains(&secs).then_some(secs)
}

fn day_secs(t: i64, off: i32) -> i64 { (t + off as i64 * 60).rem_euclid(86400) }

/// The time words at the start of `w` and how many words they used: `daily HH:MM`, `+N{s|m|h}`, or `HH:MM` (the next time it comes).
pub fn parse_when(w: &[&str], now: i64, off: i32) -> Result<(When, usize), String> {
    let bad = || TIME_HELP.to_string();
    match w {
        ["daily", t, ..] => hhmm(t).map(|(h, m)| (When::Daily(h, m), 2)).ok_or_else(bad),
        [t, ..] if t.starts_with('+') => plus(t).map(|s| (When::At(now + s), 1)).ok_or_else(bad),
        [t, ..] => hhmm(t).map(|(h, m)| {
            let at = now - day_secs(now, off) + (h * 3600 + m * 60) as i64;
            (When::At(if at <= now { at + 86400 } else { at }), 1)
        }).ok_or_else(bad),
        [] => Err(bad()),
    }
}

/// `last < time <= now`: each time is seen once. A daily reminder checks today's and yesterday's local time, so a tick across midnight does not miss it.
pub fn due(w: When, last: i64, now: i64, off: i32) -> bool {
    match w {
        When::At(t) => last < t && t <= now,
        When::Daily(h, m) => { let day0 = now - day_secs(now, off); [0, -86400].iter().any(|d| { let t = day0 + d + (h * 3600 + m * 60) as i64; last < t && t <= now }) }
    }
}

fn day_text(t: i64, now: i64, off: i32) -> String {
    let (l, days) = (clock::local(t, off), (t + off as i64 * 60).div_euclid(86400) - (now + off as i64 * 60).div_euclid(86400));
    let hm = format!("{:02}:{:02}", l.hour, l.min);
    match days { 0 => format!("today {hm}"), 1 => format!("tomorrow {hm}"), -1 => format!("yesterday {hm}"), _ => format!("{} {} {} {hm}", clock::WDAY[l.wday as usize], l.day, clock::MONTH[l.month as usize - 1]) }
}

pub fn describe(w: When, now: i64, off: i32) -> String {
    match w {
        When::Daily(h, m) => format!("daily {h:02}:{m:02}"),
        When::At(t) if t < now => format!("missed ({})", day_text(t, now, off)),
        When::At(t) if t - now < 120 => format!("in {} s", t - now),
        When::At(t) if t - now < 3600 => format!("in {} min", (t - now) / 60),
        When::At(t) => day_text(t, now, off),
    }
}

// ---- the file ----

fn path(home: &Path) -> PathBuf { home.join(".config/thir/schedule") }

pub fn parse_file(text: &str) -> Vec<Entry> {
    text.lines().filter_map(|l| {
        let mut w = l.splitn(3, ' ');
        let id = w.next()?.parse().ok()?;
        let (a, rest) = (w.next()?, w.next()?);
        let (when, text) = if a == "daily" { let (t, x) = rest.split_once(' ')?; let (h, m) = hhmm(t)?; (When::Daily(h, m), x) } else { (When::At(a.parse().ok()?), rest) };
        let text: String = printable(text.trim()).chars().take(MAX_TEXT).collect();
        (!text.is_empty()).then_some(Entry { id, when, text })
    }).take(MAX_ENTRIES).collect()
}

pub fn format_file(v: &[Entry]) -> String {
    v.iter().fold(HEADER.to_string(), |o, e| o + &match e.when {
        When::At(t) => format!("{} {t} {}\n", e.id, e.text),
        When::Daily(h, m) => format!("{} daily {h:02}:{m:02} {}\n", e.id, e.text),
    })
}

pub fn load(home: &Path) -> Result<Vec<Entry>, String> {
    let p = path(home);
    read_private(&p, unsafe { libc::getuid() }, MAX_FILE).map(|t| t.map(|t| parse_file(&t)).unwrap_or_default()).map_err(|w| format!("agent: refusing to read {}: it {w}. Delete it (rm), then try again.", p.display()))
}

fn save(home: &Path, v: &[Entry]) -> Result<(), String> {
    write_private(&private_dir(home, &[])?.join("schedule"), &format_file(v)).map_err(|e| format!("agent: cannot save the reminders: {e}"))
}

/// The text a reminder may have: printable, 1 to 80 characters, nothing that looks like a password or key.
pub fn clean_text(t: &str) -> Result<String, String> {
    let t = printable(t.trim());
    if t.is_empty() { return Err("agent: the reminder has no text".into()); }
    if t.chars().count() > MAX_TEXT { return Err(format!("agent: the reminder text is longer than {MAX_TEXT} characters")); }
    if memory::looks_secret(&t) { return Err("agent: that looks like a password or key; reminders never keep those".into()); }
    Ok(t)
}

/// Adds a reminder and returns its number. Once-only reminders more than a day old are dropped first.
pub fn add(home: &Path, when: When, text: &str, now: i64) -> Result<u32, String> {
    let text = clean_text(text)?;
    let mut v = load(home)?;
    v.retain(|e| !matches!(e.when, When::At(t) if t < now - KEEP_PAST));
    if v.len() >= MAX_ENTRIES { return Err(format!("agent: you have {MAX_ENTRIES} reminders. Remove one first: type ! remove reminder N")); }
    let id = v.iter().map(|e| e.id).max().unwrap_or(0) + 1;
    v.push(Entry { id, when, text });
    save(home, &v).map(|_| id)
}

/// For the system summary: `none`, or the count and the next one to come.
pub fn summary(v: &[Entry], now: i64, off: i32) -> String {
    let next = v.iter().filter_map(|e| {
        let t = match e.when { When::At(t) => t, When::Daily(h, m) => { let at = now - day_secs(now, off) + (h * 3600 + m * 60) as i64; if at < now { at + 86400 } else { at } } };
        (t >= now).then_some((t, &e.text))
    }).min();
    match next { None if v.is_empty() => "none".into(), None => format!("{}, none due", v.len()), Some((t, x)) => format!("{}, next {} {}", v.len(), describe(When::At(t), now, off), x) }
}

// ---- commands ----

fn list(home: &Path) -> i32 {
    let v = match load(home) { Ok(v) => v, Err(m) => { eprintln!("{m}"); return 1 } };
    if v.is_empty() { println!("no reminders. Try: ! remind me at 17:00 to call mum"); return 0; }
    let (now, off) = (clock::now(), clock::home_tz().unwrap_or(0));
    let tty = unsafe { libc::isatty(1) } == 1;
    if !tty { for e in &v { println!("  {:<2} {:<20} {}", e.id, describe(e.when, now, off), e.text); } return 0; }
    let w = progress::tw();
    let n = format!("{} reminder{}", v.len(), if v.len() == 1 { "" } else { "s" });
    println!("\n{}\n  {n}\n\n{}\n{}", progress::hdr("REMINDERS"), progress::hdr("#   WHEN                  TEXT"), progress::rule(w));
    for e in &v {
        let (d, c) = (describe(e.when, now, off), match e.when { When::Daily(..) => CYAN, When::At(t) if t < now => WARN, _ => GOOD });
        println!("  {CYAN}{:<2}{R}  {c}{d:<20}{R}  {}", e.id, fit(&e.text, w.saturating_sub(28)));
    }
    println!("{}\n{}\n  {CYAN}! remove reminder N{R} {DIM}removes one{R}", progress::rule(w), progress::dots(&[n]));
    0
}

pub fn command(c: &str, args: &[String]) -> i32 {
    if c == "daemon" { return daemon(); }
    let home = match make::home() { Ok(h) => h, Err(m) => { eprintln!("{m}"); return 1 } };
    let (now, off) = (clock::now(), clock::home_tz().unwrap_or(0));
    match (c, args) {
        ("schedule", []) => list(&home),
        ("schedule", [a]) if a == "list" => list(&home),
        ("schedule", [a, n]) if a == "rm" => {
            let mut v = match load(&home) { Ok(v) => v, Err(m) => { eprintln!("{m}"); return 1 } };
            let Some(i) = n.parse::<u32>().ok().and_then(|n| v.iter().position(|e| e.id == n)) else { eprintln!("agent: no reminder {} (!schedule lists them)", printable(n)); return 1 };
            let e = v.remove(i);
            if let Err(m) = save(&home, &v) { eprintln!("{m}"); return 1; }
            println!("{}", note_line(&format!("removed reminder {}", e.id)));
            0
        }
        ("schedule", _) => { eprintln!("usage: !schedule [list] | !schedule rm N"); 2 }
        _ => {
            let w: Vec<&str> = args.iter().map(String::as_str).collect();
            if w.is_empty() { eprintln!("usage: !remind [daily] TIME \"TEXT\""); return 2; }
            let (when, n) = match parse_when(&w, now, off) { Ok(r) => r, Err(m) => { eprintln!("{m}"); return 2 } };
            if w.len() == n { eprintln!("usage: !remind [daily] TIME \"TEXT\""); return 2; }
            let text = w[n..].join(" ");
            match add(&home, when, &text, now) {
                Ok(id) => { println!("{}", note_line(&format!("reminder {id}: {} \u{b7} {}", describe(when, now, off), printable(text.trim())))); 0 }
                Err(m) => { eprintln!("{m}"); 1 }
            }
        }
    }
}

// ---- the helper ----

/// The texts of the reminders that come due in (last, now].
pub fn fired(v: &[Entry], last: i64, now: i64, off: i32) -> Vec<String> { v.iter().filter(|e| due(e.when, last, now, off)).map(|e| e.text.clone()).collect() }

fn crash_stamps(home: &Path) -> BTreeMap<String, (i64, i64)> {
    let Ok(rd) = fs::read_dir(home.join("apps")) else { return BTreeMap::new() };
    rd.flatten().filter_map(|e| e.file_name().into_string().ok()).filter(|n| app_name_ok(n)).filter_map(|n| {
        let m = fs::symlink_metadata(home.join("apps").join(&n).join("last_error")).ok().filter(|m| m.is_file())?;
        Some((n, (m.mtime(), m.mtime_nsec())))
    }).collect()
}

/// A shell (nsh) of this user is still running.
fn alive(me: u32) -> bool {
    let Ok(rd) = fs::read_dir("/proc") else { return true };   // cannot tell: stay
    rd.flatten().any(|e| {
        let p = e.path();
        e.file_name().to_string_lossy().bytes().all(|b| b.is_ascii_digit())
            && fs::read_to_string(p.join("comm")).is_ok_and(|c| c.trim() == "nsh")
            && fs::read_to_string(p.join("status")).is_ok_and(|s| parse_status(&s).0 == me)
    })
}

fn notice(home: &Path, ts: i64, msg: &str) { if let Some(h) = home.to_str() { let _ = notify::push(h, ts, msg); } }   // under the lock `bell` shares

/// One helper per user: the lock is `~/.config/thir/agentd.lock`.
fn daemon() -> i32 {
    let home = match make::home() { Ok(h) => h, Err(m) => { eprintln!("{m}"); return 1 } };
    let dir = match private_dir(&home, &[]) { Ok(d) => d, Err(m) => { eprintln!("{m}"); return 1 } };
    let lock = match OpenOptions::new().write(true).create(true).mode(0o600).custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC).open(dir.join("agentd.lock")) { Ok(f) => f, Err(e) => { eprintln!("agent: cannot open the helper lock: {e}"); return 1 } };
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        let e = std::io::Error::last_os_error();
        if e.raw_os_error() == Some(libc::EWOULDBLOCK) { eprintln!("agent: the helper is already running"); return 0; }
        eprintln!("agent: cannot lock the helper file: {e}"); return 1;
    }
    if make::tty() { eprintln!("agent: the helper starts by itself when you log in"); return 1; }
    unsafe {
        libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0);
        let l = libc::rlimit { rlim_cur: 64 << 10, rlim_max: 64 << 10 };
        libc::setrlimit(libc::RLIMIT_FSIZE, &l);
        for s in [libc::SIGXFSZ, libc::SIGHUP] { libc::signal(s, libc::SIG_IGN); }   // a write over the limit fails instead of killing the helper
    }
    let me = unsafe { libc::getuid() };
    let (mut last, mut seen, mut ticks, mut misses) = (clock::now(), crash_stamps(&home), 0u32, 0u32);
    loop {
        sleep(TICK);
        let (now, off) = (clock::now(), clock::home_tz().unwrap_or(0));
        if let Ok(v) = load(&home) { for t in fired(&v, last, now, off) { notice(&home, now, &t); } }
        last = now;
        ticks += 1;
        if ticks % SLOW_EVERY == 0 {
            let cur = crash_stamps(&home);
            for (n, s) in &cur { if seen.get(n) != Some(s) { notice(&home, now, &format!("{n} crashed \u{b7} type ! fix {n}")); } }
            seen = cur;
            misses = if alive(me) { 0 } else { misses + 1 };
            if misses >= 2 { return 0; }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const OFF: i32 = 330;   // +05:30
    // 2026-11-17 (Tue) 14:32:05 +05:30 = 09:02:05 UTC
    const NOW: i64 = 1794906125;
    fn w(s: &str) -> Vec<&str> { s.split(' ').collect() }

    #[test] fn when_words() {
        assert_eq!(parse_when(&w("+5s tea"), NOW, OFF), Ok((When::At(NOW + 5), 1)));
        assert_eq!(parse_when(&w("+10m x"), NOW, OFF), Ok((When::At(NOW + 600), 1)));
        assert_eq!(parse_when(&w("+2h x"), NOW, OFF), Ok((When::At(NOW + 7200), 1)));
        assert_eq!(parse_when(&w("daily 07:00 open weather"), NOW, OFF), Ok((When::Daily(7, 0), 2)));
        assert_eq!(parse_when(&w("17:00 call"), NOW, OFF), Ok((When::At(NOW - 14 * 3600 - 32 * 60 - 5 + 17 * 3600), 1)));   // later today
        assert_eq!(parse_when(&w("07:00 call"), NOW, OFF), Ok((When::At(NOW - 14 * 3600 - 32 * 60 - 5 + 31 * 3600), 1)));   // tomorrow
        for bad in ["25:00", "7:5", "+0s", "+5", "+5d", "+x5m", "+99999h", "daily", "daily 25:00", "noon", "", "17:60", "+-5m"] { assert!(parse_when(&w(bad), NOW, OFF).is_err(), "{bad}"); }
        assert!(parse_when(&w("25:00 x"), NOW, OFF).unwrap_err().contains("TIME is HH:MM"));
    }
    #[test] fn once_fires_once() {
        let t = When::At(1000);
        assert!(!due(t, 998, 999, 0) && due(t, 999, 1000, 0) && !due(t, 1000, 1005, 0) && due(t, 0, 1005, 0));
    }
    #[test] fn daily_crosses_midnight_and_never_fires_twice() {
        // 00:00 +05:30 on 2026-11-18 is 18:30 UTC on 11-17 = 1794906125 - 09:02:05 + 18:30 = 1794940200
        let mid = NOW - (9 * 3600 + 2 * 60 + 5) + (18 * 3600 + 30 * 60);
        let d = When::Daily(0, 0);
        assert!(due(d, mid - 3, mid + 2, OFF) && !due(d, mid + 2, mid + 7, OFF) && !due(d, mid - 8, mid - 3, OFF));
        let d = When::Daily(23, 59);   // a tick that starts before midnight and ends after it still sees 23:59 of the day before
        assert!(due(d, mid - 70, mid + 3, OFF) && !due(d, mid + 3, mid + 8, OFF));
        let mut n = 0; let mut last = mid - 100; for i in 0..40 { let now = mid - 100 + i * 5 + 5; if due(When::Daily(0, 0), last, now, OFF) { n += 1; } last = now; }
        assert_eq!(n, 1);
        assert!(!due(When::Daily(7, 0), NOW - 5, NOW, OFF));
    }
    #[test] fn describing() {
        assert_eq!(describe(When::At(NOW + 5), NOW, OFF), "in 5 s");
        assert_eq!(describe(When::At(NOW + 119), NOW, OFF), "in 119 s");
        assert_eq!(describe(When::At(NOW + 600), NOW, OFF), "in 10 min");
        assert_eq!(describe(When::At(NOW + 3 * 3600), NOW, OFF), "today 17:32");
        assert_eq!(describe(When::At(NOW + 17 * 3600), NOW, OFF), "tomorrow 07:32");
        assert_eq!(describe(When::At(NOW + 3 * 86400), NOW, OFF), "Fri 20 Nov 14:32");
        assert_eq!(describe(When::Daily(7, 0), NOW, OFF), "daily 07:00");
        assert_eq!(describe(When::At(NOW - 3600), NOW, OFF), "missed (today 13:32)");
    }
    fn tmp(n: &str) -> PathBuf { let d = std::env::temp_dir().join(format!("thir-sched-{}-{n}", std::process::id())); let _ = fs::remove_dir_all(&d); fs::create_dir_all(&d).unwrap(); d }
    #[test] fn file_round_trip_and_limits() {
        let h = tmp("rt");
        assert_eq!(load(&h), Ok(vec![]));
        assert_eq!(add(&h, When::At(NOW + 5), "call mum", NOW), Ok(1));
        assert_eq!(add(&h, When::Daily(7, 0), "open weather", NOW), Ok(2));
        let v = load(&h).unwrap();
        assert_eq!(v, [Entry { id: 1, when: When::At(NOW + 5), text: "call mum".into() }, Entry { id: 2, when: When::Daily(7, 0), text: "open weather".into() }]);
        assert_eq!(fs::metadata(path(&h)).unwrap().mode() & 0o777, 0o600);
        assert!(fs::read_to_string(path(&h)).unwrap().contains("\n2 daily 07:00 open weather\n"));
        assert_eq!(add(&h, When::At(NOW + 9 * 86400), "later", NOW + 3 * 86400), Ok(3));   // entry 1 is over a day old now: pruned
        assert_eq!(load(&h).unwrap().iter().map(|e| e.id).collect::<Vec<_>>(), [2, 3]);
        assert!(add(&h, When::At(1), "x\u{0}y".repeat(50).as_str(), NOW).is_err());
        let _ = fs::remove_dir_all(h);
    }
    #[test] fn secrets_and_bad_text_refused() {
        let h = tmp("sec");
        assert!(add(&h, When::At(NOW), "my password is hunter2", NOW).unwrap_err().contains("looks like a password"));
        assert!(add(&h, When::At(NOW), "sk-ant-thir-smoke-not-a-key", NOW).is_err());
        assert!(add(&h, When::At(NOW), "   ", NOW).is_err());
        assert!(add(&h, When::At(NOW), &"x".repeat(81), NOW).is_err());
        assert_eq!(clean_text("a\x1b[2Jb").unwrap(), "a?[2Jb");
        assert_eq!(load(&h), Ok(vec![]));
        let _ = fs::remove_dir_all(h);
    }
    #[test] fn file_refused_when_open_to_others_and_capped() {
        let h = tmp("perm"); add(&h, When::At(NOW), "a", NOW).unwrap();
        fs::set_permissions(path(&h), std::os::unix::fs::PermissionsExt::from_mode(0o644)).unwrap();
        assert!(load(&h).unwrap_err().contains("readable by others"));
        let big = (1..60).map(|i| format!("{i} {} t\n", NOW)).collect::<String>();
        assert_eq!(parse_file(&big).len(), MAX_ENTRIES);
        assert_eq!(parse_file("1 bad\nx 5 y\n3 daily 25:00 z\n4 daily 07:00 ok\n").len(), 1);
        let _ = fs::remove_dir_all(h);
    }
    #[test] fn summary_line() {
        assert_eq!(summary(&[], NOW, OFF), "none");
        let v = [Entry { id: 1, when: When::At(NOW + 600), text: "tea".into() }, Entry { id: 2, when: When::Daily(23, 0), text: "bed".into() }, Entry { id: 3, when: When::At(NOW - 9), text: "old".into() }];
        assert_eq!(summary(&v, NOW, OFF), "3, next in 10 min tea");
        assert_eq!(summary(&v[2..], NOW, OFF), "1, none due");
    }
    #[test] fn fired_texts() {
        let v = [Entry { id: 1, when: When::At(NOW), text: "a".into() }, Entry { id: 2, when: When::At(NOW + 10), text: "b".into() }];
        assert_eq!(fired(&v, NOW - 5, NOW, OFF), ["a"]); assert!(fired(&v, NOW, NOW + 5, OFF).is_empty());
    }
}
