//! The text buffer and screen renderer of /bin/edit. Pure: keys in, bytes for the terminal out.
use crate::{files::unsafe_char, line::Key};

pub const TABW: usize = 4;

pub struct Buf {
    pub lines: Vec<Vec<char>>, pub cy: usize, pub cx: usize, want: usize, pub top: usize, pub left: usize,
    pub modified: bool, cut: Vec<Vec<char>>, last_cut: bool,
}

/// What a line looks like on screen: tabs to the next multiple of TABW, unsafe characters as '?'.
pub fn expand(line: &[char]) -> Vec<char> {
    let mut o = vec![];
    for &c in line {
        if c == '\t' { while { o.push(' '); o.len() % TABW != 0 } {} } else { o.push(if unsafe_char(c) { '?' } else { c }); }
    }
    o
}
/// Screen column of char index `cx` in `line`.
pub fn col_of(line: &[char], cx: usize) -> usize { expand(&line[..cx.min(line.len())]).len() }

impl Buf {
    /// CRLF loads as LF; a final newline does not make an extra empty line.
    pub fn from_text(s: &str) -> Buf {
        let mut lines: Vec<Vec<char>> = s.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l).chars().collect()).collect();
        if s.ends_with('\n') || s.is_empty() { lines.pop(); }
        if lines.is_empty() { lines.push(vec![]); }
        Buf { lines, cy: 0, cx: 0, want: 0, top: 0, left: 0, modified: false, cut: vec![], last_cut: false }
    }
    pub fn text(&self) -> String {
        if self.lines.len() == 1 && self.lines[0].is_empty() { return String::new() }
        self.lines.iter().fold(String::new(), |mut o, l| { o.extend(l); o.push('\n'); o })
    }
    fn len(&self) -> usize { self.lines[self.cy].len() }
    fn edited(&mut self) { self.modified = true; self.want = self.cx; }

    pub fn insert(&mut self, c: char) { self.lines[self.cy].insert(self.cx, c); self.cx += 1; self.edited(); }
    pub fn newline(&mut self) {
        let rest = self.lines[self.cy].split_off(self.cx);
        self.cy += 1; self.lines.insert(self.cy, rest); self.cx = 0; self.edited();
    }
    pub fn backspace(&mut self) {
        if self.cx > 0 { self.cx -= 1; self.lines[self.cy].remove(self.cx); }
        else if self.cy > 0 { let l = self.lines.remove(self.cy); self.cy -= 1; self.cx = self.len(); self.lines[self.cy].extend(l); }
        else { return }
        self.edited();
    }
    pub fn delete(&mut self) {
        if self.cx < self.len() { self.lines[self.cy].remove(self.cx); }
        else if self.cy + 1 < self.lines.len() { let l = self.lines.remove(self.cy + 1); self.lines[self.cy].extend(l); }
        else { return }
        self.edited();
    }

    pub fn left(&mut self) {
        if self.cx > 0 { self.cx -= 1; } else if self.cy > 0 { self.cy -= 1; self.cx = self.len(); }
        self.want = self.cx;
    }
    pub fn right(&mut self) {
        if self.cx < self.len() { self.cx += 1; } else if self.cy + 1 < self.lines.len() { self.cy += 1; self.cx = 0; }
        self.want = self.cx;
    }
    pub fn up(&mut self) { if self.cy > 0 { self.cy -= 1; self.cx = self.want.min(self.len()); } }
    pub fn down(&mut self) { if self.cy + 1 < self.lines.len() { self.cy += 1; self.cx = self.want.min(self.len()); } }
    pub fn home(&mut self) { self.cx = 0; self.want = 0; }
    pub fn end(&mut self) { self.cx = self.len(); self.want = self.cx; }
    pub fn page(&mut self, rows: usize, down: bool) {
        let n = rows.saturating_sub(3).max(1);
        self.cy = if down { (self.cy + n).min(self.lines.len() - 1) } else { self.cy.saturating_sub(n) };
        self.cx = self.want.min(self.len());
    }

    /// Consecutive cuts collect into one paste buffer.
    pub fn cut_line(&mut self) {
        if !self.last_cut { self.cut.clear(); }
        self.cut.push(self.lines.remove(self.cy));
        if self.lines.is_empty() { self.lines.push(vec![]); }
        self.cy = self.cy.min(self.lines.len() - 1); self.cx = self.want.min(self.len());
        self.last_cut = true; self.modified = true;
    }
    pub fn paste(&mut self) {
        if self.cut.is_empty() { return }
        let n = self.cut.len();
        self.lines.splice(self.cy..self.cy, self.cut.clone());
        self.cy += n; self.cx = self.want.min(self.len()); self.modified = true;
    }

    /// Next match after the cursor, wrapping; false if there is none.
    pub fn find(&mut self, pat: &str) -> bool {
        if pat.is_empty() { return false }
        let n = self.lines.len();
        for i in 0..=n {
            let y = (self.cy + i) % n;
            let start = if i == 0 { self.cx + 1 } else { 0 }.min(self.lines[y].len());
            let s: String = self.lines[y][start..].iter().collect();
            if let Some(b) = s.find(pat) { self.cy = y; self.cx = start + s[..b].chars().count(); self.want = self.cx; return true }
        }
        false
    }

    /// Edit keys only; save, quit and find belong to the binary.
    pub fn key(&mut self, k: &Key, rows: usize) {
        if !matches!(k, Key::Ctrl(b'k')) { self.last_cut = false; }
        match k {
            Key::Char(c) => self.insert(*c), Key::Tab => self.insert('\t'), Key::Enter => self.newline(),
            Key::Backspace => self.backspace(), Key::Delete => self.delete(),
            Key::Left => self.left(), Key::Right => self.right(), Key::Up => self.up(), Key::Down => self.down(),
            Key::Home | Key::Ctrl(b'a') => self.home(), Key::End | Key::Ctrl(b'e') => self.end(),
            Key::PgUp => self.page(rows, false), Key::PgDn => self.page(rows, true),
            Key::Ctrl(b'k') => self.cut_line(), Key::Ctrl(b'u') => self.paste(),
            Key::Ctrl(_) | Key::Paste(_) => {}   // the editor never asks for bracketed paste: a paste arrives as typed keys
        }
    }

    /// Keep the cursor inside the text area (rows - 3 lines, cols - 1 columns).
    pub fn scroll(&mut self, rows: usize, cols: usize) {
        let (th, tw) = (rows.saturating_sub(3).max(1), cols.saturating_sub(1).max(1));
        if self.cy < self.top { self.top = self.cy; } else if self.cy >= self.top + th { self.top = self.cy + 1 - th; }
        let c = col_of(&self.lines[self.cy], self.cx);
        if c < self.left { self.left = c; } else if c >= self.left + tw { self.left = c + 1 - tw; }
    }
}

fn fit(s: &str, w: usize) -> String { s.chars().take(w).collect() }

/// The whole frame. Every row is addressed absolutely and cleared to its end; no row is longer than cols - 1 characters.
pub fn render(b: &Buf, cols: usize, rows: usize, name: &str, msg: &str) -> Vec<u8> {
    if rows < 4 || cols < 20 { return b"\x1b[H\x1b[2Jedit: terminal too small".to_vec() }
    let w = cols - 1;
    let mut o = String::from("\x1b[?25l");
    let pos = format!("Ln {}, Col {}", b.cy + 1, col_of(&b.lines[b.cy], b.cx) + 1);
    let name = format!("  {}{}", crate::files::printable(name), if b.modified { "  [Modified]" } else { "" });
    let name = fit(&name, w.saturating_sub(5 + pos.len() + 2));
    let pad = w.saturating_sub(4 + name.chars().count() + pos.len() + 1);
    o += &format!("\x1b[1;1H {}edit{}{name}{}{pos} {}\x1b[K", crate::tui::ACC, crate::tui::DIM, " ".repeat(pad), crate::tui::R);
    for r in 0..rows - 3 {
        let t: String = b.lines.get(b.top + r).map_or(String::new(), |l| expand(l).into_iter().skip(b.left).take(w).collect());
        o += &format!("\x1b[{};1H{t}\x1b[K", r + 2);
    }
    o += &format!("\x1b[{};1H{}\x1b[K", rows - 1, fit(&crate::files::printable(msg), w));
    let mut keys = vec![("ctrl+s", "save"), ("ctrl+f", "find"), ("ctrl+k", "cut line"), ("ctrl+u", "paste"), ("ctrl+q", "quit")];
    while keys.len() > 1 && crate::tui::vis(&crate::tui::hints(&keys)) > w { keys.remove(keys.len() - 2); }   // narrow terminal: drop the middle ones, never `quit`
    o += &format!("\x1b[{rows};1H{}\x1b[K", crate::tui::hints(&keys));
    o += &format!("\x1b[{};{}H\x1b[?25h", 2 + b.cy - b.top, 1 + col_of(&b.lines[b.cy], b.cx) - b.left);
    o.into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn buf(s: &str) -> Buf { Buf::from_text(s) }
    fn t(b: &Buf) -> String { b.text() }
    #[test] fn round_trip_empty_crlf() {
        assert_eq!(t(&buf("a\nb\n")), "a\nb\n"); assert_eq!(t(&buf("")), ""); assert_eq!(buf("").lines.len(), 1);
        assert_eq!(t(&buf("a\r\nb\r\n")), "a\nb\n"); assert_eq!(t(&buf("x")), "x\n"); assert_eq!(buf("a\n\nb").lines.len(), 3);
    }
    #[test] fn insert_utf8_and_newline() {
        let mut b = buf(""); for c in "aéb".chars() { b.insert(c); }
        assert_eq!((t(&b).as_str(), b.cx, b.modified), ("aéb\n", 3, true));
        b.left(); b.newline(); assert_eq!((t(&b).as_str(), b.cy, b.cx), ("aé\nb\n", 1, 0));
    }
    #[test] fn backspace_delete_join() {
        let mut b = buf("ab\ncd\n"); b.down(); b.backspace(); assert_eq!((t(&b).as_str(), b.cy, b.cx), ("abcd\n", 0, 2));
        let mut b = buf("ab\ncd\n"); b.end(); b.delete(); assert_eq!(t(&b), "abcd\n");
        let mut b = buf("a\n"); b.backspace(); b.end(); b.delete(); assert!(!b.modified);
    }
    #[test] fn sticky_column() {
        let mut b = buf("abcdef\nab\nabcdef\n"); b.end(); b.up(); b.down(); b.down();
        assert_eq!((b.cy, b.cx), (2, 6));
        let mut b = buf("abcdef\nab\nabcdef\n"); for _ in 0..4 { b.right(); } b.down(); assert_eq!(b.cx, 2); b.down(); assert_eq!(b.cx, 4);
    }
    #[test] fn paging() {
        let mut b = buf(&"x\n".repeat(50)); b.page(13, true); assert_eq!(b.cy, 10); b.page(13, false); b.page(13, false); assert_eq!(b.cy, 0);
        for _ in 0..9 { b.page(13, true); } assert_eq!(b.cy, 49);
    }
    #[test] fn cut_and_paste() {
        let mut b = buf("1\n2\n3\n4\n");
        b.key(&Key::Ctrl(b'k'), 24); b.key(&Key::Ctrl(b'k'), 24); assert_eq!(t(&b), "3\n4\n");
        b.down(); b.key(&Key::Ctrl(b'u'), 24); assert_eq!((t(&b).as_str(), b.cy), ("3\n1\n2\n4\n", 3));
        b.key(&Key::Up, 24); b.key(&Key::Ctrl(b'k'), 24); b.key(&Key::Ctrl(b'u'), 24); assert_eq!(t(&b), "3\n1\n2\n4\n");
    }
    #[test] fn cut_only_line() { let mut b = buf("x\n"); b.cut_line(); assert_eq!((t(&b).as_str(), b.lines.len()), ("", 1)); b.paste(); assert_eq!(t(&b), "x\n\n"); }   // pasted above the empty line that remains
    #[test] fn find_wraps() {
        let mut b = buf("foo bar\nbaz foo\n"); assert!(b.find("foo")); assert_eq!((b.cy, b.cx), (1, 4));
        assert!(b.find("foo")); assert_eq!((b.cy, b.cx), (0, 0)); assert!(!b.find("zzz")); assert!(!b.find(""));
        let mut b = buf("aé\n"); assert!(b.find("é")); assert_eq!(b.cx, 1);
    }
    #[test] fn expand_and_col() {
        let l: Vec<char> = "a\tb\x1b".chars().collect();
        assert_eq!(expand(&l).iter().collect::<String>(), "a   b?"); assert_eq!(col_of(&l, 2), 4); assert_eq!(col_of(&l, 0), 0); assert_eq!(col_of(&l, 99), 6);
    }
    #[test] fn scrolls() {
        let mut b = buf(&"x\n".repeat(50)); b.cy = 30; b.scroll(13, 40); assert_eq!(b.top, 21); b.cy = 5; b.scroll(13, 40); assert_eq!(b.top, 5);
        let mut b = buf(&format!("{}\n", "y".repeat(100))); b.end(); b.scroll(13, 40); assert_eq!(b.left, 100 + 1 - 39);
    }
    #[test] fn renders() {
        let mut b = buf("hello\nworld\n"); b.right(); b.scroll(10, 40);
        let s = String::from_utf8(render(&b, 40, 10, "f.txt", "hi")).unwrap();
        assert!(s.contains("edit\x1b[90m  f.txt") && !s.contains("[Modified]") && s.contains("Ln 1, Col 2") && s.ends_with("\x1b[2;2H\x1b[?25h"), "{s:?}");
        b.insert('x'); assert!(String::from_utf8(render(&b, 40, 10, "f.txt", "")).unwrap().contains("[Modified]"));
        let long = buf(&"z".repeat(500)); let s = String::from_utf8(render(&long, 30, 8, &"n".repeat(200), &"m".repeat(200))).unwrap();
        for row in s.split("\x1b[").skip(1) { let body = row.split_once('H').map_or("", |x| x.1); let vis: String = body.chars().take_while(|&c| c != '\x1b').collect(); assert!(vis.chars().count() < 30, "{vis:?}"); }
        assert!(String::from_utf8(render(&b, 10, 3, "x", "")).unwrap().contains("too small"));
    }
}
