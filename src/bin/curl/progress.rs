//! Progress display for agent build, edit and fix, and the chat's "thinking" mark. Calm: one breathing dot, grey theme, no totals.
//! The pure part (Model: rows, text counting, line formatting) is tested; Progress adds the ticker thread that redraws the live block.
//! Not a tty: Progress prints the plain lines of old (no escapes, no thread).
use std::{io::Write, sync::{atomic::{AtomicBool, Ordering::SeqCst}, Arc, Mutex}, thread, time::{Duration, Instant}};
use thir::{files::printable, term};

pub const ACC: &str = "\x1b[1m"; pub const DIM: &str = "\x1b[90m"; const GOOD: &str = "\x1b[32m"; pub const WARN: &str = "\x1b[33m"; const BAD: &str = "\x1b[31m"; pub const R: &str = "\x1b[0m";
const DOTS: [&str; 4] = ["·", "•", "●", "•"];
const TICK_MS: u64 = 120;

fn dot(ms: u64) -> &'static str { DOTS[(ms / 180 % 4) as usize] }

/// `s` cut to `max` characters, the last one `…` when something was cut.
pub fn fit(s: &str, max: usize) -> String {
    if s.chars().count() <= max { return s.to_string(); }
    if max == 0 { return String::new(); }
    s.chars().take(max - 1).chain(std::iter::once('…')).collect()
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Mark { Pending, Active, Done, Warn, Bad }

#[derive(Debug)]
pub struct Row { pub label: String, pub mark: Mark, pub pre: String, pub note: String, t0: u64 }

/// Rows of steps plus the text of the active step: how many lines came and the last complete non-empty one.
#[derive(Debug, Default)]
pub struct Model { pub rows: Vec<Row>, shown: usize, nl: usize, partial: String, pub last: String }

impl Model {
    pub fn new(labels: &[&str]) -> Model { Model { rows: labels.iter().map(|l| row(l, Mark::Pending, 0)).collect(), ..Default::default() } }
    fn active(&self) -> Option<usize> { self.rows.iter().position(|r| r.mark == Mark::Active) }
    fn reset_text(&mut self) { self.nl = 0; self.partial.clear(); self.last.clear(); }
    pub fn lines(&self) -> usize { self.nl + usize::from(!self.partial.trim().is_empty()) }

    pub fn begin(&mut self, label: &str, now: u64) {
        if let Some(r) = self.rows.iter_mut().find(|r| r.mark == Mark::Pending && r.label == label) { r.mark = Mark::Active; r.t0 = now; }
        else if !self.rows.iter().any(|r| r.mark == Mark::Pending && r.label == label) { self.rows.push(row(label, Mark::Active, now)); }   // a step the plan did not list (agent do: read)
        self.reset_text();
    }
    pub fn text(&mut self, t: &str) {
        for c in t.chars() {
            if c == '\n' { self.nl += 1; if !self.partial.trim().is_empty() && !self.partial.trim().starts_with("===") { self.last = self.partial.trim().to_string(); } /* marker lines are not a preview */ self.partial.clear(); }
            else { self.partial.push(if c == '\t' { ' ' } else { c }); }
        }
    }
    fn auto_note(&self, r: &Row, now: u64) -> String { let s = format!("{} s", now.saturating_sub(r.t0) / 1000); if self.lines() > 0 { format!("{} lines · {s}", self.lines()) } else { s } }
    /// A finished row inserted before the first step that is not done yet (at the end when none is left).
    pub fn note(&mut self, label: &str, note: &str) {
        let at = self.rows.iter().position(|r| matches!(r.mark, Mark::Pending | Mark::Active)).unwrap_or(self.rows.len());
        let mut r = row(label, Mark::Done, 0); r.note = note.to_string();
        self.rows.insert(at, r);
    }
    /// The active step is done; `note` replaces the automatic `N lines · T s`.
    pub fn finish(&mut self, note: Option<&str>, now: u64) {
        let Some(i) = self.active() else { return };
        let auto = self.auto_note(&self.rows[i], now);
        let r = &mut self.rows[i];
        r.note = [r.pre.as_str(), note.unwrap_or(&auto)].iter().filter(|s| !s.is_empty()).copied().collect::<Vec<_>>().join(" · ");
        r.mark = Mark::Done; self.reset_text();
    }
    pub fn fail(&mut self, note: &str) {
        if let Some(i) = self.active() { self.rows[i].mark = Mark::Bad; self.rows[i].note = note.to_string(); }
        self.rows.retain(|r| r.mark != Mark::Pending); self.reset_text();
    }
    /// The active step failed (`reason`): it becomes a warning, `fix N/MAX` is the new active step and check / test run follow.
    pub fn retry(&mut self, n: usize, max: usize, reason: &str, now: u64) {
        if let Some(i) = self.active() {
            // the row is already labelled "test run": "test run, after key …" becomes "after key …"
            let r = reason.strip_prefix(&format!("{}, ", self.rows[i].label)).unwrap_or(reason);
            self.rows[i].mark = Mark::Warn; self.rows[i].note = r.to_string();
        }
        self.rows.retain(|r| r.mark != Mark::Pending);
        let mut f = row(&format!("fix {n}/{max}"), Mark::Active, now); f.pre = format!("fixing ({n}/{max})");
        self.rows.push(f); self.rows.push(row("check", Mark::Pending, 0)); self.rows.push(row("test run", Mark::Pending, 0));
        self.reset_text();
    }
    /// Rows that will not change any more and were not yet taken.
    pub fn take_settled(&mut self, now: u64, w: usize) -> Vec<String> {
        let mut out = vec![];
        while self.shown < self.rows.len() && !matches!(self.rows[self.shown].mark, Mark::Pending | Mark::Active) { out.push(self.line(&self.rows[self.shown], now, w)); self.shown += 1; }
        out
    }
    fn line(&self, r: &Row, now: u64, w: usize) -> String {
        let (ms, mark, ls) = match r.mark { Mark::Pending => (DIM, "·", DIM), Mark::Active => (ACC, dot(now), ACC), Mark::Done => (GOOD, "●", ""), Mark::Warn => (WARN, "×", ""), Mark::Bad => (BAD, "×", "") };
        let note = match r.mark {
            Mark::Pending => String::new(),
            Mark::Active => { let s = now.saturating_sub(r.t0) / 1000; if self.lines() > 0 { format!("{} lines · {s} s", self.lines()) } else { format!("thinking {s} s") } }
            _ => r.note.clone(),
        };
        if note.is_empty() { format!("  {ms}{mark}{R} {ls}{}{R}", r.label) } else { format!("  {ms}{mark}{R} {ls}{:<10}{R}  {DIM}{}{R}", r.label, fit(&note, w.saturating_sub(17))) }
    }
    /// The live lines: every row not yet settled, and under the active one a preview of the last line written.
    pub fn block(&self, now: u64, w: usize) -> Vec<String> {
        let mut v = vec![];
        for r in &self.rows[self.shown..] {
            v.push(self.line(r, now, w));
            if r.mark == Mark::Active && !self.last.is_empty() { v.push(format!("    {DIM}› {}{R}", fit(&printable(&self.last), w.saturating_sub(7)))); }
        }
        v
    }
}
fn row(l: &str, mark: Mark, t0: u64) -> Row { Row { label: l.into(), mark, pre: String::new(), note: String::new(), t0 } }

struct Inner { model: Model, drawn: usize, last: Vec<String> }
struct Shared { m: Mutex<Inner>, stop: AtomicBool, t0: Instant }
impl Shared {
    fn now(&self) -> u64 { self.t0.elapsed().as_millis() as u64 }
    /// Redraws under the lock: up to the top of the old block, settled rows once and for all, then the block. Only when something changed.
    fn draw(&self, g: &mut Inner) {
        let (now, w) = (self.now(), term::size().0.max(20) - 1);
        let done = g.model.take_settled(now, w);
        let block = g.model.block(now, w);
        if done.is_empty() && block == g.last { return; }
        let mut s = String::new();
        if g.drawn > 1 { s += &format!("\x1b[{}A", g.drawn - 1); }
        s.push('\r');
        for l in &done { s += l; s += "\x1b[K\n"; }
        s += &block.join("\x1b[K\n");
        s += "\x1b[J";
        term::put(s.as_bytes());
        g.drawn = block.len(); g.last = block;
    }
}

pub struct Progress { sh: Option<Arc<Shared>>, th: Option<thread::JoinHandle<()>>, open: AtomicBool }

impl Progress {
    /// Prints the header. `plain_header` is the old one-line header for a terminal-less run.
    pub fn new(header: &str, detail: &str, plain_header: &str, labels: &[&str]) -> Progress {
        if unsafe { libc::isatty(1) } != 1 { println!("{plain_header}"); return Progress { sh: None, th: None, open: AtomicBool::new(false) }; }
        term::cursor_hide();
        term::put(format!("  {ACC}{header}{R}{DIM}{detail}{R}\n").as_bytes());
        let sh = Arc::new(Shared { m: Mutex::new(Inner { model: Model::new(labels), drawn: 0, last: vec![] }), stop: AtomicBool::new(false), t0: Instant::now() });
        let s2 = sh.clone();
        let th = thread::spawn(move || while !s2.stop.load(SeqCst) {
            for _ in 0..TICK_MS / 20 { if s2.stop.load(SeqCst) { return; } thread::sleep(Duration::from_millis(20)); }
            let mut g = s2.m.lock().unwrap(); s2.draw(&mut g);
        });
        let p = Progress { sh: Some(sh), th: Some(th), open: AtomicBool::new(false) };
        p.edit(|_, _| {}); p
    }
    fn edit(&self, f: impl FnOnce(&mut Model, u64)) {
        if let Some(sh) = &self.sh { let mut g = sh.m.lock().unwrap(); f(&mut g.model, sh.now()); sh.draw(&mut g); }
    }
    pub fn step(&self, label: &str, plain: &str) {
        if self.sh.is_none() { print!("{plain}…"); let _ = std::io::stdout().flush(); self.open.store(true, SeqCst); }
        self.edit(|m, now| m.begin(label, now));
    }
    /// Text of the active step arrived.
    pub fn text(&self, t: &str) { if let Some(sh) = &self.sh { sh.m.lock().unwrap().model.text(t); } }
    pub fn finish(&self, note: Option<&str>, plain: &str) {
        if self.sh.is_none() && self.open.swap(false, SeqCst) { if !plain.is_empty() { print!(" {plain}"); } println!(); }
        self.edit(|m, now| m.finish(note, now));
    }
    /// A finished row (for example `memory`); without a terminal the `plain` line is printed.
    pub fn note(&self, label: &str, note: &str, plain: &str) {
        if self.sh.is_none() { println!("{plain}"); }
        self.edit(|m, _| m.note(label, note));
    }
    pub fn fail(&self, note: &str) {
        if self.sh.is_none() { if self.open.swap(false, SeqCst) { println!(); } }
        self.edit(|m, _| m.fail(note));
    }
    pub fn retry(&self, n: usize, max: usize, reason: &str) {
        if self.sh.is_none() { println!("{}fixing ({n}/{max}): {reason}", if self.open.swap(false, SeqCst) { "\n" } else { "" }); }
        self.edit(|m, now| m.retry(n, max, reason, now));
    }
    fn close(&mut self) {
        let Some(sh) = self.sh.take() else { return };
        sh.stop.store(true, SeqCst);
        if let Some(t) = self.th.take() { let _ = t.join(); }
        let mut g = sh.m.lock().unwrap();
        g.model.fail("failed");   // an active step left over failed; a finished run has none
        sh.draw(&mut g);
        term::cursor_show();
    }
    /// Ends the display and prints the last line; returns it for the summary shown again after the app quits.
    /// `hint` is an optional dim second line under the result.
    pub fn done(&mut self, what: &str, secs: u64, calls: usize, k: usize, hint: &str) -> String {
        let c = format!("{calls} call{}", if calls == 1 { "" } else { "s" });
        let more = format!("{secs} s · {c} · about {k}k tokens");
        if self.sh.is_none() { let l = format!("{what} in {secs} s ({c}, about {k}k tokens)"); println!("{l}"); return l; }
        self.close();
        let mut l = format!("  {GOOD}●{R} {ACC}{what}{R}{DIM}  {more}{R}");
        if !hint.is_empty() { l += &format!("\n    {DIM}{hint}{R}"); }
        term::put(format!("\n{l}\n").as_bytes());   // a blank line sets the result apart from the step rows
        l
    }
}
/// A result line like the progress rows: `  ● text` (green) or `  × text` (red).
pub fn mark(ok: bool, s: &str) -> String { if ok { format!("  {GOOD}●{R} {s}") } else { format!("  {BAD}×{R} {s}") } }
impl Drop for Progress { fn drop(&mut self) { self.close(); } }

/// The chat's wait mark on stderr: a breathing dot, `thinking` and the seconds, erased when the first text arrives.
pub struct Think { st: Option<(Arc<AtomicBool>, thread::JoinHandle<()>)> }
pub fn think_line(ms: u64) -> String { format!("{DIM}{} thinking {} s{R}", dot(ms), ms / 1000) }
fn err(s: &str) { let _ = std::io::stderr().write_all(s.as_bytes()); }
impl Think {
    pub fn start() -> Think {
        if unsafe { libc::isatty(2) } != 1 { return Think { st: None }; }
        if unsafe { libc::isatty(1) } != 1 { err("thinking..."); return Think { st: None }; }   // stdout redirected: no cursor games, a still mark
        term::cursor_hide();
        let (stop, t0) = (Arc::new(AtomicBool::new(false)), Instant::now());
        let s2 = stop.clone();
        let th = thread::spawn(move || { let mut last = String::new(); while !s2.load(SeqCst) {
            let l = think_line(t0.elapsed().as_millis() as u64);
            if l != last { err(&format!("\r{l}\x1b[K")); last = l; }
            thread::sleep(Duration::from_millis(20)); } });
        Think { st: Some((stop, th)) }
    }
    pub fn clear(&mut self) {
        match self.st.take() { Some((stop, th)) => { stop.store(true, SeqCst); let _ = th.join(); err("\r\x1b[K"); term::cursor_show(); } None => {} }
    }
}
impl Drop for Think { fn drop(&mut self) { self.clear(); } }

#[cfg(test)]
mod tests {
    use super::*;
    fn m() -> Model { Model::new(&["plan", "code", "check", "test run"]) }
    fn plain(s: &str) -> String { let mut o = String::new(); let mut esc = false; for c in s.chars() { if c == '\x1b' { esc = true } else if esc { if c.is_ascii_alphabetic() { esc = false } } else { o.push(c) } } o }

    #[test] fn fit_cuts_with_an_ellipsis() {
        assert_eq!(fit("abcdef", 6), "abcdef"); assert_eq!(fit("abcdef", 5), "abcd…"); assert_eq!(fit("abc", 0), ""); assert_eq!(fit("é€x", 2), "é…");
    }
    #[test] fn preview_is_the_last_complete_nonempty_line_across_chunks() {
        let mut m = m(); m.begin("plan", 0);
        m.text("local fun"); assert_eq!(m.last, "");
        m.text("ction a()\n  retu"); assert_eq!(m.last, "local function a()");
        m.text("rn 1\n\n\n"); assert_eq!(m.last, "return 1"); assert_eq!(m.lines(), 4);
        m.text("\tx"); assert_eq!(m.lines(), 5);
    }
    #[test] fn a_step_not_in_the_plan_is_added_and_markers_are_no_preview() {
        let mut m = Model::new(&["plan"]); m.begin("plan", 0); m.text("a\n===END===\n"); assert_eq!(m.last, "a"); m.finish(None, 0);
        m.begin("read", 0); assert_eq!((m.rows.len(), m.rows[1].mark, m.rows[1].label.as_str()), (2, Mark::Active, "read")); m.finish(None, 0);
        m.begin("plan", 0); assert_eq!((m.rows.len(), m.rows[2].mark), (3, Mark::Active));
        assert_eq!(plain(&mark(true, "x")), "  ● x"); assert_eq!(plain(&mark(false, "x")), "  × x");
    }
    #[test] fn steps_settle_in_order_and_show_what_is_left() {
        let mut m = m(); m.begin("plan", 0); m.text("a\nb\n");
        assert_eq!(plain(&m.block(5400, 80).join("|")), "  ● plan        2 lines · 5 s|    › b|  · code|  · check|  · test run");
        m.finish(None, 9000); m.begin("code", 9000);
        let s = m.take_settled(9000, 80); assert_eq!(s.len(), 1); assert_eq!(plain(&s[0]), "  ● plan        2 lines · 9 s");
        assert_eq!(plain(&m.block(21000, 80)[0]), "  · code        thinking 12 s");
        assert!(m.take_settled(0, 80).is_empty());
    }
    #[test] fn a_failed_check_adds_a_fix_step_and_fresh_checks() {
        let mut m = m(); m.begin("plan", 0); m.finish(None, 1000); m.begin("code", 1000); m.finish(None, 2000); m.begin("check", 2000); m.finish(Some("passed"), 2000); m.begin("test run", 2000);
        m.retry(1, 3, "no function main", 3000);
        let s: Vec<String> = m.take_settled(3000, 80).iter().map(|l| plain(l)).collect();
        assert_eq!(s.last().unwrap(), "  × test run    no function main");
        let b: Vec<String> = m.block(3000, 80).iter().map(|l| plain(l)).collect();
        assert_eq!(b, ["  · fix 1/3     thinking 0 s", "  · check", "  · test run"]);
        m.finish(None, 8000);
        assert_eq!(plain(&m.take_settled(8000, 80)[0]), "  ● fix 1/3     fixing (1/3) · 5 s");
    }
    #[test] fn a_note_goes_before_the_first_unfinished_step() {
        let mut m = m(); m.begin("plan", 0); m.finish(None, 1000); m.note("memory", "city: Colombo");
        let b: Vec<String> = m.take_settled(1000, 80).iter().map(|l| plain(l)).collect();
        assert_eq!(b, ["  ● plan        1 s", "  ● memory      city: Colombo"]);
        assert_eq!(m.rows.iter().map(|r| r.label.as_str()).collect::<Vec<_>>(), ["plan", "memory", "code", "check", "test run"]);
        let mut e = Model::new(&["code"]); e.begin("code", 0); e.finish(None, 0); e.note("memory", "x"); assert_eq!(e.rows[1].label, "memory");
    }
    #[test] fn lines_fit_the_width_and_failure_drops_pending_rows() {
        let mut m = m(); m.begin("plan", 0); m.finish(Some(&"x".repeat(200)), 0);
        let l = plain(&m.take_settled(0, 40)[0]); assert_eq!(l.chars().count(), 39); assert!(l.ends_with('…'));
        m.begin("code", 0); m.text("x\n"); m.fail("declined");
        assert_eq!(m.rows.len(), 2); assert_eq!(m.rows[1].mark, Mark::Bad);
    }
}
