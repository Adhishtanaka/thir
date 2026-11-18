//! Apps on disk (~/apps/NAME/main.lua + app.json) and the commands agent build, edit, fix, run, list, undo and rm.
//! `run`, `list`, `undo` and `rm` never load an API key or the agent config. build, edit and fix use agent::ctx_for, the one path to a key.
//! Network approvals live in ~/.config/thir/approvals, outside every app directory: an app that ships its own "approved" list gets nothing from it.
use crate::{memory, progress::{self, Progress}, agent::{self, clean, read_private, sanitize, stream, write_private, Chosen, Ctx, EFFORTS}, app};
use std::{fs, io::BufRead, os::unix::fs::{DirBuilderExt, MetadataExt}, path::{Path, PathBuf}, time::Instant};
use thir::{clock, files::{app_name_ok, printable, RESERVED}, http::Url, json::{self, Json}, line::Editor, term};

const MAX_CODE: u64 = 64 << 10;
const MAX_MANIFEST: u64 = 16 << 10;
const MAX_SPEC: usize = 24 << 10;
const MAX_NET: usize = 8;
const MAX_FIX: usize = 3;
const KEEP: i64 = 20;
const MAX_HISTORY: usize = 40;
const MAX_IDEA: usize = 4000;
const BUILD_EFFORT: &str = "medium";
const SPEC_PROMPT: &str = include_str!("prompts/spec.txt");
const CODE_PROMPT: &str = include_str!("prompts/code.txt");
const FIX_PROMPT: &str = include_str!("prompts/fix.txt");
const EDIT_PROMPT: &str = include_str!("prompts/edit.txt");
/// Typed one after another in the offline test run.
const TEST_KEYS: [&str; 14] = ["down", "up", "right", "left", "enter", "tab", " ", "1", "2", "+", "=", "a", "backspace", "r"];

#[derive(Debug, PartialEq)]
pub struct Manifest { pub name: String, pub title: String, pub about: String, pub version: i64, pub net: Vec<String>, pub raw: Json }

/// An origin the manifest may list: http or https, a host and an optional port, nothing else.
pub fn origin_ok(s: &str) -> Result<Url, String> {
    let u = Url::parse(s).map_err(|e| format!("{s}: {e}"))?;
    if u.path != "/" || s.contains(['?', '#']) { return Err(format!("{s}: an origin has no path or query")); }
    Ok(u)
}

fn norm(u: &Url) -> String { format!("{}://{}", if u.https { "https" } else { "http" }, u.host_header()) }

fn origins(j: &Json, key: &str) -> Result<Vec<String>, String> {
    let Some(v) = j.get(key) else { return Ok(vec![]) };
    let Json::Arr(a) = v else { return Err(format!("{key} must be a list")) };
    if a.len() > MAX_NET { return Err(format!("{key} lists more than {MAX_NET} origins")); }
    a.iter().map(|o| { let s = o.str().ok_or_else(|| format!("{key} must list text"))?; origin_ok(s)?; Ok(s.to_string()) }).collect()
}

/// app.json. Any "approved" key in it is ignored: approvals are not read from the app's own files.
pub fn parse_manifest(text: &str) -> Result<Manifest, String> {
    let j = json::parse(text).map_err(|e| format!("app.json is not valid JSON ({e})"))?;
    if !matches!(j, Json::Obj(_)) { return Err("app.json must be an object".into()); }
    let name = j.get("name").and_then(Json::str).filter(|n| app_name_ok(n)).ok_or("app.json has no valid name")?.to_string();
    let short = |k: &str, max: usize| printable(j.get(k).and_then(Json::str).unwrap_or("")).chars().take(max).collect::<String>();
    let title = short("title", 30);
    Ok(Manifest { title: if title.is_empty() { name.clone() } else { title }, about: short("about", 120), name, version: j.get("version").and_then(Json::num).unwrap_or(1.0) as i64, net: origins(&j, "net")?, raw: j })
}

// ---- approvals: "<app> <origin>" lines in ~/.config/thir/approvals ----

fn same(a: &str, b: &str) -> bool { matches!((Url::parse(a), Url::parse(b)), (Ok(x), Ok(y)) if x.same_origin(&y)) }

/// The origins recorded for `name`.
pub fn approved_for(text: &str, name: &str) -> Vec<String> {
    text.lines().filter_map(|l| l.split_once(' ')).filter(|(n, _)| *n == name).map(|(_, o)| o.trim().to_string()).collect()
}

/// The net origins the user has not approved yet.
pub fn unapproved(m: &Manifest, approved: &[String]) -> Vec<String> { m.net.iter().filter(|n| !approved.iter().any(|a| same(a, n))).cloned().collect() }

pub fn add_approvals(text: &str, name: &str, new: &[String]) -> String {
    let mut o = text.to_string();
    if !o.is_empty() && !o.ends_with('\n') { o.push('\n'); }
    for n in new { if let Ok(u) = Url::parse(n) { o += &format!("{name} {}\n", norm(&u)); } }
    o
}

pub fn drop_approvals(text: &str, name: &str) -> String { text.lines().filter(|l| l.split_once(' ').map(|p| p.0) != Some(name)).map(|l| format!("{l}\n")).collect() }

fn approvals_path(home: &Path) -> PathBuf { home.join(".config/thir/approvals") }

fn load_approvals(home: &Path) -> Result<String, String> {
    let p = approvals_path(home);
    read_private(&p, unsafe { libc::getuid() }, 64 << 10).map(Option::unwrap_or_default).map_err(|w| format!("agent: refusing to read {}: it {w}. Delete it (rm) to forget all approvals.", p.display()))
}

fn save_approvals(home: &Path, text: &str) -> Result<(), String> {
    let dir = agent::private_dir(home, &[])?;
    write_private(&dir.join("approvals"), text).map_err(|e| format!("agent: cannot save the approvals: {e}"))
}

/// The question before an app first connects (or connects somewhere new). `had_some`: the user approved other hosts for it before.
/// With `color` it is a frame: what the app may do (risks in yellow), the keys, the default.
pub fn approval_text(m: &Manifest, new: &[String], had_some: bool, color: bool) -> String {
    let hosts: Vec<(String, bool)> = new.iter().map(|o| Url::parse(o).map_or((printable(o), false), |u| (format!("{}{}", u.host_header(), if u.https { "" } else { " (not encrypted)" }), !u.https))).collect();
    let (name, also) = (&m.name, if had_some { "now also wants to" } else { "wants to" });
    if !color {
        return format!("  {name} {also}:\n    \u{b7} connect to {}\n    \u{b7} keep its own data in ~/apps/{name}/data\n  It cannot read your other files or keys, or run programs.\n\n  Allow and run? (y/N) ", hosts.iter().map(|h| h.0.as_str()).collect::<Vec<_>>().join(", "));
    }
    let (d, w, r) = (progress::DIM, progress::WARN, progress::R);
    let mut body: Vec<String> = hosts.iter().map(|(h, plain)| format!("{}\u{25b8}{r} connect to {}{h}{r}", if *plain { w } else { d }, if *plain { w } else { "" })).collect();
    body.push(format!("{d}\u{25b8}{r} keep its own data in ~/apps/{name}/data"));
    body.push(String::new());
    body.push(format!("{d}It cannot read your other files or keys, or run programs.{r}"));
    let title = format!("{}{name}{r} {also}", progress::ACC);
    format!("\n{}\n\n  \u{203a} ", progress::boxed(&title, &body, &[progress::keys("allow and run", "cancel")], progress::box_w()))
}

/// A yes/no question that changes something for good: a frame with what happens, then the `question` (default no).
pub fn confirm(title: &str, risk: &str, note: &str, yes: &str, question: &str) -> bool {
    if unsafe { libc::isatty(1) } == 1 {
        let (d, w, r) = (progress::DIM, progress::WARN, progress::R);
        let body = [format!("{w}\u{25b8}{r} {w}{risk}{r}"), format!("{d}{note}{r}")];
        println!("\n{}\n", progress::boxed(&format!("{}{title}{r}", progress::ACC), &body, &[progress::keys(yes, "keep it")], progress::box_w()));
        return ask("  \u{203a} ");   // the frame above shows the keys
    }
    ask(question)
}

/// A finished line: `  \u{25cf} text` on a terminal, plain otherwise.
pub fn note_line(s: &str) -> String { if unsafe { libc::isatty(1) } == 1 { progress::mark(true, s) } else { s.to_string() } }

// ---- paths ----

pub fn home() -> Result<PathBuf, String> {
    std::env::var("HOME").ok().filter(|h| h.starts_with('/')).map(PathBuf::from).ok_or_else(|| "agent: no home directory (HOME is not set)".to_string())
}

/// A real directory that is yours and that nobody else can write to.
fn trusted_dir(p: &Path) -> Result<(), String> {
    let m = fs::symlink_metadata(p).map_err(|e| format!("agent: {}: {e}", p.display()))?;
    if !m.is_dir() || m.uid() != unsafe { libc::getuid() } || m.mode() & 0o022 != 0 {
        return Err(format!("agent: refusing to use {}: it must be a real directory that only you can write. Move or delete it (rm -r), then try again.", p.display()));
    }
    Ok(())
}

fn mkdir(p: &Path) -> Result<(), String> {
    if fs::symlink_metadata(p).is_err() { fs::DirBuilder::new().mode(0o700).create(p).map_err(|e| format!("agent: cannot make {}: {e}", p.display()))?; }
    trusted_dir(p)
}

pub fn ask(question: &str) -> bool {
    print!("{question}");
    let _ = std::io::Write::flush(&mut std::io::stdout());
    let mut l = String::new();
    std::io::stdin().lock().read_line(&mut l).is_ok_and(|n| n > 0) && matches!(l.trim(), "y" | "Y" | "yes")
}

pub fn tty() -> bool { unsafe { libc::isatty(0) == 1 && libc::isatty(1) == 1 } }
fn me() -> u32 { unsafe { libc::getuid() } }

/// (home, ~/apps/NAME) of an app that exists. Err = message.
fn app_dir(name: &str) -> Result<(PathBuf, PathBuf), String> {
    if !app_name_ok(name) { return Err(format!("agent: no app called {} (names use a-z, 0-9 and -; agent list shows your apps)", printable(name))); }
    let home = home()?;
    let dir = home.join("apps").join(name);
    if fs::symlink_metadata(&dir).is_err() { return Err(format!("agent: no app called {name} (agent list shows your apps)")); }
    trusted_dir(&home.join("apps")).and_then(|_| trusted_dir(&dir))?;
    Ok((home, dir))
}

fn read_app_file(dir: &Path, f: &str, max: u64) -> Result<Option<String>, String> {
    read_private(&dir.join(f), me(), max).map_err(|why| format!("agent: refusing to read {}: it {why}", dir.join(f).display()))
}

fn load_manifest(dir: &Path, name: &str) -> Result<Manifest, String> {
    let text = read_app_file(dir, "app.json", MAX_MANIFEST)?.ok_or_else(|| format!("agent: {name} has no app.json"))?;
    match parse_manifest(&text) { Ok(m) if m.name == name => Ok(m), Ok(_) => Err(format!("agent: {name}/app.json belongs to another app")), Err(e) => Err(format!("agent: {name}: {e}")) }
}

/// The saved store as (key, JSON text) pairs, for the offline test run's in-memory copy.
fn load_store(dir: &Path) -> Vec<(String, String)> {
    match read_private(&dir.join("data/store.json"), me(), (1 << 20) + 1024) {
        Ok(Some(t)) => match json::parse(&t) { Ok(Json::Obj(o)) => o.iter().map(|(k, v)| (k.clone(), json::dump(v))).collect(), _ => vec![] },
        _ => vec![],
    }
}

// ---- reply and plan parsing ----

#[derive(Debug, Default, PartialEq, Clone)]
pub struct Header { pub name: Option<String>, pub title: Option<String>, pub about: Option<String>, pub net: Option<Vec<String>> }

#[derive(Debug, PartialEq)]
pub enum Reply { No(String), App(Header, String) }

/// `none` or up to 8 origins separated by spaces or commas; each an http(s) origin with no path, query or user name.
pub fn parse_net(v: &str) -> Result<Vec<String>, String> {
    let toks: Vec<&str> = v.split(|c: char| c.is_whitespace() || c == ',').filter(|t| !t.is_empty()).collect();
    if toks.is_empty() || toks == ["none"] { return Ok(vec![]); }
    let mut out: Vec<String> = vec![];
    for t in toks { let n = norm(&origin_ok(t)?); if !out.contains(&n) { out.push(n); } }
    if out.len() > MAX_NET { return Err(format!("net lists more than {MAX_NET} origins")); }
    Ok(out)
}

pub fn no_reason(t: &str) -> Option<String> {
    let t = t.trim();
    let rest = if t.get(..3).is_some_and(|p| p.eq_ignore_ascii_case("no:")) { &t[3..] } else if t.eq_ignore_ascii_case("no") || t.eq_ignore_ascii_case("no.") { "" } else { return None };
    let r: String = printable(rest.trim()).chars().take(200).collect();
    Some(if r.is_empty() { "no reason was given".into() } else { r })
}

fn parse_header(lines: &[&str]) -> Result<Header, String> {
    let mut h = Header::default();
    for l in lines {
        let Some((k, v)) = l.split_once(':') else { continue };
        let v = v.trim();
        match k.trim().to_ascii_lowercase().as_str() {
            "name" => h.name = Some(v.to_string()).filter(|n| app_name_ok(n)),
            "title" => h.title = Some(printable(v).chars().take(30).collect::<String>()).filter(|t| !t.is_empty()),
            "about" => h.about = Some(printable(v).chars().take(120).collect::<String>()).filter(|t| !t.is_empty()),
            "net" => h.net = Some(parse_net(v)?),
            _ => {}
        }
    }
    Ok(h)
}

fn unfenced(text: &str) -> Vec<&str> { text.lines().filter(|l| !l.trim_start().starts_with("```")).collect() }

/// A model reply in the OUTPUT format. Err = what is wrong, for the fix loop.
pub fn parse_reply(text: &str, need_header: bool) -> Result<Reply, String> {
    if let Some(r) = no_reason(text) { return Ok(Reply::No(r)); }
    let lines = unfenced(text);
    let at = |m: &str| lines.iter().position(|l| l.trim() == m);
    let code_at = at("===CODE===").ok_or("the reply had no ===CODE=== section")?;
    let header = match at("===APP===") { Some(a) if a < code_at => Some(parse_header(&lines[a + 1..code_at])?), _ => None };
    if need_header && header.is_none() { return Err("the reply had no ===APP=== header".into()); }
    let body = &lines[code_at + 1..];
    let end = body.iter().position(|l| l.trim() == "===END===").unwrap_or(body.len());
    let code = body[..end].join("\n").trim().to_string();
    if code.is_empty() { return Err("the reply had no code after ===CODE===".into()); }
    if code.len() as u64 > MAX_CODE { return Err("the code is longer than 64 KiB; write a shorter app".into()); }
    Ok(Reply::App(header.unwrap_or_default(), code + "\n"))
}

#[derive(Debug, PartialEq)]
pub enum Spec { No(String), Plan(Header) }

pub fn parse_spec(text: &str) -> Result<Spec, String> {
    if let Some(r) = no_reason(text) { return Ok(Spec::No(r)); }
    let lines = unfenced(text);
    let a = lines.iter().position(|l| l.trim() == "===APP===").ok_or("the plan had no ===APP=== section")?;
    let end = lines[a + 1..].iter().position(|l| l.starts_with("===")).map_or(lines.len(), |i| a + 1 + i);
    let h = parse_header(&lines[a + 1..end])?;
    if h.name.is_none() { return Err("the plan has no valid name (a-z, 0-9 and -, at most 20, starting with a letter)".into()); }
    Ok(Spec::Plan(h))
}

/// `base`, then base-2 to base-9: the first that is not a program name and not `taken`.
pub fn pick_name(base: &str, taken: &dyn Fn(&str) -> bool) -> Option<String> {
    let stem: String = base.chars().take(18).collect();
    std::iter::once(base.to_string()).chain((2..=9).map(|n| format!("{stem}-{n}"))).find(|n| !RESERVED.contains(&n.as_str()) && !taken(n))
}

fn fill(t: &str, vals: &[(&str, &str)]) -> String {
    let (mut o, mut rest) = (String::new(), t);
    'o: while let Some(c) = rest.chars().next() {
        for (k, v) in vals { if let Some(r) = rest.strip_prefix(k) { o += v; rest = r; continue 'o; } }
        o.push(c); rest = &rest[c.len_utf8()..];
    }
    o
}

/// The spec with one more line under ===CHANGES===; the oldest lines go first when the file would grow past MAX_SPEC.
fn add_change(spec: &str, what: &str) -> String {
    let line = format!("- {}", clean(what).chars().take(300).collect::<String>());
    let (base, changes) = match spec.split_once("===CHANGES===\n") { Some((b, c)) => (b.to_string(), c.lines().map(String::from).collect::<Vec<_>>()), None => (spec.trim_end().to_string() + "\n", vec![]) };
    let mut ch = changes; ch.push(line);
    while base.len() + ch.iter().map(|l| l.len() + 1).sum::<usize>() > MAX_SPEC && ch.len() > 1 { ch.remove(0); }
    format!("{base}===CHANGES===\n{}\n", ch.join("\n"))
}

// ---- saving ----

struct Fields<'a> { name: &'a str, title: &'a str, about: &'a str, version: i64, net: &'a [String], provider: &'a str, model: &'a str, kind: &'a str, prompt: &'a str }

fn strs(v: &[String]) -> Json { Json::Arr(v.iter().cloned().map(Json::Str).collect()) }

fn manifest_json(old: Option<&Manifest>, f: &Fields, now: i64) -> String {
    let num = |n: i64| Json::Num(n as f64);
    let created = old.and_then(|m| m.raw.get("created")).and_then(Json::num).map_or(now, |n| n as i64);
    let mut hist = match old.and_then(|m| m.raw.get("history")) { Some(Json::Arr(a)) => a.clone(), _ => vec![] };
    hist.push(Json::Obj(vec![("v".into(), num(f.version)), ("at".into(), num(now)), ("kind".into(), Json::Str(f.kind.into())),
        ("prompt".into(), Json::Str(clean(f.prompt).chars().take(200).collect())), ("net".into(), strs(f.net))]));
    if hist.len() > MAX_HISTORY { hist.drain(..hist.len() - MAX_HISTORY); }
    json::dump(&Json::Obj(vec![("name".into(), Json::Str(f.name.into())), ("title".into(), Json::Str(f.title.into())), ("about".into(), Json::Str(f.about.into())), ("version".into(), num(f.version)),
        ("created".into(), num(created)), ("updated".into(), num(now)), ("provider".into(), Json::Str(f.provider.into())), ("model".into(), Json::Str(f.model.into())),
        ("net".into(), strs(f.net)), ("history".into(), Json::Arr(hist))]))
}

/// Writes versions/V.lua, main.lua, spec.txt and app.json (in that order: app.json last) under `apps`/name and drops versions older than the newest KEEP. Returns V.
fn save(apps: &Path, f: &Fields, code: &str, spec: &str, old: Option<&Manifest>) -> Result<i64, String> {
    let dir = apps.join(f.name);
    mkdir(apps)?; mkdir(&dir)?; mkdir(&dir.join("versions"))?;
    let io = |what: &str, r: std::io::Result<()>| r.map_err(|e| format!("agent: cannot save {what}: {e}"));
    io("the version", write_private(&dir.join(format!("versions/{}.lua", f.version)), code))?;
    io("main.lua", write_private(&dir.join("main.lua"), code))?;
    io("spec.txt", write_private(&dir.join("spec.txt"), spec))?;
    io("app.json", write_private(&dir.join("app.json"), &manifest_json(old, f, clock::now())))?;
    for v in 1..=(f.version - KEEP) { let _ = fs::remove_file(dir.join(format!("versions/{v}.lua"))); }
    Ok(f.version)
}

/// `agent undo`: back one version. Returns the version now current.
fn undo_app(dir: &Path, name: &str, now: i64) -> Result<i64, String> {
    let m = load_manifest(dir, name)?;
    let v = m.version;
    if v <= 1 { return Err(format!("agent: {name} is at version 1; there is nothing to undo")); }
    let code = read_app_file(dir, &format!("versions/{}.lua", v - 1), MAX_CODE)?.ok_or_else(|| format!("agent: version {} of {name} is no longer kept", v - 1))?;
    let net = match &m.raw.get("history") {
        Some(Json::Arr(h)) => h.iter().rev().find(|e| e.get("v").and_then(Json::num) == Some((v - 1) as f64)).and_then(|e| origins(e, "net").ok()).unwrap_or(m.net.clone()),
        _ => m.net.clone(),
    };
    let s = |k: &str| m.raw.get(k).and_then(Json::str).unwrap_or("").to_string();
    let f = Fields { name, title: &m.title, about: &m.about, version: v - 1, net: &net, provider: &s("provider"), model: &s("model"), kind: "undo", prompt: "" };
    write_private(&dir.join("main.lua"), &code).map_err(|e| format!("agent: cannot save main.lua: {e}"))?;
    write_private(&dir.join("app.json"), &manifest_json(Some(&m), &f, now)).map_err(|e| format!("agent: cannot save app.json: {e}"))?;
    let _ = fs::remove_file(dir.join(format!("versions/{v}.lua")));
    Ok(v - 1)
}

// ---- the generation loop ----

enum Gen { No(String), App(Header, String) }

struct Job<'a> { hint: String, c: Ctx<'a>, calls: usize, chars: usize, store: Vec<(String, String)>, t0: Instant, p: Progress }

impl Job<'_> {
    /// One model call; its text goes to the progress display. (text, cut off at the length limit)
    fn call(&mut self, system: &str, user: String) -> Result<(String, bool), String> {
        self.calls += 1; self.chars += system.len() + user.len();
        let d = stream(&self.c, system, &[("user", user)], &mut |t| self.p.text(t))?;
        self.chars += d.text.len();
        if d.stop == "refusal" { return Err(format!("agent: Claude declined to build this (category: {})", clean(d.category.as_deref().unwrap_or("unknown")))); }
        if d.stop == "content_filter" { return Err("agent: the provider declined this request".into()); }
        if d.text.trim().is_empty() { return Err(format!("agent: {} sent an empty answer", self.c.p.name)); }
        Ok((d.text, matches!(d.stop.as_str(), "max_tokens" | "length" | "model_context_window_exceeded")))
    }

    /// Code call, parse, check, test run; up to MAX_FIX fixes. The result is an app that passed every check. `plain` names the first step when there is no terminal.
    fn generate(&mut self, plain: &str, mut user: String, spec: &str, need_header: bool) -> Result<Gen, String> {
        self.p.step("code", plain);
        for attempt in 0..=MAX_FIX {
            let (text, cut) = self.call(CODE_PROMPT, user)?;
            let mut code = text.chars().take(MAX_CODE as usize).collect::<String>();
            let err = if cut { "your reply was cut off at the length limit; write a shorter app".to_string() } else {
                match parse_reply(&text, need_header) {
                    Ok(Reply::No(r)) => { self.p.fail("not buildable"); return Ok(Gen::No(r)) }
                    Ok(Reply::App(h, c)) => {
                        code = c.clone();
                        self.p.finish(None, &format!("{} lines", text.lines().count()));
                        self.p.step("check", "checking");
                        let r = app::check(&c).map(|_| self.p.finish(Some("passed"), "")).and_then(|_| { self.p.step("test run", "test run"); app::dry_run(&c, self.store.clone(), &TEST_KEYS) });
                        match r { Ok(_) => { self.p.finish(Some("passed"), ""); return Ok(Gen::App(h, c)) } Err(e) => e }
                    }
                    Err(e) => e,
                }
            };
            let one: String = clean(err.lines().next().unwrap_or("")).chars().take(100).collect();
            if attempt == MAX_FIX { return Err(format!("agent: the app still fails after {MAX_FIX} fixes: {one}. Nothing was saved. Try again or describe it differently.")); }
            self.p.retry(attempt + 1, MAX_FIX, &one);
            user = fill(FIX_PROMPT, &[("{error}", &sanitize(&err)), ("{spec}", spec), ("{code}", &code)]);
        }
        unreachable!()
    }

    /// A progress row naming the remembered facts the result used (nothing when none).
    fn memory_note(&self, used: &[String]) {
        if !used.is_empty() { self.p.note("memory", &used.join(" · "), &format!("using your memory: {}", used.join(", "))); }
    }

    fn done(&mut self, name: &str, v: i64) {
        let k = ((self.chars / 4) + 500) / 1000;   // shortcut: chars/4 estimate; parse usage from message_delta later
        let mut next = vec![("run", name.to_string()), ("edit", format!("agent edit {name} \"…\""))];
        if v > 1 { next.push(("undo", format!("agent undo {name}"))); }
        next.push(("remove", format!("agent rm {name}")));
        let line = self.p.done(&format!("saved {name} v{v}"), self.t0.elapsed().as_secs(), self.calls, k.max(1), &self.hint, &next);
        *SUMMARY.lock().unwrap() = line;   // the app clears the screen when it quits; the line is shown again after it
    }
}

/// `header` is what is being done ("building"), `name` the app if known; the display starts here.
fn job<'a>(ch: &'a Chosen, store: Vec<(String, String)>, header: &str, plain: &str, labels: &[&str]) -> Job<'a> {
    let p = Progress::new(header, &format!("  {} via {} · uses API credit", ch.model, ch.p.name), plain, labels);
    Job { hint: String::new(), c: ch.ctx(), calls: 0, chars: 0, store, t0: Instant::now(), p }
}

static SUMMARY: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());

fn not_buildable(r: &str) -> String { format!("agent: that is not an app I can build: {r}") }

fn build(ch: &Chosen, home: &Path, facts: &[String], idea: &str) -> Result<String, String> {
    let mut j = job(ch, vec![], "building", &format!("agent: building with {} via {} (API use costs money)", ch.model, ch.p.name), &["plan", "code", "check", "test run"]);
    j.p.step("plan", "planning");
    let (spec, _) = j.call(SPEC_PROMPT, format!("{}===REQUEST===\n{idea}", memory::block(facts)))?;
    let plan_lines = spec.lines().count();
    let spec: String = unfenced(&spec).join("\n").chars().take(MAX_SPEC).collect::<String>() + "\n";
    let plan = match parse_spec(&spec).map_err(|e| format!("agent: the plan was not usable: {e}. Nothing was saved. Try again or describe it differently."))? { Spec::No(r) => { j.p.fail("not buildable"); return Err(not_buildable(&r)) } Spec::Plan(h) => h };
    j.p.finish(None, &format!("{plan_lines} lines"));
    j.memory_note(&memory::used(facts, "", &spec));
    let apps = home.join("apps");
    let base = plan.name.clone().unwrap_or_default();
    let name = pick_name(&base, &|n| fs::symlink_metadata(apps.join(n)).is_ok() || Path::new("/bin").join(n).exists()).ok_or("agent: could not find a free name for the app")?;
    if name != base {
        if fs::symlink_metadata(apps.join(&base)).is_ok() {
            j.p.note("name", &format!("{name} · you already have {base}"), &format!("you already have {base}; this one is {name}. To change {base}: agent edit {base} \"…\""));
            j.hint = format!("to change {base} instead: agent edit {base} \"…\"");
        } else { j.p.note("name", &format!("{name} · {base} is a command"), &format!("{base} is the name of a command; this app is called {name}")); }
    }
    let (h, code) = match j.generate("writing code", format!("===REQUEST===\n{idea}\n===SPEC===\n{spec}\nWrite this app."), &spec, true)? { Gen::No(r) => return Err(not_buildable(&r)), Gen::App(h, c) => (h, c) };
    let (title, about, net) = (h.title.or(plan.title).unwrap_or_else(|| name.clone()), h.about.or(plan.about).unwrap_or_default(), h.net.or(plan.net).unwrap_or_default());
    let f = Fields { name: &name, title: &title, about: &about, version: 1, net: &net, provider: &ch.p.name, model: &ch.model, kind: "build", prompt: idea };
    let v = save(&apps, &f, &code, &spec, None)?;
    j.done(&name, v);
    Ok(name)
}

#[derive(Clone, Copy, PartialEq)]
enum Change { Edit, Fix }

/// edit or fix of an existing app: `text` is the change or the problem (a fix may have none and use the saved crash).
fn change(ch: &Chosen, name: &str, kind: Change, facts: &[String], text: &str) -> Result<(), String> {
    let (_, dir) = app_dir(name)?;
    let m = load_manifest(&dir, name)?;
    let code = read_app_file(&dir, "main.lua", MAX_CODE)?.ok_or_else(|| format!("agent: {name} has no main.lua"))?;
    let spec = read_app_file(&dir, "spec.txt", 64 << 10)?.unwrap_or_default();
    let saved = read_app_file(&dir, "last_error", 8192)?.unwrap_or_default();
    let (verb, label, user) = match kind {
        Change::Edit => ("editing", "changing code", fill(EDIT_PROMPT, &[("{about}", &memory::block(facts)), ("{request}", text), ("{spec}", &spec), ("{code}", &code)])),
        Change::Fix => {
            let err = [saved.as_str(), &if text.is_empty() { String::new() } else { format!("the user says: {text}") }].iter().filter(|s| !s.is_empty()).map(|s| sanitize(s)).collect::<Vec<_>>().join("\n");
            ("fixing", "fixing code", fill(FIX_PROMPT, &[("{error}", &err), ("{spec}", &spec), ("{code}", &code)]))
        }
    };
    let mut j = job(ch, load_store(&dir), &format!("{verb} {name}"), &format!("agent: {verb} {name} with {} via {} (API use costs money)", ch.model, ch.p.name), &["code", "check", "test run"]);
    let (h, new) = match j.generate(label, user, &spec, false)? { Gen::No(r) => return Err(format!("agent: cannot make that change: {r}")), Gen::App(h, c) => (h, c) };
    if kind == Change::Edit { j.memory_note(&memory::used(facts, &code, &new)); }
    let net = h.net.unwrap_or_else(|| m.net.clone());
    let (title, about) = (h.title.unwrap_or_else(|| m.title.clone()), h.about.unwrap_or_else(|| m.about.clone()));
    let (k, prompt) = if kind == Change::Edit { ("edit", text.to_string()) } else { ("fix", if text.is_empty() { "the last crash".to_string() } else { text.to_string() }) };
    let f = Fields { name, title: &title, about: &about, version: m.version + 1, net: &net, provider: &ch.p.name, model: &ch.model, kind: k, prompt: &prompt };
    let new_spec = if kind == Change::Edit { add_change(&spec, text) } else { spec };
    let v = save(&dir.parent().unwrap_or(&dir), &f, &new, &new_spec, Some(&m))?;
    if kind == Change::Fix { let _ = fs::remove_file(dir.join("last_error")); }
    j.done(name, v);
    Ok(())
}

// ---- commands ----

pub struct Cli { pub p: Option<String>, pub m: Option<String>, pub e: Option<String>, pub words: Vec<String> }

pub fn parse_cli(args: &[String]) -> Result<Cli, String> {
    let mut c = Cli { p: None, m: None, e: None, words: vec![] };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = |n: &str| it.next().cloned().ok_or_else(|| format!("agent: {n} needs a value (agent -h)"));
        match a.as_str() {
            "-p" => c.p = Some(val("-p")?), "-m" => c.m = Some(val("-m")?), "-e" => c.e = Some(val("-e")?),
            "--" => { c.words.extend(it.by_ref().cloned()); }
            s if s.starts_with('-') && s.len() > 1 => return Err(format!("agent: unknown option {} (agent -h)", clean(s))),
            _ => c.words.push(a.clone()),
        }
    }
    if c.m.as_deref().is_some_and(|m| !agent::valid_model(m)) { return Err("agent: the model name has characters agent does not accept".into()); }
    if c.e.as_deref().is_some_and(|e| !EFFORTS.contains(&e)) { return Err(format!("agent: -e must be one of: {}", EFFORTS.join(", "))); }
    Ok(c)
}

pub fn command(c: &str, args: &[String]) -> i32 {
    match c { "run" => run(args), "list" => list(args), "undo" => undo(args), "rm" => rm(args), _ => make(c, args) }
}

fn fail(m: &str) -> i32 { eprintln!("{m}"); 1 }

/// build, edit and fix: the only commands that talk to a provider.
fn make(cmd: &str, args: &[String]) -> i32 {
    let cli = match parse_cli(args) { Ok(c) => c, Err(m) => { eprintln!("{m}"); return 2 } };
    let usage = |u: &str| { eprintln!("usage: agent {u}"); 2 };
    let mut w = cli.words.into_iter();
    let (name, mut text) = match cmd {
        "build" => (None, w.collect::<Vec<_>>().join(" ")),
        _ => match w.next() { Some(n) => (Some(n), w.collect::<Vec<_>>().join(" ")), None => return usage(if cmd == "edit" { "edit APP \"CHANGE\"" } else { "fix APP [\"PROBLEM\"]" }) },
    };
    text = text.trim().to_string();
    if text.chars().count() > MAX_IDEA { return fail("agent: that text is too long"); }
    if let Some(n) = &name {   // cheap checks before any key is read or any money is spent
        let r = app_dir(n).and_then(|(_, d)| load_manifest(&d, n));
        if let Err(m) = r { return fail(&m); }
        if cmd == "fix" && text.is_empty() && !fs::symlink_metadata(app_dir(n).map(|d| d.1.join("last_error")).unwrap_or_default()).is_ok() {
            return fail(&format!("{n} has no saved crash. Describe the problem: agent fix {n} \"the total is wrong\""));
        }
    }
    if text.is_empty() && cmd != "fix" {
        if !tty() { return usage(if cmd == "edit" { "edit APP \"CHANGE\"" } else { "build \"IDEA\"" }); }
        match ask_line(&format!("What should {} be? ", if cmd == "edit" { "the change" } else { "the app" })) { Some(l) if !l.is_empty() => text = l, _ => return 1 }
    }
    let home = match home() { Ok(h) => h, Err(m) => return fail(&m) };
    let facts = match memory::load(&home) { Ok(f) => f, Err(m) => return fail(&m) };   // before any money is spent
    let cfg = match agent::load_config(&home) { Ok((c, _)) => c, Err(m) => return fail(&m) };
    let ch = match agent::ctx_for(&home, &cfg, cli.p.as_deref(), cli.m.as_deref(), cli.e.as_deref(), BUILD_EFFORT) { Ok(c) => c, Err((code, m)) => { eprintln!("{m}"); return code } };
    let name = match name {
        Some(n) => { if let Err(m) = change(&ch, &n, if cmd == "edit" { Change::Edit } else { Change::Fix }, &facts, &text) { return fail(&m); } n }
        None => match build(&ch, &home, &facts, &text) { Ok(n) => n, Err(m) => return fail(&m) },
    };
    if !tty() { return 0; }
    loop {   // open the app; after it, offer another change
        let rc = run(&[name.clone()]);
        if rc != 0 { return rc; }
        println!("\n{}", SUMMARY.lock().unwrap());
        println!("\n  Change something in {name}? {}Type it, or press Enter to finish.{}", progress::DIM, progress::R);
        let Some(l) = ask_line("  change> ") else { return 0 };
        if l.is_empty() { return 0; }
        if let Err(m) = change(&ch, &name, Change::Edit, &facts, &l) { return fail(&m); }
    }
}

pub fn ask_line(prompt: &str) -> Option<String> {
    let mut ed = Editor::new(vec![]);
    term::read_line(&mut ed, prompt, &|_, _| vec![]).map(|l| l.trim().to_string())
}

/// `agent run APP [ARGS]`.
pub fn run(args: &[String]) -> i32 {
    let Some(name) = args.first() else { eprintln!("usage: agent run APP [ARGS]"); return 2 };
    let (home, dir) = match app_dir(name) { Ok(r) => r, Err(m) => return fail(&m) };
    let src = match read_app_file(&dir, "main.lua", MAX_CODE) { Ok(Some(s)) => s, Ok(None) => return fail(&format!("agent: {name} has no main.lua")), Err(m) => return fail(&m) };
    let m = match load_manifest(&dir, name) { Ok(m) => m, Err(m) => return fail(&m) };
    let approvals = match load_approvals(&home) { Ok(t) => t, Err(m) => return fail(&m) };
    let have = approved_for(&approvals, name);
    let new = unapproved(&m, &have);
    if !new.is_empty() {
        if unsafe { libc::isatty(0) } != 1 { return fail(&format!("agent: {name} must be approved first; run it in a terminal")); }
        if !ask(&approval_text(&m, &new, !have.is_empty(), unsafe { libc::isatty(1) } == 1)) { println!("  not allowed; {name} did not run"); return 1; }
        if let Err(e) = save_approvals(&home, &add_approvals(&approvals, name, &new)) { return fail(&e); }
    }
    let data = dir.join("data");
    if let Err(m) = mkdir(&data) { return fail(&m); }
    let allowed = m.net.iter().filter_map(|o| Url::parse(o).ok()).collect();
    app::run(app::Launch { name: m.name, title: m.title, args: args[1..].to_vec(), allowed, dir, src })
}

/// Every app in ~/apps, sorted: its name and its manifest (or why it is broken).
pub fn apps(home: &Path) -> Vec<(String, Result<Manifest, String>)> {
    let dir = home.join("apps");
    let mut names: Vec<String> = fs::read_dir(&dir).map(|rd| rd.flatten().filter_map(|e| e.file_name().into_string().ok()).filter(|n| app_name_ok(n)).collect()).unwrap_or_default();
    names.sort();
    names.into_iter().filter(|n| fs::symlink_metadata(dir.join(n)).is_ok_and(|m| m.is_dir())).map(|n| {
        let m = read_private(&dir.join(&n).join("app.json"), me(), MAX_MANIFEST).map_err(|w| format!("app.json {w}")).and_then(|t| t.ok_or("no app.json".to_string())).and_then(|t| parse_manifest(&t));
        (n, m)
    }).collect()
}

/// The chat's list of installed apps ("" when there are none). It tells the model what exists; it never gets code or data.
pub fn apps_block(home: &Path) -> String {
    let l: Vec<String> = apps(home).into_iter().filter_map(|(n, m)| m.ok().map(|m| format!("- {n} v{}: {}, {}\n", m.version, m.title, m.about))).take(30).collect();
    if l.is_empty() { String::new() } else { format!("===APPS===\nApps installed in ~/apps:\n{}", l.concat()) }
}

/// `agent list`: one line per app.
pub fn list(args: &[String]) -> i32 {
    if !args.is_empty() { eprintln!("usage: agent list"); return 2; }
    let home = match home() { Ok(h) => h, Err(m) => return fail(&m) };
    let all = apps(&home);
    for (n, m) in &all {
        match m {
            Ok(m) => {
                let net = if m.net.is_empty() { "none".to_string() } else { m.net.iter().map(|o| Url::parse(o).map_or(String::new(), |u| u.host_header())).collect::<Vec<_>>().join(", ") };
                println!("{:<9} v{:<2} {:<40} net: {net}", n, m.version, m.about.chars().take(39).collect::<String>());
            }
            Err(e) => println!("{n:<9} (broken: {})", printable(e)),
        }
    }
    if all.is_empty() { println!("no apps yet; try: agent build \"a calculator\""); }
    0
}

fn undo(args: &[String]) -> i32 {
    let [name] = args else { eprintln!("usage: agent undo APP"); return 2 };
    let (_, dir) = match app_dir(name) { Ok(r) => r, Err(m) => return fail(&m) };
    match undo_app(&dir, name, clock::now()) { Ok(v) => { println!("{}", note_line(&format!("{name} is back to version {v}"))); 0 } Err(m) => fail(&m) }
}

fn rm(args: &[String]) -> i32 {
    let [name] = args else { eprintln!("usage: agent rm APP"); return 2 };
    let (home, dir) = match app_dir(name) { Ok(r) => r, Err(m) => return fail(&m) };
    if !confirm(&format!("remove {name}"), "deletes the app, its saved data and every earlier version", "this cannot be undone", "delete", &format!("  Delete {name} and its data? (y/N) ")) { println!("{}", note_line(&format!("kept {name}"))); return 1; }
    if let Err(e) = fs::remove_dir_all(&dir) { return fail(&format!("agent: cannot remove {name}: {e}")); }
    match load_approvals(&home) { Ok(t) if !approved_for(&t, name).is_empty() => { if let Err(m) = save_approvals(&home, &drop_approvals(&t, name)) { return fail(&m); } } Ok(_) => {} Err(m) => return fail(&m) }
    println!("{}", note_line(&format!("removed {name}")));
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    const OLD: &str = r#"{"name":"weather","title":"Weather","about":"Forecast","version":2,"created":1763380000,"net":["https://api.open-meteo.com","http://10.0.2.2:18080"],"approved":["https://api.open-meteo.com","http://10.0.2.2:18080"],"history":[{"v":1,"kind":"build"}]}"#;
    fn tmp(n: &str) -> PathBuf { let d = std::env::temp_dir().join(format!("thir-make-{}-{n}", std::process::id())); let _ = fs::remove_dir_all(&d); fs::create_dir_all(&d).unwrap(); d }
    fn s(v: &[&str]) -> Vec<String> { v.iter().map(|x| x.to_string()).collect() }

    #[test] fn manifest_reads_and_ignores_its_own_approved_list() {
        let m = parse_manifest(OLD).unwrap();
        assert_eq!((m.name.as_str(), m.title.as_str(), m.version, m.net.len()), ("weather", "Weather", 2, 2));
        assert_eq!(unapproved(&m, &[]).len(), 2);   // the "approved" key in app.json counts for nothing
    }
    #[test] fn manifest_rejects_bad_input() {
        for bad in ["[]", "{}", r#"{"name":"A"}"#, r#"{"name":"a","net":["ftp://x.com"]}"#, r#"{"name":"a","net":["https://x.com/path"]}"#, r#"{"name":"a","net":["https://x.com?q=1"]}"#,
            r#"{"name":"a","net":["https://u@x.com"]}"#, r#"{"name":"a","net":"https://x.com"}"#, "not json"] {
            assert!(parse_manifest(bad).is_err(), "{bad}");
        }
        let nine = format!(r#"{{"name":"a","net":[{}]}}"#, vec![r#""https://x.com""#; 9].join(","));
        assert!(parse_manifest(&nine).is_err());
        assert!(parse_manifest(r#"{"name":"a","net":["https://x.com/","http://h:8080"]}"#).is_ok());
    }
    #[test] fn approvals_are_per_app_and_removable() {
        let m = parse_manifest(OLD).unwrap();
        let t = add_approvals("", "weather", &s(&["https://api.open-meteo.com/"]));
        assert_eq!(t, "weather https://api.open-meteo.com\n");
        assert_eq!(unapproved(&m, &approved_for(&t, "weather")), ["http://10.0.2.2:18080"]);   // only the approved origin is skipped
        assert_eq!(unapproved(&m, &approved_for(&t, "other")).len(), 2);                        // app A's approval does not help app B
        let t = add_approvals(&t, "other", &s(&["http://10.0.2.2:18080"]));
        let t = add_approvals(&t, "weather", &s(&["http://10.0.2.2:18080"]));
        assert!(unapproved(&m, &approved_for(&t, "weather")).is_empty());
        let t = drop_approvals(&t, "weather");
        assert_eq!(t, "other http://10.0.2.2:18080\n"); assert_eq!(unapproved(&m, &approved_for(&t, "weather")).len(), 2);
        assert_eq!(approved_for("weather-2 https://a.example\n", "weather"), Vec::<String>::new());
    }
    #[test] fn approval_text_for_new_and_added_hosts() {
        let m = parse_manifest(r#"{"name":"hello","net":["http://10.0.2.2:18080","https://a.example"]}"#).unwrap();
        let t = approval_text(&m, &unapproved(&m, &[]), false, false);
        assert!(t.starts_with("  hello wants to:\n") && t.contains("connect to 10.0.2.2:18080 (not encrypted), a.example\n") && t.ends_with("\n\n  Allow and run? (y/N) "), "{t}");
        let have = s(&["http://10.0.2.2:18080"]);
        let t = approval_text(&m, &unapproved(&m, &have), true, false);
        assert!(t.starts_with("  hello now also wants to:") && !t.contains("10.0.2.2"), "{t}");
        assert!(unapproved(&parse_manifest(r#"{"name":"calc","net":[]}"#).unwrap(), &[]).is_empty());
    }
    #[test] fn approvals_file_round_trip_and_rm_clears_it() {
        let h = tmp("appr"); assert_eq!(load_approvals(&h).unwrap(), "");
        save_approvals(&h, &add_approvals("", "a", &s(&["https://x.example"]))).unwrap();
        assert_eq!(fs::metadata(approvals_path(&h)).unwrap().mode() & 0o777, 0o600);
        let t = load_approvals(&h).unwrap(); assert_eq!(approved_for(&t, "a"), ["https://x.example"]);
        save_approvals(&h, &drop_approvals(&t, "a")).unwrap(); assert_eq!(load_approvals(&h).unwrap(), "");
        fs::set_permissions(approvals_path(&h), std::os::unix::fs::PermissionsExt::from_mode(0o644)).unwrap(); assert!(load_approvals(&h).is_err());
        let _ = fs::remove_dir_all(h);
    }
    #[test] fn canned_manifests_are_valid() {
        for (n, t) in [("calc", include_str!("../../../test/apps/calc.json")), ("hello", include_str!("../../../test/apps/hello.json")), ("crash", include_str!("../../../test/apps/crash.json"))] {
            assert_eq!(parse_manifest(t).unwrap().name, n);
        }
    }
    #[test] fn usage_lines_fit_80_cols() { for l in crate::agent::USAGE.lines() { assert!(l.chars().count() <= 80, "{l}"); } }

    const GOOD: &str = "===APP===\nname: calc\ntitle: Calc\nabout: A calculator\nnet: none\n===CODE===\nfunction draw() end\n===END===\n";
    #[test] fn reply_parses_header_and_code() {
        let Reply::App(h, c) = parse_reply(GOOD, true).unwrap() else { panic!() };
        assert_eq!((h.name.as_deref(), h.title.as_deref(), h.about.as_deref(), h.net), (Some("calc"), Some("Calc"), Some("A calculator"), Some(vec![])));
        assert_eq!(c, "function draw() end\n");
        let fenced = format!("```\n{}```", GOOD.replace("===CODE===\n", "===CODE===\n```lua\n").replace("\n===END", "\n```\n===END"));
        assert_eq!(parse_reply(&fenced, true).unwrap(), parse_reply(GOOD, true).unwrap());
        let Reply::App(_, c) = parse_reply(&GOOD.replace("===END===\n", ""), true).unwrap() else { panic!() }; assert_eq!(c, "function draw() end\n");   // END missing
        let Reply::App(h, _) = parse_reply("===CODE===\nx = 1\n===END===", false).unwrap() else { panic!() }; assert_eq!(h, Header::default());
        let Reply::App(h, _) = parse_reply(&GOOD.replace("none", "https://a.example, http://h:8080/ https://a.example"), true).unwrap() else { panic!() };
        assert_eq!(h.net.unwrap(), ["https://a.example", "http://h:8080"]);
    }
    #[test] fn reply_no_forms_and_errors() {
        for t in ["NO: thir apps cannot play sound.", "no: x", "  No:x", "no", "No."] { assert!(matches!(parse_reply(t, true), Ok(Reply::No(_))), "{t}"); }
        assert_eq!(parse_reply("NO: a\x1b[2Jb", true).unwrap(), Reply::No("a?[2Jb".into()));
        assert!(matches!(parse_reply(&format!("NO: {}", "x".repeat(500)), true), Ok(Reply::No(r)) if r.len() == 200));
        assert!(parse_reply("===APP===\nname: a\n", true).unwrap_err().contains("no ===CODE==="));
        assert!(parse_reply("===CODE===\nx\n", true).unwrap_err().contains("no ===APP==="));
        assert!(parse_reply("===CODE===\n\n===END===", false).is_err());
        assert!(parse_reply(&format!("===CODE===\n{}\n===END===", "x".repeat(70000)), false).unwrap_err().contains("64 KiB"));
        for bad in ["https://x.com/path", "https://x.com?q=1", "ftp://x.com", "https://u@x.com", "x.com"] { assert!(parse_reply(&GOOD.replace("none", bad), true).is_err(), "{bad}"); }
        let nine = (0..9).map(|i| format!("https://h{i}.example")).collect::<Vec<_>>().join(" ");
        assert!(parse_reply(&GOOD.replace("none", &nine), true).is_err());
        let Reply::App(h, _) = parse_reply(&GOOD.replace("name: calc", "name: Bad Name").replace("title: Calc", "title: a\x1bb"), true).unwrap() else { panic!() };
        assert_eq!((h.name, h.title.as_deref()), (None, Some("a?b")));
    }
    #[test] fn spec_parses() {
        let sp = "===APP===\nname: weather\ntitle: Weather\nabout: x\nnet: https://api.open-meteo.com\n===FEATURES===\n- a\n";
        let Spec::Plan(h) = parse_spec(sp).unwrap() else { panic!() };
        assert_eq!((h.name.as_deref(), h.net.unwrap()), (Some("weather"), s(&["https://api.open-meteo.com"])));
        assert_eq!(parse_spec("NO: a recipe is not an app.").unwrap(), Spec::No("a recipe is not an app.".into()));
        assert!(parse_spec("hello").is_err()); assert!(parse_spec("===APP===\nname: A B\n").is_err()); assert!(parse_spec("===APP===\ntitle: x\n").is_err());
    }
    #[test] fn names() {
        let none = |_: &str| false;
        assert_eq!(pick_name("weather", &none).as_deref(), Some("weather"));
        assert_eq!(pick_name("weather", &|n| n == "weather").as_deref(), Some("weather-2"));
        assert_eq!(pick_name("weather", &|n| n == "weather" || n == "weather-2").as_deref(), Some("weather-3"));
        assert_eq!(pick_name("ls", &none).as_deref(), Some("ls-2")); assert_eq!(pick_name("agent", &none).as_deref(), Some("agent-2"));
        assert_eq!(pick_name("abcdefghijklmnopqrst", &|n| n == "abcdefghijklmnopqrst").as_deref(), Some("abcdefghijklmnopqr-2"));
        assert!(pick_name("a", &|_| true).is_none());
        for n in RESERVED { assert!(pick_name(n, &none).unwrap() != *n); }
    }
    #[test] fn template_fill_is_single_pass() {
        assert_eq!(fill("a {x} b {y}", &[("{x}", "{y}"), ("{y}", "2")]), "a {y} b 2");
        for t in [FIX_PROMPT, EDIT_PROMPT] { assert!(t.contains("{spec}") && t.contains("{code}")); }
        assert!(EDIT_PROMPT.contains("{about}"));
    }
    #[test] fn changes_are_appended_and_capped() {
        let a = add_change("===APP===\nname: x\n", "add memory");
        assert!(a.ends_with("===CHANGES===\n- add memory\n"), "{a}");
        let b = add_change(&a, "more"); assert!(b.ends_with("- add memory\n- more\n") && b.matches("===CHANGES===").count() == 1);
        let mut t = a; for i in 0..200 { t = add_change(&t, &format!("{i} {}", "x".repeat(250))); }
        assert!(t.len() <= MAX_SPEC + 400 && t.contains("- 199 ") && !t.contains("- add memory"));
    }
    fn fields<'a>(name: &'a str, v: i64, net: &'a [String]) -> Fields<'a> { Fields { name, title: "T", about: "A", version: v, net, provider: "p", model: "m", kind: if v == 1 { "build" } else { "edit" }, prompt: "do it" } }
    #[test] fn versions_history_cap_and_undo() {
        let apps = tmp("ver"); let dir = apps.join("calc"); let none: Vec<String> = vec![]; let net = s(&["https://a.example"]);
        assert_eq!(save(&apps, &fields("calc", 1, &none), "v1\n", "spec\n", None).unwrap(), 1);
        assert_eq!(fs::metadata(&dir).unwrap().mode() & 0o777, 0o700);
        assert!(undo_app(&dir, "calc", 5).unwrap_err().contains("nothing to undo"));
        let m = load_manifest(&dir, "calc").unwrap(); assert_eq!((m.version, m.net.len()), (1, 0));
        save(&apps, &fields("calc", 2, &net), "v2\n", "spec\n", Some(&m)).unwrap();
        let m = load_manifest(&dir, "calc").unwrap(); assert_eq!((m.version, m.net.clone()), (2, net.clone()));
        assert_eq!(m.raw.get("created"), load_manifest(&dir, "calc").unwrap().raw.get("created"));
        assert_eq!(undo_app(&dir, "calc", 9).unwrap(), 1);
        let m = load_manifest(&dir, "calc").unwrap();
        assert_eq!((m.version, m.net.len(), fs::read_to_string(dir.join("main.lua")).unwrap().as_str()), (1, 0, "v1\n"));   // net comes back from history
        assert!(!dir.join("versions/2.lua").exists() && dir.join("versions/1.lua").exists());
        assert_eq!(m.raw.get("history").and_then(|h| if let Json::Arr(a) = h { a.last() } else { None }).and_then(|e| e.get("kind")).and_then(Json::str), Some("undo"));
        let mut m = m;
        for v in 2..=23 { save(&apps, &fields("calc", v, &none), &format!("v{v}\n"), "s\n", Some(&m)).unwrap(); m = load_manifest(&dir, "calc").unwrap(); }
        let kept = fs::read_dir(dir.join("versions")).unwrap().count(); assert_eq!(kept, 20);
        assert!(!dir.join("versions/3.lua").exists() && dir.join("versions/4.lua").exists() && dir.join("versions/23.lua").exists());
        let hist = if let Some(Json::Arr(a)) = m.raw.get("history") { a.len() } else { 0 }; assert!(hist <= MAX_HISTORY);
        let _ = fs::remove_dir_all(apps);
    }
    #[test] fn dry_run_accepts_canned_replies() {
        for t in [include_str!("../../../test/apps/calc.reply"), include_str!("../../../test/apps/calc2.reply"), include_str!("../../../test/apps/weather.reply")] {
            let Reply::App(_, c) = parse_reply(t, true).unwrap() else { panic!() };
            app::check(&c).unwrap(); app::dry_run(&c, vec![], &TEST_KEYS).unwrap();
        }
        let Reply::App(_, c) = parse_reply(include_str!("../../../test/apps/calc-bad.reply"), true).unwrap() else { panic!() };
        let e = app::dry_run(&c, vec![], &TEST_KEYS).unwrap_err(); assert!(e.contains("after key") && e.contains("main.lua:"), "{e}");
        for t in [include_str!("../../../test/apps/calc.spec"), include_str!("../../../test/apps/weather.spec")] { assert!(matches!(parse_spec(t), Ok(Spec::Plan(_)))); }
    }
    #[test] fn cli_options() {
        let c = parse_cli(&s(&["-p", "mockb", "a", "calculator", "-e", "medium"])).unwrap();
        assert_eq!((c.p.as_deref(), c.e.as_deref(), c.words.join(" ").as_str()), (Some("mockb"), Some("medium"), "a calculator"));
        assert!(parse_cli(&s(&["-p"])).is_err() && parse_cli(&s(&["-x"])).is_err() && parse_cli(&s(&["-e", "huge"])).is_err() && parse_cli(&s(&["-m", "bad model"])).is_err());
    }
}
