//! contacts: name, phone numbers, emails, a note; search. ~/.config/thir/contacts/contacts, one per line:
//! ID TAB NAME TAB PHONES TAB EMAILS TAB NOTE (phones and emails are lists joined by `|`).
use crate::{files::printable, line::Key, store, tui::*};

#[derive(Clone, PartialEq, Debug, Default)]
pub struct Contact { pub id: u32, pub name: String, pub phones: Vec<String>, pub emails: Vec<String>, pub note: String }
const MAX: usize = 1000;

fn list(s: &str) -> Vec<String> { s.split('|').map(|x| printable(x.trim())).filter(|x| !x.is_empty()).collect() }
pub fn parse(text: &str) -> Vec<Contact> {
    text.lines().filter_map(|l| {
        let f = store::split(l, 5)?;
        let name = printable(f[1].trim()); if name.is_empty() { return None }
        Some(Contact { id: f[0].parse().ok()?, name: name.chars().take(60).collect(), phones: list(&f[2]), emails: list(&f[3]), note: printable(&f[4]).chars().take(200).collect() })
    }).take(MAX).collect()
}
pub fn format(v: &[Contact]) -> String { v.iter().map(|c| store::join(&[&c.id.to_string(), &c.name, &c.phones.join("|"), &c.emails.join("|"), &c.note]) + "\n").collect() }
pub fn load() -> Vec<Contact> { let mut v = parse(&store::read("contacts", "contacts")); v.sort_by_key(|c| c.name.to_lowercase()); v }
pub fn save(v: &[Contact]) { let _ = store::write("contacts", "contacts", &format(v)); }

/// A phone number: digits, + - ( ) and spaces, 3 to 20 characters with at least 3 digits.
pub fn phone_ok(p: &str) -> bool { p.len() <= 20 && p.chars().all(|c| c.is_ascii_digit() || "+-() ".contains(c)) && p.chars().filter(|c| c.is_ascii_digit()).count() >= 3 }
pub fn email_ok(e: &str) -> bool { e.len() <= 80 && !e.contains(char::is_whitespace) && e.split_once('@').map_or(false, |(a, d)| !a.is_empty() && d.contains('.') && !d.starts_with('.') && !d.ends_with('.') && !d.contains('@')) }
/// Splits a typed list (commas or spaces after commas) and checks each item; Err names the bad one.
pub fn parse_list(s: &str, ok: fn(&str) -> bool) -> Result<Vec<String>, String> {
    let v = s.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect::<Vec<_>>();
    match v.iter().find(|x| !ok(x)) { Some(b) => Err(b.clone()), None => Ok(v) }
}
/// Contacts whose name, number, email or note holds every word of `q` (case-insensitive); all of them when `q` is empty.
pub fn search<'a>(v: &'a [Contact], q: &str) -> Vec<&'a Contact> {
    let words: Vec<String> = q.split_whitespace().map(str::to_lowercase).collect();
    v.iter().filter(|c| { let hay = format!("{} {} {} {}", c.name, c.phones.join(" "), c.emails.join(" "), c.note).to_lowercase(); words.iter().all(|w| hay.contains(w)) }).collect()
}

pub fn run() -> i32 {
    let Some(mut s) = Screen::start("contacts") else { return 1 };
    let (mut sel, mut q, mut note) = (Sel::default(), String::new(), String::new());
    loop {
        let (cols, rows) = crate::term::size();
        let w = width(cols);
        let all = load();
        let shown = search(&all, &q);
        let nw = (w / 3).clamp(10, 26);
        let list_rows: Vec<String> = shown.iter().map(|c| format!("{DIM}{:<3}{R} {FG}{}{R} {DIM}{}{R} {DIM}{}{R}", c.id, pad(&fit(&c.name, nw), nw), pad(&fit(c.phones.first().map_or("", |p| p), 16), 16), fit(c.emails.first().map_or("", |e| e), w.saturating_sub(nw + 24)))).collect();
        let (np, ne) = (all.iter().map(|c| c.phones.len()).sum::<usize>(), all.iter().map(|c| c.emails.len()).sum::<usize>());
        let p = Page { title: "contacts", right: &if q.is_empty() { String::new() } else { format!("search: {q}") }, count: if q.is_empty() { count(all.len(), "contact", "contacts") } else { format!("{} of {}", shown.len(), count(all.len(), "contact", "contacts")) },
            header: format!("id  {}{}email", pad("name", nw + 1), pad("phone", 17)), rows: list_rows, foot: vec![format!("{DIM}{} total{R}", all.len()), format!("{DIM}{np} phones{R}"), format!("{DIM}{ne} emails{R}")], note: note.clone(),
            hints: vec![("a", "add"), ("e", "edit"), ("d", "delete"), ("/", "search"), ("c", "clear search"), ("q", "quit")], empty: if q.is_empty() { "No contacts yet. Press a to add one." } else { "No contact matches." } };
        let mut l = p.lines(&mut sel, cols, rows);
        let n = l.len();
        if let (Some(c), true) = (shown.get(sel.pos), n >= 12 && l[n - 3..n - 1].iter().all(|x| x.is_empty()) && l[n - 5].is_empty()) {   // the selected contact in full, in the free rows above the hints
            l[n - 5] = rule(w);
            l[n - 4] = format!(" {FG}{}{R}  {DIM}{}  {}{R}", c.name, c.phones.join(", "), c.emails.join(", "));
            if !c.note.is_empty() { l[n - 3] = format!(" {DIM}{}{R}", fit(&c.note, w)); }
        }
        draw(&l);
        let Some(keys) = s.keys(-1) else { return 0 };
        for k in keys {
            note.clear();
            if sel.key(&k, shown.len()) { continue }
            match k {
                Key::Char('q') | Key::Ctrl(b'c') => return 0,
                Key::Char('/') => { if let Some(x) = s.ask("search:", &q) { q = x; sel = Sel::default(); } }
                Key::Char('c') => { q.clear(); sel = Sel::default(); }
                Key::Char('a') => note = edit(&mut s, None),
                Key::Char('e') | Key::Enter if !shown.is_empty() => note = edit(&mut s, Some(shown[sel.pos.min(shown.len() - 1)].id)),
                Key::Char('d') if !shown.is_empty() => { let c = shown[sel.pos.min(shown.len() - 1)]; if s.confirm(&format!("delete {}?", fit(&c.name, 30))) { let mut v = load(); v.retain(|x| x.id != c.id); save(&v); } }
                _ => {}
            }
        }
    }
}

fn edit(s: &mut Screen, id: Option<u32>) -> String {
    let mut v = load();
    let old = id.and_then(|i| v.iter().find(|c| c.id == i).cloned());
    let o = old.clone().unwrap_or_default();
    let Some(name) = s.ask("name:", &o.name) else { return String::new() };
    if name.trim().is_empty() { return format!("{BAD}the contact needs a name{R}") }
    let Some(ph) = s.ask("phones (comma separated):", &o.phones.join(", ")) else { return String::new() };
    let phones = match parse_list(&ph, phone_ok) { Ok(p) => p, Err(b) => return format!("{BAD}not a phone number: {}{R}", fit(&printable(&b), 30)) };
    let Some(em) = s.ask("emails (comma separated):", &o.emails.join(", ")) else { return String::new() };
    let emails = match parse_list(&em, email_ok) { Ok(p) => p, Err(b) => return format!("{BAD}not an email address: {}{R}", fit(&printable(&b), 30)) };
    let Some(n) = s.ask("note:", &o.note) else { return String::new() };
    let c = Contact { id: old.as_ref().map_or_else(|| store::next_id(v.iter().map(|c| c.id)), |c| c.id), name: printable(name.trim()), phones, emails, note: printable(n.trim()) };
    match old { Some(_) => { if let Some(x) = v.iter_mut().find(|x| x.id == c.id) { *x = c; } } None => { if v.len() >= MAX { return format!("{BAD}too many contacts{R}") } v.push(c); } }
    save(&v); format!("{GOOD}\u{2713}{R} contact saved")
}

#[cfg(test)]
mod tests {
    use super::*;
    fn c(id: u32, n: &str, p: &[&str], e: &[&str], note: &str) -> Contact { Contact { id, name: n.into(), phones: p.iter().map(|x| x.to_string()).collect(), emails: e.iter().map(|x| x.to_string()).collect(), note: note.into() } }
    #[test] fn file_round_trip_and_bad_lines() {
        let v = vec![c(1, "Kasun Perera", &["0771234567", "+94 11 222 3333"], &["k@x.lk"], "friend from school"), c(2, "Ann", &[], &[], "")];
        assert_eq!(parse(&format(&v)), v);
        assert_eq!(parse("junk\nx\tName\t\t\t\n3\t \t\t\t\n4\tOk\t\t\t\n").len(), 1);
    }
    #[test] fn checks() {
        for p in ["0771234567", "+94 77 123 4567", "(011) 222-3333", "123"] { assert!(phone_ok(p), "{p}"); }
        for p in ["12", "abc", "077 1234 5678 9012 3456 7", "07712;34567", ""] { assert!(!phone_ok(p), "{p}"); }
        for e in ["a@b.lk", "first.last@mail.example.com"] { assert!(email_ok(e), "{e}"); }
        for e in ["a@b", "@b.lk", "a b@c.lk", "a@@b.lk", "a@.lk", "a@b."] { assert!(!email_ok(e), "{e}"); }
        assert_eq!(parse_list("077 123 4567, +94 11 222 3333,", phone_ok).unwrap().len(), 2); assert_eq!(parse_list("123, abc", phone_ok), Err("abc".into())); assert_eq!(parse_list("", phone_ok).unwrap().len(), 0);
    }
    #[test] fn searching() {
        let v = vec![c(1, "Kasun Perera", &["0771234567"], &["k@x.lk"], "friend"), c(2, "Nimal Silva", &["0112223333"], &[], "")];
        assert_eq!(search(&v, "kasun").len(), 1); assert_eq!(search(&v, "SILVA 011").len(), 1); assert_eq!(search(&v, "0771").len(), 1); assert_eq!(search(&v, "x.lk friend")[0].id, 1);
        assert_eq!(search(&v, "").len(), 2); assert_eq!(search(&v, "zzz").len(), 0); assert_eq!(search(&v, "kasun silva").len(), 0);
    }
}
