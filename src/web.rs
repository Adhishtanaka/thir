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

/// The address line while it is edited (Ctrl-L): characters and a cursor, nothing else.
#[derive(Default, Debug, PartialEq)]
pub struct Line { pub text: Vec<char>, pub at: usize }
impl Line {
    pub fn new(s: &str) -> Line { let text: Vec<char> = s.chars().collect(); Line { at: text.len(), text } }
    pub fn insert(&mut self, c: char) { if !c.is_control() && self.text.len() < MAX_URL { self.text.insert(self.at, c); self.at += 1; } }
    pub fn backspace(&mut self) { if self.at > 0 { self.at -= 1; self.text.remove(self.at); } }
    pub fn delete(&mut self) { if self.at < self.text.len() { self.text.remove(self.at); } }
    pub fn left(&mut self) { self.at = self.at.saturating_sub(1); }
    pub fn right(&mut self) { self.at = (self.at + 1).min(self.text.len()); }
    pub fn home(&mut self) { self.at = 0; }
    pub fn end(&mut self) { self.at = self.text.len(); }
    pub fn kill(&mut self) { self.text.clear(); self.at = 0; }
    pub fn as_string(&self) -> String { self.text.iter().collect() }
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
    let mut s = Spec { rx: vec![p_("/web"), p_("/etc"), p_("/sys/devices/system/cpu")], r: vec![p_("/dev/urandom")], rw_files: vec![p_("/dev/null"), p_("/dev/zero")], seccomp: true, ..Spec::default() };
    // /proc: the UI process opens all of it (it is the ceiling for its children); a child sees only its own entry and the system-wide files WebKit and Mesa read
    if p == Proc::Ui { s.rx.push(p_("/proc")); } else {
        s.rx.extend([p_("/proc/self"), p_("/proc/thread-self"), p_("/proc/sys")]);
        s.r.extend(["meminfo", "cpuinfo", "stat", "loadavg", "uptime", "version"].map(|f| PathBuf::from("/proc").join(f)));
    }
    match p {
        Proc::Ui => { s.rw = vec![data, cache, dl, p_("/dev/shm")]; }
        Proc::Web => { s.deny_tcp = true; s.scope = true; s.rx.push(p_("/dev/shm")); }
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
        let mut l = Line::new("ab"); l.insert('c'); l.left(); l.left(); l.insert('X'); assert_eq!(l.as_string(), "aXbc"); l.backspace(); l.delete(); assert_eq!(l.as_string(), "ac"); l.end(); l.backspace(); l.backspace(); l.backspace(); assert_eq!(l.as_string(), "");
        l.insert('\n'); assert!(l.text.is_empty()); l.insert('\u{e9}'); l.home(); l.delete(); assert_eq!(l, Line::default()); let mut k = Line::new("xyz"); k.kill(); assert_eq!(k.as_string(), ""); k.left(); k.right(); k.end();
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
        assert!(!w.rw.iter().any(|d| d.starts_with(h)), "the web process writes nothing in the home folder");
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
