//! nsh's line editor, pure: bytes -> keys, key -> new state + the bytes to write to the terminal.
#[derive(Debug, PartialEq, Clone)]
pub enum Key { Char(char), Enter, Tab, Backspace, Delete, Left, Right, Home, End, Up, Down, PgUp, PgDn, Ctrl(u8) }
#[derive(Debug, PartialEq)]
pub enum Act { None, Submit(String), Eof }

pub struct Editor { buf: Vec<char>, cur: usize, hist: Vec<String>, hpos: usize, stash: Vec<char>, dec: Decoder }

pub fn common_prefix(words: &[String]) -> String {
    let Some(first) = words.first() else { return String::new() };
    let mut n = first.chars().count();
    for w in &words[1..] { n = n.min(first.chars().zip(w.chars()).take_while(|(a, b)| a == b).count()); }
    let mut p: String = first.chars().take(n).collect();
    // never stop inside an escape: an odd run of trailing backslashes would escape whatever is typed next
    if p.chars().rev().take_while(|&c| c == '\\').count() % 2 == 1 { p.pop(); }
    p
}

/// Bytes from a terminal -> keys; escapes and UTF-8 may span reads.
pub struct Decoder { esc: Vec<u8>, utf8: Vec<u8> }

impl Decoder {
    pub fn new() -> Decoder { Decoder { esc: vec![], utf8: vec![] } }

    fn plain(&mut self, b: u8, out: &mut Vec<Key>) {
        match b {
            b'\r' | b'\n' => out.push(Key::Enter), b'\t' => out.push(Key::Tab), 0x7f | 0x08 => out.push(Key::Backspace),
            1..=26 => out.push(Key::Ctrl(b + 0x60)),
            0x20..=0x7e => out.push(Key::Char(b as char)),
            0x80.. => {
                self.utf8.push(b);
                match std::str::from_utf8(&self.utf8) {
                    Ok(t) => { out.extend(t.chars().next().map(Key::Char)); self.utf8.clear(); }
                    Err(e) if e.error_len().is_none() && self.utf8.len() < 4 => {}   // incomplete: wait for more
                    Err(_) => self.utf8.clear(),
                }
            }
            _ => {}
        }
    }

    /// True while the decoder holds only an ESC: a bare Esc key, or the start of a sequence whose rest has not come yet.
    pub fn waiting_esc(&self) -> bool { self.esc == [0x1b] }
    /// Gives up on that ESC (the caller waited and nothing followed): true if there was one, which was then a bare Esc key.
    pub fn lone_esc(&mut self) -> bool { let w = self.waiting_esc(); if w { self.esc.clear(); } w }

    pub fn decode(&mut self, bytes: &[u8]) -> Vec<Key> {
        let mut out = vec![];
        for &b in bytes {
            if self.esc.is_empty() {
                if b == 0x1b { self.esc.push(b); self.utf8.clear(); } else if b < 0x80 && !self.utf8.is_empty() { self.utf8.clear(); self.plain(b, &mut out) } else { self.plain(b, &mut out) }
                continue;
            }
            self.esc.push(b);
            if self.esc.len() == 2 {
                if b != b'[' && b != b'O' { self.esc.clear(); self.plain(b, &mut out); }   // lone ESC + byte: the byte counts
                continue;
            }
            let done = if self.esc[1] == b'O' { true } else { (0x40..=0x7e).contains(&b) };
            if !done && self.esc.len() < 12 { continue }
            let k = match &self.esc[1..] {
                b"[A" => Some(Key::Up), b"[B" => Some(Key::Down), b"[C" => Some(Key::Right), b"[D" => Some(Key::Left),
                b"[H" | b"[1~" | b"[7~" | b"OH" => Some(Key::Home), b"[F" | b"[4~" | b"[8~" | b"OF" => Some(Key::End),
                b"[3~" => Some(Key::Delete), b"[5~" => Some(Key::PgUp), b"[6~" => Some(Key::PgDn), _ => None,
            };
            out.extend(k); self.esc.clear();
        }
        out
    }
}

fn s(c: &[char]) -> String { c.iter().collect() }

impl Editor {
    pub fn new(hist: Vec<String>) -> Editor {
        Editor { buf: vec![], cur: 0, hpos: hist.len(), hist, stash: vec![], dec: Decoder::new() }
    }

    pub fn decode(&mut self, bytes: &[u8]) -> Vec<Key> { self.dec.decode(bytes) }

    pub fn redraw(&self, prompt: &str) -> Vec<u8> {
        let mut o = format!("\r{prompt}{}\x1b[K", s(&self.buf));
        if self.cur < self.buf.len() { o += &format!("\x1b[{}D", self.buf.len() - self.cur); }
        o.into_bytes()
    }

    fn set(&mut self, t: Vec<char>) { self.cur = t.len(); self.buf = t; }

    pub fn key(&mut self, k: Key, prompt: &str) -> (Act, Vec<u8>) {
        let (mut act, mut redraw) = (Act::None, true);
        let mut out = vec![];
        match k {
            Key::Char(c) => {
                let at_end = self.cur == self.buf.len();
                self.buf.insert(self.cur, c); self.cur += 1;
                if at_end { out = c.to_string().into_bytes(); redraw = false; }
            }
            Key::Left => { redraw = false; if self.cur > 0 { self.cur -= 1; out = b"\x1b[D".to_vec(); } }
            Key::Right => { redraw = false; if self.cur < self.buf.len() { self.cur += 1; out = b"\x1b[C".to_vec(); } }
            Key::Home | Key::Ctrl(b'a') => self.cur = 0,
            Key::End | Key::Ctrl(b'e') => self.cur = self.buf.len(),
            Key::Backspace => if self.cur > 0 { self.cur -= 1; self.buf.remove(self.cur); },
            Key::Delete => if self.cur < self.buf.len() { self.buf.remove(self.cur); },
            Key::Ctrl(b'u') => { self.buf.drain(..self.cur); self.cur = 0; }
            Key::Ctrl(b'k') => self.buf.truncate(self.cur),
            Key::Ctrl(b'w') => {
                let mut i = self.cur;
                while i > 0 && self.buf[i - 1] == ' ' { i -= 1; }
                while i > 0 && self.buf[i - 1] != ' ' { i -= 1; }
                self.buf.drain(i..self.cur); self.cur = i;
            }
            Key::Ctrl(b'l') => out = b"\x1b[H\x1b[2J".to_vec(),
            Key::Ctrl(b'c') => { self.buf.clear(); self.cur = 0; self.hpos = self.hist.len(); out = b"^C\r\n".to_vec(); }
            Key::Ctrl(b'd') => { redraw = false; if self.buf.is_empty() { act = Act::Eof; out = b"\r\n".to_vec(); } }
            Key::Up => if self.hpos > 0 {
                if self.hpos == self.hist.len() { self.stash = self.buf.clone(); }
                self.hpos -= 1; let t = self.hist[self.hpos].chars().collect(); self.set(t);
            },
            Key::Down => if self.hpos < self.hist.len() {
                self.hpos += 1;
                let t = if self.hpos == self.hist.len() { self.stash.clone() } else { self.hist[self.hpos].chars().collect() };
                self.set(t);
            },
            Key::Enter => { act = Act::Submit(s(&self.buf)); self.buf.clear(); self.cur = 0; self.hpos = self.hist.len(); out = b"\r\n".to_vec(); redraw = false; }
            Key::Tab | Key::PgUp | Key::PgDn | Key::Ctrl(_) => redraw = false,
        }
        if redraw { out.extend(self.redraw(prompt)); }
        (act, out)
    }

    /// (is_first_word, the word that ends at the cursor)
    pub fn word(&self) -> (bool, String) {
        let before = &self.buf[..self.cur];
        let start = (0..before.len()).rev().find(|&i| before[i] == ' ' && (i == 0 || before[i - 1] != '\\')).map_or(0, |i| i + 1);   // an escaped space stays in the word
        (before[..start].iter().all(|&c| c == ' '), s(&before[start..]))
    }

    pub fn complete(&mut self, cands: &[String], prompt: &str) -> Vec<u8> {
        let wl = self.word().1.chars().count();
        let rep = match cands {
            [] => return vec![],
            [c] => if c.ends_with('/') { c.clone() } else { format!("{c} ") },
            _ => {
                let cp = common_prefix(cands);
                if cp.chars().count() <= wl {
                    let mut o = format!("\r\n{}\r\n", cands.join("  ")).into_bytes();
                    o.extend(self.redraw(prompt)); return o;
                }
                cp
            }
        };
        self.buf.splice(self.cur - wl..self.cur, rep.chars());
        self.cur = self.cur - wl + rep.chars().count();
        self.redraw(prompt)
    }

    pub fn push_history(&mut self, line: &str) -> bool {
        let add = !line.is_empty() && self.hist.last().map_or(true, |l| l != line);
        if add { self.hist.push(line.to_string()); }
        self.hpos = self.hist.len();
        add
    }
    pub fn history(&self) -> &[String] { &self.hist }
}

#[cfg(test)]
mod tests {
    #[test] fn prefix_never_ends_in_lone_backslash() { assert_eq!(super::common_prefix(&["a\\ b".into(), "a\\>c".into()]), "a"); }
    use super::*;
    #[test] fn lone_esc_is_a_bare_esc_key() {
        let mut d = Decoder::new();
        assert!(d.decode(b"\x1b").is_empty()); assert!(d.waiting_esc());
        assert!(d.lone_esc()); assert!(!d.waiting_esc() && !d.lone_esc());
        assert_eq!(d.decode(b"\x1b[A"), vec![Key::Up]); assert!(!d.lone_esc());   // an arrow is not a bare Esc
        d.decode(b"\x1b["); assert!(!d.waiting_esc() && !d.lone_esc());          // half a sequence is not either
        assert_eq!(d.decode(b"Aa"), vec![Key::Up, Key::Char('a')]);
    }
    const P: &str = "$ ";
    fn feed(e: &mut Editor, b: &[u8]) -> (Vec<Act>, Vec<u8>) {
        let (mut acts, mut out) = (vec![], vec![]);
        for k in e.decode(b) { let (a, o) = e.key(k, P); out.extend(o); if a != Act::None { acts.push(a); } }
        (acts, out)
    }
    fn text(e: &Editor) -> String { s(&e.buf) }
    fn v(a: &[&str]) -> Vec<String> { a.iter().map(|x| x.to_string()).collect() }

    #[test] fn append_echoes_char_and_mid_insert_redraws() {
        let mut e = Editor::new(vec![]);
        assert_eq!(feed(&mut e, b"abc").1, b"abc");
        feed(&mut e, b"\x1b[D\x1b[D"); assert_eq!(e.cur, 1);
        let (_, o) = feed(&mut e, b"X");
        assert_eq!((text(&e).as_str(), e.cur), ("aXbc", 2));
        assert_eq!(o, b"\r$ aXbc\x1b[K\x1b[2D");
    }
    #[test] fn backspace_delete_home_end() {
        let mut e = Editor::new(vec![]); feed(&mut e, b"abcd");
        feed(&mut e, b"\x7f"); assert_eq!(text(&e), "abc");
        feed(&mut e, b"\x1b[H"); assert_eq!(e.cur, 0);
        feed(&mut e, b"\x1b[3~"); assert_eq!(text(&e), "bc");
        feed(&mut e, b"\x1b[F"); assert_eq!(e.cur, 2);
        feed(&mut e, b"\x01"); assert_eq!(e.cur, 0); feed(&mut e, b"\x05"); assert_eq!(e.cur, 2);
        feed(&mut e, b"\x1b[D\x7f"); assert_eq!((text(&e).as_str(), e.cur), ("c", 0));
    }
    #[test] fn kill_keys() {
        let mut e = Editor::new(vec![]); feed(&mut e, b"echo hi there");
        feed(&mut e, b"\x17"); assert_eq!(text(&e), "echo hi ");
        feed(&mut e, b"\x1b[D\x1b[D\x0b"); assert_eq!(text(&e), "echo h");
        feed(&mut e, b"\x1b[D\x15"); assert_eq!((text(&e).as_str(), e.cur), ("h", 0));
    }
    #[test] fn history_with_stash() {
        let mut e = Editor::new(v(&["one", "two"])); feed(&mut e, b"par");
        let (_, o) = feed(&mut e, b"\x1b[A"); assert_eq!((text(&e).as_str(), o), ("two", b"\r$ two\x1b[K".to_vec()));
        feed(&mut e, b"\x1b[A"); assert_eq!(text(&e), "one");
        feed(&mut e, b"\x1b[A"); assert_eq!(text(&e), "one");
        feed(&mut e, b"\x1b[B"); assert_eq!(text(&e), "two");
        feed(&mut e, b"\x1b[B"); assert_eq!(text(&e), "par");
        feed(&mut e, b"\x1b[B"); assert_eq!(text(&e), "par");
    }
    #[test] fn enter_submits_and_clears() {
        let mut e = Editor::new(vec![]); feed(&mut e, b"ls");
        let (a, o) = feed(&mut e, b"\r"); assert_eq!(a, vec![Act::Submit("ls".into())]); assert_eq!(o, b"\r\n"); assert_eq!(text(&e), "");
    }
    #[test] fn ctrl_d_and_ctrl_c() {
        let mut e = Editor::new(vec![]);
        let (a, o) = feed(&mut e, b"\x04"); assert_eq!((a, o), (vec![Act::Eof], b"\r\n".to_vec()));
        feed(&mut e, b"x"); assert_eq!(feed(&mut e, b"\x04"), (vec![], vec![])); assert_eq!(text(&e), "x");
        let (_, o) = feed(&mut e, b"\x03"); assert_eq!((text(&e).as_str(), o), ("", b"^C\r\n\r$ \x1b[K".to_vec()));
        assert_eq!(feed(&mut e, b"\x0c").1, b"\x1b[H\x1b[2J\r$ \x1b[K");
    }
    #[test] fn decode_split_and_odd_input() {
        let mut e = Editor::new(vec![]);
        assert_eq!(e.decode(b"\x1b["), vec![]); assert_eq!(e.decode(b"3~"), vec![Key::Delete]);
        assert_eq!(e.decode(&[0xc3]), vec![]); assert_eq!(e.decode(&[0xa9]), vec![Key::Char('é')]);
        assert_eq!(e.decode(b"\x1b[A\x1b[D"), vec![Key::Up, Key::Left]);
        assert_eq!(e.decode(b"\x1bx"), vec![Key::Char('x')]);
        assert_eq!(e.decode(b"\x1b[1;5C\x1bOHq"), vec![Key::Home, Key::Char('q')]);   // unknown CSI dropped
        assert_eq!(e.decode(&[0xff, b'a']), vec![Key::Char('a')]);
    }
    #[test] fn pgup_pgdn() { assert_eq!(Decoder::new().decode(b"\x1b[5~\x1b[6~"), vec![Key::PgUp, Key::PgDn]); }
    #[test] fn word_detection() {
        let mut e = Editor::new(vec![]); feed(&mut e, b"cat /etc/re");
        assert_eq!(e.word(), (false, "/etc/re".into()));
        feed(&mut e, b"\x01"); assert_eq!(e.word(), (true, "".into()));
        let mut e = Editor::new(vec![]); feed(&mut e, b"ec"); assert_eq!(e.word(), (true, "ec".into()));
    }
    #[test] fn complete_single_dir_list_prefix() {
        let mut e = Editor::new(vec![]); feed(&mut e, b"ca");
        assert_eq!(e.complete(&v(&["cat"]), P), b"\r$ cat \x1b[K"); assert_eq!(text(&e), "cat ");
        let mut e = Editor::new(vec![]); feed(&mut e, b"ls do");
        e.complete(&v(&["docs/"]), P); assert_eq!(text(&e), "ls docs/");
        let mut e = Editor::new(vec![]); feed(&mut e, b"e");
        assert_eq!(e.complete(&v(&["echo", "exit"]), P), b"\r\necho  exit\r\n\r$ e\x1b[K");
        let mut e = Editor::new(vec![]); feed(&mut e, b"a");
        e.complete(&v(&["abc1", "abc2"]), P); assert_eq!((text(&e).as_str(), e.cur), ("abc", 3));
        assert_eq!(e.complete(&[], P), b"");
    }
    #[test] fn history_dedup_and_prefix() {
        let mut e = Editor::new(vec![]);
        assert!(!e.push_history("")); assert!(e.push_history("a")); assert!(!e.push_history("a")); assert!(e.push_history("b"));
        assert_eq!(e.history(), &v(&["a", "b"])[..]);
        assert_eq!(common_prefix(&[]), ""); assert_eq!(common_prefix(&v(&["xé1", "xé2"])), "xé");
    }
}
