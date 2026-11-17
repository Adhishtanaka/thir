//! thir apps: a Lua 5.4 sandbox, the thir SDK, a character grid and the event loop behind `agent run`.
//! Security rules: an app sees only the SDK below (no io, os.execute, load, require, debug, env or print), draws only through the grid
//! (every string is cleaned, so it cannot emit escape sequences), reaches only the origins its manifest lists, and is stopped when a call runs
//! over 2 s, uses over 32 MiB, or the process runs over its watchdog time. Running an app never reads keys or the agent config.
#![cfg_attr(not(test), allow(dead_code))]   // check, lint and dry_run are for the build pipeline (next phase); unit tests use them now
use crate::conn::exchange;
use mlua::{chunk::ChunkMode, Error as LErr, Function, HookTriggers, IntoLuaMulti, Lua, LuaOptions, MultiValue, Result as LResult, StdLib, Table, Value, VmState};
use std::{cell::{Cell as StdCell, RefCell}, collections::BTreeMap, io::Read, path::PathBuf, rc::Rc, time::{Duration, Instant}};
use thir::{files::{printable, unsafe_char}, http::{self, Body, Url}, json::{self, Json}, line::{Decoder, Key}, term};

const MEM_LIMIT: usize = 32 << 20;
const CALL_LIMIT: Duration = Duration::from_secs(2);
const WATCHDOG_SECS: u32 = 6;   // SIGALRM ends a process whose call ignores CALL_LIMIT (pcall around an endless loop, a slow pattern match)
const HTTP_LIMIT: Duration = Duration::from_secs(15);
const MAX_BODY: u64 = 2 << 20;
const MAX_STORE: usize = 1 << 20;
const MAX_REDIRECTS: usize = 3;
const MIN_SIZE: (usize, usize) = (20, 5);
const ENC_DEPTH: usize = 32;
const ENC_MAX: usize = 4 << 20;   // a bigger JSON text from one call is refused: memory is limited too (RLIMIT_AS)

pub const UI: &[&str] = &["text", "center", "pad", "wrap", "width", "fill", "box", "title", "footer", "list", "bar", "input", "pick", "confirm", "size"];
pub const HTTP: &[&str] = &["get", "json", "post", "encode"];
pub const JSON: &[&str] = &["decode", "encode"];
pub const STORE: &[&str] = &["get", "set"];
pub const APP: &[&str] = &["every", "quit", "args", "name"];
const ROLES: [&str; 10] = ["text", "dim", "accent", "good", "warn", "bad", "info", "cyan", "magenta", "sel"];
const SGR: [&str; 10] = ["\x1b[0m", "\x1b[0;90m", "\x1b[0;1;37m", "\x1b[0;32m", "\x1b[0;33m", "\x1b[0;31m", "\x1b[0;34m", "\x1b[0;36m", "\x1b[0;35m", "\x1b[0;7m"];
const SEL: u8 = 9;
const DIM: u8 = 1;
const ACCENT: u8 = 2;

type Cell = (char, u8);

/// Text from the app that is safe to show: unsafe characters become '?', a tab a space.
fn clean(s: &str) -> String { s.chars().map(|c| if c == '\t' { ' ' } else if unsafe_char(c) { '?' } else { c }).collect() }
fn width(s: &str) -> usize { s.chars().count() }

// ---- the grid and what reaches the terminal ----

/// The bytes that turn `prev` into `cur` (w*h cells): only changed rows, each as a cursor move and coloured runs. None = nothing shown yet: clear first.
fn render(prev: Option<&[Cell]>, cur: &[Cell], w: usize) -> Vec<u8> {
    let mut o = String::new();
    if prev.is_none() { o += "\x1b[2J"; }
    for (y, row) in cur.chunks(w).enumerate() {
        if prev.is_some_and(|p| p[y * w..(y + 1) * w] == *row) { continue; }
        o += &format!("\x1b[{};1H", y + 1);
        let mut role = u8::MAX;
        for &(ch, r) in row { if r != role { o += SGR[r as usize]; role = r; } o.push(ch); }
        o += "\x1b[0m";
    }
    o.into_bytes()
}

enum Ev { K(Key), Esc }

/// Keys for the loop and the modals. The Lua hook may move bytes into `pending` while an app runs (to see Ctrl-C), so they are read first.
struct Input { dec: Decoder, pending: Vec<u8> }
impl Input {
    /// Waits up to `ms` for keys. None = end of input. A bare Esc is an ESC that nothing followed within 50 ms.
    fn events(&mut self, ms: i32) -> Option<Vec<Ev>> {
        if !self.pending.is_empty() {
            let b = std::mem::take(&mut self.pending);
            let k = self.dec.decode(&b);
            if !k.is_empty() { return Some(k.into_iter().map(Ev::K).collect()); }
        }
        let was = self.dec.waiting_esc();
        let mut keys = term::read_keys(&mut self.dec, if was { 50 } else { ms })?;
        if keys.is_empty() && self.dec.waiting_esc() {
            if was { self.dec.lone_esc(); return Some(vec![Ev::Esc]); }
            keys = term::read_keys(&mut self.dec, 50)?;
            if keys.is_empty() && self.dec.lone_esc() { return Some(vec![Ev::Esc]); }
        }
        Some(keys.into_iter().map(Ev::K).collect())
    }
}

fn key_name(k: &Key) -> Option<String> {
    Some(match k {
        Key::Char(c) => c.to_string(), Key::Enter => "enter".into(), Key::Tab => "tab".into(), Key::Backspace => "backspace".into(), Key::Delete => "delete".into(),
        Key::Left => "left".into(), Key::Right => "right".into(), Key::Up => "up".into(), Key::Down => "down".into(), Key::Home => "home".into(), Key::End => "end".into(),
        Key::PgUp => "pgup".into(), Key::PgDn => "pgdn".into(), Key::Ctrl(b) if b.is_ascii_lowercase() => format!("ctrl-{}", *b as char), Key::Ctrl(_) => return None,
    })
}

struct Shared {
    w: usize, h: usize, cells: Vec<Cell>, shown: Option<Vec<Cell>>, cursor: Option<(usize, usize)>, cursor_on: bool,
    tty: bool, offline: bool, input: Input, eof: bool, interrupted: bool, quit: bool, every: Option<Duration>,
    store: BTreeMap<String, String>, store_path: Option<PathBuf>, allowed: Vec<Url>,
}

impl Shared {
    fn resize(&mut self, w: usize, h: usize) { self.w = w; self.h = h; self.cells = vec![(' ', 0); w * h]; self.shown = None; }
    fn blank(&mut self) { self.cells.fill((' ', 0)); }
    /// Writes `s` at column x, row y (1-based), cut at both edges.
    fn put(&mut self, x: i64, y: i64, s: &str, role: u8) {
        if y < 1 || y > self.h as i64 { return; }
        for (i, ch) in s.chars().enumerate() {
            let cx = x.saturating_add(i as i64);
            if cx > self.w as i64 { break; }
            if cx >= 1 { self.cells[(y as usize - 1) * self.w + cx as usize - 1] = (ch, role); }
        }
    }
    fn fill(&mut self, x: i64, y: i64, w: i64, h: i64, role: u8) { for r in 0..h.clamp(0, 500) { self.put(x, y + r, &" ".repeat(w.clamp(0, 500) as usize), role); } }
    /// Draws the grid on the terminal (changed rows only), with the cursor where a modal asked for it.
    fn flush(&mut self) {
        if !self.tty { return; }
        let mut o = render(self.shown.as_deref(), &self.cells, self.w);
        match self.cursor {
            Some((x, y)) => { o.extend(format!("\x1b[{y};{x}H\x1b[?25h").bytes()); self.cursor_on = true; }
            None if self.cursor_on => { o.extend(b"\x1b[?25l"); self.cursor_on = false; }
            None => {}
        }
        term::put(&o);
        self.shown = Some(self.cells.clone());
    }
    /// The next key for a modal; None (and `eof`) when the input ends.
    fn next_ev(&mut self) -> Option<Ev> {
        loop {
            let Some(evs) = self.input.events(250) else { self.eof = true; return None };
            if let Some(e) = evs.into_iter().next() { return Some(e); }   // shortcut: other keys typed in the same read are dropped
        }
    }
    fn text_of(&self) -> String {
        self.cells.chunks(self.w).map(|r| r.iter().map(|c| c.0).collect::<String>().trim_end().to_string()).collect::<Vec<_>>().join("\n").trim_end().to_string()
    }
    /// Runs a modal: `base` is the last frame, `paint` draws over it, `key` answers a key (Some = done). The frame is restored after.
    fn modal<T>(&mut self, mut paint: impl FnMut(&mut Shared), mut key: impl FnMut(&mut Shared, Ev) -> Option<T>) -> Option<T> {
        let base = self.cells.clone();
        let r = loop {
            self.cells = base.clone(); self.cursor = None; paint(self); self.flush();
            let Some(e) = self.next_ev() else { break None };
            if let Some(r) = key(self, e) { break Some(r); }
        };
        self.cells = base; self.cursor = None; self.flush();
        r
    }
}

// ---- time and process limits ----

struct Ctl { dl: StdCell<Instant>, tty: bool }
impl Ctl {
    fn start(&self) { self.dl.set(Instant::now() + CALL_LIMIT); if self.tty { unsafe { libc::alarm(WATCHDOG_SECS); } } }
    fn stop(&self) { if self.tty { unsafe { libc::alarm(0); } } }
    /// For SDK calls that wait (network, the user): no limit while they run, a fresh one after.
    fn blocking<T>(&self, f: impl FnOnce() -> T) -> T { self.stop(); let r = f(); self.start(); r }
}

/// Moves waiting input bytes into `pending`; true if one of them was Ctrl-C.
fn poll_input(pending: &mut Vec<u8>) -> bool {
    let mut p = libc::pollfd { fd: 0, events: libc::POLLIN, revents: 0 };
    if pending.len() >= 4096 || unsafe { libc::poll(&mut p, 1, 0) } <= 0 || p.revents & libc::POLLIN == 0 { return pending.contains(&3); }
    let mut b = [0u8; 256];
    let n = unsafe { libc::read(0, b.as_mut_ptr() as *mut _, b.len()) };
    if n > 0 { pending.extend_from_slice(&b[..n as usize]); }
    pending.contains(&3)
}

/// The POSIX TZ value for an offset in minutes east of UTC: +05:30 is `<+0530>-05:30`, which musl's localtime understands.
pub fn posix_tz(min: i32) -> String {
    let a = min.abs();
    format!("<{}{:02}{:02}>{}{:02}:{:02}", if min < 0 { '-' } else { '+' }, a / 60, a % 60, if min < 0 { '+' } else { '-' }, a / 60, a % 60)
}

// ---- values between Lua and JSON ----

fn txt(lua: &Lua, v: &Value) -> LResult<String> {
    match lua.coerce_string(v.clone())? {
        Some(s) if matches!(v, Value::String(_) | Value::Integer(_) | Value::Number(_)) => Ok(clean(&s.to_string_lossy())),
        _ => Err(LErr::runtime(format!("expected a string or number, got {}", v.type_name()))),
    }
}

fn role(c: Option<String>) -> LResult<u8> {
    match c { None => Ok(0), Some(c) => ROLES.iter().position(|r| *r == c).map(|i| i as u8).ok_or_else(|| LErr::runtime(format!("unknown color '{}'; colors: {}", clean(&c), ROLES.join(" ")))) }
}

fn to_lua(lua: &Lua, j: &Json) -> LResult<Value> {
    Ok(match j {
        Json::Null => Value::Nil, Json::Bool(b) => Value::Boolean(*b),
        Json::Num(n) => if n.fract() == 0.0 && n.abs() < 9e15 { Value::Integer(*n as i64) } else { Value::Number(*n) },
        Json::Str(s) => Value::String(lua.create_string(s)?),
        Json::Arr(a) => { let t = lua.create_table()?; for (i, v) in a.iter().enumerate() { t.raw_set(i + 1, to_lua(lua, v)?)?; } Value::Table(t) }   // shortcut: arrays with nulls get holes
        Json::Obj(o) => { let t = lua.create_table()?; for (k, v) in o { t.raw_set(k.as_str(), to_lua(lua, v)?)?; } Value::Table(t) }
    })
}

/// A Lua value as JSON text. A table is an array if its keys are exactly 1..#t, else an object; an empty table is `[]`.
fn enc(v: &Value, depth: usize, out: &mut String) -> Result<(), String> {
    if out.len() > ENC_MAX { return Err("too big to encode (over 4 MB)".into()); }
    match v {
        Value::Nil => out.push_str("null"), Value::Boolean(b) => out.push_str(&b.to_string()), Value::Integer(i) => out.push_str(&i.to_string()),
        Value::Number(n) if n.is_finite() => out.push_str(&json::dump(&Json::Num(*n))),
        Value::Number(_) => return Err("cannot encode NaN or infinity".into()),
        Value::String(s) => out.push_str(&json::quote(&s.to_string_lossy())),
        Value::Table(t) => {
            if depth >= ENC_DEPTH { return Err(format!("too deep to encode (more than {ENC_DEPTH} levels)")); }
            let n = t.raw_len();
            let mut pairs = vec![];
            for p in t.pairs::<Value, Value>() { pairs.push(p.map_err(|e| e.to_string())?); }
            if pairs.iter().all(|(k, _)| matches!(k, Value::Integer(i) if *i >= 1 && *i as usize <= n)) && pairs.len() == n {
                out.push('[');
                for i in 1..=n { if i > 1 { out.push(','); } enc(&t.raw_get::<Value>(i).map_err(|e| e.to_string())?, depth + 1, out)?; }
                out.push(']');
            } else {
                let mut kv = vec![];
                for (k, val) in pairs {
                    let k = match k { Value::String(s) => s.to_string_lossy().to_string(), Value::Integer(i) => i.to_string(), Value::Number(f) => f.to_string(), o => return Err(format!("cannot use a {} as a key", o.type_name())) };
                    kv.push((k, val));
                }
                kv.sort_by(|a, b| a.0.cmp(&b.0));
                out.push('{');
                for (i, (k, val)) in kv.iter().enumerate() { if i > 0 { out.push(','); } out.push_str(&json::quote(k)); out.push(':'); enc(val, depth + 1, out)?; }
                out.push('}');
            }
        }
        o => return Err(format!("cannot encode a {}", o.type_name())),
    }
    Ok(())
}

fn encode_url(s: &str) -> String {
    s.bytes().fold(String::new(), |mut o, b| { if b.is_ascii_alphanumeric() || b"-._~".contains(&b) { o.push(b as char) } else { o += &format!("%{b:02X}") } o })
}

// ---- the SDK: http ----

fn origin(u: &Url) -> String { format!("{}://{}", if u.https { "https" } else { "http" }, u.host_header()) }

/// One request to an origin the manifest allows, with the redirects (GET only, at most 3) that stay on allowed origins.
/// Returns (body, status). Err = a message for the app (nil, message).
fn fetch(st: &Rc<RefCell<Shared>>, ctl: &Ctl, method: &str, url: &str, body: Option<Vec<u8>>, mut headers: Vec<(String, String)>) -> Result<(Vec<u8>, u16), String> {
    let allowed = st.borrow().allowed.clone();
    if st.borrow().offline { return Err("offline (test run)".into()); }
    let mut url = Url::parse(url).map_err(|e| format!("bad URL: {e}"))?;
    let ok = |u: &Url| allowed.iter().any(|a| a.same_origin(u));
    if !ok(&url) { return Err(format!("{} is not allowed (the app did not declare it)", origin(&url))); }
    if !headers.iter().any(|(n, _)| n.eq_ignore_ascii_case("user-agent")) { headers.insert(0, ("User-Agent".into(), "thir-app/0.1".into())); }
    let deadline = Instant::now() + HTTP_LIMIT;
    ctl.blocking(|| {
        {   // the last frame stays; the bottom row says what is going on
            let mut s = st.borrow_mut();
            if s.tty {
                let (h, line) = (s.h as i64, format!("{:<w$}", format!("loading {}…", clean(&url.host)), w = s.w));
                let keep = s.cells.clone(); s.put(1, h, &line, DIM); s.flush(); s.cells = keep;
            }
        }
        for hop in 0.. {
            let (head, framing, reader) = exchange(&url, method, &headers, body.as_deref(), Some(deadline), None, None).map_err(|e| e.msg)?;
            let loc = head.get("location").map(str::to_string);
            if let (true, 301 | 302 | 303 | 307 | 308, Some(loc)) = (method == "GET", head.status, loc) {
                if hop >= MAX_REDIRECTS { return Err("too many redirects".into()); }
                let next = url.join(&loc).map_err(|e| format!("bad redirect: {e}"))?;
                if !ok(&next) { return Err(format!("the server redirected to {}, which is not allowed", origin(&next))); }
                url = next; continue;
            }
            let mut b = vec![];
            Body::new(reader, framing).take(MAX_BODY + 1).read_to_end(&mut b).map_err(|e| crate::conn::body_err(&e, framing, url.https).msg)?;
            if b.len() as u64 > MAX_BODY { return Err("the answer is bigger than 2 MiB".into()); }
            return Ok((b, head.status));
        }
        unreachable!()
    })
}

fn header_list(t: Option<Table>) -> Result<Vec<(String, String)>, String> {
    let mut h = vec![];
    let Some(t) = t else { return Ok(h) };
    for p in t.pairs::<String, Value>() {
        let (n, v) = p.map_err(|_| "headers must be a table of strings".to_string())?;
        let v = match v { Value::String(s) => s.to_string_lossy().to_string(), Value::Integer(i) => i.to_string(), _ => return Err(format!("header {n}: the value must be a string")) };
        if !http::token(&n) || !http::value_ok(&v) { return Err(format!("bad header {}", clean(&n))); }
        if ["host", "content-length", "connection", "transfer-encoding"].contains(&n.to_ascii_lowercase().as_str()) { return Err(format!("the header {n} is set by thir")); }
        h.push((n, v));
    }
    Ok(h)
}

/// (body, status) or (nil, message), the shape of http.get and http.post.
fn reply(lua: &Lua, r: Result<(Vec<u8>, u16), String>) -> LResult<MultiValue> {
    match r {
        Ok((b, s)) => (Value::String(lua.create_string(&b)?), Value::Integer(s as i64)).into_lua_multi(lua),
        Err(m) => (Value::Nil, clean(&m)).into_lua_multi(lua),
    }
}

// ---- the sandbox ----

pub struct Setup { pub name: String, pub args: Vec<String>, pub allowed: Vec<Url>, pub store_dir: Option<PathBuf>, pub store: Vec<(String, String)>, pub tty: bool, pub offline: bool, pub size: (usize, usize) }

pub struct Sandbox { lua: Lua, st: Rc<RefCell<Shared>>, ctl: Rc<Ctl> }

macro_rules! sdk {
    ($lua:expr, $t:expr, $st:expr, $name:literal, |$s:ident, $l:ident, $a:ident : $ty:ty| $body:expr) => {{
        let $s = $st.clone();
        $t.set($name, $lua.create_function(move |$l: &Lua, $a: $ty| -> LResult<_> { $body })?)?;
    }};
}

impl Sandbox {
    pub fn new(o: Setup) -> LResult<Sandbox> {
        let lua = Lua::new_with(StdLib::TABLE | StdLib::STRING | StdLib::MATH | StdLib::UTF8 | StdLib::OS, LuaOptions::default())?;
        lua.set_memory_limit(MEM_LIMIT)?;
        let mut store = BTreeMap::new();
        for (k, v) in o.store { store.insert(k, v); }
        let st = Rc::new(RefCell::new(Shared {
            w: 0, h: 0, cells: vec![], shown: None, cursor: None, cursor_on: false, tty: o.tty, offline: o.offline, input: Input { dec: Decoder::new(), pending: vec![] }, eof: false, interrupted: false, quit: false,
            every: None, store, store_path: o.store_dir.map(|d| d.join("store.json")), allowed: o.allowed,
        }));
        st.borrow_mut().resize(o.size.0, o.size.1);
        let ctl = Rc::new(Ctl { dl: StdCell::new(Instant::now() + CALL_LIMIT), tty: o.tty });
        let g = lua.globals();
        for n in ["dofile", "loadfile", "load", "require", "warn"] { g.set(n, Value::Nil)?; }
        g.get::<Table>("string")?.set("dump", Value::Nil)?;
        let os: Table = g.get("os")?;
        let safe = lua.create_table()?;
        for n in ["time", "date", "clock", "difftime"] { safe.set(n, os.get::<Value>(n)?)?; }
        g.set("os", safe)?;
        g.set("print", lua.create_function(|_, _: MultiValue| -> LResult<()> { Err(LErr::runtime("print does not work in apps; use ui.text in draw()")) })?)?;
        Self::sdk(&lua, &st, &ctl, &o.name, &o.args)?;
        let (h_st, h_ctl) = (st.clone(), ctl.clone());
        lua.set_hook(HookTriggers::new().every_nth_instruction(10_000), move |_, _| {
            if Instant::now() > h_ctl.dl.get() { return Err(LErr::runtime("the app ran for more than 2 s without stopping (an endless loop?)")); }
            let mut s = h_st.borrow_mut();
            if s.tty && poll_input(&mut s.input.pending) { s.interrupted = true; return Err(LErr::runtime("stopped with Ctrl-C")); }
            Ok(VmState::Continue)
        })?;
        drop(g);
        Ok(Sandbox { lua, st, ctl })
    }

    fn sdk(lua: &Lua, st: &Rc<RefCell<Shared>>, ctl: &Rc<Ctl>, name: &str, args: &[String]) -> LResult<()> {
        let (ui, http, js, store, app) = (lua.create_table()?, lua.create_table()?, lua.create_table()?, lua.create_table()?, lua.create_table()?);
        sdk!(lua, ui, st, "text", |s, l, a: (f64, f64, Value, Option<String>)| { let t = txt(l, &a.2)?; s.borrow_mut().put(a.0 as i64, a.1 as i64, &t, role(a.3)?); Ok(()) });
        sdk!(lua, ui, st, "center", |s, l, a: (f64, Value, Option<String>)| {
            let t = txt(l, &a.1)?; let mut s = s.borrow_mut(); let x = (s.w as i64 - width(&t) as i64) / 2 + 1;
            s.put(x, a.0 as i64, &t, role(a.2)?); Ok(())
        });
        sdk!(lua, ui, st, "pad", |_s, l, a: (Value, f64)| { let t = txt(l, &a.0)?; let w = a.1.clamp(0.0, 1000.0) as usize; Ok(format!("{:<w$}", t.chars().take(w).collect::<String>())) });
        sdk!(lua, ui, st, "wrap", |_s, l, a: (Value, f64)| {
            let (t, w) = (txt_keep_nl(l, &a.0)?, a.1.clamp(1.0, 1000.0) as usize);
            let out = l.create_table()?;
            for line in wrap(&t, w) { out.push(line)?; }
            Ok(out)
        });
        sdk!(lua, ui, st, "width", |_s, l, a: Value| Ok(width(&txt(l, &a)?) as i64));
        sdk!(lua, ui, st, "fill", |s, _l, a: (f64, f64, f64, f64, Option<String>)| { s.borrow_mut().fill(a.0 as i64, a.1 as i64, a.2 as i64, a.3 as i64, role(a.4)?); Ok(()) });
        sdk!(lua, ui, st, "box", |s, l, a: (f64, f64, f64, f64, Option<Value>)| {
            let (x, y, w, h) = (a.0 as i64, a.1 as i64, a.2.clamp(2.0, 500.0) as i64, a.3.clamp(2.0, 500.0) as i64);
            let title = match &a.4 { Some(v) if !v.is_nil() => Some(txt(l, v)?), _ => None };
            let mut s = s.borrow_mut();
            s.put(x, y, &format!("┌{}┐", "─".repeat(w as usize - 2)), 0);
            for r in 1..h - 1 { s.put(x, y + r, "│", 0); s.put(x + w - 1, y + r, "│", 0); }
            s.put(x, y + h - 1, &format!("└{}┘", "─".repeat(w as usize - 2)), 0);
            if let Some(t) = title { let t: String = format!(" {t} ").chars().take((w - 4).max(0) as usize).collect(); s.put(x + 2, y, &t, ACCENT); }
            Ok(())
        });
        sdk!(lua, ui, st, "title", |s, l, a: (Value, Option<Value>)| {
            let left = txt(l, &a.0)?; let right = match &a.1 { Some(v) if !v.is_nil() => txt(l, v)?, _ => String::new() };
            let mut s = s.borrow_mut(); let w = s.w as i64;
            s.put(1, 1, &" ".repeat(w as usize), 0); s.put(2, 1, &left, ACCENT);
            s.put((w - width(&right) as i64).max(width(&left) as i64 + 4), 1, &right, DIM); Ok(())
        });
        sdk!(lua, ui, st, "footer", |s, l, a: Value| { let t = txt(l, &a)?; let mut s = s.borrow_mut(); let h = s.h as i64; s.put(2, h, &t, DIM); Ok(()) });
        sdk!(lua, ui, st, "list", |s, l, a: (f64, f64, f64, f64, Table, Option<f64>)| {
            let items: Vec<String> = a.4.sequence_values::<Value>().take(10_000).map(|v| v.and_then(|v| txt(l, &v))).collect::<LResult<_>>()?;
            let (x, y, w, h) = (a.0 as i64, a.1 as i64, a.2.clamp(0.0, 500.0) as usize, a.3.clamp(0.0, 500.0) as usize);
            let sel = (a.5.unwrap_or(0.0) as i64 - 1).clamp(0, items.len().max(1) as i64 - 1) as usize;
            let top = sel.saturating_sub(h / 2).min(items.len().saturating_sub(h));
            let mut s = s.borrow_mut();
            for (r, it) in items.iter().skip(top).take(h).enumerate() {
                let line: String = format!("{:<w$}", it.chars().take(w).collect::<String>());
                s.put(x, y + r as i64, &line, if top + r == sel && a.5.is_some() { SEL } else { 0 });
            }
            Ok(())
        });
        sdk!(lua, ui, st, "bar", |s, _l, a: (f64, f64, f64, f64, Option<String>)| {
            let (w, f) = (a.2.clamp(0.0, 500.0) as usize, if a.3.is_nan() { 0.0 } else { a.3.clamp(0.0, 1.0) });
            let full = (f * w as f64).round() as usize; let c = role(a.4)?;
            let mut s = s.borrow_mut(); s.put(a.0 as i64, a.1 as i64, &"█".repeat(full), c); s.put(a.0 as i64 + full as i64, a.1 as i64, &"░".repeat(w - full), DIM); Ok(())
        });
        let c = ctl.clone();
        sdk!(lua, ui, st, "input", |s, l, a: (Value, Option<Value>)| {
            let label = txt(l, &a.0)?; let init: String = match &a.1 { Some(v) if !v.is_nil() => txt(l, v)?, _ => String::new() };
            if !s.borrow().tty { return Ok(Some("1".to_string())); }
            let buf = RefCell::new(init.chars().take(256).collect::<String>());
            Ok(c.blocking(|| s.borrow_mut().modal(
                |s| {
                    let (w, h) = (s.w, s.h as i64); let b = buf.borrow();
                    let room = w.saturating_sub(width(&label) + 2).max(1);
                    let shown: String = { let v: Vec<char> = b.chars().collect(); v[v.len().saturating_sub(room)..].iter().collect() };
                    s.put(1, h, &" ".repeat(w), 0); s.put(1, h, &label, ACCENT); s.put(width(&label) as i64 + 1, h, &shown, 0);
                    s.cursor = Some(((width(&label) + width(&shown) + 1).min(w), h as usize));
                },
                |_, e| match e {
                    Ev::Esc | Ev::K(Key::Ctrl(b'c')) => Some(None),
                    Ev::K(Key::Enter) => Some(Some(buf.borrow().clone())),
                    Ev::K(Key::Backspace) => { buf.borrow_mut().pop(); None }
                    Ev::K(Key::Ctrl(b'u')) => { buf.borrow_mut().clear(); None }
                    Ev::K(Key::Char(ch)) if !unsafe_char(ch) && buf.borrow().chars().count() < 256 => { buf.borrow_mut().push(ch); None }
                    _ => None,
                },
            ).flatten()))
        });
        let c = ctl.clone();
        sdk!(lua, ui, st, "pick", |s, l, a: (Value, Table)| {
            let title = txt(l, &a.0)?;
            let items: Vec<String> = a.1.sequence_values::<Value>().take(200).map(|v| v.and_then(|v| txt(l, &v))).collect::<LResult<_>>()?;
            if items.is_empty() { return Ok(None); }
            if !s.borrow().tty { return Ok(Some(1i64)); }
            let sel = StdCell::new(0usize);
            Ok(c.blocking(|| s.borrow_mut().modal(
                |s| {
                    let bw = (items.iter().map(|i| width(i)).max().unwrap_or(0).max(width(&title)) + 6).min(s.w.saturating_sub(2)).max(4);
                    let bh = (items.len() + 2).min(s.h.saturating_sub(2)).max(3);
                    let (x, y) = ((s.w.saturating_sub(bw)) as i64 / 2 + 1, (s.h.saturating_sub(bh)) as i64 / 2 + 1);
                    let inner = bh - 2;
                    let top = sel.get().saturating_sub(inner / 2).min(items.len().saturating_sub(inner));
                    s.fill(x, y, bw as i64, bh as i64, 0);
                    s.put(x, y, &format!("┌{}┐", "─".repeat(bw - 2)), 0); s.put(x, y + bh as i64 - 1, &format!("└{}┘", "─".repeat(bw - 2)), 0);
                    for r in 1..bh as i64 - 1 { s.put(x, y + r, "│", 0); s.put(x + bw as i64 - 1, y + r, "│", 0); }
                    s.put(x + 2, y, &format!(" {title} ").chars().take(bw.saturating_sub(4)).collect::<String>(), ACCENT);
                    for (r, it) in items.iter().skip(top).take(inner).enumerate() {
                        s.put(x + 1, y + 1 + r as i64, &format!(" {:<w$}", it.chars().take(bw - 4).collect::<String>(), w = bw - 3), if top + r == sel.get() { SEL } else { 0 });
                    }
                },
                |_, e| match e {
                    Ev::Esc | Ev::K(Key::Ctrl(b'c')) => Some(None),
                    Ev::K(Key::Enter) => Some(Some(sel.get() as i64 + 1)),
                    Ev::K(Key::Up) => { sel.set(sel.get().saturating_sub(1)); None }
                    Ev::K(Key::Down) => { sel.set((sel.get() + 1).min(items.len() - 1)); None }
                    Ev::K(Key::Home) => { sel.set(0); None }
                    Ev::K(Key::End) => { sel.set(items.len() - 1); None }
                    _ => None,
                },
            ).flatten()))
        });
        let c = ctl.clone();
        sdk!(lua, ui, st, "confirm", |s, l, a: Value| {
            let q = txt(l, &a)?;
            if !s.borrow().tty { return Ok(false); }
            Ok(c.blocking(|| s.borrow_mut().modal(
                |s| { let h = s.h as i64; let w = s.w; s.put(1, h, &" ".repeat(w), 0); s.put(1, h, &format!("{q} (y/n)"), ACCENT); },
                |_, e| match e { Ev::K(Key::Char('y' | 'Y')) => Some(true), Ev::K(Key::Char('n' | 'N')) | Ev::K(Key::Enter) | Ev::Esc | Ev::K(Key::Ctrl(b'c')) => Some(false), _ => None },
            ).unwrap_or(false)))
        });
        sdk!(lua, ui, st, "size", |s, _l, _a: ()| { let s = s.borrow(); Ok((s.w as i64, s.h as i64)) });

        for (n, m) in [("get", "GET"), ("json", "GET")] {
            let (s, c) = (st.clone(), ctl.clone());
            http.set(n, lua.create_function(move |lua, (url, h): (String, Option<Table>)| {
                let r = header_list(h).and_then(|h| fetch(&s, &c, m, &url, None, h));
                if n == "get" { return reply(lua, r); }
                match r {
                    Ok((_, code)) if code >= 400 => (Value::Nil, format!("the server answered {code}")).into_lua_multi(lua),
                    Ok((b, _)) => match json::parse(&String::from_utf8_lossy(&b)) { Ok(j) => to_lua(lua, &j)?.into_lua_multi(lua), Err(e) => (Value::Nil, format!("the answer is not valid JSON ({e})")).into_lua_multi(lua) },
                    Err(m) => (Value::Nil, clean(&m)).into_lua_multi(lua),
                }
            })?)?;
        }
        let (s, c) = (st.clone(), ctl.clone());
        http.set("post", lua.create_function(move |lua, (url, body, h): (String, Value, Option<Table>)| {
            let mut hdr = match header_list(h) { Ok(h) => h, Err(m) => return reply(lua, Err(m)) };
            let (bytes, ctype) = match &body {
                Value::String(b) => (b.as_bytes().to_vec(), "application/x-www-form-urlencoded"),
                Value::Table(_) => { let mut o = String::new(); if let Err(m) = enc(&body, 0, &mut o) { return reply(lua, Err(m)); } (o.into_bytes(), "application/json") }
                _ => return reply(lua, Err("http.post: the body must be a string or a table".into())),
            };
            if !hdr.iter().any(|(n, _)| n.eq_ignore_ascii_case("content-type")) { hdr.push(("Content-Type".into(), ctype.into())); }
            reply(lua, fetch(&s, &c, "POST", &url, Some(bytes), hdr))
        })?)?;
        http.set("encode", lua.create_function(|l, v: Value| Ok(encode_url(&txt(l, &v)?)))?)?;

        js.set("decode", lua.create_function(|lua, s: mlua::LuaString| if s.as_bytes().len() as u64 > MAX_BODY { (Value::Nil, "too big to decode (over 2 MB)").into_lua_multi(lua) } else { match json::parse(&s.to_string_lossy()) {
            Ok(j) => (to_lua(lua, &j)?, Value::Nil).into_lua_multi(lua),
            Err(e) => (Value::Nil, format!("not valid JSON ({e})")).into_lua_multi(lua),
        } })?)?;
        js.set("encode", lua.create_function(|_, v: Value| { let mut o = String::new(); enc(&v, 0, &mut o).map_err(LErr::runtime)?; Ok(o) })?)?;

        sdk!(lua, store, st, "get", |s, lua, a: (String, Value)| match s.borrow().store.get(&a.0) {
            Some(j) => to_lua(lua, &json::parse(j).map_err(LErr::runtime)?),
            None => Ok(a.1),
        });
        sdk!(lua, store, st, "set", |s, _l, a: (String, Value)| {
            if a.0.is_empty() || a.0.len() > 256 { return Err(LErr::runtime("store.set: the key must be 1 to 256 bytes long")); }
            let mut s = s.borrow_mut();
            let old = if a.1.is_nil() { s.store.remove(&a.0) } else {
                let mut o = String::new(); enc(&a.1, 0, &mut o).map_err(|m| LErr::runtime(format!("store.set: {m}")))?;
                s.store.insert(a.0.clone(), o)
            };
            let text = format!("{{{}}}", s.store.iter().map(|(k, v)| format!("{}:{v}", json::quote(k))).collect::<Vec<_>>().join(","));
            if text.len() > MAX_STORE {
                match old { Some(o) => { s.store.insert(a.0, o); } None => { s.store.remove(&a.0); } }
                return Err(LErr::runtime("store.set: the store is full (1 MB in total)"));
            }
            if let Some(p) = s.store_path.clone() { crate::agent::write_private(&p, &text).map_err(|e| LErr::runtime(format!("store.set: cannot save: {e}")))?; }
            Ok(())
        });

        sdk!(lua, app, st, "every", |s, _l, a: f64| {
            if !(0.1..=86400.0).contains(&a) { return Err(LErr::runtime("app.every: use 0.1 seconds or more")); }
            s.borrow_mut().every = Some(Duration::from_secs_f64(a)); Ok(())
        });
        sdk!(lua, app, st, "quit", |s, _l, _a: ()| { s.borrow_mut().quit = true; Ok(()) });
        let t = lua.create_table()?;
        for a in args { t.push(a.as_str())?; }
        app.set("args", t)?; app.set("name", name)?;
        let g = lua.globals();
        for (n, t) in [("ui", ui), ("http", http), ("json", js), ("store", store), ("app", app)] { g.set(n, t)?; }
        Ok(())
    }

    /// Compiles and runs the file's top level.
    pub fn load(&self, src: &str) -> LResult<()> {
        let f = self.lua.load(src).set_name("=main.lua").set_mode(ChunkMode::Text).into_function()?;
        self.ctl.start(); let r = f.call::<()>(()); self.ctl.stop(); r
    }
    fn call(&self, name: &str, args: impl IntoLuaMulti) -> LResult<()> {
        let Some(f) = self.lua.globals().get::<Option<Function>>(name)? else { return Ok(()) };
        self.ctl.start(); let r = f.call::<()>(args); self.ctl.stop(); r
    }
    pub fn has_draw(&self) -> bool { matches!(self.lua.globals().get::<Value>("draw"), Ok(Value::Function(_))) }
    /// Clears the grid, calls draw(w, h) and shows the result.
    pub fn draw(&self, w: usize, h: usize) -> LResult<()> {
        {
            let mut s = self.st.borrow_mut();
            if (s.w, s.h) != (w, h) { s.resize(w, h); }
            s.blank();
            if w < MIN_SIZE.0 || h < MIN_SIZE.1 { s.put(1, 1, "window too small", 0); s.flush(); return Ok(()); }
        }
        self.call("draw", (w as i64, h as i64))?;
        self.st.borrow_mut().flush();
        Ok(())
    }
    pub fn key(&self, k: &str) -> LResult<()> { self.call("key", k) }
    pub fn tick(&self) -> LResult<()> { self.call("tick", ()) }
    pub fn every(&self) -> Option<Duration> { self.st.borrow().every }
    pub fn quit(&self) -> bool { self.st.borrow().quit }
    pub fn interrupted(&self) -> bool { self.st.borrow().interrupted }
    pub fn eof(&self) -> bool { self.st.borrow().eof }
    pub fn grid_text(&self) -> String { self.st.borrow().text_of() }
    fn events(&self, ms: i32) -> Option<Vec<Ev>> { self.st.borrow_mut().input.events(ms) }
}

/// Like `txt` but keeps newlines, for ui.wrap.
fn txt_keep_nl(lua: &Lua, v: &Value) -> LResult<String> {
    match v {
        Value::String(s) => Ok(s.to_string_lossy().to_string()),
        o => txt(lua, o),
    }
}

/// Lines of at most `w` characters: breaks at spaces, newlines start new lines, a word longer than a line is cut.
pub fn wrap(s: &str, w: usize) -> Vec<String> {
    let mut out = vec![];
    for para in s.split('\n') {
        let mut line = String::new();
        for word in clean(para).split(' ').filter(|x| !x.is_empty()) {
            let mut word: Vec<char> = word.chars().collect();
            while word.len() > w { if !line.is_empty() { out.push(std::mem::take(&mut line)); } out.push(word.drain(..w).collect()); }
            let word: String = word.into_iter().collect();
            if line.is_empty() { line = word } else if width(&line) + 1 + width(&word) <= w { line.push(' '); line += &word } else { out.push(std::mem::replace(&mut line, word)); }
        }
        out.push(line);
    }
    out
}

// ---- errors ----

/// The message of an mlua error with its traceback, without the "runtime error:" wrappers.
fn flat(e: &LErr) -> String {
    match e {
        LErr::CallbackError { traceback, cause } => format!("{}\n{}", flat(cause), traceback),
        LErr::RuntimeError(s) | LErr::SyntaxError { message: s, .. } => s.clone(),
        LErr::MemoryError(s) => format!("the app used more than 32 MiB ({s})"),
        o => o.to_string(),
    }
}

/// What the user and the AI see: `ctx` + the message, the source line of the first `main.lua:N:`, then the traceback; at most 1500 characters.
pub fn error_text(e: &LErr, src: &str, ctx: &str) -> String {
    let m = crate::agent::sanitize(&flat(e));
    let (first, rest) = m.split_once('\n').unwrap_or((&m, ""));
    let line = m.find("main.lua:").and_then(|i| { let d: String = m[i + 9..].chars().take_while(char::is_ascii_digit).collect(); d.parse::<usize>().ok() })
        .and_then(|n| src.lines().nth(n.wrapping_sub(1)).map(|l| format!("\n  line {n}: {}", l.trim())));
    let mut t = format!("{ctx}{first}{}{}", line.unwrap_or_default(), if rest.is_empty() { String::new() } else { format!("\n{rest}") });
    if t.chars().count() > 1500 { t = t.chars().take(1500).collect(); }
    t
}

// ---- checks ----

/// Names an app may use: the globals the SDK defines, as "ui.text" and so on.
pub fn sdk_names() -> Vec<String> {
    [("ui", UI), ("http", HTTP), ("json", JSON), ("store", STORE), ("app", APP)].iter().flat_map(|(t, f)| f.iter().map(move |n| format!("{t}.{n}"))).collect()
}

/// Compile check and SDK-name lint. Err = a message in the format of a run error, for the AI to fix.
pub fn check(src: &str) -> Result<(), String> {
    let sb = Sandbox::new(Setup { name: "check".into(), args: vec![], allowed: vec![], store_dir: None, store: vec![], tty: false, offline: false, size: (80, 24) }).map_err(|e| e.to_string())?;
    sb.lua.load(src).set_name("=main.lua").set_mode(ChunkMode::Text).into_function().map_err(|e| error_text(&e, src, ""))?;
    lint(src)
}

/// Rough scanner: `ui.x`, `http.x`, `json.x`, `store.x` and `app.x` outside strings and comments must name SDK functions.
/// A name the app declares itself (`local app`, `app = ...`) is skipped.
fn lint(src: &str) -> Result<(), String> {
    #[derive(PartialEq)]
    enum T { Id(String), P(String) }
    let (b, mut i, mut line, mut toks): (Vec<char>, usize, usize, Vec<(T, usize)>) = (src.chars().collect(), 0, 1, vec![]);
    let long = |b: &[char], i: usize| -> Option<usize> {   // `[==[` at i: the level
        if b.get(i) != Some(&'[') { return None; }
        let n = b[i + 1..].iter().take_while(|c| **c == '=').count();
        (b.get(i + 1 + n) == Some(&'[')).then_some(n)
    };
    let skip_long = |b: &[char], mut i: usize, n: usize, line: &mut usize| -> usize {
        let close: Vec<char> = std::iter::once(']').chain(std::iter::repeat('=').take(n)).chain(std::iter::once(']')).collect();
        i += n + 2;
        while i < b.len() && b[i..].get(..close.len()) != Some(&close[..]) { if b[i] == '\n' { *line += 1; } i += 1; }
        (i + close.len()).min(b.len())
    };
    while i < b.len() {
        let c = b[i];
        if c == '\n' { line += 1; i += 1; }
        else if c.is_whitespace() { i += 1; }
        else if c == '-' && b.get(i + 1) == Some(&'-') {
            if let Some(n) = long(&b, i + 2) { i = skip_long(&b, i + 2, n, &mut line); } else { while i < b.len() && b[i] != '\n' { i += 1; } }
        } else if c == '"' || c == '\'' {
            i += 1;
            while i < b.len() && b[i] != c && b[i] != '\n' { if b[i] == '\\' { i += 1; } if b.get(i) == Some(&'\n') { line += 1; } i += 1; }
            i += 1;
        } else if let Some(n) = long(&b, i) { i = skip_long(&b, i, n, &mut line); }
        else if c.is_alphabetic() || c == '_' {
            let s = i; while i < b.len() && (b[i].is_alphanumeric() || b[i] == '_') { i += 1; }
            toks.push((T::Id(b[s..i].iter().collect()), line));
        } else if c.is_ascii_digit() { while i < b.len() && (b[i].is_alphanumeric() || b[i] == '.') { i += 1; } toks.push((T::P("0".into()), line)); }
        else {
            let two: String = b[i..].iter().take(2).collect();
            let p = if ["==", "~=", "<=", ">=", "..", "::"].contains(&two.as_str()) { two } else { c.to_string() };
            i += p.len(); toks.push((T::P(p), line));
        }
    }
    let tables = ["ui", "http", "json", "store", "app"];
    let mut own: Vec<&str> = vec![];
    for (k, (t, _)) in toks.iter().enumerate() {
        if let T::Id(n) = t {
            let declared = (k > 0 && toks[k - 1].0 == T::Id("local".into())) || (toks.get(k + 1).is_some_and(|x| x.0 == T::P("=".into())) && !(k > 0 && toks[k - 1].0 == T::P(".".into())));
            if tables.contains(&n.as_str()) && declared { own.push(n.as_str()); }
        }
    }
    for k in 0..toks.len().saturating_sub(2) {
        let (T::Id(t), T::P(dot), T::Id(f)) = (&toks[k].0, &toks[k + 1].0, &toks[k + 2].0) else { continue };
        if dot != "." || !tables.contains(&t.as_str()) || own.contains(&t.as_str()) || (k > 0 && matches!(&toks[k - 1].0, T::P(p) if p == "." || p == ":")) { continue; }
        let list = match t.as_str() { "ui" => UI, "http" => HTTP, "json" => JSON, "store" => STORE, _ => APP };
        if !list.contains(&f.as_str()) { return Err(format!("main.lua:{}: {t}.{f} does not exist; {t} has: {}", toks[k].1, list.join(", "))); }
    }
    Ok(())
}

/// The offline test run of a new or changed app, in this process with no output. `keys` are typed one after another (a draw after each); then the
/// tick if the app has one; the grid at 80x24 is returned; then draw is called at 40x12 and 132x40. Network off, `store` is an in-memory copy,
/// modals answer: input "1", pick 1, confirm no.
pub fn dry_run(src: &str, store: Vec<(String, String)>, keys: &[&str]) -> Result<String, String> {
    let sb = Sandbox::new(Setup { name: "test".into(), args: vec![], allowed: vec![], store_dir: None, store, tty: false, offline: true, size: (80, 24) }).map_err(|e| e.to_string())?;
    let step = |ctx: String, r: LResult<()>| r.map_err(|e| error_text(&e, src, &ctx));
    step("test run, loading the file: ".into(), sb.load(src))?;
    if !sb.has_draw() { return Err("the app has no draw(w, h) function".into()); }
    step("test run, first draw: ".into(), sb.draw(80, 24))?;
    for k in keys { step(format!("test run, after key \"{k}\": "), sb.key(k).and_then(|_| sb.draw(80, 24)))?; }
    if sb.every().is_some() { step("test run, tick: ".into(), sb.tick().and_then(|_| sb.draw(80, 24)))?; }
    let text = sb.grid_text();
    for (w, h) in [(40, 12), (132, 40)] { step(format!("test run, screen {w}x{h}: "), sb.draw(w, h))?; }
    Ok(text)
}

// ---- running for real ----

pub struct Launch { pub name: String, pub title: String, pub args: Vec<String>, pub allowed: Vec<Url>, pub dir: PathBuf, pub src: String }

struct Screen { _raw: term::Raw }   // dropped after the Drop below has cleaned the screen: Raw then restores the terminal mode
impl Drop for Screen { fn drop(&mut self) { term::put(b"\x1b]2;\x07\x1b[0m\x1b[2J\x1b[H\x1b[?25h"); } }

fn limit(res: libc::c_int, v: u64) { let l = libc::rlimit { rlim_cur: v as _, rlim_max: v as _ }; unsafe { libc::setrlimit(res as _, &l); } }

/// Runs the app until it quits. Exit code 0, or 1 after a crash (message printed, saved in ~/apps/NAME/last_error).
pub fn run(l: Launch) -> i32 {
    if unsafe { libc::getuid() } == 0 { eprintln!("agent: apps do not run as root; log in as a normal user"); return 1; }
    if unsafe { libc::isatty(0) != 1 || libc::isatty(1) != 1 } { eprintln!("agent: apps need a terminal"); return 1; }
    limit(libc::RLIMIT_CORE as _, 0); limit(libc::RLIMIT_FSIZE as _, 4 << 20); limit(libc::RLIMIT_AS as _, 192 << 20);
    std::env::set_var("TZ", posix_tz(thir::clock::home_tz().unwrap_or(0)));   // musl's localtime reads it, so os.date shows local time
    match session(&l) {
        Ok(()) => 0,
        Err((ctx, text)) => {
            eprintln!("{} stopped: {}\n(agent fix {} asks the AI to fix it)", l.name, text.lines().next().unwrap_or(""), l.name);
            let saved: String = format!("{ctx}{text}").chars().take(4096).collect();   // the saved copy says what the app was doing
            let _ = crate::agent::write_private(&l.dir.join("last_error"), &saved);
            1
        }
    }
}

/// Err((what the app was doing, the error text)).
fn session(l: &Launch) -> Result<(), (String, String)> {
    let plain = |t: String| (String::new(), t);
    let store_dir = l.dir.join("data");
    let store = match crate::agent::read_private(&store_dir.join("store.json"), unsafe { libc::getuid() }, MAX_STORE as u64 + 1024) {
        Ok(Some(t)) => match json::parse(&t) { Ok(Json::Obj(o)) => o.iter().map(|(k, v)| (k.clone(), json::dump(v))).collect(), _ => vec![] },
        _ => vec![],
    };
    let (w, h) = term::size();
    let sb = Sandbox::new(Setup { name: l.name.clone(), args: l.args.clone(), allowed: l.allowed.clone(), store_dir: Some(store_dir), store, tty: true, offline: false, size: (w, h) }).map_err(|e| plain(e.to_string()))?;
    sb.load(&l.src).map_err(|e| plain(error_text(&e, &l.src, "")))?;
    if !sb.has_draw() { return Err(plain("the app has no draw(w, h) function".into())); }
    let Some(raw) = term::Raw::enter() else { return Err(plain("apps need a terminal".into())) };
    let _screen = Screen { _raw: raw };
    term::put(format!("\x1b]2;{}\x07\x1b[?25l", printable(&l.title)).as_bytes());
    let (mut w, mut h) = (w, h);
    let mut next_tick: Option<Instant> = None;
    let crash = |e: LErr, ctx: &str| -> Result<(), (String, String)> { if sb.interrupted() { Ok(()) } else { Err((ctx.to_string(), error_text(&e, &l.src, ""))) } };
    macro_rules! go { ($r:expr, $ctx:expr) => { if let Err(e) = $r { return crash(e, $ctx); } } }
    go!(sb.draw(w, h), "");
    loop {
        if sb.quit() || sb.eof() { return Ok(()); }
        if next_tick.is_none() && sb.every().is_some() { next_tick = Some(Instant::now()); }
        let ms = next_tick.map_or(250, |t| t.saturating_duration_since(Instant::now()).as_millis().min(250) as i32);
        let Some(evs) = sb.events(ms) else { return Ok(()) };
        let mut redraw = false;
        for ev in evs {
            let name = match ev { Ev::Esc | Ev::K(Key::Ctrl(b'c')) | Ev::K(Key::Char('q')) => return Ok(()), Ev::K(k) => key_name(&k) };
            if let Some(n) = name { go!(sb.key(&n), &format!("after key \"{n}\": ")); redraw = true; }
            if sb.quit() { return Ok(()); }
        }
        if term::size() != (w, h) { (w, h) = term::size(); redraw = true; }
        if next_tick.is_some_and(|t| Instant::now() >= t) {
            go!(sb.tick(), "in tick(): ");
            next_tick = sb.every().map(|d| Instant::now() + d); redraw = true;
        }
        if redraw { go!(sb.draw(w, h), "in draw(): "); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const CODE_PROMPT: &str = include_str!("prompts/code.txt");
    const CALC: &str = include_str!("../../../test/apps/calc.lua");
    const CALC_BAD: &str = include_str!("../../../test/apps/crash.lua");

    fn sb() -> Sandbox { Sandbox::new(Setup { name: "t".into(), args: vec!["a".into(), "b".into()], allowed: vec![Url::parse("http://10.0.2.2:18080").unwrap()], store_dir: None, store: vec![], tty: false, offline: false, size: (80, 24) }).unwrap() }
    fn eval(code: &str) -> Result<String, String> {
        let s = sb(); s.load(&format!("function draw(w, h) ui.text(1, 1, tostring((function() {code} end)())) end")).map_err(|e| e.to_string())?;
        s.draw(80, 24).map_err(|e| error_text(&e, "", ""))?; Ok(s.grid_text().lines().next().unwrap_or("").to_string())
    }
    fn run_keys(src: &str, keys: &[&str]) -> String { dry_run(src, vec![], keys).unwrap() }

    #[test] fn removed_functions_are_nil() {
        for n in ["io", "require", "load", "dofile", "loadfile", "package", "debug", "warn", "coroutine", "string.dump", "os.execute", "os.getenv", "os.remove", "os.rename", "os.exit", "os.tmpname", "os.setlocale"] {
            assert_eq!(eval(&format!("return {n}")).unwrap(), "nil", "{n}");
        }
    }
    #[test] fn print_raises() { assert!(eval("print('x')").unwrap_err().contains("print does not work in apps; use ui.text in draw()")); }
    #[test] fn allowed_things_work() {
        assert_eq!(eval("return os.time() > 1e9 and os.date('!%Y', 0)").unwrap(), "1970");
        assert_eq!(eval("local ok, e = pcall(error, 'boom'); return tostring(ok) .. e").unwrap(), "falseboom");
        assert_eq!(eval("return string.format('%.2f', 3.14159) .. #utf8.char(8364) .. math.floor(2.5)").unwrap(), "3.1432");
        assert_eq!(eval("return app.name .. #app.args .. app.args[2]").unwrap(), "t2b");
    }
    #[test] fn memory_is_limited() { assert!(eval("return #string.rep('x', 1e9)").unwrap_err().contains("memory")); }
    #[test] fn endless_loop_stops_within_a_few_seconds() {
        let t = Instant::now();
        let e = eval("while true do end").unwrap_err();
        assert!(e.contains("more than 2 s") && t.elapsed() < Duration::from_secs(5), "{e} {:?}", t.elapsed());
    }
    #[test] fn http_checks_origins_and_test_runs_are_offline() {
        assert!(eval("local b, e = http.get('https://evil.example/x'); return tostring(b) .. e").unwrap().starts_with("nilhttps://evil.example is not allowed"));
        assert!(eval("local b, e = http.get('http://10.0.2.2:18081/x'); return e").unwrap().contains("is not allowed"));   // another port is another origin
        let s = Sandbox::new(Setup { name: "t".into(), args: vec![], allowed: vec![], store_dir: None, store: vec![], tty: false, offline: false, size: (80, 24) }).unwrap();
        s.load("function draw() local b, e = http.get('http://10.0.2.2:18080/x'); ui.text(1, 1, tostring(b) .. tostring(e)) end").unwrap();
        s.draw(80, 24).unwrap(); assert!(s.grid_text().starts_with("nilhttp://10.0.2.2:18080 is not allowed"), "{}", s.grid_text());
    }
    #[test] fn escapes_never_reach_the_grid() {
        let s = sb(); s.load("function draw() ui.text(1, 3, 'a\\27[2Jb\\tc\\r\\n\\u{202e}'); ui.center(4, '\\27]0;x\\7'); ui.title('\\27', '\\1') end").unwrap(); s.draw(80, 24).unwrap();
        let t = s.grid_text(); assert!(t.contains("a?[2Jb c???"), "{t}"); assert!(!t.contains('\x1b') && !t.contains('\x07') && !t.contains('\x01'));
    }
    #[test] fn lint_flags_unknown_sdk_names() {
        let e = check("function draw()\n  ui.button(1, 2)\nend").unwrap_err();
        assert!(e.starts_with("main.lua:2: ui.button does not exist; ui has: text, center"), "{e}");
        assert!(check("local x = 'ui.nothing' -- store.nope\n--[[ http.zip ]] local s = [[json.bad]]\nui.text(1,1,'x')\nlocal t = {}\nt.ui.zzz = 1").is_ok());
        assert!(check("local app = {}\napp.state = 1").is_ok());   // the app's own table
        assert!(check("function draw(").unwrap_err().contains("main.lua:1:"));
    }
    #[test] fn json_bridge() {
        assert_eq!(eval("local v = json.decode('{\"a\":[1,2.5,\"x\",null,true],\"b\":{}}'); return math.type(v.a[1]) .. math.type(v.a[2]) .. v.a[3] .. tostring(v.a[4]) .. tostring(v.a[5]) .. type(v.b)").unwrap(), "integerfloatxniltruetable");
        assert_eq!(eval("return json.encode({1, 2, {x = 1}})").unwrap(), "[1,2,{\"x\":1}]");
        assert_eq!(eval("return json.encode({}) .. json.encode({a = 1.5, b = 'q\"'}) .. json.encode({[1] = 1, [3] = 3})").unwrap(), "[]{\"a\":1.5,\"b\":\"q\\\"\"}{\"1\":1,\"3\":3}");
        assert!(eval("local t = {}; t.t = t; return json.encode(t)").unwrap_err().contains("too deep"));
        assert_eq!(eval("local v, e = json.decode('{'); return tostring(v) .. (e and 'e' or '')").unwrap(), "nile");
        assert!(eval("return json.encode(function() end)").unwrap_err().contains("cannot encode a function"));
        assert!(eval("local v, e = json.decode(string.rep('[', 3000000)); return tostring(v) .. e").unwrap().contains("too big"));
        assert!(eval("local t = {}; for i = 1, 100 do t[i] = string.rep('x', 100000) end; return json.encode(t)").unwrap_err().contains("too big to encode"));
        assert_eq!(eval("ui.fill(1, 1, 1e18, 1e18); ui.text(-9e18, 1e18, 'x'); ui.text(1e30, 1, 'y'); ui.list(1, 1, 1e18, 1e18, {'a'}, 1e18); return 'ok'").unwrap(), "ok");
    }
    #[test] fn store_round_trip_and_limit() {
        let s = Sandbox::new(Setup { name: "t".into(), args: vec![], allowed: vec![], store_dir: None, store: vec![("old".into(), "[1,2]".into())], tty: false, offline: false, size: (80, 24) }).unwrap();
        s.load("function draw() store.set('n', 5); store.set('t', {a = {1, 2}}); store.set('gone', 1); store.set('gone', nil)
          local ok = pcall(store.set, 'big', string.rep('x', 1100000))
          ui.text(1, 1, #store.get('old') .. store.get('n') .. store.get('t').a[2] .. tostring(store.get('gone', 'dflt')) .. tostring(ok) .. tostring(store.get('big'))) end").unwrap();
        s.draw(80, 24).unwrap(); assert_eq!(s.grid_text(), "252dfltfalsenil");
    }
    #[test] fn grid_diff_bytes() {
        let a: Vec<Cell> = "ab cd ".chars().map(|c| (c, 0)).collect();
        let mut b = a.clone(); b[4] = ('X', 3);
        assert_eq!(render(None, &a, 3), b"\x1b[2J\x1b[1;1H\x1b[0mab \x1b[0m\x1b[2;1H\x1b[0mcd \x1b[0m");
        assert_eq!(render(Some(&a), &a, 3), b"");
        assert_eq!(render(Some(&a), &b, 3), b"\x1b[2;1H\x1b[0mc\x1b[0;32mX\x1b[0m \x1b[0m");
    }
    #[test] fn wrap_and_widths() {
        assert_eq!(wrap("one two three four", 7), ["one two", "three", "four"]);
        assert_eq!(wrap("abcdefghij k", 4), ["abcd", "efgh", "ij k"]);
        assert_eq!(wrap("a\n\nb", 5), ["a", "", "b"]);
        assert_eq!(eval("return ui.pad('héllo wörld', 7) .. '|' .. ui.width('€€') .. ui.pad('ab', 4) .. '|'").unwrap(), "héllo w|2ab  |");
    }
    #[test] fn tz_string() { assert_eq!(posix_tz(330), "<+0530>-05:30"); assert_eq!(posix_tz(-60), "<-0100>+01:00"); assert_eq!(posix_tz(0), "<+0000>-00:00"); }
    #[test] fn calc_works_in_a_test_run() {
        assert!(run_keys(CALC, &["1", "+", "2", "="]).contains("= 3"));
        assert!(dry_run(CALC, vec![], &[]).is_ok());
        assert!(run_keys(CALC, &["1", "/", "0", "="]).contains("division by zero"));
        let off = dry_run("function draw() local b, e = http.get('http://10.0.2.2:18080/x'); ui.text(1, 1, tostring(b) .. tostring(e)) end", vec![], &[]).unwrap();
        assert_eq!(off, "niloffline (test run)");
    }
    #[test] fn crashing_app_reports_the_line() {
        let e = dry_run(CALC_BAD, vec![], &["x"]).unwrap_err();
        assert!(e.starts_with("test run, after key \"x\": main.lua:") && e.contains("\n  line "), "{e}");
        assert!(dry_run("x = 1", vec![], &[]).unwrap_err().contains("no draw(w, h)"));
    }
    #[test] fn too_small_window() { let s = sb(); s.load("function draw() ui.text(1, 1, 'app') end").unwrap(); s.draw(19, 4).unwrap(); assert_eq!(s.grid_text().lines().next().unwrap(), "window too small"); }
    #[test] fn sdk_names_match_the_prompt() {
        let mut in_prompt: Vec<String> = CODE_PROMPT.lines().filter_map(|l| {
            let (t, rest) = l.split_once('.')?;
            if !["ui", "http", "json", "store", "app"].contains(&t) { return None; }
            let n: String = rest.chars().take_while(|c| c.is_ascii_lowercase() || *c == '_').collect();
            Some(format!("{t}.{n}"))
        }).collect();
        in_prompt.sort(); in_prompt.dedup();
        let mut real = sdk_names(); real.sort();
        assert_eq!(in_prompt, real);
        // and every registered function really exists in the sandbox
        for n in &real { assert_ne!(eval(&format!("return {n}")).unwrap(), "nil", "{n}"); }
    }
    #[test] fn key_names() {
        assert_eq!(key_name(&Key::Char(' ')).as_deref(), Some(" ")); assert_eq!(key_name(&Key::Ctrl(b'a')).as_deref(), Some("ctrl-a")); assert_eq!(key_name(&Key::PgDn).as_deref(), Some("pgdn"));
    }
}
