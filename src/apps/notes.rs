//! notes: notes with created and modified times, tags and todo lines. ~/.config/thir/notes/notes, one per line:
//! ID TAB CREATED TAB MODIFIED TAB TAGS (comma list) TAB TITLE TAB BODY. The body is lines joined by newlines; a line that starts with `[ ] ` or `[x] ` is a todo you can tick.
use crate::{clock, files::printable, line::Key, store, tui::*};

#[derive(Clone, PartialEq, Debug, Default)]
pub struct Note { pub id: u32, pub created: i64, pub modified: i64, pub tags: Vec<String>, pub title: String, pub body: Vec<String> }
const MAX: usize = 500;
const MAX_LINES: usize = 200;

pub fn parse(text: &str) -> Vec<Note> {
    text.lines().filter_map(|l| {
        let f = store::split(l, 6)?;
        let title = printable(f[4].trim()); if title.is_empty() { return None }
        Some(Note { id: f[0].parse().ok()?, created: f[1].parse().ok()?, modified: f[2].parse().ok()?, tags: f[3].split(',').filter_map(tag).take(8).collect(), title: title.chars().take(80).collect(),
            body: f[5].split('\n').map(|x| printable(x).chars().take(200).collect::<String>()).filter(|x: &String| !x.trim().is_empty()).take(MAX_LINES).collect() })
    }).take(MAX).collect()
}
pub fn format(v: &[Note]) -> String { v.iter().map(|n| store::join(&[&n.id.to_string(), &n.created.to_string(), &n.modified.to_string(), &n.tags.join(","), &n.title, &n.body.join("\n")]) + "\n").collect() }
pub fn load() -> Vec<Note> { let mut v = parse(&store::read("notes", "notes")); v.sort_by_key(|n| std::cmp::Reverse(n.modified)); v }
pub fn save(v: &[Note]) { let _ = store::write("notes", "notes", &format(v)); }

/// A tag: lower case letters, digits, - and _, at most 20; None for anything else (after trimming and a leading #).
pub fn tag(s: &str) -> Option<String> { let t = s.trim().trim_start_matches('#').to_lowercase(); (!t.is_empty() && t.len() <= 20 && t.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')).then_some(t) }
pub fn parse_tags(s: &str) -> Result<Vec<String>, String> {
    let mut v: Vec<String> = vec![];
    for t in s.split([',', ' ']).filter(|t| !t.trim().is_empty()) { let t = tag(t).ok_or_else(|| t.trim().to_string())?; if !v.contains(&t) { v.push(t); } }
    if v.len() > 8 { return Err("more than 8 tags".into()) } Ok(v)
}
/// Some(done) for a todo line, None for plain text.
pub fn todo(line: &str) -> Option<bool> { if line.starts_with("[ ] ") { Some(false) } else if line.starts_with("[x] ") || line.starts_with("[X] ") { Some(true) } else { None } }
/// The line with its box ticked or unticked; plain text is left alone.
pub fn toggle(line: &str) -> String { match todo(line) { Some(d) => format!("{} {}", if d { "[ ]" } else { "[x]" }, &line[4..]), None => line.to_string() } }
/// (done, total) todo lines.
pub fn progress(n: &Note) -> (usize, usize) { let t: Vec<bool> = n.body.iter().filter_map(|l| todo(l)).collect(); (t.iter().filter(|d| **d).count(), t.len()) }
pub fn search<'a>(v: &'a [Note], q: &str) -> Vec<&'a Note> {
    let words: Vec<String> = q.split_whitespace().map(str::to_lowercase).collect();
    v.iter().filter(|n| { let hay = format!("{} {} {}", n.title, n.tags.join(" "), n.body.join(" ")).to_lowercase(); words.iter().all(|w| hay.contains(w)) }).collect()
}
pub fn stamp(t: i64, off: i32) -> String { let l = clock::local(t, off); format!("{:04}-{:02}-{:02} {:02}:{:02}", l.year, l.month, l.day, l.hour, l.min) }
/// A new note with a clean title and body.
pub fn new_note(v: &[Note], title: &str, body: Vec<String>, now: i64) -> Note { Note { id: store::next_id(v.iter().map(|n| n.id)), created: now, modified: now, tags: vec![], title: printable(title.trim()).chars().take(80).collect(), body } }

pub fn run() -> i32 {
    let Some(mut s) = Screen::start("notes") else { return 1 };
    let (mut sel, mut q, mut note) = (Sel::default(), String::new(), String::new());
    let off = clock::home_tz().unwrap_or(0);
    loop {
        let (cols, rows) = crate::term::size();
        let w = width(cols);
        let all = load();
        let shown = search(&all, &q);
        let tw = w.saturating_sub(33).max(10);
        let list_rows: Vec<String> = shown.iter().map(|n| {
            let (d, t) = progress(n);
            let (g, c) = if t == 0 { ("\u{25cb}", DIM) } else if d == t { ("\u{2713}", GOOD) } else { ("\u{25d0}", WARN) };
            format!("{DIM}{:<3}{R} {c}{g}{R}  {}{ACC}{}{R}  {DIM}{}{R}", n.id, if t > 0 { format!("{c}{:<4}{R}", format!("{d}/{t}")) } else { "    ".into() }, pad(&fit(&n.title, tw.saturating_sub(4)), tw.saturating_sub(4)), stamp(n.modified, off))
        }).collect();
        let (n_open, done): (usize, usize) = all.iter().map(progress).fold((0, 0), |a, (d, t)| (a.0 + t - d, a.1 + d));
        let p = Page { title: "NOTES", right: &if q.is_empty() { String::new() } else { format!("search: {q}") }, count: if q.is_empty() { count(all.len(), "note", "notes") } else { format!("{} of {}", shown.len(), count(all.len(), "note", "notes")) },
            header: format!("#    S  TODO  {}MODIFIED", pad("TITLE", tw.saturating_sub(4) + 2)), rows: list_rows, foot: vec![format!("{} total", all.len()), format!("{WARN}{n_open} todo open{R}"), format!("{GOOD}{done} done{R}")], note: note.clone(),
            hints: vec![("enter", "open"), ("a", "new"), ("d", "delete"), ("/", "search"), ("c", "clear"), ("q", "quit")], empty: if q.is_empty() { "No notes yet. Press a to write one." } else { "No note matches." } };
        let mut l = p.lines(&mut sel, cols, rows.saturating_sub(1));
        if let Some(n) = shown.get(sel.pos) {
            l.insert(l.len() - 4, format!("  {DIM}tags{R} {}  {DIM}created {}{R}", if n.tags.is_empty() { format!("{DIM}-{R}") } else { n.tags.iter().map(|t| format!("{CYAN}#{t}{R}")).collect::<Vec<_>>().join(" ") }, stamp(n.created, off)));
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
                Key::Char('a') => {
                    let Some(t) = s.ask("title:", "") else { continue };
                    if t.trim().is_empty() { note = format!("{BAD}the note needs a title{R}"); continue }
                    let mut v = load(); let n = new_note(&v, &t, vec![], clock::now()); let id = n.id; v.push(n); save(&v);
                    open(&mut s, id, off); sel = Sel::default();
                }
                Key::Enter if !shown.is_empty() => { let id = shown[sel.pos.min(shown.len() - 1)].id; open(&mut s, id, off); }
                Key::Char('d') if !shown.is_empty() => { let n = shown[sel.pos.min(shown.len() - 1)]; if s.confirm(&format!("delete note {}?", fit(&n.title, 30))) { let mut v = load(); v.retain(|x| x.id != n.id); save(&v); } }
                _ => {}
            }
        }
    }
}

/// One note: its lines with a cursor; space ticks a todo.
fn open(s: &mut Screen, id: u32, off: i32) {
    let mut sel = Sel::default(); let mut msg = String::new();
    loop {
        let mut v = load();
        let Some(i) = v.iter().position(|n| n.id == id) else { return };
        let (cols, rows) = crate::term::size(); let w = width(cols);
        let n = &v[i]; let (d, t) = progress(n);
        let body: Vec<String> = n.body.iter().map(|b| match todo(b) { Some(true) => format!("{GOOD}\u{2611}{R} {DIM}{}{R}", &b[4..]), Some(false) => format!("{WARN}\u{2610}{R} {}", &b[4..]), None => format!("  {}", b) }).collect();
        let mut l = vec![String::new(), head(&n.title.to_uppercase(), &stamp(n.modified, off), w),
            format!("  {DIM}created {}  \u{b7}  {}  \u{b7}  {} lines{R}", stamp(n.created, off), if n.tags.is_empty() { "no tags".to_string() } else { n.tags.iter().map(|t| format!("#{t}")).collect::<Vec<_>>().join(" ") }, n.body.len()), String::new(), hdr("TEXT"), rule(w)];
        let win = sel.window(body.len(), rows.saturating_sub(l.len() + 4));
        for k in win { l.push(format!("{}{}", if k == sel.pos { format!(" {ACC}\u{25b8}{R}") } else { "  ".into() }, body[k].clone())); }
        if body.is_empty() { l.push(format!("  {DIM}Empty. Press a to add a line, or t to add a todo.{R}")); }
        l.push(rule(w)); l.push(dots(&[format!("{} lines", n.body.len()), format!("{WARN}{} open{R}", t - d), format!("{GOOD}{d} done{R}")]));
        l.push(if msg.is_empty() { String::new() } else { format!("  {msg}") });
        l.push(hints(&[("space", "tick"), ("a", "line"), ("t", "todo"), ("e", "edit"), ("d", "delete"), ("r", "title"), ("g", "tags"), ("q", "back")]));
        draw(&l);
        let Some(keys) = s.keys(-1) else { return };
        for k in keys {
            msg.clear();
            let n = v[i].body.len();
            if sel.key(&k, n) { continue }
            let mut changed = true;
            match k {
                Key::Char('q') | Key::Left | Key::Ctrl(b'c') => return,
                Key::Char(' ') | Key::Enter if n > 0 => { let p = sel.pos.min(n - 1); v[i].body[p] = toggle(&v[i].body[p]); }
                Key::Char('a') | Key::Char('t') if n < MAX_LINES => {
                    let todo_new = k == Key::Char('t');
                    match s.ask(if todo_new { "todo:" } else { "line:" }, "") { Some(x) if !x.trim().is_empty() => { let x = printable(x.trim()); let at = if n == 0 { 0 } else { sel.pos.min(n - 1) + 1 }; v[i].body.insert(at, if todo_new && todo(&x).is_none() { format!("[ ] {x}") } else { x }); sel.pos = at; } _ => changed = false }
                }
                Key::Char('e') if n > 0 => { let p = sel.pos.min(n - 1); match s.ask("line:", &v[i].body[p]) { Some(x) if !x.trim().is_empty() => v[i].body[p] = printable(x.trim()), _ => changed = false } }
                Key::Char('d') if n > 0 => { let p = sel.pos.min(n - 1); v[i].body.remove(p); }
                Key::Char('r') => match s.ask("title:", &v[i].title) { Some(x) if !x.trim().is_empty() => v[i].title = printable(x.trim()).chars().take(80).collect(), _ => changed = false },
                Key::Char('g') => match s.ask("tags (comma separated):", &v[i].tags.join(", ")) { Some(x) => match parse_tags(&x) { Ok(t) => v[i].tags = t, Err(b) => { msg = format!("{BAD}bad tag: {}{R}", fit(&printable(&b), 20)); changed = false } }, None => changed = false },
                _ => changed = false,
            }
            if changed { v[i].modified = clock::now(); save(&v); }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn n(id: u32, t: &str, tags: &[&str], body: &[&str]) -> Note { Note { id, created: 100, modified: 200, tags: tags.iter().map(|x| x.to_string()).collect(), title: t.into(), body: body.iter().map(|x| x.to_string()).collect() } }
    #[test] fn file_round_trip_and_bad_lines() {
        let v = vec![n(1, "Shopping", &["home", "list"], &["[ ] buy milk", "[x] eggs", "plain text"]), n(2, "Empty", &[], &[])];
        assert_eq!(parse(&format(&v)), v);
        assert_eq!(parse("junk\n1\tx\t2\t\tT\tb\n2\t1\t2\t\t \tb\n3\t1\t2\tA B,ok\tTitle\t\n").len(), 1);
        assert_eq!(parse("3\t1\t2\tA B,ok\tTitle\t\n")[0].tags, ["ok"]);
    }
    #[test] fn todos_toggle_and_count() {
        assert_eq!((todo("[ ] a"), todo("[x] a"), todo("[X] a"), todo("[ ]a"), todo("a")), (Some(false), Some(true), Some(true), None, None));
        assert_eq!(toggle("[ ] milk"), "[x] milk"); assert_eq!(toggle("[x] milk"), "[ ] milk"); assert_eq!(toggle("[X] milk"), "[ ] milk"); assert_eq!(toggle("note"), "note");
        assert_eq!(progress(&n(1, "x", &[], &["[ ] a", "[x] b", "c", "[x] d"])), (2, 3));
    }
    #[test] fn tags() {
        assert_eq!(parse_tags("Work, #home  x-1"), Ok(vec!["work".into(), "home".into(), "x-1".into()])); assert_eq!(parse_tags("a, a"), Ok(vec!["a".into()]));
        assert_eq!(parse_tags("ok, bad!"), Err("bad!".into())); assert!(parse_tags(&"abcdefghijkl".repeat(2)).is_err()); assert!(parse_tags("a b c d e f g h i").is_err()); assert_eq!(parse_tags(""), Ok(vec![]));
    }
    #[test] fn searching_and_stamps() {
        let v = vec![n(1, "Shopping", &["home"], &["[ ] buy milk"]), n(2, "Work", &["office"], &["call Ann"])];
        assert_eq!(search(&v, "milk")[0].id, 1); assert_eq!(search(&v, "OFFICE ann")[0].id, 2); assert_eq!(search(&v, "milk ann").len(), 0); assert_eq!(search(&v, "").len(), 2);
        assert_eq!(stamp(1794906125, 330), "2026-11-17 14:32");
        let x = new_note(&v, "  New\x1b[2J ", vec!["[ ] a".into()], 5); assert_eq!((x.id, x.created, x.title.as_str()), (3, 5, "New?[2J"));
    }
}
