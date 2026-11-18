//! The look shared by every app: a bold UPPERCASE title, a count line, a header row and rule, rows with coloured glyphs, a rule, a coloured footer, key hints.
//! Colour carries meaning: green done / ok, cyan active / info, yellow warning, red failure; dim only for hints. The pure parts are tested.
use crate::{line::{Decoder, Key}, term::{self, put, Raw}};

pub const ACC: &str = "\x1b[1;97m"; pub const DIM: &str = "\x1b[90m"; pub const GOOD: &str = "\x1b[32m"; pub const CYAN: &str = "\x1b[36m";
pub const WARN: &str = "\x1b[33m"; pub const BAD: &str = "\x1b[31m"; pub const R: &str = "\x1b[0m"; pub const MAG: &str = "\x1b[35m";
const LINE: &str = "\x1b[37m";

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
/// Width of a table: the terminal minus the margins, between 30 and 72.
pub fn width(cols: usize) -> usize { cols.saturating_sub(4).clamp(30, 72) }
pub fn rule(w: usize) -> String { format!("  {LINE}{}{R}", "\u{2500}".repeat(w)) }
pub fn hdr(s: &str) -> String { format!("  {ACC}{s}{R}") }
pub fn head(title: &str, right: &str, w: usize) -> String { format!("  {ACC}{title}{R}{}{CYAN}{right}{R}", " ".repeat(w.saturating_sub(vis(title) + vis(right)).max(2))) }
pub fn dots(parts: &[String]) -> String { format!("  {}", parts.join(&format!(" {DIM}\u{b7}{R} "))) }
/// `  a add   d delete`: the key in cyan, its meaning dim.
pub fn hints(h: &[(&str, &str)]) -> String { format!("  {}", h.iter().map(|(k, d)| format!("{CYAN}{k}{R} {DIM}{d}{R}")).collect::<Vec<_>>().join("   ")) }
/// `3 contacts` / `1 contact`.
pub fn count(n: usize, one: &str, many: &str) -> String { format!("{n} {}", if n == 1 { one } else { many }) }

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

/// A list screen. `rows` hold the text of each row (the ▸ marker is added here).
pub struct Page<'a> { pub title: &'a str, pub right: &'a str, pub count: String, pub header: String, pub rows: Vec<String>, pub foot: Vec<String>, pub note: String, pub hints: Vec<(&'a str, &'a str)>, pub empty: &'a str }
impl Page<'_> {
    pub fn lines(&self, sel: &mut Sel, cols: usize, rows: usize) -> Vec<String> {
        let w = width(cols);
        let mut v = vec![String::new(), head(self.title, self.right, w), format!("  {DIM}{}{R}", self.count), String::new(), hdr(&self.header), rule(w)];
        let room = rows.saturating_sub(v.len() + 4);
        let win = sel.window(self.rows.len(), room);
        for i in win { v.push(format!("{}{}", if i == sel.pos { format!(" {ACC}\u{25b8}{R}") } else { "  ".into() }, self.rows[i])); }
        if self.rows.is_empty() { v.push(format!("  {DIM}{}{R}", self.empty)); }
        v.push(rule(w)); v.push(dots(&self.foot));
        v.push(if self.note.is_empty() { String::new() } else { format!("  {}", self.note) });
        v.push(hints(&self.hints));
        v
    }
}

/// Puts `lines` on the screen from the top; each line is cut to the terminal and the rest of the screen is cleared.
pub fn draw(lines: &[String]) {
    let (cols, rows) = term::size();
    let mut o = String::from("\x1b[?25l\x1b[H");
    for l in lines.iter().take(rows) { o += &cut(l, cols.saturating_sub(1)); o += "\x1b[0m\x1b[K\r\n"; }
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
        let r = term::prompt_with(&mut self.dec, rows, &format!("{CYAN}{label}{R} "), initial);
        put(b"\x1b[?25l"); r
    }
    /// A yes/no question on the last row; only `y` is yes.
    pub fn confirm(&mut self, q: &str) -> bool {
        let rows = term::size().1;
        put(format!("\x1b[{rows};1H\x1b[K  {WARN}{q}{R} {DIM}[y/N]{R}").as_bytes());
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
        let p = Page { title: "CONTACTS", right: "", count: "1 contact".into(), header: "NAME".into(), rows: vec!["  Ann".into()], foot: vec!["1 total".into()], note: String::new(), hints: vec![("a", "add")], empty: "none" };
        let l = p.lines(&mut Sel::default(), 80, 24);
        assert!(l[1].contains("CONTACTS") && l[2].contains("1 contact") && l[4].contains("NAME") && l[6].contains('\u{25b8}') && l.last().unwrap().contains("add"));
    }
}
