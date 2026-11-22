//! Terminal grid plus a small escape-sequence parser. Never panics, whatever bytes it gets.
use super::theme::{FG, PALETTE, SLOTS};
pub const DEF_FG: u8 = 16;
pub const DEF_BG: u8 = 17;

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Cell { pub ch: char, pub fg: u8, pub bg: u8, pub bold: bool, pub rev: bool }
const BLANK: Cell = Cell { ch: ' ', fg: DEF_FG, bg: DEF_BG, bold: false, rev: false };

#[derive(Clone, Copy, PartialEq)]
enum St { Ground, Esc, EscSkip, Csi, Osc }

pub struct Term {
    pub cols: usize, pub rows: usize, pub cells: Vec<Cell>,
    pub cx: usize, pub cy: usize, pub cursor_on: bool, pub dirty: bool,
    /// A full-screen app is running (`ESC [ ? 1049 h`, also 1047 and 47): den draws the pane opaque. shortcut: only the flag, the shell's screen is not saved (apps clear theirs when they end).
    pub alt: bool,
    /// The app asked for bracketed paste (`?2004`) and for mouse events (`?1000`, `?1002`, `?1003`; `?1006` = SGR form).
    pub bracketed: bool, pub mouse: bool, pub mouse_sgr: bool,
    /// Window title set by `ESC ] 0 ; text BEL` or `ESC ] 2 ; text BEL` (printable, at most 40 characters); `title_changed` tells the bar to redraw.
    pub title: String, pub title_changed: bool,
    pen: Cell, wrap: bool, st: St, params: Vec<u32>, private: bool, need: u8, acc: u32, osc: Vec<u8>,
}

impl Term {
    pub fn new(cols: usize, rows: usize) -> Term {
        let (cols, rows) = (cols.max(1), rows.max(1));
        Term { cols, rows, cells: vec![BLANK; cols * rows], cx: 0, cy: 0, cursor_on: true, dirty: true, alt: false, bracketed: false, mouse: false, mouse_sgr: false, title: String::new(), title_changed: false, pen: BLANK, wrap: false, st: St::Ground, params: vec![0], private: false, need: 0, acc: 0, osc: vec![] }
    }

    /// Keeps the top-left part of the old content.
    pub fn resize(&mut self, cols: usize, rows: usize) {
        let (cols, rows) = (cols.max(1), rows.max(1));
        let mut cells = vec![BLANK; cols * rows];
        for y in 0..rows.min(self.rows) { for x in 0..cols.min(self.cols) { cells[y * cols + x] = self.cells[y * self.cols + x]; } }
        self.cells = cells; self.cols = cols; self.rows = rows;
        self.cx = self.cx.min(cols - 1); self.cy = self.cy.min(rows - 1); self.wrap = false; self.dirty = true;
    }

    pub fn feed(&mut self, bytes: &[u8]) { for &b in bytes { self.byte(b); } self.dirty = true; }

    fn blank(&self) -> Cell { Cell { bold: false, rev: false, fg: DEF_FG, ch: ' ', ..self.pen } }   // erase uses the current background
    fn lf(&mut self) {
        self.wrap = false;
        if self.cy + 1 < self.rows { self.cy += 1 } else {
            let c = self.cols; self.cells.copy_within(c.., 0);
            let b = self.blank(); let n = self.cells.len(); self.cells[n - c..].fill(b);
        }
    }
    fn put(&mut self, ch: char) {
        if self.wrap { self.cx = 0; self.lf(); }
        self.cells[self.cy * self.cols + self.cx] = Cell { ch, ..self.pen };
        if self.cx + 1 == self.cols { self.wrap = true } else { self.cx += 1 }
    }

    fn byte(&mut self, b: u8) {
        if self.need > 0 {
            if b & 0xC0 == 0x80 {
                self.acc = self.acc << 6 | (b & 0x3F) as u32; self.need -= 1;
                if self.need == 0 { self.put(char::from_u32(self.acc).unwrap_or('\u{FFFD}')); }
                return;
            }
            self.need = 0; self.put('\u{FFFD}');
        }
        match (self.st, b) {
            (St::Osc, 0x07) => { self.st = St::Ground; self.end_osc(); }
            (St::Osc, 0x1B) => { self.st = St::Esc; self.end_osc(); }   // ESC \ ends it too; the \ is then swallowed as usual
            (_, 0x1B) => self.st = St::Esc,
            (_, 0x18 | 0x1A) => self.st = St::Ground,   // CAN and SUB abandon an OSC without applying it
            (St::Osc, _) => if self.osc.len() < 128 { self.osc.push(b) },
            (_, 0x00..=0x1F) => self.control(b),
            (St::Esc, b'[') => { self.st = St::Csi; self.params.clear(); self.params.push(0); self.private = false; }
            (St::Esc, b']') => { self.st = St::Osc; self.osc.clear(); }
            (St::Esc, 0x20..=0x2F) => self.st = St::EscSkip,   // e.g. ESC ( B: one more byte to ignore
            (St::Esc | St::EscSkip, _) => self.st = St::Ground,
            (St::Csi, b'0'..=b'9') => { let l = self.params.last_mut().unwrap(); *l = (*l * 10 + (b - b'0') as u32).min(9999); }
            (St::Csi, b';') => if self.params.len() < 16 { self.params.push(0) },
            (St::Csi, b'<'..=b'?') => self.private = true,
            (St::Csi, 0x20..=0x2F) => {}
            (St::Csi, 0x40..=0x7E) => { self.st = St::Ground; self.csi(b); }
            (St::Csi, _) => self.st = St::Ground,
            (St::Ground, 0x20..=0x7E) => self.put(b as char),
            (St::Ground, 0xC2..=0xDF) => { self.need = 1; self.acc = (b & 0x1F) as u32; }
            (St::Ground, 0xE0..=0xEF) => { self.need = 2; self.acc = (b & 0x0F) as u32; }
            (St::Ground, 0xF0..=0xF4) => { self.need = 3; self.acc = (b & 0x07) as u32; }
            (St::Ground, 0x7F) => {}
            (St::Ground, _) => self.put('\u{FFFD}'),
        }
    }

    fn end_osc(&mut self) {
        let t = String::from_utf8_lossy(&std::mem::take(&mut self.osc)).into_owned();
        let Some(text) = t.strip_prefix("0;").or_else(|| t.strip_prefix("2;")) else { return };
        let title: String = text.chars().filter(|c| !crate::files::unsafe_char(*c)).take(40).collect();
        if title != self.title { self.title = title; self.title_changed = true; }
    }

    fn control(&mut self, b: u8) {
        match b {
            b'\r' => { self.cx = 0; self.wrap = false; }
            b'\n' | 0x0B | 0x0C => self.lf(),
            0x08 => { self.cx = self.cx.saturating_sub(1); self.wrap = false; }
            b'\t' => { self.cx = ((self.cx / 8 + 1) * 8).min(self.cols - 1); self.wrap = false; }
            _ => {}   // BEL and the rest
        }
    }

    fn p(&self, i: usize, def: usize) -> usize { match self.params.get(i) { Some(&v) if v > 0 => v as usize, _ => def } }

    fn csi(&mut self, f: u8) {
        let (c, r) = (self.cols, self.rows);
        match f {
            b'A' => self.cy = self.cy.saturating_sub(self.p(0, 1)),
            b'B' => self.cy = (self.cy + self.p(0, 1)).min(r - 1),
            b'C' => self.cx = (self.cx + self.p(0, 1)).min(c - 1),
            b'D' => self.cx = self.cx.saturating_sub(self.p(0, 1)),
            b'H' | b'f' => { self.cy = (self.p(0, 1) - 1).min(r - 1); self.cx = (self.p(1, 1) - 1).min(c - 1); }
            b'J' => {
                let (a, e) = (self.cy * c + self.cx, self.cells.len());
                let (s, t) = match self.params[0] { 0 => (a, e), 1 => (0, a + 1), _ => (0, e) };
                let b = self.blank(); self.cells[s..t.min(e)].fill(b);
            }
            b'K' => {
                let row = self.cy * c;
                let (s, t) = match self.params[0] { 0 => (self.cx, c), 1 => (0, self.cx + 1), _ => (0, c) };
                let b = self.blank(); self.cells[row + s..row + t.min(c)].fill(b);
            }
            b'm' => self.sgr(),
            b'h' | b'l' if self.private && matches!(self.params[0], 47 | 1047 | 1049) => { self.alt = f == b'h'; if !self.alt { self.mouse = false; self.mouse_sgr = false; } }
            b'h' | b'l' if self.private && self.params[0] == 2004 => self.bracketed = f == b'h',
            b'h' | b'l' if self.private && matches!(self.params[0], 1000 | 1002 | 1003) => self.mouse = f == b'h',
            b'h' | b'l' if self.private && self.params[0] == 1006 => self.mouse_sgr = f == b'h',
            b'h' | b'l' if self.private && self.params[0] == 25 => self.cursor_on = f == b'h',
            _ => return,
        }
        self.wrap = false;
    }

    fn sgr(&mut self) {
        let mut i = 0;
        while i < self.params.len() {
            let n = self.params[i];
            if n == 38 || n == 48 {   // 38;2;r;g;b (24 bit) or 38;5;n (256 colours): kept as the nearest theme colour
                let c = match self.params.get(i + 1) { Some(2) if i + 4 < self.params.len() => { let c = nearest(self.params[i + 2], self.params[i + 3], self.params[i + 4]); i += 4; c } Some(5) => { i += 2; DEF_FG } _ => DEF_FG };
                if n == 38 { self.pen.fg = c } else { self.pen.bg = if c == DEF_FG { DEF_BG } else { c } }
                i += 1; continue;
            }
            let p = &mut self.pen;
            match n {
                0 => *p = BLANK,
                1 => p.bold = true, 22 => p.bold = false,
                7 => p.rev = true, 27 => p.rev = false,
                30..=37 => p.fg = (n - 30) as u8, 39 => p.fg = DEF_FG,
                40..=47 => p.bg = (n - 40) as u8, 49 => p.bg = DEF_BG,
                90..=97 => p.fg = (n - 90 + 8) as u8,
                100..=107 => p.bg = (n - 100 + 8) as u8,
                _ => {}
            }
            i += 1;
        }
    }
}

/// The cell color (0..15 palette or a theme slot) closest to an RGB; exact theme colors map to their slot.
fn nearest(r: u32, g: u32, b: u32) -> u8 {
    let rgb = (r.min(255) << 16) | (g.min(255) << 8) | b.min(255);
    if rgb == 0xd4dcec { return DEF_FG }
    let d = |c: u32| { let f = |s: u32| ((c >> s & 255) as i32 - (rgb >> s & 255) as i32).pow(2); f(16) + f(8) + f(0) };
    PALETTE.iter().enumerate().map(|(i, &c)| (i as u8, c)).chain(SLOTS).chain([(DEF_FG, FG)]).min_by_key(|x| d(x.1)).map_or(DEF_FG, |x| x.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn row(t: &Term, y: usize) -> String { t.cells[y * t.cols..(y + 1) * t.cols].iter().map(|c| c.ch).collect::<String>().trim_end().to_string() }
    #[test] fn text_and_cr_lf() { let mut t = Term::new(10, 3); t.feed(b"ab\r\ncd"); assert_eq!((row(&t, 0), row(&t, 1)), ("ab".into(), "cd".into())); assert_eq!((t.cx, t.cy), (2, 1)); }
    #[test] fn cursor_moves_clamp() {
        let mut t = Term::new(10, 5);
        t.feed(b"\x1b[3;4H"); assert_eq!((t.cx, t.cy), (3, 2));
        t.feed(b"\x1b[2A\x1b[C\x1b[2C"); assert_eq!((t.cx, t.cy), (6, 0));
        t.feed(b"\x1b[99B\x1b[99C"); assert_eq!((t.cx, t.cy), (9, 4));
        t.feed(b"\x1b[D\x1b[100D\x1b[100A"); assert_eq!((t.cx, t.cy), (0, 0));
        t.feed(b"\x1b[7;7f"); assert_eq!((t.cx, t.cy), (6, 4));
        t.feed(b"\x1b[H"); assert_eq!((t.cx, t.cy), (0, 0));
    }
    #[test] fn wraps_and_scrolls() {
        let mut t = Term::new(3, 2);
        t.feed(b"abcd"); assert_eq!((row(&t, 0), row(&t, 1)), ("abc".into(), "d".into()));
        t.feed(b"efgh"); assert_eq!((row(&t, 0), row(&t, 1)), ("def".into(), "gh".into()));
        t.feed(b"\n"); assert_eq!((row(&t, 0), row(&t, 1)), ("gh".into(), "".into()));
    }
    #[test] fn last_column_waits_before_wrap() { let mut t = Term::new(3, 2); t.feed(b"abc\r\n"); assert_eq!((t.cy, row(&t, 1)), (1, "".into())); }
    #[test] fn sgr_colors() {
        let mut t = Term::new(10, 1);
        t.feed(b"\x1b[1;31;44mA\x1b[0mB\x1b[97;107;7mC\x1b[22;27;39;49mD");
        assert_eq!((t.cells[0].fg, t.cells[0].bg, t.cells[0].bold), (1, 4, true));
        assert_eq!(t.cells[1], Cell { ch: 'B', ..BLANK });
        assert_eq!((t.cells[2].fg, t.cells[2].bg, t.cells[2].rev), (15, 15, true));
        assert_eq!(t.cells[3], Cell { ch: 'D', ..BLANK });
        t.feed(b"\x1b[m\x1b[36mE"); assert_eq!(t.cells[4].fg, 6);
    }
    #[test] fn truecolor_maps_to_theme_slots() {
        let mut t = Term::new(10, 1);
        t.feed(b"\x1b[38;2;240;178;127;48;2;21;41;74mA\x1b[0m\x1b[38;2;143;180;240mB\x1b[1mC");
        assert_eq!((t.cells[0].fg, t.cells[0].bg), (18, 19));
        assert_eq!(t.cells[1].fg, 4);   // close to ansi 4 (#7fb2f0)
        assert!(t.cells[2].bold);
    }
    #[test] fn erase() {
        let mut t = Term::new(4, 3); t.feed(b"aaaa\r\nbbbb\r\ncccc");
        t.feed(b"\x1b[2;3H\x1b[K"); assert_eq!(row(&t, 1), "bb");
        t.feed(b"\x1b[1K"); assert_eq!(row(&t, 1), "");
        t.feed(b"\x1b[2;1H\x1b[J"); assert_eq!((row(&t, 0), row(&t, 2)), ("aaaa".into(), "".into()));
        t.feed(b"\x1b[2J"); assert_eq!(row(&t, 0), "");
    }
    #[test] fn erase_uses_background() { let mut t = Term::new(2, 1); t.feed(b"\x1b[41m\x1b[K"); assert_eq!(t.cells[1].bg, 1); }
    #[test] fn alt_screen_flag() { let mut t = Term::new(4, 2); assert!(!t.alt); t.feed(b"\x1b[?1049h"); assert!(t.alt); t.feed(b"\x1b[?25l"); assert!(t.alt); t.feed(b"\x1b[?1049l"); assert!(!t.alt); }
    #[test] fn paste_and_mouse_modes() {
        let mut t = Term::new(4, 2); t.feed(b"\x1b[?2004h\x1b[?1000h\x1b[?1006h"); assert!(t.bracketed && t.mouse && t.mouse_sgr);
        t.feed(b"\x1b[?2004l"); assert!(!t.bracketed); t.feed(b"\x1b[?1049h\x1b[?1049l"); assert!(!t.mouse && !t.mouse_sgr);
    }
    #[test] fn cursor_visibility() { let mut t = Term::new(2, 1); t.feed(b"\x1b[?25l"); assert!(!t.cursor_on); t.feed(b"\x1b[?25h"); assert!(t.cursor_on); }
    #[test] fn backspace_tab_utf8() {
        let mut t = Term::new(20, 1); t.feed("a\u{8}b\tc é€".as_bytes());
        assert_eq!(row(&t, 0), "b       c é€");
    }
    #[test] fn split_utf8_and_unknown_sequences() {
        let mut t = Term::new(10, 1); t.feed(&[0xE2, 0x82]); t.feed(&[0xAC]); t.feed(b"\x1b]0;title\x07\x1b(B\x1b[?1049h\x1b[5nx");
        assert_eq!(row(&t, 0), "€x");
    }
    #[test] fn osc_title() {
        let mut t = Term::new(10, 1);
        t.feed(b"\x1b]2;weather\x07"); assert_eq!(t.title, "weather"); assert!(std::mem::take(&mut t.title_changed));
        t.feed(b"\x1b]2;weather\x07"); assert!(!t.title_changed);
        t.feed(b"\x1b]0;a b\x1b\\x"); assert_eq!(t.title, "a b"); assert_eq!(row(&t, 0), "x");
        t.feed(b"\x1b]2;\x07"); assert_eq!(t.title, "");
        t.feed(b"\x1b]2;a\x01\x7fb\x1b[2Jc\x07"); assert_eq!(t.title, "ab");   // controls dropped; the ESC ends the title and the rest is a normal sequence
    }
    #[test] fn osc_title_limits() {
        let mut t = Term::new(10, 1);
        t.feed(b"\x1b]2;"); t.feed(&[b'z'; 200]); t.feed(b"\x07"); assert_eq!(t.title.chars().count(), 40);
        t.feed(b"\x1b]2;gone\x18"); t.feed(b"\x07"); assert_eq!(t.title.chars().count(), 40);   // CAN: not applied
        t.feed(b"\x1b]52;c;aGk=\x07"); assert_eq!(t.title.chars().count(), 40);                  // other OSC numbers are ignored
    }
    #[test] fn random_bytes_never_panic() {
        let mut s = 0x1234_5678u32; let mut t = Term::new(7, 4);
        for round in 0..200 {
            let buf: Vec<u8> = (0..500).map(|_| { s = s.wrapping_mul(1664525).wrapping_add(1013904223); let b = (s >> 24) as u8; if round % 2 == 0 && b % 3 == 0 { b"\x1b[;0123456789mHJKABCDfhl?"[(s >> 8) as usize % 24] } else { b } }).collect();
            t.feed(&buf); if round % 50 == 0 { t.resize(1 + round % 9, 1 + round % 5); }
            assert!(t.cx < t.cols && t.cy < t.rows && t.cells.len() == t.cols * t.rows);
        }
    }
}
