//! The den-wide clipboard, pure parts: the size cap, what a paste sends to a pane, and what a mouse selection copies. Text only.
use super::vt::Cell;

/// The most text the clipboard keeps, and the most one surface message carries (bytes).
pub const MAX: usize = 65536;

/// `s` cut to at most MAX bytes, on a character boundary.
pub fn cap(s: &str) -> &str {
    if s.len() <= MAX { return s }
    let mut n = MAX; while !s.is_char_boundary(n) { n -= 1; }
    &s[..n]
}

/// The bytes a paste writes into a pane. Control characters are dropped, except tab and newline. Not bracketed: a newline is Enter (`\r`), as when typed.
/// Bracketed (the app asked for `?2004`): the text sits between ESC[200~ and ESC[201~, newlines stay `\n`, and the ESC that would end it early is gone with the other controls.
pub fn paste_bytes(text: &str, bracketed: bool) -> Vec<u8> {
    let text = cap(text).replace("\r\n", "\n").replace('\r', "\n");
    let body: String = text.chars().filter(|&c| c == '\t' || c == '\n' || !(c.is_control())).collect();
    if bracketed { return [&b"\x1b[200~"[..], body.as_bytes(), b"\x1b[201~"].concat() }
    body.replace('\n', "\r").into_bytes()
}

/// A paste for a one-line editor: control characters dropped, every run of line breaks (and the tabs) turned into one space, the ends trimmed. Nothing in it can run before the user presses Enter.
pub fn line_of(text: &str) -> String {
    let mut out = String::new();
    for c in cap(text).chars() { if c == '\n' || c == '\r' || c == '\t' { if !out.ends_with(' ') { out.push(' ') } } else if !c.is_control() { out.push(c) } }
    out.trim().to_string()
}

/// Which cell is first: (x, y) in reading order.
pub fn ordered(a: (usize, usize), b: (usize, usize)) -> ((usize, usize), (usize, usize)) { if (a.1, a.0) <= (b.1, b.0) { (a, b) } else { (b, a) } }
/// Is the cell inside the selection from `a` to `b` (both ends included, reading order)?
pub fn selected(a: (usize, usize), b: (usize, usize), x: usize, y: usize) -> bool { let (s, e) = ordered(a, b); (y, x) >= (s.1, s.0) && (y, x) <= (e.1, e.0) }

/// The text from `a` to `b`: rows joined by newlines, the blanks at the end of each row dropped.
pub fn text(cells: &[Cell], cols: usize, a: (usize, usize), b: (usize, usize)) -> String {
    let (s, e) = ordered(a, b); let mut out = String::new();
    for y in s.1..=e.1 {
        let (x0, x1) = (if y == s.1 { s.0 } else { 0 }, if y == e.1 { e.0 } else { cols.saturating_sub(1) });
        let row: String = (x0..=x1).filter_map(|x| cells.get(y * cols + x)).map(|c| c.ch).collect();
        out.push_str(row.trim_end());
        if y < e.1 { out.push('\n'); }
    }
    out
}

/// The word around column `x` of a row: (first, last) column. A blank is a word of its own.
pub fn word(cells: &[Cell], cols: usize, x: usize, y: usize) -> (usize, usize) {
    let ch = |i: usize| cells.get(y * cols + i).map_or(' ', |c| c.ch);
    let class = |c: char| if c.is_alphanumeric() || "_-./~".contains(c) { 1 } else if c == ' ' { 0 } else { 2 };
    let k = class(ch(x)); let (mut a, mut b) = (x, x);
    while a > 0 && class(ch(a - 1)) == k { a -= 1; }
    while b + 1 < cols && class(ch(b + 1)) == k { b += 1; }
    (a, b)
}

/// Click number: a click on the same cell within 500 ms of the last one counts up (1, 2, 3, then 1 again).
pub fn clicks(prev: Option<(u64, (usize, usize), u8)>, now: u64, at: (usize, usize)) -> u8 {
    match prev { Some((t, p, n)) if p == at && now.saturating_sub(t) <= 500 => n % 3 + 1, _ => 1 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui::vt::Term;
    fn term(s: &str) -> Term { let mut t = Term::new(10, 5); t.feed(s.as_bytes()); t }
    #[test] fn cap_cuts_on_a_character() {
        assert_eq!(cap("hi"), "hi");
        let s = "é".repeat(MAX); assert!(cap(&s).len() <= MAX && cap(&s).len() >= MAX - 1 && cap(&s).chars().all(|c| c == 'é'));
        assert_eq!(cap(&"a".repeat(MAX + 5)).len(), MAX);
    }
    #[test] fn plain_paste_drops_controls_and_types_enter() {
        assert_eq!(paste_bytes("ls\x1b[31m\x07 -l\nwho\tx\r\ny", false), b"ls[31m -l\rwho\tx\ry".to_vec());
        assert_eq!(paste_bytes("a\x00b\x7fc", false), b"abc".to_vec());
    }
    #[test] fn bracketed_paste_is_wrapped_and_cannot_end_early() {
        assert_eq!(paste_bytes("a\nb", true), b"\x1b[200~a\nb\x1b[201~".to_vec());
        let b = paste_bytes("x\x1b[201~rm -rf /\n", true);
        assert_eq!(b.windows(6).filter(|w| *w == b"\x1b[201~").count(), 1, "only the real end marker");
        assert!(b.ends_with(b"\x1b[201~"));
    }
    #[test] fn one_line_paste_joins_lines() {
        assert_eq!(line_of("ls\npwd\r\n\nwho\x1b[0m\t"), "ls pwd who[0m"); assert_eq!(line_of("  hi  "), "hi"); assert_eq!(line_of(""), "");
    }
    #[test] fn paste_is_capped() { assert!(paste_bytes(&"a".repeat(MAX * 2), false).len() <= MAX); }
    #[test] fn selection_text_one_row_and_many() {
        let t = term("hello world\r\nsecond  \r\nthird");   // 10 columns: "hello worl" wraps to "d"
        assert_eq!(text(&t.cells, 10, (0, 0), (4, 0)), "hello");
        assert_eq!(text(&t.cells, 10, (4, 0), (0, 0)), "hello", "backwards");
        assert_eq!(text(&t.cells, 10, (6, 0), (2, 3)), "worl\nd\nsecond\nthi", "rows are joined by newlines, trailing blanks dropped");
    }
    #[test] fn selected_cells() {
        assert!(selected((2, 0), (1, 1), 5, 0) && selected((2, 0), (1, 1), 0, 1) && selected((2, 0), (1, 1), 1, 1));
        assert!(!selected((2, 0), (1, 1), 1, 0) && !selected((2, 0), (1, 1), 2, 1) && selected((1, 1), (2, 0), 9, 0));
    }
    #[test] fn double_click_takes_a_word() {
        let t = term("ls -la /etc/x\r\n");
        assert_eq!(word(&t.cells, 10, 1, 0), (0, 1)); assert_eq!(word(&t.cells, 10, 4, 0), (3, 5));   // "ls", then "-la"
        assert_eq!(word(&t.cells, 10, 2, 0), (2, 2), "a blank is its own word");
    }
    #[test] fn click_counting() {
        assert_eq!(clicks(None, 0, (1, 1)), 1);
        assert_eq!(clicks(Some((0, (1, 1), 1)), 300, (1, 1)), 2); assert_eq!(clicks(Some((300, (1, 1), 2)), 500, (1, 1)), 3); assert_eq!(clicks(Some((500, (1, 1), 3)), 600, (1, 1)), 1);
        assert_eq!(clicks(Some((0, (1, 1), 1)), 300, (2, 1)), 1); assert_eq!(clicks(Some((0, (1, 1), 1)), 501, (1, 1)), 1);
    }
}
