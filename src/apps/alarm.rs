//! clock: big time, stopwatch, countdown timer and alarms. Alarms and timers live in ~/.config/thir/clock/alarms and are rung by `bell`, even when this app is closed.
//! Line: ID TAB WHEN TAB ON TAB LABEL. WHEN is `@UNIX` (once), `HH:MM` (every day) or `HH:MM/135` (weekdays, 0 = Sunday). A timer is a once alarm labelled `Timer SECONDS`.
use crate::{clock, line::Key, store, tui::*};
use std::time::Instant;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum When { At(i64), Daily(u32, u32, u8) }
#[derive(Clone, PartialEq, Debug)]
pub struct Alarm { pub id: u32, pub when: When, pub on: bool, pub label: String }
pub const ALL_DAYS: u8 = 127;
const MAX_ALARMS: usize = 100;

pub fn hhmm(s: &str) -> Option<(u32, u32)> {
    let (h, m) = s.trim().split_once(':')?;
    if !(1..=2).contains(&h.len()) || m.len() != 2 || !h.bytes().chain(m.bytes()).all(|b| b.is_ascii_digit()) { return None }
    let (h, m) = (h.parse().ok()?, m.parse().ok()?);
    (h < 24 && m < 60).then_some((h, m))
}
fn secs_of_day(t: i64, off: i32) -> i64 { (t + off as i64 * 60).rem_euclid(86400) }

pub fn parse(text: &str) -> Vec<Alarm> {
    text.lines().filter_map(|l| {
        let f = store::split(l, 4)?;
        let when = if let Some(u) = f[1].strip_prefix('@') { When::At(u.parse().ok()?) } else {
            let (t, d) = f[1].split_once('/').unwrap_or((&f[1], "1234560"));
            let mask = d.bytes().try_fold(0u8, |m, b| (b'0'..=b'6').contains(&b).then(|| m | 1 << (b - b'0')))?;
            let (h, m) = hhmm(t)?; if mask == 0 { return None }
            When::Daily(h, m, mask)
        };
        Some(Alarm { id: f[0].parse().ok()?, when, on: f[2] == "1", label: crate::files::printable(&f[3]).chars().take(60).collect() })
    }).take(MAX_ALARMS).collect()
}
pub fn format(v: &[Alarm]) -> String {
    v.iter().map(|a| {
        let w = match a.when { When::At(t) => format!("@{t}"), When::Daily(h, m, d) if d == ALL_DAYS => format!("{h:02}:{m:02}"),
            When::Daily(h, m, d) => format!("{h:02}:{m:02}/{}", (0..7).filter(|i| d >> i & 1 == 1).map(|i| i.to_string()).collect::<String>()) };
        store::join(&[&a.id.to_string(), &w, if a.on { "1" } else { "0" }, &a.label]) + "\n"
    }).collect()
}
pub fn load() -> Vec<Alarm> { parse(&store::read("clock", "alarms")) }
pub fn save(v: &[Alarm]) { let _ = store::write("clock", "alarms", &format(v)); }

/// The next time HH:MM comes (today when still ahead, else tomorrow), as a unix time.
pub fn next_at(h: u32, m: u32, now: i64, off: i32) -> i64 { let at = now - secs_of_day(now, off) + (h * 3600 + m * 60) as i64; if at <= now { at + 86400 } else { at } }

/// Does the alarm come due in (last, now]? A daily alarm looks at today and yesterday, so a tick across midnight misses nothing.
pub fn due(w: When, last: i64, now: i64, off: i32) -> bool {
    match w {
        When::At(t) => last < t && t <= now,
        When::Daily(h, m, days) => { let day0 = now - secs_of_day(now, off); [0, -86400].iter().any(|d| { let t = day0 + d + (h * 3600 + m * 60) as i64; last < t && t <= now && days >> clock::local(t, off).wday & 1 == 1 }) }
    }
}
/// The next time the alarm rings after `now`, if it ever does.
pub fn next_ring(w: When, now: i64, off: i32) -> Option<i64> {
    match w {
        When::At(t) => (t > now).then_some(t),
        When::Daily(h, m, days) => (0..8).map(|d| now - secs_of_day(now, off) + d * 86400 + (h * 3600 + m * 60) as i64).find(|&t| t > now && days >> clock::local(t, off).wday & 1 == 1),
    }
}

/// `mon,wed`, `weekdays`, `daily`, `weekends` -> day mask; "" and `once` are not days (None).
pub fn parse_days(s: &str) -> Option<u8> {
    let s = s.trim().to_lowercase();
    match s.as_str() { "daily" | "every day" | "all" => return Some(ALL_DAYS), "weekdays" => return Some(0b0111110), "weekends" => return Some(0b1000001), _ => {} }
    let mut m = 0u8;
    for w in s.split([',', ' ']).filter(|w| !w.is_empty()) { m |= 1 << clock::WDAY.iter().position(|d| d.to_lowercase() == w.get(..3).unwrap_or(w))?; }
    (m != 0).then_some(m)
}
pub fn days_text(d: u8) -> String {
    match d { ALL_DAYS => "daily".into(), 0b0111110 => "weekdays".into(), 0b1000001 => "weekends".into(),
        _ => (0..7).filter(|i| d >> i & 1 == 1).map(|i| clock::WDAY[i]).collect::<Vec<_>>().join(" ") }
}
/// `5m`, `1h30m`, `90s`, `45` (minutes): seconds, 1 s to 24 h.
pub fn parse_duration(s: &str) -> Option<i64> {
    let (mut total, mut num, s) = (0i64, String::new(), s.trim().to_lowercase());
    for c in s.chars() {
        match c { '0'..='9' => num.push(c), 'h' | 'm' | 's' if !num.is_empty() => { total += num.parse::<i64>().ok()? * match c { 'h' => 3600, 'm' => 60, _ => 1 }; num.clear(); } ' ' => {} _ => return None }
    }
    if !num.is_empty() { total += num.parse::<i64>().ok()? * 60; }
    (1..=86400).contains(&total).then_some(total)
}
pub fn dur_text(s: i64) -> String { let (h, m, x) = (s / 3600, s / 60 % 60, s % 60); if h > 0 { format!("{h}:{m:02}:{x:02}") } else { format!("{m:02}:{x:02}") } }
/// A timer alarm: label `Timer SECONDS`, rings once.
pub fn timer_secs(a: &Alarm) -> Option<i64> { a.label.strip_prefix("Timer ")?.parse().ok() }

fn day_text(t: i64, now: i64, off: i32) -> String {
    let (l, days) = (clock::local(t, off), (t + off as i64 * 60).div_euclid(86400) - (now + off as i64 * 60).div_euclid(86400));
    let hm = format!("{:02}:{:02}", l.hour, l.min);
    match days { 0 => format!("today {hm}"), 1 => format!("tomorrow {hm}"), _ => format!("{} {} {} {hm}", clock::WDAY[l.wday as usize], l.day, clock::MONTH[l.month as usize - 1]) }
}
/// The REPEAT column and the time it shows.
pub fn describe(w: When, now: i64, off: i32) -> (String, String) {
    match w { When::At(t) => { let l = clock::local(t, off); (format!("{:02}:{:02}", l.hour, l.min), if t < now { format!("done ({})", day_text(t, now, off)) } else { format!("once, {}", day_text(t, now, off)) }) }
        When::Daily(h, m, d) => (format!("{h:02}:{m:02}"), days_text(d)) }
}

// ---- big digits ----

const GLYPHS: [(char, [&str; 5]); 12] = [
    ('0', ["###", "#.#", "#.#", "#.#", "###"]), ('1', [".#.", "##.", ".#.", ".#.", "###"]), ('2', ["###", "..#", "###", "#..", "###"]), ('3', ["###", "..#", "###", "..#", "###"]),
    ('4', ["#.#", "#.#", "###", "..#", "..#"]), ('5', ["###", "#..", "###", "..#", "###"]), ('6', ["###", "#..", "###", "#.#", "###"]), ('7', ["###", "..#", "..#", "..#", "..#"]),
    ('8', ["###", "#.#", "###", "#.#", "###"]), ('9', ["###", "#.#", "###", "..#", "###"]), (':', [".", "#", ".", "#", "."]), ('.', [".", ".", ".", ".", "#"]),
];
/// `text` (digits, ':' and '.') in 5 lines of blocks, each dot two characters wide so the digits look square.
pub fn big(text: &str, color: &str) -> Vec<String> {
    // Every row has the same width (blank cells are kept, only the gap after the last digit goes): the rows are centred one by one, so rows of different widths would slide sideways against each other.
    (0..5).map(|r| {
        let s = text.chars().filter_map(|c| GLYPHS.iter().find(|g| g.0 == c)).map(|g| g.1[r].chars().map(|p| if p == '#' { "\u{2588}\u{2588}" } else { "  " }).collect::<String>()).collect::<Vec<_>>().join("  ");
        format!("{color}{s}{R}")
    }).collect()
}
fn centre(l: &str, cols: usize) -> String { format!("{}{l}", " ".repeat(cols.saturating_sub(vis(l)) / 2)) }

// ---- the app ----

struct Watch { start: Option<Instant>, before: u64 /* ms */, laps: Vec<u64> }
impl Watch { fn ms(&self) -> u64 { self.before + self.start.map_or(0, |s| s.elapsed().as_millis() as u64) } }
fn watch_text(ms: u64) -> String { format!("{:02}:{:02}.{:02}", ms / 60000, ms / 1000 % 60, ms / 10 % 100) }

const TABS: [&str; 4] = ["clock", "stopwatch", "timer", "alarms"];
fn tabs(t: usize, w: usize) -> Vec<String> {
    let row: Vec<String> = TABS.iter().enumerate().map(|(i, n)| if i == t { format!("{ACC}{} {n}{R}", i + 1) } else { format!("{DIM}{} {n}{R}", i + 1) }).collect();
    vec![head("clock", &chrono_zone(), w), String::new(), format!(" {}", row.join("   ")), String::new()]
}
fn chrono_zone() -> String { format!("UTC{}", clock::format_offset(clock::home_tz().unwrap_or(0))) }

pub fn run() -> i32 {
    let Some(mut s) = Screen::start("clock") else { return 1 };
    let (mut tab, mut sel, mut sw) = (0usize, Sel::default(), Watch { start: None, before: 0, laps: vec![] });
    let mut note = String::new();
    loop {
        let (cols, rows) = crate::term::size();
        let (now, off, w) = (clock::now(), clock::home_tz().unwrap_or(0), width(cols));
        let mut alarms = load();
        let mut l = tabs(tab, w);
        let sc = |l: &mut Vec<String>, x: Vec<String>| l.extend(x.iter().map(|r| centre(r, w + 4)));
        match tab {
            0 => {
                let t = clock::local(now, off);
                sc(&mut l, big(&format!("{:02}:{:02}:{:02}", t.hour, t.min, t.sec), FG));
                l.push(String::new());
                l.push(centre(&format!("{FG}{} {:02} {} {}{R}", clock::WDAY_FULL[t.wday as usize], t.day, clock::MONTH[t.month as usize - 1], t.year), w + 4));
                let next = alarms.iter().filter(|a| a.on && timer_secs(a).is_none()).filter_map(|a| next_ring(a.when, now, off).map(|t| (t, a))).min_by_key(|x| x.0);
                l.push(String::new());
                l.push(centre(&match next { Some((t, _)) => format!("{DIM}next alarm{R} {FG}{}{R} {DIM}in {}{R}", day_text(t, now, off), dur_text(t - now)), None => format!("{DIM}No alarm set. Press 4, then a to add one.{R}") }, w + 4));
            }
            1 => {
                sc(&mut l, big(&watch_text(sw.ms()), if sw.start.is_some() { GOOD } else { FG }));
                l.push(String::new());
                l.push(centre(&if sw.start.is_some() { format!("{ACC}\u{25d0}{R} {DIM}running{R}") } else if sw.before > 0 { format!("{WARN}stopped{R}") } else { format!("{DIM}\u{25cb} ready{R}") }, w + 4));
                for (i, x) in sw.laps.iter().enumerate().rev().take(rows.saturating_sub(18)) { l.push(format!(" {DIM}lap {:<3}{R} {FG}{}{R}  {DIM}+{}{R}", i + 1, watch_text(*x), watch_text(x - if i > 0 { sw.laps[i - 1] } else { 0 }))); }
            }
            2 => match alarms.iter().find(|a| timer_secs(a).is_some()) {
                Some(a) => {
                    let (total, left) = (timer_secs(a).unwrap_or(1), match a.when { When::At(t) => t - now, _ => 0 });
                    let (txt, c) = if left > 0 { (dur_text(left), if left <= 10 { WARN } else { FG }) } else { ("00:00".into(), BAD) };
                    sc(&mut l, big(&txt, c)); l.push(String::new());
                    let filled = (((total - left.max(0)) as usize * (w - 2)) / total.max(1) as usize).min(w - 2);
                    l.push(centre(&format!("{ACC}{}{R}{BDIM}{}{R}", "\u{25ae}".repeat(filled), "\u{2500}".repeat(w - 2 - filled)), w + 4));
                    l.push(centre(&if left > 0 { format!("{ACC}\u{25d0}{R} {DIM}counting down from {}{R}", dur_text(total)) } else { format!("{BAD}time is up{R}") }, w + 4));
                }
                None => { sc(&mut l, big("00:00", DIM)); l.push(String::new()); l.push(centre(&format!("{DIM}No timer. Press {FG}t{DIM} to start one.{R}"), w + 4)); }
            },
            _ => {
                let rowsv: Vec<String> = alarms.iter().map(|a| {
                    let (t, rep) = describe(a.when, now, off);
                    let (g, c) = if !a.on { ("\u{25cb}", DIM) } else if matches!(a.when, When::At(x) if x <= now) { ("\u{2713}", GOOD) } else { ("\u{25cf}", ACC) };
                    format!("{DIM}{:<2}{R} {c}{g}{R}  {FG}{t}{R}  {:<22} {}", a.id, fit(&rep, 22), fit(&a.label, w.saturating_sub(36)))
                }).collect();
                let (on, done) = (alarms.iter().filter(|a| a.on).count(), alarms.iter().filter(|a| matches!(a.when, When::At(x) if x <= now)).count());
                let p = Page { title: "", right: "", count: count(alarms.len(), "alarm", "alarms"), header: "#   s  time   repeat                 label".into(), rows: rowsv,
                    foot: vec![format!("{} total", alarms.len()), format!("{GOOD}{on} on{R}"), format!("{DIM}{done} done{R}")], note: note.clone(), hints: vec![("a", "add"), ("e", "edit"), ("space", "on/off"), ("d", "delete"), ("1-4", "tab"), ("q", "quit")], empty: "No alarms yet. Press a to add one." };
                let mut pl = p.lines(&mut sel, cols, rows.saturating_sub(2));
                pl.drain(..2); l.extend(pl);   // the tab header replaces the page title
            }
        }
        if tab != 3 {
            while l.len() + 2 < rows { l.push(String::new()); }
            l.push(if note.is_empty() { String::new() } else { format!(" {note}") });
            l.push(hints(&match tab { 0 => vec![("1-4", "tab"), ("q", "quit")], 1 => vec![("space", "start/stop"), ("l", "lap"), ("r", "reset"), ("1-4", "tab"), ("q", "quit")], _ => vec![("t", "new timer"), ("c", "cancel"), ("1-4", "tab"), ("q", "quit")] }));
        }
        draw(&l);
        let Some(keys) = s.keys(if tab == 1 && sw.start.is_some() { 50 } else { 250 }) else { return 0 };
        for k in keys {
            note.clear();
            if tab == 3 && sel.key(&k, alarms.len()) { continue }
            match k {
                Key::Char('q') | Key::Ctrl(b'c') => return 0,
                Key::Char(c @ '1'..='4') => tab = c as usize - '1' as usize,
                Key::Tab | Key::Right => tab = (tab + 1) % 4, Key::Left => tab = (tab + 3) % 4,
                Key::Char(' ') if tab == 1 => { if let Some(st) = sw.start.take() { sw.before += st.elapsed().as_millis() as u64; } else { sw.start = Some(Instant::now()); } }
                Key::Char('l') if tab == 1 && sw.start.is_some() => sw.laps.push(sw.ms()),
                Key::Char('r') if tab == 1 => sw = Watch { start: None, before: 0, laps: vec![] },
                Key::Char('t') if tab == 2 => {
                    let Some(a) = s.ask("timer (5m, 1h30m, 90s):", "") else { continue };
                    match parse_duration(&a) {
                        Some(d) => { alarms.retain(|x| timer_secs(x).is_none()); alarms.push(Alarm { id: store::next_id(alarms.iter().map(|x| x.id)), when: When::At(now + d), on: true, label: format!("Timer {d}") }); save(&alarms); }
                        None => note = format!("{BAD}use 5m, 1h30m, 90s or 45 (minutes){R}"),
                    }
                }
                Key::Char('c') if tab == 2 => { alarms.retain(|x| timer_secs(x).is_none()); save(&alarms); }
                Key::Char('a') if tab == 3 => note = edit(&mut s, &mut alarms, None, now, off),
                Key::Char('e') | Key::Enter if tab == 3 && !alarms.is_empty() => note = edit(&mut s, &mut alarms, Some(sel.pos), now, off),
                Key::Char(' ') if tab == 3 && !alarms.is_empty() => { let a = &mut alarms[sel.pos]; a.on = !a.on; save(&alarms); }
                Key::Char('d') if tab == 3 && !alarms.is_empty() => { if s.confirm(&format!("delete alarm {}?", alarms[sel.pos].id)) { alarms.remove(sel.pos); save(&alarms); } }
                _ => {}
            }
        }
    }
}

/// Asks for time, repeat and label (starting from the alarm at `at` when editing); saves; a message for the screen.
fn edit(s: &mut Screen, v: &mut Vec<Alarm>, at: Option<usize>, now: i64, off: i32) -> String {
    let old = at.map(|i| v[i].clone());
    let (t0, r0, l0) = match &old { Some(a) => { let (t, r) = describe(a.when, now, off); (t, if matches!(a.when, When::At(_)) { "once".into() } else { r }, a.label.clone()) } None => (String::new(), "once".into(), String::new()) };
    let Some(t) = s.ask("time HH:MM:", &t0) else { return String::new() };
    let Some((h, m)) = hhmm(&t) else { return format!("{BAD}the time must look like 06:30{R}") };
    let Some(r) = s.ask("repeat (once, daily, weekdays, mon,wed):", &r0) else { return String::new() };
    let when = if r.trim().is_empty() || r.trim().eq_ignore_ascii_case("once") { When::At(next_at(h, m, now, off)) } else { match parse_days(&r) { Some(d) => When::Daily(h, m, d), None => return format!("{BAD}repeat: once, daily, weekdays, weekends or days like mon,wed{R}") } };
    let Some(label) = s.ask("label:", &l0) else { return String::new() };
    let label: String = crate::files::printable(label.trim()).chars().take(60).collect();
    match at { Some(i) => { v[i].when = when; v[i].label = label; v[i].on = true; }
        None => { if v.len() >= MAX_ALARMS { return format!("{BAD}too many alarms{R}") } v.push(Alarm { id: store::next_id(v.iter().map(|a| a.id)), when, on: true, label }); } }
    save(v); format!("{GOOD}\u{2713}{R} alarm saved")
}

#[cfg(test)]
mod tests {
    use super::*;
    const OFF: i32 = 330;
    const NOW: i64 = 1794906125;   // Tue 17 Nov 2026 14:32:05 +05:30
    #[test] fn file_round_trip_and_bad_lines() {
        let v = vec![Alarm { id: 1, when: When::At(NOW + 60), on: true, label: "tea time".into() }, Alarm { id: 2, when: When::Daily(6, 30, ALL_DAYS), on: false, label: "wake".into() },
            Alarm { id: 3, when: When::Daily(7, 5, 0b0111110), on: true, label: String::new() }];
        let t = format(&v);
        assert!(t.contains("\t06:30\t0\t") && t.contains("\t07:05/12345\t"));
        assert_eq!(parse(&t), v);
        assert_eq!(parse("junk\n1\t25:00\t1\tx\n2\t07:00/9\t1\tx\n3\t07:00\t1\tok\nx\t07:00\t1\ty\n4\t@abc\t1\tz\n").len(), 1);
    }
    #[test] fn once_fires_once() { let w = When::At(1000); assert!(!due(w, 998, 999, 0) && due(w, 999, 1000, 0) && !due(w, 1000, 1005, 0)); }
    #[test] fn daily_follows_days_and_midnight() {
        let mid = NOW - (9 * 3600 + 2 * 60 + 5) + (18 * 3600 + 30 * 60);   // Wed 18 Nov 00:00 +05:30
        assert!(due(When::Daily(0, 0, ALL_DAYS), mid - 3, mid + 2, OFF) && !due(When::Daily(0, 0, ALL_DAYS), mid + 2, mid + 7, OFF));
        assert!(due(When::Daily(23, 59, ALL_DAYS), mid - 70, mid + 3, OFF));
        assert!(due(When::Daily(0, 0, 1 << 3), mid - 3, mid + 2, OFF));    // Wednesday only
        assert!(!due(When::Daily(0, 0, 1 << 2), mid - 3, mid + 2, OFF));   // Tuesday only: not on Wednesday
        assert!(due(When::Daily(23, 59, 1 << 2), mid - 70, mid + 3, OFF));  // Tuesday's 23:59 seen just after midnight
    }
    #[test] fn next_times() {
        assert_eq!(next_at(17, 0, NOW, OFF), NOW - 52325 + 17 * 3600); assert_eq!(next_at(7, 0, NOW, OFF), NOW - 52325 + 31 * 3600);
        assert_eq!(next_ring(When::Daily(7, 0, 1 << 6), NOW, OFF), Some(NOW - 52325 + (4 * 86400) + 7 * 3600));   // Saturday 21 Nov... Tue + 4 days
        assert_eq!(next_ring(When::At(NOW - 1), NOW, OFF), None);
    }
    #[test] fn days_and_durations() {
        assert_eq!(parse_days("mon,wed"), Some(0b0001010)); assert_eq!(parse_days("Weekdays"), Some(62)); assert_eq!(parse_days("daily"), Some(127)); assert_eq!(parse_days("xyz"), None); assert_eq!(parse_days(""), None);
        assert_eq!(days_text(0b0001010), "Mon Wed"); assert_eq!(days_text(127), "daily");
        for (s, d) in [("5m", 300), ("1h30m", 5400), ("90s", 90), ("45", 2700), ("1h", 3600), (" 2m 30s ", 150)] { assert_eq!(parse_duration(s), Some(d), "{s}"); }
        for s in ["", "0", "0s", "x", "25h", "5x", "-5m", "m"] { assert_eq!(parse_duration(s), None, "{s}"); }
        assert_eq!((dur_text(75), dur_text(3725)), ("01:15".into(), "1:02:05".into()));
        assert_eq!(timer_secs(&Alarm { id: 1, when: When::At(1), on: true, label: "Timer 300".into() }), Some(300));
    }
    #[test] fn describing() {
        assert_eq!(describe(When::At(NOW + 3 * 3600), NOW, OFF), ("17:32".into(), "once, today 17:32".into()));
        assert_eq!(describe(When::At(NOW + 17 * 3600), NOW, OFF).1, "once, tomorrow 07:32");
        assert!(describe(When::At(NOW - 60), NOW, OFF).1.starts_with("done"));
        assert_eq!(describe(When::Daily(6, 30, 62), NOW, OFF), ("06:30".into(), "weekdays".into()));
    }
    #[test] fn big_digits_have_five_rows() {
        let b = big("12:30", ""); assert_eq!(b.len(), 5); assert!(b[0].contains('\u{2588}'));
        for t in ["11:11", "00:00", "12:47.89", "01:28"] { let w = big(t, ""); assert!(w.iter().all(|r| vis(r) == vis(&w[0])), "{t}: rows differ in width"); }
        let plain: Vec<usize> = b.iter().map(|l| vis(l)).collect(); assert!(plain.iter().all(|&n| n == plain[0] || n >= 10));
    }
}
