//! The look shared by every app ("calm navy and peach"): a peach lower-case title with a dim context on the right, dim column heads over a hairline rule,
//! rows with the selection as a filled row and a peach bar at its left edge, a key-hint footer pinned to the bottom ending in `q quit`.
//! Colour carries meaning: OK green done / on, peach active / here, WARN waiting, ERR failure; blue only for links and folders; dim for hints. The pure parts are tested.
//! Colours are 16-colour SGR (see theme::PALETTE) plus a few 24-bit ones den maps to its theme slots.
use crate::{line::{Decoder, Key}, term::{self, put, Raw}};

pub const ACC: &str = "\x1b[38;2;240;178;127m"; pub const DIM: &str = "\x1b[90m"; pub const GOOD: &str = "\x1b[32m"; pub const CYAN: &str = "\x1b[34m";   // CYAN is the link blue
pub const WARN: &str = "\x1b[33m"; pub const BAD: &str = "\x1b[31m"; pub const R: &str = "\x1b[0m"; pub const MAG: &str = "\x1b[35m";
pub const FG: &str = "\x1b[39m";
pub const BDIM: &str = "\x1b[38;2;30;48;80m";   // BORDER_DIM
pub const SELBG: &str = "\x1b[48;2;21;41;74m";   // SEL
pub const WALLFG: &str = "\x1b[38;2;8;21;43m"; pub const ACCBG: &str = "\x1b[48;2;240;178;127m";   // a cursor or today cell: WALL text on a peach fill

/// Characters of `s` that show: escape sequences do not count.
pub fn vis(s: &str) -> usize { let mut esc = false; s.chars().filter(|&c| { if c == '\x1b' { esc = true; false } else if esc { esc = !c.is_ascii_alphabetic(); false } else { true } }).count() }
/// `s` cut to `max` characters, the last one an ellipsis when something was cut (plain text only).
pub fn fit(s: &str, max: usize) -> String {
    if s.chars().count() <= max { return s.to_string() }
    if max == 0 { return String::new() }
    s.chars().take(max - 1).chain(std::iter::once('\u{2026}')).collect()
}
/// `s` padded with spaces to `w` visible characters.
pub fn pad(s: &str, w: usize) -> String { format!("{s}{}", " ".repeat(w.saturating_sub(vis(s)))) }
/// Width of a table: the terminal minus the margins, between 30 and 160.
pub fn width(cols: usize) -> usize { cols.saturating_sub(3).clamp(30, 160) }
pub fn rule(w: usize) -> String { format!(" {BDIM}{}{R}", "\u{2500}".repeat(w)) }
/// Column heads: dim, lower case.
pub fn hdr(s: &str) -> String { format!(" {DIM}{}{R}", s.to_lowercase()) }
/// Line 1 of every app: the title in peach (lower case, as the caller gives it) on the left, one dim context on the right.
pub fn head(title: &str, right: &str, w: usize) -> String { format!(" {ACC}{title}{R}{}{DIM}{right}{R}", " ".repeat(w.saturating_sub(vis(title) + vis(right)).max(2))) }
pub fn dots(parts: &[String]) -> String { format!(" {}", parts.join(&format!(" {DIM}\u{b7}{R} "))) }
/// ` a add   d delete`: the key in FG, its meaning dim, three spaces between pairs.
pub fn hints(h: &[(&str, &str)]) -> String { format!(" {}", h.iter().map(|(k, d)| format!("{FG}{k}{R} {DIM}{d}{R}")).collect::<Vec<_>>().join("   ")) }
/// `3 contacts` / `1 contact`.
pub fn count(n: usize, one: &str, many: &str) -> String { format!("{n} {}", if n == 1 { one } else { many }) }
/// A selected row: the whole row on the SEL fill with a peach bar at the left edge; `text` starts at the second column. Width `w` + 1.
pub fn sel_row(text: &str, w: usize) -> String { format!("{SELBG}{ACC}\u{258c}{R}{SELBG}{}{}{R}", text.replace(R, &format!("{R}{SELBG}")), " ".repeat((w + 1).saturating_sub(vis(text) + 1))) }

/// The selected row of a list and the first row on screen.
#[derive(Default)]
pub struct Sel { pub pos: usize, pub top: usize }
impl Sel {
    /// Moves for Up, Down, PgUp, PgDn, Home, End; true when the key was one of those.
    pub fn key(&mut self, k: &Key, n: usize) -> bool {
        match k {
            Key::Up => self.pos = self.pos.saturating_sub(1), Key::Down => self.pos = (self.pos + 1).min(n.saturating_sub(1)),
            Key::PgUp => self.pos = self.pos.saturating_sub(8), Key::PgDn => self.pos = (self.pos + 8).min(n.saturating_sub(1)),
            Key::Home => self.pos = 0, Key::End => self.pos = n.saturating_sub(1), _ => return false,
        }
        true
    }
    /// The rows to show in `h` lines of `n`, with the selection kept inside.
    pub fn window(&mut self, n: usize, h: usize) -> std::ops::Range<usize> {
        self.pos = self.pos.min(n.saturating_sub(1)); let h = h.max(1);
        if self.pos < self.top { self.top = self.pos } else if self.pos >= self.top + h { self.top = self.pos + 1 - h }
        self.top = self.top.min(n.saturating_sub(h));
        self.top..(self.top + h).min(n)
    }
}

/// A list screen. `rows` hold the text of each row (the selection marker is added here). `count` is the dim context on the right of the title.
pub struct Page<'a> { pub title: &'a str, pub right: &'a str, pub count: String, pub header: String, pub rows: Vec<String>, pub foot: Vec<String>, pub note: String, pub hints: Vec<(&'a str, &'a str)>, pub empty: &'a str }
impl Page<'_> {
    /// Exactly `rows` lines when there is room: the key hints are on the last one, the note above it.
    pub fn lines(&self, sel: &mut Sel, cols: usize, rows: usize) -> Vec<String> {
        let w = width(cols);
        let mut v = vec![head(self.title, if self.right.is_empty() { &self.count } else { self.right }, w), String::new(), hdr(&self.header), rule(w)];
        let tail = if self.foot.is_empty() { 0 } else { 2 };
        let room = rows.saturating_sub(v.len() + tail + 3);
        let win = sel.window(self.rows.len(), room);
        for i in win { v.push(if i == sel.pos { sel_row(&self.rows[i], w) } else { format!(" {}", self.rows[i]) }); }
        if self.rows.is_empty() { v.push(format!(" {DIM}{}{R}", self.empty)); }
        if tail > 0 { v.push(rule(w)); v.push(dots(&self.foot)); }
        while v.len() + 2 < rows { v.push(String::new()); }
        v.push(if self.note.is_empty() { String::new() } else { format!(" {}", self.note) });
        v.push(hints(&self.hints));
        v
    }
}

/// Puts `lines` on the screen from the top; each line is cut to the terminal and the rest of the screen is cleared.
pub fn draw(lines: &[String]) {
    let (cols, rows) = term::size();
    let mut o = String::from("\x1b[?25l\x1b[H");
    for (i, l) in lines.iter().take(rows).enumerate() { if i > 0 { o += "\r\n"; } o += &cut(l, cols.saturating_sub(1)); o += "\x1b[0m\x1b[K"; }   // no newline after the last line: a full screen must not scroll
    o += "\x1b[J"; put(o.as_bytes());
}
/// `l` cut to `max` visible characters (escape sequences stay whole).
pub fn cut(l: &str, max: usize) -> String {
    let (mut o, mut n, mut esc) = (String::new(), 0, false);
    for c in l.chars() { if esc { esc = !c.is_ascii_alphabetic(); o.push(c); } else if c == '\x1b' { esc = true; o.push(c); } else if n < max { n += 1; o.push(c); } }
    o
}

/// Raw mode, hidden cursor and the app name as the pane title; all undone on drop. None (with a message) when not on a terminal.
pub struct Screen { _raw: Raw, pub dec: Decoder }
impl Screen {
    pub fn start(app: &str) -> Option<Screen> {
        if unsafe { libc::isatty(0) != 1 || libc::isatty(1) != 1 } { eprintln!("{app}: not a terminal"); return None }
        let raw = Raw::enter()?;
        put(format!("\x1b]2;{app}\x07\x1b[?25l\x1b[2J").as_bytes());
        Some(Screen { _raw: raw, dec: Decoder::new() })
    }
    /// Waits up to `ms` for keys; None = input ended.
    pub fn keys(&mut self, ms: i32) -> Option<Vec<Key>> { term::read_keys(&mut self.dec, ms) }
    /// A line typed on the last row, starting with `initial`; Enter returns it, Ctrl-C cancels (None).
    pub fn ask(&mut self, label: &str, initial: &str) -> Option<String> {
        let rows = term::size().1;
        let r = term::prompt_with(&mut self.dec, rows, &format!("{FG}{label}{R} "), initial);
        put(b"\x1b[?25l"); r
    }
    /// A yes/no question on the last row; only `y` is yes.
    pub fn confirm(&mut self, q: &str) -> bool {
        let rows = term::size().1;
        let key = if q.starts_with("delete") || q.starts_with("remove") { BAD } else { GOOD };
        put(format!("\x1b[{rows};1H\x1b[K {FG}{q}{R}   {key}[y]{R} yes   {DIM}[n] no (default){R}").as_bytes());
        loop { match self.keys(-1) { Some(k) => for k in k { match k { Key::Char('y') | Key::Char('Y') => return true, Key::Char(_) | Key::Enter | Key::Ctrl(_) | Key::Backspace => return false, _ => {} } }, None => return false } }
    }
}
impl Drop for Screen { fn drop(&mut self) { put(b"\x1b[0m\x1b]2;\x07\x1b[H\x1b[2J\x1b[?25h"); } }

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn visible_width_ignores_escapes() { assert_eq!(vis("\x1b[1;97mab\x1b[0mc"), 3); assert_eq!(pad("\x1b[32mab\x1b[0m", 4), "\x1b[32mab\x1b[0m  "); }
    #[test] fn fit_cuts_with_ellipsis() { assert_eq!(fit("abcdef", 4), "abc\u{2026}"); assert_eq!(fit("abc", 4), "abc"); assert_eq!(fit("abc", 0), ""); }
    #[test] fn cut_keeps_escapes_whole() { assert_eq!(cut("\x1b[32mabcdef\x1b[0m", 3), "\x1b[32mabc\x1b[0m"); }
    #[test] fn selection_stays_in_window() {
        let mut s = Sel::default();
        for _ in 0..12 { s.key(&Key::Down, 20); }
        assert_eq!(s.pos, 12); assert_eq!(s.window(20, 5), 8..13);
        s.key(&Key::End, 20); assert_eq!(s.window(20, 5), 15..20);
        s.key(&Key::Home, 20); assert_eq!(s.window(20, 5), 0..5);
        assert_eq!(Sel::default().window(0, 5), 0..0);
        let mut s = Sel { pos: 9, top: 0 }; assert_eq!(s.window(3, 5), 0..3); assert_eq!(s.pos, 2);
    }
    #[test] fn page_has_the_table_shape() {
        let p = Page { title: "contacts", right: "", count: "1 contact".into(), header: "NAME".into(), rows: vec!["Ann".into()], foot: vec!["1 total".into()], note: String::new(), hints: vec![("a", "add")], empty: "none" };
        let l = p.lines(&mut Sel::default(), 80, 24);
        assert_eq!(l.len(), 24);
        assert!(l[0].contains("contacts") && l[0].contains("1 contact")  && l[2].contains("name") && l[4].contains('\u{258c}') && l[4].contains(SELBG) && l.last().unwrap().contains("add"));
        assert_eq!((vis(&l[0]), vis(&l[4])), (vis(&rule(77)), vis(&rule(77))));   // the table is as wide as the rule
        assert!(hints(&[("q", "quit")]).ends_with(&format!("{DIM}quit{R}")));
    }
}
