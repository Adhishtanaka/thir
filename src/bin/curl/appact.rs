//! The AI's verbs for the built-in apps (contacts, notes, calendar, clock, calc). The model only picks a verb and fills its fields; every field is checked here
//! with the same rules the apps use, nothing the model writes is run, and a change happens only after the user says yes (route.rs asks).
//! Reads (CONTACT_FIND, NOTE_FIND, CALENDAR) never send the whole data: only what the question names, a few records, at most MAX_DATA characters, marked as data.
use crate::{agent::clean, memory::looks_secret};
use thir::{apps::{alarm, cal, calc, contacts, notes}, clock, files::printable};

const MAX_ARG: usize = 400;
const MAX_DATA: usize = 1500;
const MAX_HITS: usize = 5;

#[derive(Debug, PartialEq)]
pub enum Act {
    ContactAdd { name: String, phones: Vec<String>, emails: Vec<String>, note: String },
    NoteAdd { title: String, line: String },
    EventAdd { date: (i64, u32, u32), time: Option<(u32, u32)>, title: String, remind: i32 },
    AlarmAdd { h: u32, m: u32, days: Option<u8>, label: String },
    Timer(i64),
    Calc(String),
    ContactFind(String), NoteFind(String), Calendar((i64, u32, u32), (i64, u32, u32)),
}

pub const VERBS: &[&str] = &["CONTACT_ADD", "CONTACT_FIND", "NOTE_ADD", "NOTE_FIND", "EVENT_ADD", "CALENDAR", "ALARM_ADD", "TIMER", "CALC"];

fn text(s: &str, max: usize, what: &str) -> Result<String, String> {
    let t = printable(s.trim());
    if t.chars().count() > max { return Err(format!("the {what} is too long")) }
    if looks_secret(&t) { return Err(format!("the {what} looks like a password or key; the apps never keep those")) }
    Ok(t)
}
fn list(s: &str, ok: fn(&str) -> bool, what: &str) -> Result<Vec<String>, String> { contacts::parse_list(s, ok).map_err(|b| format!("\"{}\" is not a {what}", clean(&b))) }

/// `KEYWORD arg` from the model's first line; Err = why it was refused.
pub fn parse(kw: &str, arg: &str) -> Result<Act, String> {
    if arg.chars().count() > MAX_ARG { return Err(format!("{kw} is too long")) }
    let f: Vec<&str> = arg.split('|').map(str::trim).collect();
    let get = |i: usize| f.get(i).copied().unwrap_or("");
    match kw {
        "CONTACT_ADD" => {
            let name = text(get(0), 60, "name")?;
            if name.is_empty() { return Err("CONTACT_ADD has no name".into()) }
            Ok(Act::ContactAdd { name, phones: list(get(1), contacts::phone_ok, "phone number")?, emails: list(get(2), contacts::email_ok, "email address")?, note: text(get(3), 200, "note")? })
        }
        "NOTE_ADD" => {
            let (title, line) = (text(get(0), 80, "title")?, text(get(1), 200, "text")?);
            if title.is_empty() || line.is_empty() { return Err("NOTE_ADD needs a title and a text".into()) }
            Ok(Act::NoteAdd { title, line })
        }
        "EVENT_ADD" => {
            let date = cal::parse_date(get(0)).ok_or("EVENT_ADD: the date must be YYYY-MM-DD")?;
            let time = if get(1).is_empty() || get(1) == "-" { None } else { Some(cal::parse_time(get(1)).ok_or("EVENT_ADD: the time must be HH:MM")?) };
            let title = text(get(2), 80, "title")?;
            if title.is_empty() { return Err("EVENT_ADD has no title".into()) }
            Ok(Act::EventAdd { date, time, title, remind: cal::parse_remind(if f.len() > 3 { get(3) } else { "15m" }).ok_or("EVENT_ADD: remind must be 15m, 2h, 1d or none")? })
        }
        "ALARM_ADD" => {
            let (h, m) = alarm::hhmm(get(0)).ok_or("ALARM_ADD: the time must be HH:MM")?;
            let r = get(1).to_lowercase();
            let days = if r.is_empty() || r == "once" { None } else { Some(alarm::parse_days(&r).ok_or("ALARM_ADD: repeat must be once, daily, weekdays, weekends or days like mon,wed")?) };
            Ok(Act::AlarmAdd { h, m, days, label: text(get(2), 60, "label")? })
        }
        "TIMER" => alarm::parse_duration(arg).map(Act::Timer).ok_or_else(|| "TIMER: use 5m, 1h30m or 90s (at most 24 h)".into()),
        "CALC" => { let e = text(arg, 200, "expression")?; if e.is_empty() { Err("CALC has no expression".into()) } else { Ok(Act::Calc(e)) } }
        "CONTACT_FIND" | "NOTE_FIND" => {
            let q = text(arg, 60, "search words")?;
            if q.is_empty() { return Err(format!("{kw} has no words")) }
            Ok(if kw == "CONTACT_FIND" { Act::ContactFind(q) } else { Act::NoteFind(q) })
        }
        "CALENDAR" => {
            let a = cal::parse_date(get(0)).ok_or("CALENDAR: the date must be YYYY-MM-DD")?;
            let b = if f.len() > 1 && !get(1).is_empty() { cal::parse_date(get(1)).ok_or("CALENDAR: the date must be YYYY-MM-DD")? } else { a };
            let days = |d: (i64, u32, u32)| cal::days_from_civil(d.0, d.1, d.2);
            if days(b) < days(a) || days(b) - days(a) > 31 { return Err("CALENDAR: at most 31 days, in order".into()) }
            Ok(Act::Calendar(a, b))
        }
        _ => Err(format!("unknown action {}", clean(kw))),
    }
}

/// (label, what, cost note) for the confirm line of a change; None for a read.
pub fn describe(a: &Act) -> Option<(&'static str, String, String)> {
    let (now, off) = (clock::now(), clock::home_tz().unwrap_or(0));
    Some(match a {
        Act::ContactAdd { name, phones, emails, .. } => ("contacts:", format!("add {name}{}{}", if phones.is_empty() { String::new() } else { format!(" \u{b7} {}", phones.join(", ")) }, if emails.is_empty() { String::new() } else { format!(" \u{b7} {}", emails.join(", ")) }), String::new()),
        Act::NoteAdd { title, line } => ("notes:", format!("add \"{line}\" to the note {title}"), String::new()),
        Act::EventAdd { date, time, title, remind } => ("calendar:", format!("{title} \u{b7} {} {}{}", cal::date_text(*date), time.map_or("all day".to_string(), |t| format!("{:02}:{:02}", t.0, t.1)), if *remind >= 0 { format!(" \u{b7} remind {}", cal::remind_text(*remind)) } else { String::new() }), String::new()),
        Act::AlarmAdd { h, m, days, label } => ("alarm:", format!("{h:02}:{m:02} {}{}", days.map_or_else(|| { let (_, r) = alarm::describe(alarm::When::At(alarm::next_at(*h, *m, now, off)), now, off); r }, alarm::days_text), if label.is_empty() { String::new() } else { format!(" \u{b7} {label}") }), "it rings even with no app open".into()),
        Act::Timer(s) => ("timer:", alarm::dur_text(*s), "it rings even with no app open".into()),
        _ => return None,
    })
}

/// Does the change; the text to show.
pub fn apply(a: &Act) -> Result<String, String> {
    let (now, off) = (clock::now(), clock::home_tz().unwrap_or(0));
    match a {
        Act::ContactAdd { name, phones, emails, note } => {
            let mut v = contacts::load();
            if v.len() >= 1000 { return Err("agent: you have 1000 contacts; delete one first".into()) }
            v.push(contacts::Contact { id: thir::store::next_id(v.iter().map(|c| c.id)), name: name.clone(), phones: phones.clone(), emails: emails.clone(), note: note.clone() });
            contacts::save(&v); Ok(format!("saved {name} in contacts"))
        }
        Act::NoteAdd { title, line } => {
            let mut v = notes::load();
            match v.iter_mut().find(|n| n.title.eq_ignore_ascii_case(title)) {
                Some(n) => { if n.body.len() >= 200 { return Err("agent: that note has 200 lines; start another".into()) } n.body.push(line.clone()); n.modified = now; }
                None => { if v.len() >= 500 { return Err("agent: you have 500 notes".into()) } v.push(notes::new_note(&v, title, vec![line.clone()], now)); }
            }
            notes::save(&v); Ok(format!("added to the note {title}"))
        }
        Act::EventAdd { date, time, title, remind } => {
            let mut v = cal::load();
            if v.len() >= 500 { return Err("agent: you have 500 events; delete one first".into()) }
            v.push(cal::Event { id: thir::store::next_id(v.iter().map(|e| e.id)), date: *date, time: *time, remind: *remind, title: title.clone() });
            cal::save(&v); Ok(format!("saved {title} in the calendar"))
        }
        Act::AlarmAdd { h, m, days, label } => {
            let mut v = alarm::load();
            if v.len() >= 100 { return Err("agent: you have 100 alarms; delete one first".into()) }
            let when = days.map_or_else(|| alarm::When::At(alarm::next_at(*h, *m, now, off)), |d| alarm::When::Daily(*h, *m, d));
            v.push(alarm::Alarm { id: thir::store::next_id(v.iter().map(|a| a.id)), when, on: true, label: label.clone() });
            alarm::save(&v); Ok("alarm set".into())
        }
        Act::Timer(s) => {
            let mut v = alarm::load(); v.retain(|a| alarm::timer_secs(a).is_none());
            v.push(alarm::Alarm { id: thir::store::next_id(v.iter().map(|a| a.id)), when: alarm::When::At(now + s), on: true, label: format!("Timer {s}") });
            alarm::save(&v); Ok("timer started".into())
        }
        _ => Err("agent: not a change".into()),
    }
}

/// `CALC expr`: the result, or why not (a read of nothing private, so no question is asked).
pub fn calculate(e: &str) -> Result<String, String> { calc::calc(e, 0.0).map(|r| format!("{e} = {r}")) }

/// What a read sends to the AI: (what to tell the user, the data block). None when nothing matches (then nothing is sent).
/// The block holds at most MAX_HITS records, each cut short, MAX_DATA characters in all.
pub fn read_block(a: &Act) -> Option<(String, String)> {
    let cut = |s: String| -> String { s.chars().take(MAX_DATA).collect() };
    match a {
        Act::ContactFind(q) => {
            let all = contacts::load();
            let hits: Vec<String> = contacts::search(&all, q).into_iter().take(MAX_HITS).map(|c| format!("- {} | phones: {} | emails: {} | note: {}", c.name, c.phones.join(", "), c.emails.join(", "), c.note.chars().take(80).collect::<String>())).collect();
            (!hits.is_empty()).then(|| (format!("sends {} contact{} matching \"{q}\" (name, phones, emails, note)", hits.len(), if hits.len() == 1 { "" } else { "s" }), cut(hits.join("\n"))))
        }
        Act::NoteFind(q) => {
            let all = notes::load();
            let hits: Vec<String> = notes::search(&all, q).into_iter().take(3).map(|n| format!("- {} | tags: {} | {}", n.title, n.tags.join(", "), n.body.iter().take(8).map(|l| l.chars().take(80).collect::<String>()).collect::<Vec<_>>().join(" / "))).collect();
            (!hits.is_empty()).then(|| (format!("sends {} note{} matching \"{q}\" (title, tags, first lines)", hits.len(), if hits.len() == 1 { "" } else { "s" }), cut(hits.join("\n"))))
        }
        Act::Calendar(a, b) => {
            let (d0, d1) = (cal::days_from_civil(a.0, a.1, a.2), cal::days_from_civil(b.0, b.1, b.2));
            let mut ev = cal::load(); ev.sort_by_key(|e| (e.date, e.time, e.id));
            let hits: Vec<String> = ev.iter().filter(|e| (d0..=d1).contains(&cal::days_from_civil(e.date.0, e.date.1, e.date.2))).take(20)
                .map(|e| format!("- {} {} {}", cal::date_text(e.date), e.time.map_or("all day".to_string(), |t| format!("{:02}:{:02}", t.0, t.1)), e.title)).collect();
            let span = if a == b { cal::date_text(*a) } else { format!("{} to {}", cal::date_text(*a), cal::date_text(*b)) };
            (!hits.is_empty()).then(|| (format!("sends {} event{} from {span} (date, time, title)", hits.len(), if hits.len() == 1 { "" } else { "s" }), cut(hits.join("\n"))))
        }
        _ => None,
    }
}
/// The message of the empty case of a read.
pub fn nothing(a: &Act) -> String {
    match a { Act::ContactFind(q) => format!("no contact matches \"{q}\""), Act::NoteFind(q) => format!("no note matches \"{q}\""), Act::Calendar(a, b) => format!("nothing on the calendar {}", if a == b { cal::date_text(*a) } else { format!("{} to {}", cal::date_text(*a), cal::date_text(*b)) }), _ => String::new() }
}
pub fn is_read(a: &Act) -> bool { matches!(a, Act::ContactFind(_) | Act::NoteFind(_) | Act::Calendar(..)) }

#[cfg(test)]
mod tests {
    use super::*;
    fn s(x: &str) -> String { x.to_string() }
    #[test] fn good_verbs_parse() {
        assert_eq!(parse("CONTACT_ADD", "Kasun | 0771234567"), Ok(Act::ContactAdd { name: s("Kasun"), phones: vec![s("0771234567")], emails: vec![], note: s("") }));
        assert_eq!(parse("CONTACT_ADD", "Ann Lee | 077 123 4567, 011 222 3333 | ann@x.lk | dentist"), Ok(Act::ContactAdd { name: s("Ann Lee"), phones: vec![s("077 123 4567"), s("011 222 3333")], emails: vec![s("ann@x.lk")], note: s("dentist") }));
        assert_eq!(parse("NOTE_ADD", "Todo | [ ] buy milk"), Ok(Act::NoteAdd { title: s("Todo"), line: s("[ ] buy milk") }));
        assert_eq!(parse("EVENT_ADD", "2026-11-20 | 14:30 | Dentist | 15m"), Ok(Act::EventAdd { date: (2026, 11, 20), time: Some((14, 30)), title: s("Dentist"), remind: 15 }));
        assert_eq!(parse("EVENT_ADD", "2026-12-25 | | Holiday"), Ok(Act::EventAdd { date: (2026, 12, 25), time: None, title: s("Holiday"), remind: 15 }));
        assert_eq!(parse("ALARM_ADD", "06:30 | once | wake up"), Ok(Act::AlarmAdd { h: 6, m: 30, days: None, label: s("wake up") }));
        assert_eq!(parse("ALARM_ADD", "6:30 | weekdays | "), Ok(Act::AlarmAdd { h: 6, m: 30, days: Some(62), label: s("") }));
        assert_eq!(parse("TIMER", "1h30m"), Ok(Act::Timer(5400))); assert_eq!(parse("CALC", "12*15"), Ok(Act::Calc(s("12*15"))));
        assert_eq!(parse("CONTACT_FIND", "kasun"), Ok(Act::ContactFind(s("kasun")))); assert_eq!(parse("NOTE_FIND", "milk"), Ok(Act::NoteFind(s("milk"))));
        assert_eq!(parse("CALENDAR", "2026-11-20"), Ok(Act::Calendar((2026, 11, 20), (2026, 11, 20)))); assert_eq!(parse("CALENDAR", "2026-11-20 | 2026-11-22"), Ok(Act::Calendar((2026, 11, 20), (2026, 11, 22))));
    }
    #[test] fn bad_fields_are_refused() {
        for (k, a) in [("CONTACT_ADD", ""), ("CONTACT_ADD", "Kasun | $(rm -rf ~)"), ("CONTACT_ADD", "Kasun | 077 | not-an-email"), ("CONTACT_ADD", "Kasun | | | my password is x"), ("CONTACT_ADD", &"x".repeat(500)),
            ("NOTE_ADD", "only a title"), ("NOTE_ADD", "T | api key sk-ant-123"), ("EVENT_ADD", "2026-02-30 | | x"), ("EVENT_ADD", "2026-11-20 | 25:00 | x"), ("EVENT_ADD", "2026-11-20 | | "), ("EVENT_ADD", "2026-11-20 | | x | soon"),
            ("ALARM_ADD", "25:00 | once | x"), ("ALARM_ADD", "06:30 | sometimes | x"), ("TIMER", "0"), ("TIMER", "tomorrow"), ("CALC", ""), ("CONTACT_FIND", ""), ("CALENDAR", "tomorrow"), ("CALENDAR", "2026-11-20 | 2027-01-01"), ("CALENDAR", "2026-11-20 | 2026-11-10"),
            ("DELETE_ALL", "contacts")] { assert!(parse(k, a).is_err(), "{k} {a}"); }
        assert_eq!(parse("NOTE_ADD", "T | a\x1b[2Jb"), Ok(Act::NoteAdd { title: s("T"), line: s("a?[2Jb") }));
    }
    #[test] fn changes_say_what_they_do_and_reads_say_what_they_send() {
        let (l, w, _) = describe(&parse("CONTACT_ADD", "Kasun | 0771234567").unwrap()).unwrap(); assert_eq!((l, w.as_str()), ("contacts:", "add Kasun \u{b7} 0771234567"));
        assert!(describe(&parse("ALARM_ADD", "06:30 | weekdays | run").unwrap()).unwrap().1.contains("06:30 weekdays \u{b7} run"));
        assert!(describe(&Act::ContactFind(s("x"))).is_none() && is_read(&Act::ContactFind(s("x"))) && !is_read(&Act::Timer(5)));
        assert_eq!(calculate("12*15"), Ok(s("12*15 = 180"))); assert!(calculate("1/0").is_err());
    }
}
