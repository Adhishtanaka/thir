//! The logic of the `web` browser that needs no WebKit: which URLs are allowed, what the address bar makes of typed words, download names, the address line editor, the strip text,
//! and the sandbox rules of the three browser processes. The program itself (web/ crate) is glue between this, the surface protocol and WPE WebKit.
use crate::{files::{printable, unsafe_char}, sandbox::Spec};
use std::path::{Path, PathBuf};

/// Where words without a dot go. One constant: change it here.
pub const SEARCH: &str = "https://duckduckgo.com/?q=";
const MAX_URL: usize = 4096;

/// Only plain http and https addresses (a host, no spaces or control characters). file:, data:, javascript:, about:, blob:, ftp:... are refused.
pub fn url_ok(s: &str) -> bool {
    if s.len() > MAX_URL || s.chars().any(|c| unsafe_char(c) || c.is_whitespace()) { return false }
    let Some((scheme, rest)) = s.split_once("://") else { return false };
    if !(scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https")) { return false }
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = host.rsplit_once('@').map_or(host, |(_, h)| h);   // user@host
    !host.is_empty() && !host.starts_with(':')
}

fn encode(s: &str) -> String {
    s.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else if b == b' ' { "+".into() } else { format!("%{b:02X}") }).collect()
}
fn is_ip4(h: &str) -> bool { let p: Vec<&str> = h.split('.').collect(); p.len() == 4 && p.iter().all(|x| !x.is_empty() && x.len() <= 3 && x.bytes().all(|b| b.is_ascii_digit())) }

/// What a typed address becomes: an http(s) URL, a search, or an error (a scheme the browser refuses).
/// A full URL stays as it is; `host.tld/path` gets https://; an IP address or localhost gets http://; anything else (no dot, or spaces) is a search.
pub fn address(input: &str) -> Result<String, &'static str> {
    let t = input.trim();
    if t.is_empty() { return Err("empty address") }
    if t.contains("://") { return if url_ok(t) { Ok(t.to_string()) } else { Err("only http and https addresses") } }
    let head = t.split(['/', '?', '#']).next().unwrap_or("");
    if let Some((scheme, rest)) = head.split_once(':') {   // javascript:alert(1), data:text/html,...; but host:8080 is fine
        if !rest.bytes().all(|b| b.is_ascii_digit()) && scheme.bytes().all(|b| b.is_ascii_alphanumeric() || b"+-.".contains(&b)) && !scheme.contains('.') { return Err("only http and https addresses") }
    }
    let host = head.rsplit_once('@').map_or(head, |(_, h)| h);
    let host = host.rsplit_once(':').filter(|(_, p)| p.bytes().all(|b| b.is_ascii_digit())).map_or(host, |(h, _)| h);
    let plain = t.chars().all(|c| !c.is_whitespace() && !c.is_control());
    if plain && (is_ip4(host) || host == "localhost") { return Ok(format!("http://{t}")) }
    if plain && host.contains('.') && !host.starts_with('.') && !host.ends_with('.') { return Ok(format!("https://{t}")) }
    Ok(format!("{SEARCH}{}", encode(t)))
}

/// "1.4 MB". `None` (unknown size) gives "unknown size".
pub fn size_text(n: Option<u64>) -> String {
    let Some(n) = n else { return "unknown size".into() };
    if n < 1024 { return format!("{n} B") }
    let (mut v, mut u) = (n as f64 / 1024.0, "KB");
    for next in ["MB", "GB"] { if v >= 1024.0 { v /= 1024.0; u = next; } }
    format!("{v:.1} {u}")
}

/// A safe file name for a download: the last part of what the server suggested, control and bidi characters as '?', no '/', no leading '.', at most 100 bytes, never empty.
pub fn clean_name(suggested: &str) -> String {
    let last = suggested.rsplit(['/', '\\']).next().unwrap_or("");
    let mut n: String = printable(last).trim().trim_start_matches('.').trim().to_string();
    while n.len() > 100 { n.pop(); }
    if n.is_empty() { "download".into() } else { n }
}
/// `clean_name`, then " (1)", " (2)"... before the extension until `exists` says the name is free: a download never replaces a file.
pub fn download_name(suggested: &str, exists: impl Fn(&str) -> bool) -> String {
    let n = clean_name(suggested);
    if !exists(&n) { return n }
    let (stem, ext) = match n.rfind('.') { Some(i) if i > 0 => (&n[..i], &n[i..]), _ => (&n[..], "") };
    (1..1000).map(|k| format!("{stem} ({k}){ext}")).find(|c| !exists(c)).unwrap_or_else(|| format!("{stem} (many){ext}"))
}
/// The question in the strip.
pub fn download_prompt(name: &str, size: Option<u64>) -> String { format!("save {} ({}) to ~/Downloads? y/n", printable(name), size_text(size)) }

/// A line being edited (address, find, prompt): characters, a cursor, and an optional selection (`sel` = the anchor, the cursor is the other end).
#[derive(Default, Debug, PartialEq)]
pub struct Line { pub text: Vec<char>, pub at: usize, pub sel: Option<usize> }
impl Line {
    pub fn new(s: &str) -> Line { let text: Vec<char> = s.chars().collect(); Line { at: text.len(), text, sel: None } }
    /// The selected range (start, end), None when nothing is selected.
    pub fn selection(&self) -> Option<(usize, usize)> { self.sel.filter(|&a| a != self.at).map(|a| (a.min(self.at), a.max(self.at))) }
    pub fn selected(&self) -> String { self.selection().map_or(String::new(), |(a, b)| self.text[a..b].iter().collect()) }
    /// Removes the selection (the cursor goes to its start); false when there was none.
    pub fn delete_selection(&mut self) -> bool {
        let r = self.selection(); self.sel = None;
        let Some((a, b)) = r else { return false };
        self.text.drain(a..b); self.at = a; true
    }
    pub fn select_all(&mut self) { self.sel = Some(0); self.at = self.text.len(); }
    /// Typing replaces the selection.
    pub fn insert(&mut self, c: char) { if c.is_control() { return } self.delete_selection(); if self.text.len() < MAX_URL { self.text.insert(self.at, c); self.at += 1; } }
    /// Paste: the text with newlines and tabs as spaces and other control characters dropped; replaces the selection.
    pub fn paste(&mut self, s: &str) { self.delete_selection(); for c in s.chars().map(|c| if matches!(c, '\n' | '\r' | '\t') { ' ' } else { c }) { self.insert(c) } }
    /// Cut: the selected text (removed); empty when nothing was selected.
    pub fn cut(&mut self) -> String { let t = self.selected(); self.delete_selection(); t }
    pub fn backspace(&mut self) { if self.delete_selection() { return } if self.at > 0 { self.at -= 1; self.text.remove(self.at); } }
    pub fn delete(&mut self) { if self.delete_selection() { return } if self.at < self.text.len() { self.text.remove(self.at); } }
    /// The cursor moves to `pos`; `extend` (Shift) keeps or starts a selection, otherwise it ends.
    pub fn go(&mut self, pos: usize, extend: bool) { if extend { if self.sel.is_none() { self.sel = Some(self.at) } } else { self.sel = None } self.at = pos.min(self.text.len()); }
    /// Left and right with a selection and no Shift: the cursor goes to that edge of the selection.
    pub fn left(&mut self, extend: bool) { match (self.selection(), extend) { (Some((a, _)), false) => { self.sel = None; self.at = a } _ => self.go(self.at.saturating_sub(1), extend) } }
    pub fn right(&mut self, extend: bool) { match (self.selection(), extend) { (Some((_, b)), false) => { self.sel = None; self.at = b } _ => self.go(self.at + 1, extend) } }
    pub fn home(&mut self, extend: bool) { self.go(0, extend) }
    pub fn end(&mut self, extend: bool) { self.go(self.text.len(), extend) }
    /// Ctrl+Left: to the start of the word before the cursor (a word = letters and digits).
    pub fn word_left(&mut self, extend: bool) {
        let w = |c: &char| c.is_alphanumeric(); let mut i = self.at;
        while i > 0 && !w(&self.text[i - 1]) { i -= 1 }
        while i > 0 && w(&self.text[i - 1]) { i -= 1 }
        self.go(i, extend)
    }
    /// Ctrl+Right: to the end of the word after the cursor.
    pub fn word_right(&mut self, extend: bool) {
        let w = |c: &char| c.is_alphanumeric(); let mut i = self.at; let n = self.text.len();
        while i < n && !w(&self.text[i]) { i += 1 }
        while i < n && w(&self.text[i]) { i += 1 }
        self.go(i, extend)
    }
    pub fn kill(&mut self) { self.text.clear(); self.at = 0; self.sel = None; }
    pub fn as_string(&self) -> String { self.text.iter().collect() }
}

/// What a key does in a line editor, with its own clipboard text (`clip`): the keys every line (address, find, prompt) shares. `code` is an evdev code; returns true when the key was a line key.
/// Ctrl+A select all, Ctrl+C copy, Ctrl+X cut, Ctrl+V paste (`paste` = the clipboard), Ctrl+U clear, Ctrl+E end, Home, End, Left, Right, Ctrl+Left/Right by word, Backspace, Delete; Shift extends the selection.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum LineKey { Back, Del, Left, Right, Home, End, All, Copy, Cut, Paste, Kill, EndEmacs, HomeEmacs }
pub fn line_key(code: u32, ctrl: bool) -> Option<LineKey> {
    use LineKey::*;
    Some(match (code, ctrl) {
        (14, _) => Back, (111, _) => Del, (105, _) => Left, (106, _) => Right, (102, _) => Home, (107, _) => End,
        (30, true) => All, (46, true) => Copy, (45, true) => Cut, (47, true) => Paste, (22, true) => Kill, (18, true) => EndEmacs,
        _ => return None,
    })
}
/// Applies a line key. `clip` is the shared clipboard (read for Paste, written by Copy and Cut). Returns true when the text changed.
pub fn line_apply(l: &mut Line, k: LineKey, ctrl: bool, shift: bool, clip: &mut String) -> bool {
    use LineKey::*;
    let was = l.as_string();
    match k {
        Back => l.backspace(), Del => l.delete(),
        Left => if ctrl { l.word_left(shift) } else { l.left(shift) }, Right => if ctrl { l.word_right(shift) } else { l.right(shift) },
        Home | HomeEmacs => l.home(shift), End | EndEmacs => l.end(shift),
        All => l.select_all(), Copy => { let t = l.selected(); if !t.is_empty() { *clip = t } }
        Cut => { let t = l.cut(); if !t.is_empty() { *clip = t } } Paste => { let c = clip.clone(); l.paste(&c) } Kill => l.kill(),
    }
    l.as_string() != was
}

// ---- history ----
/// One visited page.
#[derive(Clone, Debug, PartialEq)]
pub struct Visit { pub t: i64, pub url: String, pub title: String }
/// The history: unique addresses, the newest last. Stored as `time TAB url TAB title` lines in ~/.local/share/thir-web/history (0600).
#[derive(Default, Debug)]
pub struct History { pub items: Vec<Visit> }
pub const MAX_HISTORY: usize = 1000;
impl History {
    pub fn parse(s: &str) -> History {
        let mut h = History::default();
        for l in s.lines() {
            let mut p = l.splitn(3, '\t');
            if let (Some(t), Some(u), title) = (p.next().and_then(|t| t.parse().ok()), p.next(), p.next().unwrap_or("")) { if url_ok(u) { h.items.push(Visit { t, url: u.into(), title: printable(title) }) } }
        }
        let n = h.items.len(); if n > MAX_HISTORY { h.items.drain(..n - MAX_HISTORY); }
        h
    }
    pub fn serialize(&self) -> String { self.items.iter().map(|v| format!("{}\t{}\t{}\n", v.t, v.url, v.title)).collect() }
    /// A page was shown: it moves to the end (once per address) with the newest title. Only http(s) pages count; the list stays within MAX_HISTORY.
    pub fn visit(&mut self, url: &str, title: &str, t: i64) {
        if !url_ok(url) { return }
        let title: String = printable(title).chars().take(200).collect();
        let old = self.items.iter().position(|v| v.url == url).map(|i| self.items.remove(i));
        let title = if title.is_empty() { old.map_or(String::new(), |o| o.title) } else { title };
        self.items.push(Visit { t, url: url.into(), title });
        let n = self.items.len(); if n > MAX_HISTORY { self.items.drain(..n - MAX_HISTORY); }
    }
    /// The title arrived after the visit: it belongs to the newest entry with this address.
    pub fn set_title(&mut self, url: &str, title: &str) { if let Some(v) = self.items.iter_mut().rev().find(|v| v.url == url) { v.title = printable(title).chars().take(200).collect() } }
    pub fn remove(&mut self, url: &str) -> bool { let n = self.items.len(); self.items.retain(|v| v.url != url); self.items.len() != n }
    pub fn clear(&mut self) { self.items.clear() }
    /// The entries matching every word of `q` (in the address or the title), newest first. No query: all.
    pub fn search(&self, q: &str) -> Vec<&Visit> {
        let words: Vec<String> = q.split_whitespace().map(|w| w.to_lowercase()).collect();
        self.items.iter().rev().filter(|v| { let hay = format!("{} {}", v.url, v.title).to_lowercase(); words.iter().all(|w| hay.contains(w.as_str())) }).collect()
    }
    /// The grey completion of what is typed in the address line: the rest of the newest address that starts with it (the scheme and `www.` may be left out). None when nothing fits or nothing is typed.
    pub fn suggest(&self, typed: &str) -> Option<String> {
        let t = typed.trim_start();
        if t.len() < 2 || t.contains(' ') { return None }
        let t_ = t.to_lowercase();
        for v in self.items.iter().rev() {
            let u = v.url.to_lowercase();
            for skip in [0, u.find("://").map_or(0, |i| i + 3)].into_iter().chain(u.find("://").and_then(|i| u[i + 3..].starts_with("www.").then_some(i + 7))) {
                if u.len() > skip + t_.len() && u[skip..].starts_with(&t_) { return Some(v.url[skip + t_.len()..].to_string()) }
            }
        }
        None
    }
}
/// "14:02" for today, "Thu 14:02" within a week, else "21 Nov" (`off` = minutes east of UTC).
pub fn visit_time(t: i64, now: i64, off: i32) -> String {
    let (a, n) = (crate::clock::local(t, off), crate::clock::local(now, off));
    let days = (now + off as i64 * 60).div_euclid(86400) - (t + off as i64 * 60).div_euclid(86400);
    if days == 0 { format!("{:02}:{:02}", a.hour, a.min) } else if (1..7).contains(&days) { format!("{} {:02}:{:02}", crate::clock::WDAY[a.wday as usize], a.hour, a.min) }
    else { let _ = n; format!("{} {}", a.day, crate::clock::MONTH[a.month as usize - 1]) }
}

// ---- downloads ----
/// How a download stands. Pausing and Resuming are the short times between asking and WebKit answering.
#[derive(Clone, Debug, PartialEq)]
pub enum DlState { Running, Pausing, Paused, Resuming, Done, Failed(String), Cancelled }
/// Bytes per second over the last few seconds.
#[derive(Default, Debug)]
pub struct Speed { s: std::collections::VecDeque<(u64, u64)> }
impl Speed {
    pub fn push(&mut self, ms: u64, bytes: u64) { self.s.push_back((ms, bytes)); while self.s.len() > 2 && ms.saturating_sub(self.s[1].0) >= 4000 { self.s.pop_front(); } }
    pub fn rate(&self) -> u64 {
        let (Some(a), Some(b)) = (self.s.front(), self.s.back()) else { return 0 };
        if b.0 <= a.0 { 0 } else { b.1.saturating_sub(a.1) * 1000 / (b.0 - a.0) }
    }
    pub fn reset(&mut self) { self.s.clear() }
}
/// The Range header that continues a file of `done` bytes.
pub fn range_header(done: u64) -> String { format!("bytes={done}-") }
/// Is the answer to a Range request the continuation we asked for? A 206 whose Content-Range starts at `done` (and, when the total is known, ends at it).
pub fn resume_ok(done: u64, status: u32, content_range: Option<&str>, total: Option<u64>) -> bool {
    if status != 206 { return false }
    let Some(r) = content_range.and_then(|r| r.trim().strip_prefix("bytes ")) else { return false };
    let Some((span, all)) = r.split_once('/') else { return false };
    let Some((a, b)) = span.split_once('-') else { return false };
    a.trim().parse::<u64>() == Ok(done) && b.trim().parse::<u64>().is_ok_and(|b| b >= done)
        && (all.trim() == "*" || all.trim().parse::<u64>().is_ok_and(|n| total.is_none_or(|t| t == n)))
}
/// One download in the manager. `done`: bytes already in the .part file; `seg`: bytes WebKit has written in the current piece.
#[derive(Debug)]
pub struct Dl { pub id: u32, pub name: String, pub url: String, pub total: Option<u64>, pub done: u64, pub seg: u64, pub state: DlState, pub ranges: bool, pub speed: Speed }
impl Dl {
    pub fn new(id: u32, name: &str, url: &str, total: Option<u64>) -> Dl { Dl { id, name: name.into(), url: url.into(), total, done: 0, seg: 0, state: DlState::Running, ranges: false, speed: Speed::default() } }
    pub fn got(&self) -> u64 { self.done + self.seg }
    pub fn active(&self) -> bool { matches!(self.state, DlState::Running | DlState::Pausing | DlState::Resuming) }
    /// Pause = cancel now, continue later with a Range request: only for a server that said it can, with a known size.
    pub fn pause(&mut self) -> Result<(), &'static str> {
        match self.state {
            DlState::Running if !self.ranges || self.total.is_none() => Err("cannot pause: this server does not allow resuming"),
            DlState::Running => { self.state = DlState::Pausing; Ok(()) }
            _ => Err("not running"),
        }
    }
    /// WebKit stopped the piece: `seg_bytes` are on disk and now belong to the .part file.
    pub fn stopped(&mut self, seg_bytes: u64) { if self.state == DlState::Pausing { self.done += seg_bytes; self.seg = 0; self.state = DlState::Paused; self.speed.reset(); } }
    /// Resume: the Range header to send, None when this download is not paused.
    pub fn resume(&mut self) -> Option<String> { if self.state != DlState::Paused { return None } self.state = DlState::Resuming; Some(range_header(self.done)) }
    /// The first answer of a resumed piece.
    pub fn answered(&mut self, status: u32, content_range: Option<&str>) -> Result<(), &'static str> {
        if self.state != DlState::Resuming { return Ok(()) }
        if resume_ok(self.done, status, content_range, self.total) { self.state = DlState::Running; Ok(()) } else { self.state = DlState::Failed("the server did not continue the file".into()); Err("the server did not continue the file") }
    }
    pub fn progress(&mut self, seg_bytes: u64, ms: u64) { if matches!(self.state, DlState::Running | DlState::Pausing) { self.seg = seg_bytes; let g = self.got(); self.speed.push(ms, g) } }
    pub fn finish(&mut self) { if let Some(t) = self.total { self.done = t; } else { self.done += self.seg; } self.seg = 0; self.state = DlState::Done }
    pub fn fail(&mut self, why: &str) { if self.active() { self.state = DlState::Failed(printable(why)) } }
    pub fn cancel(&mut self) { if !matches!(self.state, DlState::Done) { self.state = DlState::Cancelled } }
    /// 0-100 when the size is known.
    pub fn percent(&self) -> Option<u8> { self.total.filter(|&t| t > 0).map(|t| (self.got().min(t) * 100 / t) as u8) }
    pub fn status_text(&self) -> String {
        match &self.state {
            DlState::Running => { let r = self.speed.rate(); if r > 0 { format!("{}/s", size_text(Some(r))) } else { "starting".into() } }
            DlState::Pausing => "pausing".into(), DlState::Paused => "paused".into(), DlState::Resuming => "resuming".into(),
            DlState::Done => "done".into(), DlState::Failed(w) => format!("failed: {w}"), DlState::Cancelled => "cancelled".into(),
        }
    }
}
/// A progress bar of `w` cells: `(filled, w)`.
pub fn bar_cells(percent: Option<u8>, w: usize) -> usize { percent.map_or(0, |p| (p.min(100) as usize * w + 50) / 100) }

// ---- the <select> popup ----
/// The options of a page's <select>: label, enabled, is a group heading. The choice skips headings and disabled items.
#[derive(Debug, Default)]
pub struct Menu { pub items: Vec<(String, bool, bool)>, pub at: usize }
impl Menu {
    fn ok(&self, i: usize) -> bool { self.items.get(i).is_some_and(|x| x.1 && !x.2) }
    /// Starts on `selected` (or the first choosable item).
    pub fn new(items: Vec<(String, bool, bool)>, selected: Option<usize>) -> Menu {
        let mut m = Menu { items, at: 0 };
        let first = selected.filter(|&i| m.ok(i)).or_else(|| (0..m.items.len()).find(|&i| m.ok(i)));
        m.at = first.unwrap_or(0); m
    }
    pub fn up(&mut self) { if let Some(i) = (0..self.at).rev().find(|&i| self.ok(i)) { self.at = i } }
    pub fn down(&mut self) { if let Some(i) = (self.at + 1..self.items.len()).find(|&i| self.ok(i)) { self.at = i } }
    pub fn first(&mut self) { if let Some(i) = (0..self.items.len()).find(|&i| self.ok(i)) { self.at = i } }
    pub fn last(&mut self) { if let Some(i) = (0..self.items.len()).rev().find(|&i| self.ok(i)) { self.at = i } }
    /// Typing a letter jumps to the next choosable item that starts with it.
    pub fn jump(&mut self, c: char) { let n = self.items.len(); let c = c.to_lowercase().next().unwrap_or(c); if let Some(i) = (1..=n).map(|k| (self.at + k) % n).find(|&i| self.ok(i) && self.items[i].0.to_lowercase().starts_with(c)) { self.at = i } }
    pub fn chosen(&self) -> Option<usize> { self.ok(self.at).then_some(self.at) }
    /// The rows to draw (at most `max`): the first index and the count, keeping `at` in view.
    pub fn window(&self, max: usize) -> (usize, usize) { let n = self.items.len().min(max.max(1)); let first = (self.at + 1).saturating_sub(n).min(self.items.len() - n); (first, n) }
}

/// `s` cut to `cols` characters with a trailing '~' when it was longer.
pub fn fit(s: &str, cols: usize) -> String {
    let n = s.chars().count();
    if n <= cols { s.to_string() } else if cols == 0 { String::new() } else { s.chars().take(cols - 1).chain(std::iter::once('~')).collect() }
}
/// The strip's three parts for `cols` columns: (address, title, right-hand text). The address gets up to 3/5 of the width, the title the rest; the right-hand text is the load progress.
pub fn strip(url: &str, title: &str, progress: Option<u8>, cols: usize) -> (String, String, String) {
    let right = progress.map_or(String::new(), |p| format!("{p}%"));
    let room = cols.saturating_sub(right.chars().count() + 7);
    let (u, t) = (printable(url), printable(title));
    let ua = fit(&u, (room * 3 / 5).max(room.saturating_sub(t.chars().count())));
    let ta = fit(&t, room.saturating_sub(ua.chars().count()));
    (ua, ta, right)
}

/// At most this many tabs (Ctrl-T and pages opening windows stop here).
pub const MAX_TABS: usize = 8;
/// The tab list: one current tab. `T` is whatever the program keeps per tab.
#[derive(Debug)]
pub struct Tabs<T> { pub items: Vec<T>, pub cur: usize }
impl<T> Tabs<T> {
    pub fn new(first: T) -> Tabs<T> { Tabs { items: vec![first], cur: 0 } }
    /// Adds a tab at the end and makes it current; false (nothing added) when the list is full.
    pub fn open(&mut self, t: T) -> bool { if self.items.len() >= MAX_TABS { return false } self.items.push(t); self.cur = self.items.len() - 1; true }
    pub fn next(&mut self) { self.cur = (self.cur + 1) % self.items.len(); }
    pub fn prev(&mut self) { self.cur = (self.cur + self.items.len() - 1) % self.items.len(); }
    pub fn at(&self, i: usize) -> Option<&T> { self.items.get(i) }
    pub fn current(&self) -> &T { &self.items[self.cur] }
    pub fn current_mut(&mut self) -> &mut T { &mut self.items[self.cur] }
    /// Removes tab `i`; the current tab stays the same one, or the next to the right (the one on the left for the last tab) when it was removed. The last tab cannot be removed.
    pub fn close(&mut self, i: usize) -> Option<T> {
        if self.items.len() <= 1 || i >= self.items.len() { return None }
        let t = self.items.remove(i);
        if i < self.cur { self.cur -= 1 } else if self.cur >= self.items.len() { self.cur = self.items.len() - 1 }
        Some(t)
    }
}

/// A short name for a tab: the site's own name (`en.wikipedia.org` -> `wikipedia`), else the first word of the title, else `new tab`.
/// shortcut: the second-level label of a host, skipping `co`/`com`/`org`/`ac`/`gov`/`net`/`edu` (`bbc.co.uk` -> `bbc`); no public-suffix list, so odd suffixes give a long name that `fit` cuts.
pub fn tab_label(url: &str, title: &str) -> String {
    let host = url.split_once("://").map_or("", |(_, r)| r.split(['/', '?', '#']).next().unwrap_or(""));
    let host = host.rsplit_once('@').map_or(host, |(_, h)| h);
    let host = host.rsplit_once(':').filter(|(_, p)| p.bytes().all(|b| b.is_ascii_digit())).map_or(host, |(h, _)| h);
    let name = if host.is_empty() || is_ip4(host) || host == "localhost" { host.to_string() } else {
        let parts: Vec<&str> = host.split('.').filter(|p| !p.is_empty()).collect();
        let mut i = parts.len().saturating_sub(2);
        if parts.len() >= 3 && ["co", "com", "org", "ac", "gov", "net", "edu"].contains(&parts[i]) { i -= 1 }
        parts.get(i).copied().unwrap_or(host).to_string()
    };
    let name = printable(&name);
    if !name.is_empty() { return name }
    let w = title.split_whitespace().next().unwrap_or("");
    if w.is_empty() { "new tab".into() } else { printable(w) }
}
/// The tab list for the strip within `width` columns: cells `N label` (label cut to fit), the current one flagged. Cells are drawn with 3 columns between them.
pub fn strip_tabs(labels: &[String], cur: usize, width: usize) -> Vec<(String, bool)> {
    let n = labels.len().max(1);
    let per = (width.saturating_sub(3 * (n - 1)) / n).saturating_sub(2).clamp(1, 14);   // columns for a label: the number and the space come first
    labels.iter().enumerate().map(|(i, l)| (format!("{} {}", i + 1, fit(l, per)), i == cur)).collect()
}
/// The columns `strip_tabs` cells take with their separators.
pub fn tabs_cols(cells: &[(String, bool)]) -> usize { cells.iter().map(|c| c.0.chars().count()).sum::<usize>() + 3 * cells.len().saturating_sub(1) }

/// The find line in the strip: what was typed, and how the search went.
#[derive(Default, Debug)]
pub struct Find { pub line: Line, pub matches: Option<u32> }
impl Find {
    /// The right-hand text: nothing before the first answer, then the number of matches.
    pub fn status(&self) -> String {
        if self.line.text.is_empty() { return String::new() }
        match self.matches { None => String::new(), Some(0) => "not found".into(), Some(1) => "1 match".into(), Some(n) => format!("{n} matches") }
    }
}

/// The kinds of page dialog (the codes WebKit uses).
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum DialogKind { Alert, Confirm, Prompt, Leave }
impl DialogKind { pub fn from_code(c: i32) -> DialogKind { match c { 1 => DialogKind::Confirm, 2 => DialogKind::Prompt, 3 => DialogKind::Leave, _ => DialogKind::Alert } } }
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum DKey { Enter, Esc, Char(char) }
/// What the strip says for a page dialog: the page's words (one line, no control characters) and the keys that answer it.
pub fn dialog_text(kind: DialogKind, msg: &str) -> String {
    let m = printable(&msg.replace(['\n', '\r', '\t'], " "));
    match kind {
        DialogKind::Alert => format!("{m}  [Enter ok]"), DialogKind::Confirm => format!("{m}  y/n"),
        DialogKind::Prompt => format!("{m}  "), DialogKind::Leave => if m.trim().is_empty() { "leave this page? y/n".into() } else { format!("{m}  leave? y/n") },
    }
}
/// A key at a dialog: Some(true) = ok / yes, Some(false) = cancel / no, None = the key does nothing. A prompt is a line edit: Enter and Esc only (the caller edits the text).
pub fn dialog_key(kind: DialogKind, k: DKey) -> Option<bool> {
    match (kind, k) {
        (DialogKind::Alert, DKey::Enter | DKey::Esc | DKey::Char('y')) => Some(true),
        (DialogKind::Confirm | DialogKind::Leave, DKey::Char('y')) => Some(true),
        (DialogKind::Confirm | DialogKind::Leave, DKey::Char('n') | DKey::Esc) => Some(false),
        (DialogKind::Prompt, DKey::Enter) => Some(true), (DialogKind::Prompt, DKey::Esc) => Some(false),
        _ => None,
    }
}

/// The files `<input type=file>` may offer: plain files in `dir` (~/Downloads) with a clean name, not hidden, sorted, at most 200.
pub fn list_uploads(dir: &Path) -> Vec<String> {
    let Ok(rd) = std::fs::read_dir(dir) else { return vec![] };
    let mut v: Vec<String> = rd.flatten().filter(|e| e.file_type().is_ok_and(|t| t.is_file())).filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| !n.starts_with('.') && printable(n) == *n && !n.contains('/')).collect();
    v.sort(); v.truncate(200); v
}
/// The picker in the strip: up and down choose, Enter takes the file.
#[derive(Debug, Default)]
pub struct Picker { pub names: Vec<String>, pub at: usize }
impl Picker {
    pub fn up(&mut self) { self.at = self.at.saturating_sub(1); }
    pub fn down(&mut self) { if self.at + 1 < self.names.len() { self.at += 1 } }
    pub fn chosen(&self) -> Option<&str> { self.names.get(self.at).map(String::as_str) }
    pub fn text(&self) -> String {
        match self.chosen() { Some(n) => format!("upload: {n} ({}/{})  up/down, Enter, Esc", self.at + 1, self.names.len()), None => "upload: ~/Downloads has no files  Esc".into() }
    }
}

/// Real zones with no daylight saving, by offset in minutes east of UTC. WebKit (ICU) takes the host zone from TZ only when it is an IANA name without digits, so thir's offset is passed as the
/// name of such a zone. shortcut: offsets no such zone has now (-3:30, +10:30, +12:45...) get UTC; a user in them sees UTC in JavaScript dates until this has a better table.
const ZONES: &[(i32, &str)] = &[(-660, "Pacific/Pago_Pago"), (-600, "Pacific/Honolulu"), (-570, "Pacific/Marquesas"), (-540, "Pacific/Gambier"), (-480, "Pacific/Pitcairn"), (-420, "America/Phoenix"),
    (-360, "America/Regina"), (-300, "America/Panama"), (-240, "America/Puerto_Rico"), (-180, "America/Sao_Paulo"), (-120, "America/Noronha"), (-60, "Atlantic/Cape_Verde"), (0, "UTC"), (60, "Africa/Lagos"),
    (120, "Africa/Maputo"), (180, "Africa/Nairobi"), (210, "Asia/Tehran"), (240, "Asia/Dubai"), (270, "Asia/Kabul"), (300, "Asia/Karachi"), (330, "Asia/Kolkata"), (345, "Asia/Kathmandu"), (360, "Asia/Dhaka"),
    (390, "Asia/Yangon"), (420, "Asia/Bangkok"), (480, "Asia/Singapore"), (525, "Australia/Eucla"), (540, "Asia/Tokyo"), (570, "Australia/Darwin"), (600, "Australia/Brisbane"), (660, "Pacific/Noumea"),
    (720, "Pacific/Tarawa"), (780, "Pacific/Tongatapu"), (840, "Pacific/Kiritimati")];
/// The TZ value for a UTC offset in minutes (see ZONES).
pub fn tz_env(minutes: i32) -> String { ZONES.iter().find(|z| z.0 == minutes).map_or("UTC", |z| z.1).to_string() }

/// Which of the browser's three kinds of process a sandbox profile is for.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Proc { Ui, Web, Network }
/// argv[0] of a shim -> the process kind. The shim is installed under the names of the two WebKit helper programs.
pub fn proc_of(argv0: &str) -> Option<Proc> {
    match Path::new(argv0).file_name()?.to_str()? { "WPEWebProcess" => Some(Proc::Web), "WPENetworkProcess" => Some(Proc::Network), _ => None }
}
/// The user's browser folders (profile, cache, downloads) under `home`.
pub fn dirs(home: &Path) -> [PathBuf; 3] { [home.join(".local/share/thir-web"), home.join(".cache/thir-web"), home.join("Downloads")] }
/// The Landlock/seccomp rules of each process. Everything that is not listed is denied: nothing else in the home folder (no ~/.config/thir).
/// UI: what both children may do at most (Landlock only ever narrows). Web: reads only, no TCP. Network: writes the profile, cache and Downloads; TCP allowed.
pub fn profile(p: Proc, home: &Path) -> Spec {
    let [data, cache, dl] = dirs(home);
    let p_ = |s: &str| PathBuf::from(s);
    let mut s = Spec { rx: vec![p_("/web"), p_("/etc"), p_("/sys/devices/system/cpu")], r: vec![p_("/dev/urandom"), p_("/sys/firmware/acpi/pm_profile")], rw_files: vec![p_("/dev/null"), p_("/dev/zero")], seccomp: true, ..Spec::default() };
    // /proc: the UI process opens all of it (it is the ceiling for its children); a child sees only its own entry and the system-wide files WebKit and Mesa read
    if p == Proc::Ui { s.rx.push(p_("/proc")); } else {
        s.rx.extend([p_("/proc/self"), p_("/proc/thread-self"), p_("/proc/sys")]);
        s.r.extend(["meminfo", "cpuinfo", "stat", "loadavg", "uptime", "version"].map(|f| PathBuf::from("/proc").join(f)));
    }
    match p {
        Proc::Ui => { s.rw = vec![data, cache, dl, p_("/dev/shm")]; s.rw_files.extend([p_("/dev/snd/pcmC0D0p"), p_("/dev/snd/controlC0")]); }   // the ceiling for the web process
        // the web process plays the page's sound (GStreamer, in this process; there is no GPU process): the playback device and its own registry folder, nothing else; TCP stays denied
        Proc::Web => { s.deny_tcp = true; s.scope = true; s.rx.push(p_("/dev/shm")); s.rw = vec![cache.join("gstreamer")]; s.rw_files.extend([p_("/dev/snd/pcmC0D0p"), p_("/dev/snd/controlC0")]); s.rx.push(p_("/proc/asound")); }
        Proc::Network => { s.scope = true; s.rw = vec![data, cache, dl, p_("/dev/shm")]; }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn only_http_and_https_urls() {
        for u in ["http://a.b", "https://example.com/x?y=1#z", "HTTP://X.COM", "http://10.0.2.2:18080/js.html", "https://u:p@host/"] { assert!(url_ok(u), "{u}"); }
        for u in ["file:///etc/passwd", "data:text/html,hi", "javascript:alert(1)", "about:blank", "ftp://x/y", "blob:http://a/b", "view-source:http://a", "http://", "https:///x", "http:/a", "example.com", "",
                  "http://a b/", "http://a\n.b", "http://a\u{202e}.b", "httpx://a.b", "http://:80/", "ws://a.b"] { assert!(!url_ok(u), "{u:?}"); }
        assert!(!url_ok(&format!("http://a/{}", "x".repeat(5000))));
    }
    #[test] fn typed_addresses() {
        assert_eq!(address("example.com").unwrap(), "https://example.com"); assert_eq!(address("  en.wikipedia.org/wiki/Sri_Lanka ").unwrap(), "https://en.wikipedia.org/wiki/Sri_Lanka");
        assert_eq!(address("https://a.b/c").unwrap(), "https://a.b/c"); assert_eq!(address("http://a.b").unwrap(), "http://a.b");
        assert_eq!(address("10.0.2.2:18080/js.html").unwrap(), "http://10.0.2.2:18080/js.html"); assert_eq!(address("localhost:8080").unwrap(), "http://localhost:8080");
        assert_eq!(address("host.lan:8080/x").unwrap(), "https://host.lan:8080/x");
        assert_eq!(address("sri lanka").unwrap(), format!("{SEARCH}sri+lanka")); assert_eq!(address("cats").unwrap(), format!("{SEARCH}cats"));
        assert_eq!(address("what is 1.5 + 2?").unwrap(), format!("{SEARCH}what+is+1.5+%2B+2%3F")); assert_eq!(address("caf\u{e9}").unwrap(), format!("{SEARCH}caf%C3%A9"));
        for bad in ["file:///etc/passwd", "javascript:alert(1)", "data:text/html,x", "JavaScript:alert(1)", "about:blank", "ftp://x.y/z", "view-source:http://a.b", "http://"] { assert!(address(bad).is_err(), "{bad}"); }
        assert!(address("").is_err()); assert!(address("   ").is_err());
    }
    #[test] fn download_names() {
        assert_eq!(clean_name("report.pdf"), "report.pdf"); assert_eq!(clean_name("../../etc/passwd"), "passwd"); assert_eq!(clean_name("a\\b\\c.txt"), "c.txt");
        assert_eq!(clean_name(".bashrc"), "bashrc"); assert_eq!(clean_name("...x"), "x"); assert_eq!(clean_name(""), "download"); assert_eq!(clean_name("/"), "download"); assert_eq!(clean_name(".."), "download");
        assert_eq!(clean_name("a\x1b[2Jb\u{202e}.txt"), "a?[2Jb?.txt"); assert!(clean_name(&"x".repeat(300)).len() <= 100); assert_eq!(clean_name("\u{e9}".repeat(80).as_str()).chars().count(), 50);
        let taken = ["f.txt", "f (1).txt", "noext", ".hid", "a.tar.gz"];
        let ex = |n: &str| taken.contains(&n);
        assert_eq!(download_name("g.txt", ex), "g.txt"); assert_eq!(download_name("f.txt", ex), "f (2).txt"); assert_eq!(download_name("noext", ex), "noext (1)");
        assert_eq!(download_name("a.tar.gz", ex), "a.tar (1).gz"); assert_eq!(download_name("/x/f.txt", ex), "f (2).txt");
        assert_eq!(download_name("q", |_| true), "q (many)");
    }
    #[test] fn prompt_and_sizes() {
        assert_eq!(size_text(Some(0)), "0 B"); assert_eq!(size_text(Some(1023)), "1023 B"); assert_eq!(size_text(Some(1536)), "1.5 KB"); assert_eq!(size_text(Some(5 << 20)), "5.0 MB"); assert_eq!(size_text(Some(3 << 30)), "3.0 GB"); assert_eq!(size_text(None), "unknown size");
        assert_eq!(download_prompt("a\x1bb.zip", Some(2048)), "save a?b.zip (2.0 KB) to ~/Downloads? y/n");
    }
    #[test] fn line_editor() {
        let mut l = Line::new("ab"); l.insert('c'); l.left(false); l.left(false); l.insert('X'); assert_eq!(l.as_string(), "aXbc"); l.backspace(); l.delete(); assert_eq!(l.as_string(), "ac"); l.end(false); l.backspace(); l.backspace(); l.backspace(); assert_eq!(l.as_string(), "");
        l.insert('\n'); assert!(l.text.is_empty()); l.insert('\u{e9}'); l.home(false); l.delete(); assert_eq!(l, Line::default()); let mut k = Line::new("xyz"); k.kill(); assert_eq!(k.as_string(), ""); k.left(false); k.right(false); k.end(false);
    }
    #[test] fn line_selection_editing() {
        let mut l = Line::new("hello world"); l.select_all(); assert_eq!(l.selection(), Some((0, 11))); assert_eq!(l.selected(), "hello world");
        l.insert('x'); assert_eq!(l.as_string(), "x", "typing replaces the selection");
        let mut l = Line::new("abc"); l.select_all(); l.backspace(); assert_eq!(l.as_string(), ""); let mut l = Line::new("abc"); l.select_all(); l.delete(); assert_eq!((l.as_string(), l.at), (String::new(), 0));
        let mut l = Line::new("abcdef"); l.home(false); l.right(true); l.right(true); assert_eq!(l.selected(), "ab"); l.left(true); assert_eq!(l.selected(), "a"); l.left(true); assert_eq!(l.selection(), None, "anchor = cursor: nothing selected");
        l.right(true); l.right(true); l.right(false); assert_eq!((l.sel, l.at), (None, 2), "Right with a selection goes to its end"); l.left(true); l.left(true); l.left(false); assert_eq!(l.at, 0);
        let mut l = Line::new("abc"); l.select_all(); assert_eq!(l.cut(), "abc"); assert_eq!(l.as_string(), ""); assert_eq!(l.cut(), "");
        let mut l = Line::new("ab"); l.left(false); l.paste("X\nY\tZ\x1b"); assert_eq!(l.as_string(), "aX Y Zb"); l.select_all(); l.paste("new"); assert_eq!(l.as_string(), "new");
        let mut l = Line::new("one two  three"); l.word_left(false); assert_eq!(l.at, 9); l.word_left(false); assert_eq!(l.at, 4); l.word_left(false); assert_eq!(l.at, 0); l.word_left(false); assert_eq!(l.at, 0);
        l.word_right(false); assert_eq!(l.at, 3); l.word_right(false); assert_eq!(l.at, 7); l.word_right(true); assert_eq!(l.selected(), "  three"); l.word_right(false); assert_eq!(l.at, 14);
        let mut l = Line::new("ab cd"); l.home(false); l.end(true); assert_eq!(l.selected(), "ab cd"); l.home(false); assert_eq!(l.selection(), None);
        let mut l = Line::new("é ü"); l.select_all(); assert_eq!(l.selected(), "é ü");
    }
    #[test] fn line_keys_and_clipboard() {
        let mut clip = String::new(); let mut l = Line::new("hello world");
        let k = |code, ctrl| line_key(code, ctrl).unwrap();
        assert!(line_apply(&mut l, k(30, true), true, false, &mut clip) == false); assert_eq!(l.selected(), "hello world");   // Ctrl+A
        line_apply(&mut l, k(46, true), true, false, &mut clip); assert_eq!(clip, "hello world", "Ctrl+C copies");
        assert!(line_apply(&mut l, k(45, true), true, false, &mut clip)); assert_eq!(l.as_string(), "", "Ctrl+X cuts");
        assert!(line_apply(&mut l, k(47, true), true, false, &mut clip)); assert_eq!(l.as_string(), "hello world", "Ctrl+V pastes");
        l.home(false); line_apply(&mut l, k(106, true), true, true, &mut clip); assert_eq!(l.selected(), "hello", "Ctrl+Shift+Right selects a word");
        line_apply(&mut l, k(46, true), true, false, &mut clip); assert_eq!(clip, "hello"); l.end(false); line_apply(&mut l, k(47, true), true, false, &mut clip); assert_eq!(l.as_string(), "hello worldhello");
        let mut e = Line::new("x"); line_apply(&mut e, k(46, true), true, false, &mut clip); assert_eq!(clip, "hello", "copy with nothing selected keeps the clipboard");
        for c in [14, 111, 105, 106, 102, 107] { assert!(line_key(c, false).is_some()); } assert!(line_key(30, false).is_none() && line_key(46, false).is_none());
    }
    #[test] fn history_store() {
        let mut h = History::default();
        h.visit("https://a.example/", "A page", 100); h.visit("https://b.example/x", "", 200); h.visit("file:///etc/passwd", "no", 300); h.visit("https://a.example/", "", 400);
        assert_eq!(h.items.len(), 2, "one entry per address, file: refused"); assert_eq!(h.items[1], Visit { t: 400, url: "https://a.example/".into(), title: "A page".into() }, "a revisit moves to the end and keeps the title");
        h.set_title("https://b.example/x", "B\x1b[2J page"); assert_eq!(h.items[0].title, "B?[2J page");
        let back = History::parse(&h.serialize()); assert_eq!(back.items, h.items);
        let junk = History::parse("x\ty\tz\n100\tjavascript:alert(1)\tbad\n100\thttps://ok.example/\tOK\n\n300"); assert_eq!(junk.items.len(), 1);
        assert_eq!(h.search("page").len(), 2); assert_eq!(h.search("b.example PAGE")[0].url, "https://b.example/x"); assert_eq!(h.search("zzz").len(), 0); assert_eq!(h.search("")[0].t, 400, "newest first");
        assert!(h.remove("https://b.example/x") && !h.remove("https://b.example/x")); h.clear(); assert!(h.items.is_empty());
        let mut big = History::default(); for i in 0..MAX_HISTORY + 50 { big.visit(&format!("https://h{i}.example/"), "t", i as i64); } assert_eq!(big.items.len(), MAX_HISTORY); assert_eq!(big.items[0].url, "https://h50.example/");
        big.visit("https://x.example/", &"t".repeat(900), 1); assert_eq!(big.items.last().unwrap().title.len(), 200);
    }
    #[test] fn history_suggestions_and_times() {
        let mut h = History::default(); h.visit("https://en.wikipedia.org/wiki/Sri_Lanka", "Sri Lanka", 1); h.visit("https://www.example.com/docs", "Docs", 2); h.visit("https://example.org/", "Org", 3);
        assert_eq!(h.suggest("en.wiki").as_deref(), Some("pedia.org/wiki/Sri_Lanka")); assert_eq!(h.suggest("https://en.w").as_deref(), Some("ikipedia.org/wiki/Sri_Lanka"));
        assert_eq!(h.suggest("example.c").as_deref(), Some("om/docs"), "www. may be left out"); assert_eq!(h.suggest("EXAMPLE.").as_deref(), Some("org/"), "newest first, case-insensitive");
        assert_eq!(h.suggest("e"), None, "one letter is too little"); assert_eq!(h.suggest("example.org/"), None, "nothing left to add"); assert_eq!(h.suggest("two words"), None); assert_eq!(h.suggest("zzz"), None);
        let now = 1_700_000_000; assert_eq!(visit_time(now - 60, now, 0).len(), 5); assert!(visit_time(now - 86400 * 2, now, 0).contains(' ') && visit_time(now - 86400 * 20, now, 0).split(' ').count() == 2);
        assert_eq!(visit_time(1_700_000_000, 1_700_000_100, 330), "03:43"); assert_eq!(visit_time(1_700_000_000 - 86400 * 30, 1_700_000_000, 0), "15 Oct");
    }
    #[test] fn range_resume_math() {
        assert_eq!(range_header(0), "bytes=0-"); assert_eq!(range_header(1048576), "bytes=1048576-");
        assert!(resume_ok(1000, 206, Some("bytes 1000-1999/2000"), Some(2000))); assert!(resume_ok(1000, 206, Some("bytes 1000-1999/*"), Some(2000))); assert!(resume_ok(5, 206, Some("bytes 5-9/10"), None));
        assert!(!resume_ok(1000, 200, None, Some(2000)), "a 200 starts over"); assert!(!resume_ok(1000, 206, Some("bytes 0-1999/2000"), Some(2000)), "wrong start"); assert!(!resume_ok(1000, 206, None, None));
        assert!(!resume_ok(1000, 206, Some("bytes 1000-1999/3000"), Some(2000)), "the file changed size"); assert!(!resume_ok(1000, 206, Some("bytes 1000-x/2000"), None)); assert!(!resume_ok(1000, 206, Some("items 1000-1999/2000"), None));
        assert!(!resume_ok(1000, 206, Some("bytes 1000-999/2000"), None));
    }
    #[test] fn download_state_machine() {
        let mut d = Dl::new(1, "f.bin", "http://h/f.bin", Some(1000)); assert!(d.active()); assert_eq!(d.pause(), Err("cannot pause: this server does not allow resuming"));
        d.ranges = true; d.progress(300, 1000); assert_eq!((d.got(), d.percent()), (300, Some(30))); assert_eq!(d.pause(), Ok(())); assert_eq!(d.state, DlState::Pausing); assert!(d.pause().is_err());
        d.progress(320, 1100); d.stopped(320); assert_eq!((d.state.clone(), d.done, d.seg, d.got()), (DlState::Paused, 320, 0, 320)); assert!(!d.active());
        assert_eq!(d.resume(), Some("bytes=320-".into())); assert_eq!(d.state, DlState::Resuming); assert_eq!(d.resume(), None);
        assert!(d.answered(200, None).is_err()); assert!(matches!(d.state, DlState::Failed(_)));
        let mut d = Dl::new(2, "g", "http://h/g", Some(1000)); d.ranges = true; d.done = 400; d.state = DlState::Resuming;
        assert!(d.answered(206, Some("bytes 400-999/1000")).is_ok()); assert_eq!(d.state, DlState::Running); d.progress(600, 5000); assert_eq!((d.got(), d.percent()), (1000, Some(100))); d.finish(); assert_eq!((d.done, d.state.clone()), (1000, DlState::Done));
        d.fail("late"); assert_eq!(d.state, DlState::Done, "a finished download cannot fail"); d.cancel(); assert_eq!(d.state, DlState::Done);
        let mut u = Dl::new(3, "u", "http://h/u", None); u.progress(50, 0); u.finish(); assert_eq!(u.done, 50); assert_eq!(Dl::new(4, "x", "u", None).percent(), None);
        let mut f = Dl::new(5, "x", "u", Some(10)); f.fail("disk \x1b full"); assert_eq!(f.status_text(), "failed: disk ?[ full".replace("?[ ", "? ").replace("disk ? full", "disk ? full")); let mut c = Dl::new(6, "x", "u", None); c.cancel(); assert_eq!(c.status_text(), "cancelled");
    }
    #[test] fn speed_and_bars() {
        let mut s = Speed::default(); assert_eq!(s.rate(), 0); s.push(0, 0); assert_eq!(s.rate(), 0); s.push(1000, 100_000); assert_eq!(s.rate(), 100_000); s.push(2000, 300_000); assert_eq!(s.rate(), 150_000);
        for i in 3..20 { s.push(i * 1000, i * 200_000); } assert_eq!(s.rate(), 200_000, "old samples drop out"); s.reset(); assert_eq!(s.rate(), 0);
        assert_eq!((bar_cells(None, 10), bar_cells(Some(0), 10), bar_cells(Some(50), 10), bar_cells(Some(100), 10), bar_cells(Some(250), 10)), (0, 0, 5, 10, 10));
    }
    #[test] fn option_menu_choice() {
        let it = |l: &str, e: bool, g: bool| (l.to_string(), e, g);
        let mut m = Menu::new(vec![it("Fruit", true, true), it("Apple", true, false), it("Pear", false, false), it("Plum", true, false), it("Veg", true, true), it("Leek", true, false)], None);
        assert_eq!(m.chosen(), Some(1), "starts on the first choosable item"); m.down(); assert_eq!(m.at, 3, "skips the disabled one"); m.down(); assert_eq!(m.at, 5, "and the heading"); m.down(); assert_eq!(m.at, 5); m.up(); assert_eq!(m.at, 3);
        m.first(); assert_eq!(m.at, 1); m.last(); assert_eq!(m.at, 5); m.jump('p'); assert_eq!(m.at, 3); m.jump('A'); assert_eq!(m.at, 1); m.jump('z'); assert_eq!(m.at, 1);
        let m = Menu::new(vec![it("a", true, false), it("b", true, false)], Some(1)); assert_eq!(m.chosen(), Some(1)); let m = Menu::new(vec![it("a", false, false)], Some(0)); assert_eq!(m.chosen(), None); assert!(Menu::default().chosen().is_none());
        let m = Menu { items: (0..20).map(|i| it(&i.to_string(), true, false)).collect(), at: 15 }; assert_eq!(m.window(8), (8, 8)); let m = Menu { at: 2, ..m }; assert_eq!(m.window(8), (0, 8)); assert_eq!(Menu::default().window(5), (0, 0));
    }
    #[test] fn strip_fits_the_width() {
        let (u, t, r) = strip("https://example.com/", "Example Domain", Some(64), 80); assert_eq!((u.as_str(), t.as_str(), r.as_str()), ("https://example.com/", "Example Domain", "64%"));
        let (u, t, r) = strip(&"u".repeat(200), &"t".repeat(200), None, 40); assert!(u.chars().count() + t.chars().count() + 3 <= 40, "{u} {t}"); assert!(u.ends_with('~') && r.is_empty());
        let (u, t, _) = strip("a\x1b[2Jb", "t\u{202e}", None, 20); assert_eq!((u.as_str(), t.as_str()), ("a?[2Jb", "t?")); let _ = strip("x", "y", Some(100), 0); let _ = strip("x", "y", None, 3);
        assert_eq!(fit("abcdef", 4), "abc~"); assert_eq!(fit("abc", 3), "abc"); assert_eq!(fit("abc", 0), "");
    }
    #[test] fn profiles_keep_the_home_folder_closed() {
        let h = Path::new("/data/home/alice");
        for p in [Proc::Ui, Proc::Web, Proc::Network] {
            let s = profile(p, h);
            for dir in s.rx.iter().chain(&s.r).chain(&s.rw).chain(&s.rw_files) { assert!(!dir.starts_with(h.join(".config")) && dir != h && !h.starts_with(dir) || dir == Path::new("/"), "{p:?} {dir:?}"); }
            assert!(s.seccomp); assert!(!s.rx.iter().any(|d| d == Path::new("/")));
            assert_eq!(s.rx.contains(&PathBuf::from("/proc")), p == Proc::Ui, "{p:?}: only the UI process opens all of /proc");
        }
        let (u, w, n) = (profile(Proc::Ui, h), profile(Proc::Web, h), profile(Proc::Network, h));
        assert!(w.deny_tcp && w.scope && !n.deny_tcp && !u.deny_tcp);
        assert!(w.rw == [h.join(".cache/thir-web/gstreamer")], "the web process writes only its GStreamer registry folder in the home folder");
        assert!(w.rw_files.contains(&PathBuf::from("/dev/snd/pcmC0D0p")) && !n.rw_files.iter().any(|f| f.starts_with("/dev/snd")), "the network process cannot play sound");
        assert!(n.rw.contains(&h.join("Downloads")) && n.rw.contains(&h.join(".local/share/thir-web")) && n.rw.contains(&h.join(".cache/thir-web")));
        assert!(u.rw.contains(&h.join("Downloads")));
        assert_eq!(proc_of("/usr/libexec/wpe-webkit-2.0/WPEWebProcess"), Some(Proc::Web)); assert_eq!(proc_of("WPENetworkProcess"), Some(Proc::Network)); assert_eq!(proc_of("sh"), None); assert_eq!(proc_of(""), None);
    }
    #[test] fn tab_list_logic() {
        let mut t = Tabs::new("a"); assert!(t.close(0).is_none(), "the last tab stays"); assert!(t.open("b") && t.open("c")); assert_eq!((t.cur, *t.current()), (2, "c"));
        t.next(); assert_eq!(*t.current(), "a"); t.prev(); assert_eq!(*t.current(), "c"); t.cur = 1;
        assert_eq!(t.close(0), Some("a")); assert_eq!((t.cur, *t.current()), (0, "b"), "closing a tab on the left keeps the same current tab");
        assert_eq!(t.close(0), Some("b")); assert_eq!((t.cur, *t.current()), (0, "c"), "closing the current tab shows its right neighbour");
        let mut t = Tabs::new(0); for i in 1..MAX_TABS { assert!(t.open(i)); } assert!(!t.open(99), "no more than {MAX_TABS}"); assert_eq!(t.items.len(), MAX_TABS);
        assert_eq!(t.close(MAX_TABS - 1), Some(MAX_TABS - 1)); assert_eq!(t.cur, MAX_TABS - 2, "the last tab closed: the one on its left");
        assert!(t.close(50).is_none()); t.cur = 0; t.prev(); assert_eq!(t.cur, MAX_TABS - 2);
    }
    #[test] fn tab_names_and_strip_cells() {
        for (u, t, want) in [("https://en.wikipedia.org/wiki/Sri_Lanka", "Sri Lanka", "wikipedia"), ("https://github.com/", "", "github"), ("https://www.bbc.co.uk/news", "", "bbc"), ("http://10.0.2.2:18080/x", "", "10.0.2.2"),
            ("http://localhost:8080", "", "localhost"), ("", "Hello world", "Hello"), ("", "", "new tab"), ("https://u:p@example.com:99/", "", "example"), ("https://a\u{1b}b.com", "", "a?b")] { assert_eq!(tab_label(u, t), want, "{u}"); }
        let l: Vec<String> = ["wikipedia", "github"].iter().map(|s| s.to_string()).collect();
        let c = strip_tabs(&l, 1, 60); assert_eq!(c, vec![("1 wikipedia".to_string(), false), ("2 github".to_string(), true)]); assert_eq!(tabs_cols(&c), 11 + 3 + 8);
        let l: Vec<String> = (0..8).map(|i| format!("{i}-averyveryverylongname")).collect();
        for w in [24, 40, 80] { let c = strip_tabs(&l, 3, w); assert_eq!(c.iter().filter(|x| x.1).count(), 1); assert!(tabs_cols(&c) <= w.max(8 * 4 + 21), "{w}: {}", tabs_cols(&c)); assert!(c.iter().all(|x| x.0.chars().count() >= 3)); }
        assert!(strip_tabs(&[], 0, 10).is_empty() && tabs_cols(&[]) == 0);
    }
    #[test] fn find_status_text() {
        let mut f = Find::default(); assert_eq!(f.status(), ""); f.line = Line::new("cat"); assert_eq!(f.status(), ""); f.matches = Some(0); assert_eq!(f.status(), "not found");
        f.matches = Some(1); assert_eq!(f.status(), "1 match"); f.matches = Some(12); assert_eq!(f.status(), "12 matches"); f.line.kill(); assert_eq!(f.status(), "");
    }
    #[test] fn dialogs_in_the_strip() {
        use DialogKind::*; assert_eq!([0, 1, 2, 3, 9].map(DialogKind::from_code), [Alert, Confirm, Prompt, Leave, Alert]);
        assert_eq!(dialog_text(Alert, "hi\nthere\x1b[2J"), "hi there?[2J  [Enter ok]"); assert_eq!(dialog_text(Confirm, "sure?"), "sure?  y/n"); assert_eq!(dialog_text(Leave, ""), "leave this page? y/n"); assert_eq!(dialog_text(Prompt, "name"), "name  ");
        assert_eq!(dialog_key(Alert, DKey::Enter), Some(true)); assert_eq!(dialog_key(Alert, DKey::Char('x')), None);
        assert_eq!((dialog_key(Confirm, DKey::Char('y')), dialog_key(Confirm, DKey::Char('n')), dialog_key(Confirm, DKey::Esc), dialog_key(Confirm, DKey::Enter)), (Some(true), Some(false), Some(false), None));
        assert_eq!((dialog_key(Leave, DKey::Char('y')), dialog_key(Leave, DKey::Esc)), (Some(true), Some(false)));
        assert_eq!((dialog_key(Prompt, DKey::Enter), dialog_key(Prompt, DKey::Esc), dialog_key(Prompt, DKey::Char('y'))), (Some(true), Some(false), None));
    }
    #[test] fn upload_list_and_picker() {
        let d = std::env::temp_dir().join(format!("thir-web-up-{}", std::process::id())); let _ = std::fs::remove_dir_all(&d); std::fs::create_dir_all(d.join("sub")).unwrap();
        for f in ["b.txt", "a.png", ".hidden", "esc\u{1b}x"] { std::fs::write(d.join(f), "x").unwrap(); }
        std::os::unix::fs::symlink("/etc/passwd", d.join("link")).unwrap();
        let v = list_uploads(&d); assert_eq!(v, ["a.png", "b.txt"], "no folders, links, hidden or unprintable names"); assert!(list_uploads(&d.join("nope")).is_empty());
        let mut p = Picker { names: v, at: 0 }; assert_eq!(p.text(), "upload: a.png (1/2)  up/down, Enter, Esc"); p.up(); assert_eq!(p.chosen(), Some("a.png")); p.down(); p.down(); assert_eq!(p.chosen(), Some("b.txt"));
        assert!(Picker::default().chosen().is_none() && Picker::default().text().contains("no files")); let _ = std::fs::remove_dir_all(&d);
    }
    #[test] fn time_zone_for_javascript() {
        assert_eq!(tz_env(330), "Asia/Kolkata"); assert_eq!(tz_env(0), "UTC"); assert_eq!(tz_env(-300), "America/Panama"); assert_eq!(tz_env(-210), "UTC", "no zone without daylight saving: UTC");
        for w in ZONES.windows(2) { assert!(w[0].0 < w[1].0); } assert!(ZONES.iter().all(|z| !z.1.bytes().any(|b| b.is_ascii_digit())), "ICU ignores a TZ with digits");
        for m in (-14 * 60..=14 * 60).step_by(15) { assert!(!tz_env(m).is_empty()); }
    }
}
