//! bell: the per-user background helper that rings alarms, timers and calendar reminders while no app is open, and `beep`.
//! login starts it (double fork, as the user). Its power is small on purpose: it reads the clock and calendar data files, writes ~/.notify and its lock,
//! and plays sound. No network, no programs started, PR_SET_NO_NEW_PRIVS, and a 64 KiB file size limit. One copy per user (flock).
use super::{alarm, cal};
use crate::{clock, notify, procinfo::parse_status, sound, store};
use std::{fs::{self, OpenOptions}, os::{fd::AsRawFd, unix::fs::OpenOptionsExt}, thread::sleep, time::Duration};

const TICK: Duration = Duration::from_secs(2);
const CATCH_UP: i64 = 90;   // a login within this many seconds after a ring still shows it

/// What rings in (last, now]: the notice texts, each with the time it came due.
pub fn fired(alarms: &[alarm::Alarm], events: &[cal::Event], last: i64, now: i64, off: i32) -> Vec<String> {
    let mut v = vec![];
    for a in alarms.iter().filter(|a| a.on && alarm::due(a.when, last, now, off)) {
        let (t, _) = alarm::describe(a.when, now, off);
        v.push(if alarm::timer_secs(a).is_some() { "Timer finished".to_string() } else if a.label.is_empty() { format!("Alarm {t}") } else { format!("Alarm {t} {}", a.label) });
    }
    for e in events { if cal::remind_at(e, off).map_or(false, |t| last < t && t <= now) { v.push(cal::notice(e)); } }
    v
}

fn alive(me: u32) -> bool {
    let Ok(rd) = fs::read_dir("/proc") else { return true };   // cannot tell: stay
    rd.flatten().any(|e| {
        let p = e.path();
        e.file_name().to_string_lossy().bytes().all(|b| b.is_ascii_digit()) && fs::read_to_string(p.join("comm")).is_ok_and(|c| c.trim() == "nsh")
            && fs::read_to_string(p.join("status")).is_ok_and(|s| parse_status(&s).0 == me)
    })
}

/// Rings three times when sound is on and a device answers; false when it stayed silent (the bar notice is the alert then).
fn ring() -> bool {
    let (on, vol) = sound::config(&store::read_settings());
    if !on { return false }
    let c = sound::chime(vol);
    (0..3).all(|_| sound::play(&c).is_ok())
}

pub fn run() -> i32 {
    let me = unsafe { libc::getuid() };
    let home = store::home();
    let Ok(dir) = store::dir("") else { eprintln!("bell: cannot make ~/.config/thir"); return 1 };
    let Ok(lock) = OpenOptions::new().write(true).create(true).mode(0o600).custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC).open(dir.join("bell.lock")) else { eprintln!("bell: cannot open the lock file"); return 1 };
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 { return 0 }   // already running
    if unsafe { libc::isatty(1) } == 1 { eprintln!("bell: starts by itself when you log in; try `beep` to hear the chime"); return 1 }
    unsafe {
        libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0);
        let l = libc::rlimit { rlim_cur: 64 << 10, rlim_max: 64 << 10 };
        libc::setrlimit(libc::RLIMIT_FSIZE, &l);
        for s in [libc::SIGXFSZ, libc::SIGHUP] { libc::signal(s, libc::SIG_IGN); }
    }
    let (mut last, mut misses, mut ticks) = (clock::now() - CATCH_UP, 0u32, 0u32);
    loop {
        let (now, off) = (clock::now(), clock::home_tz().unwrap_or(0));
        let f = fired(&alarm::load(), &cal::load(), last, now, off);
        last = now;
        for t in &f { let _ = notify::push(&home, now, t); }
        if !f.is_empty() && !ring() { let _ = notify::push(&home, now + 1, "(no sound: check settings)"); }
        sleep(TICK);
        ticks += 1;
        if ticks % 30 == 0 { misses = if alive(me) { 0 } else { misses + 1 }; if misses >= 2 { return 0; } }
    }
}

/// `beep` plays the alarm chime; `beep HZ MS` one note.
pub fn beep(args: &[String]) -> i32 {
    let (on, vol) = sound::config(&store::read_settings());
    if !on { eprintln!("beep: sound is off (settings)"); return 1 }
    let pcm = match args { [] => sound::chime(vol), [hz, ms] => match (hz.parse::<f32>(), ms.parse::<u32>()) {
        (Ok(h), Ok(m)) if (20.0..=20000.0).contains(&h) && (1..=5000).contains(&m) => sound::tone(h, m, vol as f32 / 100.0), _ => { eprintln!("usage: beep [HZ(20-20000) MS(1-5000)]"); return 2 } },
        _ => { eprintln!("usage: beep [HZ MS]"); return 2 } };
    match sound::play(&pcm) { Ok(()) => 0, Err(e) => { eprintln!("beep: no sound ({e})"); 1 } }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alarm::{Alarm, When};
    const OFF: i32 = 330; const NOW: i64 = 1794906125;
    #[test] fn alarms_timers_and_events_fire_once_each() {
        let al = vec![Alarm { id: 1, when: When::At(NOW), on: true, label: "tea".into() }, Alarm { id: 2, when: When::At(NOW), on: false, label: "off".into() },
            Alarm { id: 3, when: When::At(NOW), on: true, label: "Timer 60".into() }, Alarm { id: 4, when: When::At(NOW + 100), on: true, label: "later".into() }];
        let ev = vec![cal::Event { id: 1, date: (2026, 11, 17), time: Some((14, 47)), remind: 15, title: "Dentist".into() }];
        assert_eq!(fired(&al, &ev, NOW - 6, NOW, OFF), ["Alarm 14:32 tea", "Timer finished", "Soon 14:47 Dentist"]);   // 14:47 - 15 min = 14:32:00 (before NOW), checked next
        assert!(fired(&al, &ev, NOW, NOW + 5, OFF).is_empty());
    }
}
