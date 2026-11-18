//! help: one guide to thir: the desktop, the shell, the apps and their keys. Scroll with the arrow keys; 1 to 8 jump to a section.
use crate::{line::Key, tui::*};

/// (section, rows of (keys or name, what it does)); a row with no keys is a plain line.
pub const GUIDE: &[(&str, &[(&str, &str)])] = &[
    ("THE DESKTOP", &[("", "thir shows terminal panes side by side on a coloured wallpaper. The top bar has the time and your notices."),
        ("Option + Enter", "split the screen: a new pane (Cmd + Enter works too)"), ("Option + arrows", "move between panes"), ("Option + q", "close a pane; the last one logs you out"), ("Option + f", "make a pane full screen")]),
    ("THE SHELL", &[("Tab", "complete a name; press it twice to list the choices"), ("Up / Down", "history; Right accepts the grey suggestion"), ("Ctrl-A / Ctrl-E", "start / end of the line"),
        ("Ctrl-U / Ctrl-K", "cut to the start / end"), ("Ctrl-W", "cut one word"), ("Ctrl-L", "clear the screen"), ("Ctrl-C", "cancel; Ctrl-D leaves"), ("cmd > file", "send the output to a file (>> adds to it)")]),
    ("FILES AND SYSTEM", &[("ls cat cd pwd", "look around: list, show a file, change directory, where am I"), ("mkdir rmdir touch", "make and remove directories, make an empty file"), ("rm mv cp", "remove (-r for directories), move or rename, copy"),
        ("echo write", "print text; write file stores lines until a line with only ."), ("edit  find", "the text editor (also nano); find files by name"), ("top  fetch", "CPU and memory; system summary with a bear (neofetch)"),
        ("curl URL", "download a page or file over HTTP or HTTPS"), ("date  tz", "show the time; tz +05:30 sets the time zone"), ("passwd  whoami", "change your password; who am I"), ("poweroff  reboot", "shut down or restart")]),
    ("CLOCK", &[("clock", "big time, stopwatch, timer and alarms. Keys 1-4 or Tab switch the views"), ("Stopwatch", "space start/stop, l lap, r reset"), ("Timer", "t starts one (5m, 1h30m, 90s), c cancels it"),
        ("Alarms", "a add, e edit, space on/off, d delete. Repeat: once, daily, weekdays or days like mon,wed"), ("", "Alarms and timers ring even when the clock is closed: a helper runs in the background after login.")]),
    ("CALENDAR", &[("calendar", "a month with a dot on days that have events"), ("arrows", "move by day or week; [ and ] change month; t is today"), ("a e d", "add, edit, delete an event on the chosen day"),
        ("", "Each event can have a reminder (15m, 2h, 1d before). It shows in the top bar and rings.")]),
    ("CALC", &[("calc", "type an expression and press Enter: 2+3*4, (1+2)^3, sqrt(2), 10%3, pi, ans*2"), ("Up / Down", "bring back an earlier line; Ctrl-U clears; Ctrl-D leaves"), ("", "Functions: sqrt abs sin cos tan ln log exp floor ceil round.")]),
    ("CONTACTS AND NOTES", &[("contacts", "a add, e edit, d delete, / search, c clear the search; each has phones, emails and a note"), ("notes", "a new note, Enter opens it, d deletes, / searches title, tags and text"),
        ("In a note", "space ticks a todo; a adds a line; t adds a todo ([ ] or [x]); e edits; d deletes; r renames; g sets tags")]),
    ("SOUND AND SETTINGS", &[("settings", "time zone, wallpaper (blue and four more), sound on/off, volume, test sound"), ("beep", "play the alarm chime; beep 440 300 plays one note (Hz, ms)"),
        ("", "Without a sound device the top bar notice flashes instead."), ("Your data", "~/.config/thir/<app>/ in plain text files only you can read")]),
];

/// The lines of the guide for a screen `w` wide: (text, is a heading).
pub fn lines(w: usize) -> Vec<(String, bool)> {
    let mut v = vec![];
    for (sec, rows) in GUIDE {
        v.push((String::new(), false)); v.push((sec.to_string(), true));
        for (k, d) in *rows {
            if k.is_empty() { for part in wrap(d, w.saturating_sub(2)) { v.push((format!("  {DIM}{part}{R}"), false)); } continue }
            let kw = 18.min(w / 3);
            let mut it = wrap(d, w.saturating_sub(kw + 3)).into_iter();
            v.push((format!("  {CYAN}{}{R} {}", pad(&fit(k, kw), kw), it.next().unwrap_or_default()), false));
            for more in it { v.push((format!("  {}  {more}", " ".repeat(kw)), false)); }
        }
    }
    v
}
/// `s` broken into lines of at most `w` characters at spaces.
pub fn wrap(s: &str, w: usize) -> Vec<String> {
    let (mut out, mut cur) = (vec![], String::new());
    for word in s.split(' ') {
        if !cur.is_empty() && cur.chars().count() + 1 + word.chars().count() > w.max(8) { out.push(std::mem::take(&mut cur)); }
        if !cur.is_empty() { cur.push(' '); } cur.push_str(word);
    }
    if !cur.is_empty() { out.push(cur); } out
}

pub fn run() -> i32 {
    let Some(mut s) = Screen::start("help") else { return 1 };
    let mut top = 0usize;
    loop {
        let (cols, rows) = crate::term::size(); let w = width(cols);
        let all = lines(w);
        let room = rows.saturating_sub(7);
        top = top.min(all.len().saturating_sub(room));
        let mut l = vec![String::new(), head("HELP", "the thir guide", w), format!("  {DIM}{}{R}", count(GUIDE.len(), "section", "sections")), String::new(), hdr("KEYS AND COMMANDS"), rule(w)];
        for (t, h) in all.iter().skip(top).take(room) { l.push(if *h { format!("  {ACC}{t}{R}") } else { t.clone() }); }
        while l.len() < rows.saturating_sub(3) { l.push(String::new()); }
        l.push(rule(w)); l.push(dots(&[format!("{GOOD}{} sections{R}", GUIDE.len()), format!("{}%", if all.len() <= room { 100 } else { (top + room) * 100 / all.len() })]));
        l.push(hints(&[("up/down", "scroll"), ("space", "page"), ("1-8", "section"), ("q", "quit")]));
        draw(&l);
        let Some(keys) = s.keys(-1) else { return 0 };
        for k in keys {
            match k {
                Key::Char('q') | Key::Ctrl(b'c') | Key::Ctrl(b'd') => return 0,
                Key::Up => top = top.saturating_sub(1), Key::Down => top += 1, Key::PgUp => top = top.saturating_sub(room), Key::PgDn | Key::Char(' ') => top += room,
                Key::Home => top = 0, Key::End => top = usize::MAX / 2,
                Key::Char(c @ '1'..='8') => { let name = GUIDE[(c as usize - '1' as usize).min(GUIDE.len() - 1)].0; top = all.iter().position(|(t, h)| *h && t == name).unwrap_or(0); }
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn wrapping() { assert_eq!(wrap("aaa bbb ccc", 8), ["aaa bbb", "ccc"]); assert_eq!(wrap("x", 3), ["x"]); assert!(wrap("", 10).is_empty()); assert_eq!(wrap("averyveryverylongword end", 8)[0], "averyveryverylongword"); }
    #[test] fn every_app_is_in_the_guide() {
        let text: String = GUIDE.iter().flat_map(|s| s.1.iter()).map(|r| format!("{} {}\n", r.0, r.1)).collect();
        for a in crate::apps::NAMES.iter().filter(|a| **a != "bell") { assert!(text.contains(a), "{a} is missing from help"); }
        assert!(GUIDE.len() <= 8);
    }
    #[test] fn lines_fit_the_width() { for w in [30, 56, 72] { assert!(lines(w).iter().all(|(t, _)| vis(t) <= w + 4), "{w}"); } }
}
