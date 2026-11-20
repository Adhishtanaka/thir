//! calendar: a month view and events with reminders. ~/.config/thir/calendar/events, one per line:
//! ID TAB YYYY-MM-DD TAB HH:MM (empty = all day) TAB REMIND (minutes before, -1 = none) TAB TITLE. `bell` shows the reminders, even when this app is closed.
use crate::{clock, line::Key, store, tui::*};

#[derive(Clone, PartialEq, Debug)]
pub struct Event { pub id: u32, pub date: (i64, u32, u32), pub time: Option<(u32, u32)>, pub remind: i32, pub title: String }
const MAX_EVENTS: usize = 500;

pub fn days_in(y: i64, m: u32) -> u32 { match m { 2 => if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 { 29 } else { 28 }, 4 | 6 | 9 | 11 => 30, _ => 31 } }
/// Days since 1970-01-01 of a civil date (the inverse of clock::civil_from_days).
pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = y - (m <= 2) as i64; let era = y.div_euclid(400); let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) as i64 + 2) / 5 + d as i64 - 1;
    era * 146097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719468
}
/// Weekday (0 = Sunday) of a civil date.
pub fn wday(y: i64, m: u32, d: u32) -> u32 { (days_from_civil(y, m, d) + 4).rem_euclid(7) as u32 }

pub fn parse_date(s: &str) -> Option<(i64, u32, u32)> {
    let mut p = s.trim().split('-');
    let (y, m, d): (i64, u32, u32) = (p.next()?.parse().ok()?, p.next()?.parse().ok()?, p.next()?.parse().ok()?);
    if p.next().is_some() || !(1970..=2200).contains(&y) || !(1..=12).contains(&m) || d < 1 || d > days_in(y, m) { return None }
    Some((y, m, d))
}
pub fn parse_time(s: &str) -> Option<(u32, u32)> {
    let (h, m) = s.trim().split_once(':')?;
    if !(1..=2).contains(&h.len()) || m.len() != 2 || !h.bytes().chain(m.bytes()).all(|b| b.is_ascii_digit()) { return None }
    let (h, m) = (h.parse().ok()?, m.parse().ok()?); (h < 24 && m < 60).then_some((h, m))
}
pub fn date_text(d: (i64, u32, u32)) -> String { format!("{:04}-{:02}-{:02}", d.0, d.1, d.2) }

pub fn parse(text: &str) -> Vec<Event> {
    text.lines().filter_map(|l| {
        let f = store::split(l, 5)?;
        let time = if f[2].is_empty() { None } else { Some(parse_time(&f[2])?) };
        let title: String = crate::files::printable(&f[4]).chars().take(80).collect();
        Some(Event { id: f[0].parse().ok()?, date: parse_date(&f[1])?, time, remind: f[3].parse().ok().filter(|r| (-1..=10080).contains(r))?, title }).filter(|e: &Event| !e.title.trim().is_empty())
    }).take(MAX_EVENTS).collect()
}
pub fn format(v: &[Event]) -> String {
    v.iter().map(|e| store::join(&[&e.id.to_string(), &date_text(e.date), &e.time.map_or(String::new(), |t| format!("{:02}:{:02}", t.0, t.1)), &e.remind.to_string(), &e.title]) + "\n").collect()
}
pub fn load() -> Vec<Event> { parse(&store::read("calendar", "events")) }
pub fn save(v: &[Event]) { let _ = store::write("calendar", "events", &format(v)); }

/// When the event starts (unix), all-day events at 00:00 local.
pub fn start(e: &Event, off: i32) -> i64 { days_from_civil(e.date.0, e.date.1, e.date.2) * 86400 + e.time.map_or(0, |t| (t.0 * 3600 + t.1 * 60) as i64) - off as i64 * 60 }
/// When the reminder shows: `remind` minutes before the start; for an all-day event, before 09:00 of that day. None = no reminder.
pub fn remind_at(e: &Event, off: i32) -> Option<i64> { (e.remind >= 0).then(|| start(e, off) + if e.time.is_none() { 9 * 3600 } else { 0 } - e.remind as i64 * 60) }
pub fn remind_text(m: i32) -> String { match m { -1 => "none".into(), 0 => "at start".into(), m if m % 1440 == 0 => format!("{} d before", m / 1440), m if m % 60 == 0 => format!("{} h before", m / 60), m => format!("{m} min before") } }
/// `15`, `30m`, `2h`, `1d`, `none`, `0` -> minutes before (-1 none).
pub fn parse_remind(s: &str) -> Option<i32> {
    let s = s.trim().to_lowercase();
    if s.is_empty() || s == "none" || s == "no" { return Some(-1) }
    let (n, u) = match s.chars().last()? { 'm' => (&s[..s.len() - 1], 1), 'h' => (&s[..s.len() - 1], 60), 'd' => (&s[..s.len() - 1], 1440), _ => (&s[..], 1) };
    n.trim().parse::<i32>().ok().filter(|n| *n >= 0).map(|n| n * u).filter(|m| *m <= 10080)
}
/// The text of the notice for a reminder.
pub fn notice(e: &Event) -> String { match e.time { Some((h, m)) => format!("{} {:02}:{:02} {}", if e.remind == 0 { "Now" } else { "Soon" }, h, m, e.title), None => format!("Today: {}", e.title) } }

/// Events on a date, by time (all-day first).
pub fn on(v: &[Event], d: (i64, u32, u32)) -> Vec<&Event> { let mut x: Vec<&Event> = v.iter().filter(|e| e.date == d).collect(); x.sort_by_key(|e| (e.time, e.id)); x }
/// The month as 6 week rows of 7 days: None outside the month.
pub fn grid(y: i64, m: u32) -> Vec<[Option<u32>; 7]> {
    let (first, n) = (wday(y, m, 1) as usize, days_in(y, m) as usize);
    (0..6).map(|r| { let mut w = [None; 7]; for c in 0..7 { let i = r * 7 + c; if i >= first && i < first + n { w[c] = Some((i - first + 1) as u32); } } w }).collect()
}
fn add_months(y: i64, m: u32, d: i32) -> (i64, u32) { let i = y * 12 + m as i64 - 1 + d as i64; (i.div_euclid(12), i.rem_euclid(12) as u32 + 1) }
fn add_days(d: (i64, u32, u32), n: i64) -> (i64, u32, u32) { crate::clock::civil_from_days(days_from_civil(d.0, d.1, d.2) + n) }

const MONTHS: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];

pub fn run() -> i32 {
    let Some(mut s) = Screen::start("calendar") else { return 1 };
    let today = { let l = clock::local(clock::now(), clock::home_tz().unwrap_or(0)); (l.year, l.month, l.day) };
    let (mut cur, mut sel, mut note) = (today, Sel::default(), String::new());
    let mut focus_list = false;
    loop {
        let (cols, rows) = crate::term::size();
        let w = width(cols);
        let mut ev = load(); ev.sort_by_key(|e| (e.date, e.time, e.id));
        let day: Vec<Event> = on(&ev, cur).into_iter().cloned().collect();
        let month = count(ev.iter().filter(|e| e.date.0 == cur.0 && e.date.1 == cur.1).count(), "event", "events");
        let mut l = vec![head("calendar", &format!("{} {}", MONTHS[cur.1 as usize - 1], cur.0), w), String::new(),
            format!(" {DIM}{}{R}", clock::WDAY.iter().map(|d| format!("{:<4}", &d[..2])).collect::<String>().trim_end()), rule(28)];
        for wk in grid(cur.0, cur.1) {
            if wk.iter().all(|d| d.is_none()) { continue }
            l.push(format!(" {}", wk.iter().map(|d| match d {
                None => "    ".to_string(),
                Some(n) => { let dd = (cur.0, cur.1, *n); let busy = ev.iter().any(|e| e.date == dd);
                    // the cursor day is a peach cell with wall-coloured text; today is peach text; days with events are FG, empty ones dim
                    let c = if dd == cur { format!("{ACCBG}{WALLFG}") } else if dd == today { ACC.to_string() } else if busy { FG.to_string() } else { DIM.to_string() };
                    format!("{c}{:>2}{R}  ", n) } }).collect::<String>().trim_end().to_string()));
        }
        l.push(rule(28)); l.push(String::new());
        let day_name = format!("{} {} {}", clock::WDAY[wday(cur.0, cur.1, cur.2) as usize], cur.2, clock::MONTH[cur.1 as usize - 1]);
        l.push(format!(" {FG}{day_name}{R}{}{DIM}{month} this month{R}", " ".repeat(w.saturating_sub(day_name.len() + month.len() + 11).max(2))));
        let list_rows: Vec<String> = day.iter().map(|e| format!("{DIM}{:<2}{R}  {FG}{}{R}  {}  {DIM}{}{R}", e.id, e.time.map_or("all day".to_string(), |t| format!("{:02}:{:02}  ", t.0, t.1)), fit(&e.title, w.saturating_sub(34)), if e.remind >= 0 { format!("remind {}", remind_text(e.remind)) } else { String::new() })).collect();
        let p = Page { title: "", right: "", count: String::new(), header: "id  time     event".into(), rows: list_rows, foot: vec![count(day.len(), "event", "events"), format!("{} reminders", day.iter().filter(|e| e.remind >= 0).count()), format!("{} total", ev.len())],
            note: note.clone(), hints: vec![("arrows", "day"), ("[ ]", "month"), ("t", "today"), ("a", "add"), ("e", "edit"), ("d", "delete"), ("tab", "list"), ("q", "quit")], empty: "Nothing on this day. Press a to add an event." };
        let mut pl = p.lines(&mut sel, cols, rows.saturating_sub(l.len()) + 2);
        pl.drain(..2); l.extend(pl);
        draw(&l);
        let Some(keys) = s.keys(2000) else { return 0 };
        for k in keys {
            note.clear();
            if focus_list && sel.key(&k, day.len()) { continue }
            match k {
                Key::Char('q') | Key::Ctrl(b'c') => return 0,
                Key::Tab => focus_list = !focus_list,
                Key::Left => cur = add_days(cur, -1), Key::Right => cur = add_days(cur, 1), Key::Up => cur = add_days(cur, -7), Key::Down => cur = add_days(cur, 7),
                Key::Char('[') | Key::PgUp => { let (y, m) = add_months(cur.0, cur.1, -1); cur = (y, m, cur.2.min(days_in(y, m))); }
                Key::Char(']') | Key::PgDn => { let (y, m) = add_months(cur.0, cur.1, 1); cur = (y, m, cur.2.min(days_in(y, m))); }
                Key::Char('t') => cur = today,
                Key::Char('a') => note = edit(&mut s, &mut ev, None, cur),
                Key::Char('e') | Key::Enter if !day.is_empty() => { let id = day[sel.pos.min(day.len() - 1)].id; let i = ev.iter().position(|e| e.id == id); note = edit(&mut s, &mut ev, i, cur); }
                Key::Char('d') if !day.is_empty() => { let id = day[sel.pos.min(day.len() - 1)].id; if s.confirm(&format!("delete event {id}?")) { ev.retain(|e| e.id != id); save(&ev); } }
                _ => {}
            }
        }
    }
}

fn edit(s: &mut Screen, v: &mut Vec<Event>, at: Option<usize>, cur: (i64, u32, u32)) -> String {
    let old = at.map(|i| v[i].clone());
    let (d0, t0, r0, n0) = match &old { Some(e) => (date_text(e.date), e.time.map_or(String::new(), |t| format!("{:02}:{:02}", t.0, t.1)), if e.remind < 0 { "none".into() } else { e.remind.to_string() + "m" }, e.title.clone()), None => (date_text(cur), String::new(), "15m".into(), String::new()) };
    let Some(title) = s.ask("title:", &n0) else { return String::new() };
    if title.trim().is_empty() { return format!("{BAD}the event needs a title{R}") }
    let Some(d) = s.ask("date YYYY-MM-DD:", &d0) else { return String::new() };
    let Some(date) = parse_date(&d) else { return format!("{BAD}the date must look like 2026-11-30{R}") };
    let Some(t) = s.ask("time HH:MM (empty = all day):", &t0) else { return String::new() };
    let time = if t.trim().is_empty() { None } else { match parse_time(&t) { Some(x) => Some(x), None => return format!("{BAD}the time must look like 14:30{R}") } };
    let Some(r) = s.ask("remind before (15m, 2h, 1d, none):", &r0) else { return String::new() };
    let Some(remind) = parse_remind(&r) else { return format!("{BAD}remind: use 15m, 2h, 1d or none{R}") };
    let title: String = crate::files::printable(title.trim()).chars().take(80).collect();
    match at { Some(i) => v[i] = Event { id: v[i].id, date, time, remind, title },
        None => { if v.len() >= MAX_EVENTS { return format!("{BAD}too many events{R}") } v.push(Event { id: store::next_id(v.iter().map(|e| e.id)), date, time, remind, title }); } }
    save(v); format!("{GOOD}\u{2713}{R} event saved")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn calendar_math() {
        assert_eq!((days_in(2024, 2), days_in(2026, 2), days_in(1900, 2), days_in(2000, 2), days_in(2026, 11)), (29, 28, 28, 29, 30));
        assert_eq!(days_from_civil(1970, 1, 1), 0); assert_eq!(wday(2026, 11, 17), 2); assert_eq!(wday(2000, 2, 29), 2);
        for z in [-1000, 0, 11000, 20400, 47000] { let (y, m, d) = clock::civil_from_days(z); assert_eq!(days_from_civil(y, m, d), z); }
        assert_eq!(add_days((2026, 12, 31), 1), (2027, 1, 1)); assert_eq!(add_months(2026, 1, -1), (2025, 12)); assert_eq!(add_months(2026, 12, 1), (2027, 1));
    }
    #[test] fn grid_of_a_month() {
        let g = grid(2026, 11);   // 1 Nov 2026 is a Sunday
        assert_eq!(g[0][0], Some(1)); assert_eq!(g[4][1], Some(30)); assert!(g[5].iter().all(|d| d.is_none()));
        assert_eq!(grid(2026, 2).iter().flatten().flatten().count(), 28);
    }
    #[test] fn inputs() {
        assert_eq!(parse_date("2026-02-29"), None); assert_eq!(parse_date("2024-02-29"), Some((2024, 2, 29))); assert_eq!(parse_date("2026-13-01"), None); assert_eq!(parse_date("x"), None);
        assert_eq!(parse_time("9:05"), Some((9, 5))); assert_eq!(parse_time("24:00"), None);
        for (s, m) in [("15", 15), ("30m", 30), ("2h", 120), ("1d", 1440), ("none", -1), ("", -1), ("0", 0)] { assert_eq!(parse_remind(s), Some(m), "{s}"); }
        for s in ["-5", "x", "99d", "1w"] { assert_eq!(parse_remind(s), None, "{s}"); }
        assert_eq!((remind_text(-1), remind_text(0), remind_text(90), remind_text(120), remind_text(1440)), ("none".into(), "at start".into(), "90 min before".into(), "2 h before".into(), "1 d before".into()));
    }
    #[test] fn file_round_trip_and_bad_lines() {
        let v = vec![Event { id: 1, date: (2026, 11, 20), time: Some((14, 30)), remind: 15, title: "Dentist visit".into() }, Event { id: 2, date: (2026, 12, 25), time: None, remind: -1, title: "Holiday".into() }];
        assert_eq!(parse(&format(&v)), v);
        assert_eq!(parse("junk\n1\t2026-02-30\t\t0\tx\n2\t2026-01-01\t25:00\t0\tx\n3\t2026-01-01\t\t99999\tx\n4\t2026-01-01\t\t0\t \n5\t2026-01-01\t\t0\tok\n").len(), 1);
    }
    #[test] fn reminders() {
        let e = Event { id: 1, date: (2026, 11, 20), time: Some((14, 30)), remind: 15, title: "x".into() };
        let t = start(&e, 330); assert_eq!(clock::local(t, 330).hour, 14); assert_eq!(remind_at(&e, 330), Some(t - 900));
        let a = Event { time: None, remind: 60, ..e.clone() }; assert_eq!(remind_at(&a, 330), Some(start(&a, 330) + 8 * 3600));
        assert_eq!(remind_at(&Event { remind: -1, ..e.clone() }, 330), None);
        assert_eq!(notice(&e), "Soon 14:30 x"); assert_eq!(notice(&a), "Today: x");
        assert_eq!(on(&[e.clone(), a.clone()], (2026, 11, 20))[0].time, None);   // all-day first
    }
}
