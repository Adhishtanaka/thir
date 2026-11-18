//! agent: a terminal chat with an AI provider, over the Anthropic Messages API or the OpenAI-style chat completions API.
//! Security rules: a key goes only to its own provider's base_url over verified TLS, never over plain http, and is never printed.
use crate::conn::{body_err, exchange, Kind};
use std::{fs::{self, DirBuilder, OpenOptions}, io::{Read, Write}, os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt}, path::{Path, PathBuf}, time::Duration};
use thir::{files::unsafe_char, http::{Body, Sse, Url}, json::{self, Json}, line::Editor, term, user};

const SYS: &str = "The user reads your reply in a plain-text terminal, so prefer plain text over Markdown.";
const FALLBACK_MODELS: [&str; 4] = ["claude-opus-5-5", "claude-opus-5", "claude-fable-5-1", "claude-sonnet-5-5"];
pub const EFFORTS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];
const MAX_ANSWER: usize = 4 << 20;
const MAX_CONFIG: u64 = 16 * 1024;
const IDLE: Duration = Duration::from_secs(120);

pub const USAGE: &str = "usage: agent [-p PROVIDER] [-m MODEL] [-e EFFORT] [QUESTION...]
       agent build \"IDEA\"        make a new app with AI, test it and open it
       agent edit APP \"CHANGE\"   change an app (asks what to change if no text)
       agent fix APP [\"PROBLEM\"] fix the last crash, or the problem you describe
       agent run APP [ARGS]      open an app (or just type its name)
       agent list | undo APP | rm APP
       agent remember \"FACT\"     keep a fact for next time, e.g. \"city: Colombo\"
       agent memory | forget N|WORD|all  see or remove what agent remembers
       agent --login PROVIDER | --logout PROVIDER | --providers
       agent --use PROVIDER [-m MODEL]
  agent \"question\"   one answer, then exit;  agent   a chat (/clear, /exit)
  -p  provider: anthropic (default), openai, gemini, deepseek, ollama, or yours
  -m  model; anthropic uses claude-opus-5-5 unless you choose another
  -e  effort for anthropic: low (default), medium, high, xhigh, max
Your own providers go in ~/.config/thir/agent, one per line:
  provider NAME anthropic|openai URL [MODEL]
agent --login PROVIDER asks for the API key without showing it and keeps it in
~/.config/thir/keys (only you can read it). Using an API costs money.
Apps live in ~/apps. They can only draw, use the keys, keep their own data and
reach the web sites you allow. Building costs API money (about 2 to 5 calls).
Memory is in ~/.config/thir/memory (only you can read it). agent sends it to
your AI provider with chats and builds; apps never see it.
";

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Proto { Anthropic, OpenAi }
impl Proto { fn name(self) -> &'static str { match self { Proto::Anthropic => "anthropic", Proto::OpenAi => "openai" } } }

#[derive(Clone, Debug, PartialEq)]
pub struct Provider { pub name: String, pub proto: Proto, pub base: String, pub env: Option<&'static str>, pub model: Option<String> }
impl Provider {
    /// Plain http providers never get a key: that is the rule that stops a key leaving over an unencrypted link.
    pub fn needs_key(&self) -> bool { Url::parse(&self.base).map_or(true, |u| u.https) }
}

const PRESETS: [(&str, Proto, &str, Option<&str>, Option<&str>); 5] = [
    ("anthropic", Proto::Anthropic, "https://api.anthropic.com/v1", Some("ANTHROPIC_API_KEY"), Some("claude-opus-5-5")),
    ("openai", Proto::OpenAi, "https://api.openai.com/v1", Some("OPENAI_API_KEY"), None),
    ("gemini", Proto::OpenAi, "https://generativelanguage.googleapis.com/v1beta/openai", Some("GEMINI_API_KEY"), None),
    ("deepseek", Proto::OpenAi, "https://api.deepseek.com", Some("DEEPSEEK_API_KEY"), None),
    ("ollama", Proto::OpenAi, "http://10.0.2.2:11434/v1", None, None),
];

#[derive(Debug, Default, PartialEq)]
pub struct Config { pub user: Vec<Provider>, pub default: Option<(String, Option<String>)> }

pub fn valid_name(s: &str) -> bool { (1..=32).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-') }
pub fn valid_model(s: &str) -> bool { (1..=128).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_alphanumeric() || b".:/@_-".contains(&b)) }
/// Printable ASCII, no spaces: it goes into a header, so nothing here may split it.
pub fn valid_key(s: &str) -> bool { (8..=512).contains(&s.len()) && s.bytes().all(|b| (0x21..0x7f).contains(&b)) }

pub fn parse_config(text: &str) -> Result<Config, String> {
    let mut c = Config::default();
    for (n, line) in text.lines().enumerate() {
        let bad = |m: &str| format!("config line {}: {m}", n + 1);
        let w: Vec<&str> = line.split_whitespace().collect();
        match w.as_slice() {
            [] => {}
            [c0, ..] if c0.starts_with('#') => {}
            ["provider", name, proto, url, rest @ ..] if rest.len() <= 1 => {
                if !valid_name(name) { return Err(bad("bad provider name (use a-z, 0-9, _ and -)")); }
                if PRESETS.iter().any(|p| p.0 == *name) { return Err(bad("this name is built in; choose another")); }
                if c.user.iter().any(|p| p.name == *name) { return Err(bad("provider defined twice")); }
                let proto = match *proto { "anthropic" => Proto::Anthropic, "openai" => Proto::OpenAi, _ => return Err(bad("protocol must be anthropic or openai")) };
                if url.contains(['?', '#']) || Url::parse(url).is_err() { return Err(bad("bad URL")); }
                let model = match rest.first() { Some(m) if !valid_model(m) => return Err(bad("bad model name")), m => m.map(|m| m.to_string()) };
                c.user.push(Provider { name: name.to_string(), proto, base: url.trim_end_matches('/').to_string(), env: None, model });
            }
            ["default", name, rest @ ..] if rest.len() <= 1 => {
                if !valid_name(name) { return Err(bad("bad provider name")); }
                if rest.first().is_some_and(|m| !valid_model(m)) { return Err(bad("bad model name")); }
                c.default = Some((name.to_string(), rest.first().map(|m| m.to_string())));
            }
            _ => return Err(bad("expected: provider NAME anthropic|openai URL [MODEL]  or  default NAME [MODEL]")),
        }
    }
    Ok(c)
}

pub fn providers(cfg: &Config) -> Vec<Provider> {
    PRESETS.iter().map(|p| Provider { name: p.0.into(), proto: p.1, base: p.2.into(), env: p.3, model: p.4.map(Into::into) }).chain(cfg.user.iter().cloned()).collect()
}

/// (provider, model) for the command line. Err = message for exit 2.
pub fn resolve(cfg: &Config, p: Option<&str>, m: Option<&str>) -> Result<(Provider, String), String> {
    let name = p.or(cfg.default.as_ref().map(|d| d.0.as_str())).unwrap_or("anthropic");
    let prov = providers(cfg).into_iter().find(|x| x.name == name).ok_or_else(|| format!("agent: no provider called {name} (see agent --providers)"))?;
    let saved = cfg.default.as_ref().filter(|d| d.0 == name).and_then(|d| d.1.clone());
    let model = m.map(String::from).or(saved).or_else(|| prov.model.clone()).or_else(|| (prov.proto == Proto::Anthropic).then(|| "claude-opus-5-5".to_string()));   // the anthropic protocol has a default model
    match model {
        Some(model) => Ok((prov, model)),
        None => Err(format!("agent: {name} needs a model: agent -p {name} -m MODEL, or agent --use {name} -m MODEL (see the provider's model list)")),
    }
}

/// THE gate for keys: the only place that decides whether a key is sent. Ok(None) = send none.
pub fn key_for(p: &Provider, env: &dyn Fn(&str) -> Option<String>, saved: Option<&str>) -> Result<Option<String>, String> {
    if !p.needs_key() { return Ok(None); }   // plain http: files and environment are never looked at
    if let Some(v) = p.env {
        if let Some(k) = env(v).filter(|k| !k.is_empty()) {
            return if valid_key(&k) { Ok(Some(k)) } else { Err(format!("agent: ${v} does not look like an API key")) };
        }
    }
    if let Some(s) = saved {
        let (base, key) = s.trim_end().split_once(' ').ok_or_else(|| format!("agent: the saved key for {} is damaged: run agent --login {}", p.name, p.name))?;
        if base != p.base { return Err(format!("agent: the saved key for {} was made for another URL: run agent --login {}", p.name, p.name)); }
        return if valid_key(key) { Ok(Some(key.to_string())) } else { Err(format!("agent: the saved key for {} is damaged: run agent --login {}", p.name, p.name)) };
    }
    Err(format!("agent: no API key for {}: run agent --login {}", p.name, p.name))
}

pub type Msg = (&'static str, String);

pub fn anthropic_body(model: &str, effort: &str, system: &str, msgs: &[Msg]) -> String {
    let fb = if FALLBACK_MODELS.contains(&model) { "\"fallbacks\":\"default\"," } else { "" };
    format!("{{\"model\":{},\"max_tokens\":16000,\"stream\":true,\"output_config\":{{\"effort\":{}}},{fb}\"system\":{},\"messages\":[{}]}}",
        json::quote(model), json::quote(effort), json::quote(system), msgs.iter().map(|(r, t)| format!("{{\"role\":\"{r}\",\"content\":{}}}", json::quote(t))).collect::<Vec<_>>().join(","))
}

pub fn openai_body(model: &str, system: &str, msgs: &[Msg]) -> String {
    let sys = format!("{{\"role\":\"system\",\"content\":{}}}", json::quote(system));
    let rest = msgs.iter().map(|(r, t)| format!("{{\"role\":\"{r}\",\"content\":{}}}", json::quote(t)));
    format!("{{\"model\":{},\"stream\":true,\"messages\":[{}]}}", json::quote(model), std::iter::once(sys).chain(rest).collect::<Vec<_>>().join(","))
}

pub fn headers(p: &Provider, model: &str, key: Option<&str>) -> Vec<(String, String)> {
    let mut h = vec![("content-type".to_string(), "application/json".to_string())];
    match p.proto {
        Proto::Anthropic => {
            if let Some(k) = key { h.push(("x-api-key".into(), k.into())); }
            h.push(("anthropic-version".into(), "2026-10-21".into()));
            if FALLBACK_MODELS.contains(&model) { h.push(("anthropic-beta".into(), "server-side-fallback-2026-07-01".into())); }
        }
        Proto::OpenAi => if let Some(k) = key { h.push(("authorization".into(), format!("Bearer {k}"))); },
    }
    h.push(("accept".into(), "text/event-stream".into()));
    h.push(("user-agent".into(), "thir-agent/0.3".into()));
    h
}

// ---- private files ----

/// A file only its owner can read, or None if it does not exist. Err = why it is refused (finishes "it ...").
pub fn read_private(path: &Path, owner: u32, max: u64) -> Result<Option<String>, String> {
    let f = match OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC).open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) if e.raw_os_error() == Some(libc::ELOOP) => return Err("is a symlink".into()),
        Err(e) => return Err(format!("cannot be read ({e})")),
    };
    let m = f.metadata().map_err(|e| format!("cannot be read ({e})"))?;
    if !m.file_type().is_file() { return Err("is not a regular file".into()); }
    if m.uid() != owner { return Err("is not yours".into()); }
    if m.mode() & 0o077 != 0 { return Err("is readable by others".into()); }
    if m.nlink() != 1 { return Err("has more than one link".into()); }
    if m.len() > max { return Err("is too big".into()); }
    let mut s = String::new();
    f.take(max + 1).read_to_string(&mut s).map_err(|_| "is not text".to_string())?;
    Ok(Some(s))
}

/// ~/.config/thir[/sub...], each part a real directory that is yours and closed to others (~/.config: not writable by others). Made 0700 when missing.
pub fn private_dir(home: &Path, sub: &[&str]) -> Result<PathBuf, String> {
    let me = unsafe { libc::getuid() };
    let mut p = home.to_path_buf();
    for (i, part) in [".config", "thir"].iter().chain(sub.iter()).enumerate() {
        p.push(part);
        if fs::symlink_metadata(&p).is_err() { DirBuilder::new().mode(0o700).create(&p).map_err(|e| format!("agent: cannot make {}: {e}", p.display()))?; }
        let m = fs::symlink_metadata(&p).map_err(|e| format!("agent: {}: {e}", p.display()))?;
        let bad = if i == 0 { 0o022 } else { 0o077 };
        if !m.is_dir() || m.uid() != me || m.mode() & bad != 0 {
            return Err(format!("agent: refusing to use {}: it must be a real directory that only you can write. Move or delete it (rm -r), then try again.", p.display()));
        }
    }
    Ok(p)
}

/// Temp file (0600, new) -> fsync -> rename over `path`.
pub fn write_private(path: &Path, data: &str) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    let _ = fs::remove_file(&tmp);
    let mut f = OpenOptions::new().write(true).create_new(true).mode(0o600).custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC).open(&tmp)?;
    let r = f.write_all(data.as_bytes()).and_then(|_| f.sync_all());
    if r.is_err() { let _ = fs::remove_file(&tmp); r?; }
    fs::rename(&tmp, path).inspect_err(|_| { let _ = fs::remove_file(&tmp); })
}

// ---- answers ----

/// Model text is untrusted: no control or format characters reach the terminal, except newline and tab.
pub fn sanitize(s: &str) -> String { s.chars().filter(|&c| c == '\n' || c == '\t' || !unsafe_char(c)).collect() }
/// One short line from provider text.
pub fn clean(s: &str) -> String { sanitize(s).chars().map(|c| if c == '\n' || c == '\t' { ' ' } else { c }).take(300).collect() }

#[derive(Default, Debug)]
pub struct St { pub stop: Option<String>, pub category: Option<String>, pub model: Option<String>, pub done: bool }

/// One SSE data payload. `emit` gets the (unsanitized) text deltas.
pub fn event(proto: Proto, data: &str, st: &mut St, emit: &mut dyn FnMut(&str)) -> Result<(), String> {
    if proto == Proto::OpenAi && data.trim() == "[DONE]" { st.done = true; return Ok(()); }
    let j = json::parse(data).map_err(|_| "the provider sent data agent cannot read".to_string())?;
    if let Some(m) = error_message(&j) { return Err(m); }
    match proto {
        Proto::Anthropic => match j.get("type").and_then(Json::str) {
            Some("message_start") => st.model = j.get("message").and_then(|m| m.get("model")).and_then(Json::str).map(String::from),
            Some("content_block_delta") => {
                let d = j.get("delta");
                if d.and_then(|d| d.get("type")).and_then(Json::str) == Some("text_delta") { if let Some(t) = d.and_then(|d| d.get("text")).and_then(Json::str) { emit(t); } }
            }
            Some("message_delta") => {
                let d = j.get("delta");
                if let Some(s) = d.and_then(|d| d.get("stop_reason")).and_then(Json::str) { st.stop = Some(s.into()); }
                let cat = |x: Option<&Json>| x.and_then(|x| x.get("stop_details")).and_then(|x| x.get("category")).and_then(Json::str).map(String::from);
                if let Some(c) = cat(d).or_else(|| cat(Some(&j))) { st.category = Some(c); }
            }
            Some("message_stop") => st.done = true,
            _ => {}   // ping, thinking, fallback and unknown blocks are ignored
        },
        Proto::OpenAi => {
            let ch = j.get("choices").and_then(|c| c.idx(0));
            if let Some(t) = ch.and_then(|c| c.get("delta")).and_then(|d| d.get("content")).and_then(Json::str) { emit(t); }
            if let Some(f) = ch.and_then(|c| c.get("finish_reason")).and_then(Json::str) { st.stop = Some(f.into()); }
        }
    }
    Ok(())
}

/// `error.message` of an error body or event; also a Gemini array and a bare string error.
pub fn error_message(j: &Json) -> Option<String> {
    let j = if let Json::Arr(_) = j { j.idx(0)? } else { j };
    match j.get("error")? { Json::Str(s) => Some(s.clone()), e => Some(e.get("message").and_then(Json::str).unwrap_or("unknown error").to_string()) }
}

/// A provider, model and key ready to talk to. The reusable part: `stream` takes a system prompt and messages and hands text back through a callback.
pub struct Ctx<'a> { pub p: &'a Provider, pub model: &'a str, pub key: Option<String>, pub effort: &'a str }

/// How a streamed answer ended.
pub struct Done { pub text: String, pub stop: String, pub category: Option<String>, pub answered_by: Option<String> }

/// What the user is told for an HTTP error status.
pub fn status_msg(p: &Provider, status: u16, retry_after: Option<&str>, msg: &str) -> String {
    let n = &p.name;
    match status {
        300..=399 => "agent: unexpected redirect; not followed".into(),
        401 | 403 if p.needs_key() => format!("agent: {n} rejected the API key (run agent --login {n}){}", if msg.is_empty() { String::new() } else { format!(": {msg}") }),
        429 => format!("agent: rate limited; try again in {} s", retry_after.filter(|r| !r.is_empty() && r.len() < 6 && r.bytes().all(|b| b.is_ascii_digit())).unwrap_or("a few")),
        529 => format!("agent: {n} is overloaded; try again shortly"),
        500..=599 => format!("agent: {n} had a server error ({status}); try again"),
        _ => format!("agent: {n} said: {}", if msg.is_empty() { format!("HTTP {status}") } else { msg.to_string() }),
    }
}

/// Sends `msgs` with `system`; every text delta (already sanitized) goes to `on_text` as it arrives. Err = a message for the user.
pub fn stream(c: &Ctx, system: &str, msgs: &[Msg], on_text: &mut dyn FnMut(&str)) -> Result<Done, String> {
    let (path, body) = match c.p.proto { Proto::Anthropic => ("/messages", anthropic_body(c.model, c.effort, system, msgs)), Proto::OpenAi => ("/chat/completions", openai_body(c.model, system, msgs)) };
    let url = Url::parse(&format!("{}{path}", c.p.base)).map_err(|e| format!("agent: bad URL for {}: {e}", c.p.name))?;
    let (head, framing, reader) = exchange(&url, "POST", &headers(c.p, c.model, c.key.as_deref()), Some(body.as_bytes()), None, Some(IDLE), None).map_err(|e| format!("agent: {}", clean(&e.msg)))?;
    let tls = reader.get_ref().is_tls();
    let mut rd = Body::new(reader, framing);
    let mut buf = [0u8; 4096];
    if !(200..300).contains(&head.status) {
        let mut b = vec![]; let _ = (&mut rd).take(65536).read_to_end(&mut b);
        let msg = json::parse(&String::from_utf8_lossy(&b)).ok().and_then(|j| error_message(&j)).map(|m| clean(&m)).unwrap_or_default();
        return Err(status_msg(c.p, head.status, head.get("retry-after"), &msg));
    }
    let (mut sse, mut evs, mut st, mut answer) = (Sse::default(), vec![], St::default(), String::new());
    'read: loop {
        let n = rd.read(&mut buf).map_err(|e| { let e = body_err(&e, framing, tls); if e.kind == Kind::Timeout { format!("agent: {} did not answer for {} s", c.p.name, IDLE.as_secs()) } else { format!("agent: the answer was cut off ({})", clean(&e.msg)) } })?;
        if n == 0 { break; }
        evs.clear();
        sse.feed(&buf[..n], &mut evs).map_err(|e| format!("agent: {e}"))?;
        for (_, data) in &evs {
            let mut too_big = false;
            event(c.p.proto, data, &mut st, &mut |t| {
                let t = sanitize(t);
                if t.is_empty() { return; }
                if answer.len() + t.len() > MAX_ANSWER { too_big = true; return; }
                answer.push_str(&t); on_text(&t);
            }).map_err(|m| format!("agent: {} said: {}", c.p.name, clean(&m)))?;
            if too_big { return Err("agent: the answer is longer than 4 MiB; stopped".into()); }
            if st.done { break 'read; }
        }
    }
    if !st.done && !(c.p.proto == Proto::OpenAi && st.stop.is_some()) { return Err("agent: the answer was cut off".into()); }
    Ok(Done { text: answer, stop: st.stop.unwrap_or_default(), category: st.category, answered_by: st.model.filter(|m| m != c.model) })
}

/// The chat's system prompt: how to answer, what thir is, the remembered facts and the installed apps (names and descriptions only).
fn system(home: &Path) -> Result<String, String> {
    let facts = crate::memory::load(home)?;
    Ok(format!("{SYS}\nYou run on thir, a tiny operating system. You cannot run commands; tell the user what to type. Typing an app's name opens it. agent build \"IDEA\" makes an app, agent edit APP \"CHANGE\" changes one, agent list lists them, agent remember \"FACT\" keeps a fact.\n{}{}", crate::memory::block(&facts), crate::make::apps_block(home)))
}

/// The chat command: streams to the terminal with a "thinking..." mark, then the notes. Ok(answer) or Err(message).
fn chat(c: &Ctx, home: &Path, msgs: &[Msg]) -> Result<String, String> {
    let system = system(home)?;   // a refused memory file fails the chat, like approvals do
    let mut think = crate::progress::Think::start();
    let mut out = std::io::stdout();
    let r = stream(c, &system, msgs, &mut |t| { think.clear(); let _ = out.write_all(t.as_bytes()); let _ = out.flush(); });
    think.clear();
    let d = r?;
    if !d.text.is_empty() && !d.text.ends_with('\n') { let _ = writeln!(out); }
    if d.stop == "refusal" { return Err(format!("agent: Claude declined this request (category: {})", clean(d.category.as_deref().unwrap_or("unknown")))); }
    if d.stop == "content_filter" { return Err("agent: the provider declined this request".into()); }
    if d.text.is_empty() { return Err(format!("agent: {} sent an empty answer", c.p.name)); }
    if let Some(m) = &d.answered_by { println!("(answered by {})", clean(m)); }
    if matches!(d.stop.as_str(), "max_tokens" | "length" | "model_context_window_exceeded") { println!("agent: the answer hit the length limit and was cut off"); }
    Ok(d.text)
}

fn home() -> PathBuf { PathBuf::from(std::env::var("HOME").ok().filter(|h| h.starts_with('/')).unwrap_or_else(|| { eprintln!("agent: no home directory (HOME is not set)"); std::process::exit(1) })) }
fn me() -> u32 { unsafe { libc::getuid() } }
fn refuse(path: &Path, why: &str) -> String { format!("agent: refusing to read {}: it {why}. Delete it (rm) and run agent --login again.", path.display()) }

/// (config, its text). Missing file = empty config.
pub fn load_config(home: &Path) -> Result<(Config, String), String> {
    let path = home.join(".config/thir/agent");
    let text = read_private(&path, me(), MAX_CONFIG).map_err(|w| refuse(&path, &w))?.unwrap_or_default();
    parse_config(&text).map(|c| (c, text)).map_err(|e| format!("agent: {}: {e}", path.display()))
}

pub fn saved_key(home: &Path, name: &str) -> Result<Option<String>, String> {
    let path = home.join(".config/thir/keys").join(name);
    read_private(&path, me(), 1024).map_err(|w| refuse(&path, &w))
}

fn key_status(home: &Path, p: &Provider) -> String {
    if !p.needs_key() { return "no key needed".into(); }
    if let Some(v) = p.env { if std::env::var(v).is_ok_and(|k| !k.is_empty()) { return format!("key from ${v}"); } }
    match saved_key(home, &p.name) { Ok(Some(s)) if s.trim_end().split_once(' ').is_some_and(|(b, k)| b == p.base && valid_key(k)) => "key saved".into(), Ok(_) => "no key".into(), Err(_) => "key file refused".into() }
}

/// Provider, model and key chosen from the command line: the one path to a key. Err = (exit code, message).
pub struct Chosen { pub p: Provider, pub model: String, pub key: Option<String>, pub effort: String }
impl Chosen { pub fn ctx(&self) -> Ctx<'_> { Ctx { p: &self.p, model: &self.model, key: self.key.clone(), effort: &self.effort } } }
pub fn ctx_for(home: &Path, cfg: &Config, prov: Option<&str>, model: Option<&str>, effort: Option<&str>, default_effort: &str) -> Result<Chosen, (i32, String)> {
    let (p, model) = resolve(cfg, prov, model).map_err(|m| (2, m))?;
    if effort.is_some() && p.proto != Proto::Anthropic { return Err((2, "agent: -e works only with the anthropic protocol".into())); }
    let saved = if p.needs_key() { saved_key(home, &p.name).map_err(|m| (1, m))? } else { None };
    let key = key_for(&p, &|v| std::env::var(v).ok(), saved.as_deref()).map_err(|m| (1, m))?;
    Ok(Chosen { p, model, key, effort: effort.unwrap_or(default_effort).to_string() })
}

fn fail(m: &str) -> i32 { eprintln!("{m}"); 1 }

fn val(it: &mut impl Iterator<Item = String>, n: &str) -> String { it.next().unwrap_or_else(|| { eprintln!("agent: {n} needs a value (agent -h)"); std::process::exit(2) }) }

pub fn main(args: Vec<String>) -> i32 {
    // app commands come first and never reach the key code: running an app reads no key and no provider config
    match args.first().map(String::as_str) {
        Some(c @ ("run" | "list" | "undo" | "rm" | "build" | "edit" | "fix")) => return crate::make::command(c, &args[1..]),
        Some(c @ ("remember" | "forget" | "memory")) => return crate::memory::command(c, &args[1..]),
        _ => {}
    }
    let (mut prov, mut model, mut effort, mut words) = (None, None, None, vec![]);
    let (mut login, mut logout, mut use_p, mut list) = (None, None, None, false);
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-h" | "--help" => { print!("{USAGE}"); return 0; }
            "-p" => prov = Some(val(&mut it, "-p")), "-m" => model = Some(val(&mut it, "-m")), "-e" => effort = Some(val(&mut it, "-e")),
            "--login" => login = Some(val(&mut it, "--login")), "--logout" => logout = Some(val(&mut it, "--logout")), "--use" => use_p = Some(val(&mut it, "--use")),
            "--providers" => list = true,
            "--" => { words.extend(it.by_ref()); }
            s if s.starts_with('-') && s.len() > 1 => { eprint!("agent: unknown option {}\n{USAGE}", clean(s)); return 2; }
            _ => words.push(a),
        }
    }
    if model.as_deref().is_some_and(|m| !valid_model(m)) { eprintln!("agent: the model name has characters agent does not accept"); return 2; }
    if let Some(e) = &effort { if !EFFORTS.contains(&e.as_str()) { eprintln!("agent: -e must be one of: {}", EFFORTS.join(", ")); return 2; } }
    let home = home();
    if let Some(n) = logout {   // works for any valid name, so an old key can always be removed
        if !valid_name(&n) { eprintln!("agent: bad provider name"); return 2; }
        return match fs::remove_file(home.join(".config/thir/keys").join(&n)) { Ok(()) => { println!("removed the saved key for {n}"); 0 } Err(_) => { println!("no saved key for {n}"); 0 } };   // shortcut: unlink is not a secure wipe of the ext4 blocks; fine for a hobby OS
    }
    let (cfg, text) = match load_config(&home) { Ok(c) => c, Err(m) => return fail(&m) };
    if list {
        let def = resolve(&cfg, None, Some("x")).map(|r| r.0.name).unwrap_or_default();
        for p in providers(&cfg) { println!("{:<10} {:<10} {:<56} {}{}", p.name, p.proto.name(), p.base, key_status(&home, &p), if p.name == def { "  [default]" } else { "" }); }
        return 0;
    }
    if let Some(n) = login {
        let Some(p) = providers(&cfg).into_iter().find(|p| p.name == n) else { eprintln!("agent: no provider called {} (see agent --providers)", clean(&n)); return 2 };
        if !p.needs_key() { eprintln!("agent: {n} uses plain http, so agent never sends it a key and never needs one"); return 1; }
        let k = match user::read_secret(&format!("API key for {n} (typing is hidden): ")) { Ok(k) => k, Err(_) => return fail("agent: could not read the key (a terminal is needed)") };
        if !valid_key(&k) { return fail("agent: that does not look like an API key (printable characters, no spaces, 8 to 512 long)"); }
        let dir = match private_dir(&home, &["keys"]) { Ok(d) => d, Err(m) => return fail(&m) };
        return match write_private(&dir.join(&n), &format!("{} {k}\n", p.base)) { Ok(()) => { println!("saved the key for {n} in ~/.config/thir/keys (only you can read it)"); 0 } Err(e) => fail(&format!("agent: cannot save the key: {e}")) };
    }
    if let Some(n) = use_p {
        let (p, m) = match resolve(&cfg, Some(&n), model.as_deref()) { Ok(r) => r, Err(m) => { eprintln!("{m}"); return 2 } };
        let mut out: String = text.lines().filter(|l| l.split_whitespace().next() != Some("default")).map(|l| format!("{l}\n")).collect();
        out += &format!("default {}{}\n", p.name, model.as_ref().map_or(String::new(), |m| format!(" {m}")));
        let dir = match private_dir(&home, &[]) { Ok(d) => d, Err(m) => return fail(&m) };
        return match write_private(&dir.join("agent"), &out) { Ok(()) => { println!("agent will use {} ({m}) from now on", p.name); 0 } Err(e) => fail(&format!("agent: cannot save: {e}")) };
    }
    let ch = match ctx_for(&home, &cfg, prov.as_deref(), model.as_deref(), effort.as_deref(), "low") { Ok(c) => c, Err((code, m)) => { eprintln!("{m}"); return code } };
    let (p, model) = (&ch.p, &ch.model);
    let c = ch.ctx();
    let mut msgs: Vec<Msg> = vec![];
    if !words.is_empty() {
        msgs.push(("user", words.join(" ")));
        return match chat(&c, &home, &msgs) { Ok(_) => 0, Err(m) => fail(&m) };
    }
    println!("agent: {model} via {}. /clear forgets the chat, /exit or Ctrl-D quits.", p.name);
    let mut ed = Editor::new(vec![]);
    loop {
        let Some(l) = term::read_line(&mut ed, "agent> ", &|_, _| vec![]) else { println!(); return 0 };
        let l = l.trim();
        match l {
            "" => continue,
            "/exit" => return 0,
            "/clear" => { msgs.clear(); println!("(chat cleared)"); continue; }
            _ if l.starts_with('/') && !l.contains(' ') => { println!("(unknown command {}; /clear and /exit exist)", clean(l)); continue; }
            _ => {}
        }
        msgs.push(("user", l.to_string()));
        match chat(&c, &home, &msgs) { Ok(a) => msgs.push(("assistant", a)), Err(m) => { eprintln!("{m}"); msgs.pop(); } }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{symlink, PermissionsExt};
    fn tmp(n: &str) -> PathBuf { let d = std::env::temp_dir().join(format!("thir-agent-{}-{n}", std::process::id())); let _ = fs::remove_dir_all(&d); fs::create_dir_all(&d).unwrap(); d }
    fn cfg(t: &str) -> Config { parse_config(t).unwrap() }
    fn prov(c: &Config, n: &str) -> Provider { providers(c).into_iter().find(|p| p.name == n).unwrap() }
    const KEY: &str = "sk-ant-thir-smoke-not-a-key";

    #[test] fn config_good() {
        let c = cfg("# c\nprovider mine openai https://x.example/v1/ m-1\nprovider loc anthropic http://10.0.2.2:18080/a/v1\ndefault mine gpt-x\n");
        assert_eq!(c.user.len(), 2); assert_eq!(c.user[0].base, "https://x.example/v1"); assert_eq!(c.user[0].model.as_deref(), Some("m-1"));
        assert_eq!(c.default, Some(("mine".into(), Some("gpt-x".into()))));
    }
    #[test] fn config_bad() {
        for t in ["provider openai openai https://x/v1", "provider a openai ftp://x", "provider a openai https://x/v1?k=1", "provider A openai https://x", "provider a grpc https://x",
                  "provider a openai https://x\nprovider a openai https://y", "provider a openai https://x bad!model", "nonsense", "default", "provider a openai https://u@x/"] { assert!(parse_config(t).is_err(), "{t}"); }
    }
    #[test] fn resolving() {
        let c = cfg("provider m openai http://10.0.2.2:1/v1 mm\ndefault m other");
        assert_eq!(resolve(&c, None, None).unwrap().1, "other"); assert_eq!(resolve(&c, None, Some("x")).unwrap().1, "x");
        assert_eq!(resolve(&c, Some("m"), None).unwrap().1, "other"); assert_eq!(resolve(&c, Some("anthropic"), None).unwrap().1, "claude-opus-5-5");
        assert!(resolve(&c, Some("openai"), None).unwrap_err().contains("needs a model")); assert!(resolve(&c, Some("zz"), None).is_err());
        assert_eq!(resolve(&Config::default(), None, None).unwrap().0.name, "anthropic");
        assert_eq!(resolve(&cfg("provider a anthropic http://10.0.2.2:1/v1"), Some("a"), None).unwrap().1, "claude-opus-5-5");
    }
    #[test] fn key_gate() {
        let c = cfg("provider loc openai http://10.0.2.2:1/v1 m\nprovider sec openai https://h.example/v1 m");
        let env = |_: &str| Some(KEY.to_string()); let none = |_: &str| None;
        let (a, loc, sec) = (prov(&c, "anthropic"), prov(&c, "loc"), prov(&c, "sec"));
        assert_eq!(key_for(&loc, &env, Some(&format!("http://10.0.2.2:1/v1 {KEY}"))), Ok(None));   // plain http: nothing is ever sent
        assert_eq!(key_for(&prov(&c, "ollama"), &env, None), Ok(None));
        assert_eq!(key_for(&a, &env, Some(&format!("https://api.anthropic.com/v1 other-key-123"))), Ok(Some(KEY.into())));   // env wins
        assert_eq!(key_for(&a, &none, Some(&format!("https://api.anthropic.com/v1 {KEY}\n"))), Ok(Some(KEY.into())));
        assert!(key_for(&a, &none, Some(&format!("https://evil.example/v1 {KEY}"))).is_err());   // another base_url
        assert!(key_for(&a, &none, None).unwrap_err().contains("no API key"));
        assert!(key_for(&sec, &env, None).is_err());   // env is for presets only
        assert_eq!(key_for(&sec, &none, Some(&format!("https://h.example/v1 {KEY}"))), Ok(Some(KEY.into())));
        assert!(key_for(&a, &|_| Some("bad key".into()), None).is_err()); assert!(key_for(&a, &none, Some("https://api.anthropic.com/v1 short")).is_err());
    }
    #[test] fn bodies() {
        let m = [("user", "hi \"x\"\n".to_string())];
        let b = anthropic_body("claude-opus-5-5", "low", SYS, &m);
        assert!(b.contains("\"fallbacks\":\"default\"") && b.contains("\"output_config\":{\"effort\":\"low\"}") && b.contains("\"max_tokens\":16000"));
        for bad in ["temperature", "top_p", "top_k", "thinking"] { assert!(!b.contains(bad), "{bad}"); }
        assert!(json::parse(&b).is_ok());
        assert!(!anthropic_body("claude-haiku-5-5", "low", SYS, &m).contains("fallbacks"));
        let o = openai_body("m", SYS, &m); assert!(json::parse(&o).unwrap().get("messages").unwrap().idx(0).unwrap().get("role").unwrap().str() == Some("system"));
    }
    #[test] fn headers_per_provider() {
        let c = cfg("provider loc anthropic http://10.0.2.2:1/v1");
        let has = |h: &[(String, String)], n: &str| h.iter().any(|(k, _)| k == n);
        let h = headers(&prov(&c, "loc"), "claude-opus-5-5", None);
        assert!(!has(&h, "x-api-key") && !has(&h, "authorization") && has(&h, "anthropic-beta"));
        let h = headers(&prov(&c, "anthropic"), "claude-opus-5-5", Some(KEY)); assert!(has(&h, "x-api-key") && has(&h, "anthropic-version"));
        assert!(!has(&headers(&prov(&c, "anthropic"), "claude-haiku-5-5", Some(KEY)), "anthropic-beta"));
        let h = headers(&prov(&c, "openai"), "m", Some(KEY)); assert!(h.iter().any(|(k, v)| k == "authorization" && v == &format!("Bearer {KEY}")) && !has(&h, "x-api-key"));
    }
    #[test] fn anthropic_events() {
        let mut st = St::default(); let mut got = String::new();
        for d in [r#"{"type":"message_start","message":{"model":"claude-x"}}"#, r#"{"type":"ping"}"#, r#"{"type":"content_block_start","content_block":{"type":"fallback"}}"#,
                  r#"{"type":"content_block_delta","delta":{"type":"signature_delta","signature":"zz"}}"#, r#"{"type":"content_block_delta","delta":{"type":"text_delta","text":"hi \u001b[2J"}}"#,
                  r#"{"type":"message_delta","delta":{"stop_reason":"refusal","stop_details":{"type":"refusal","category":"cyber"}}}"#, r#"{"type":"message_stop"}"#] {
            event(Proto::Anthropic, d, &mut st, &mut |t| got += t).unwrap();
        }
        assert_eq!(sanitize(&got), "hi [2J"); assert_eq!((st.stop.as_deref(), st.category.as_deref(), st.model.as_deref(), st.done), (Some("refusal"), Some("cyber"), Some("claude-x"), true));
        assert_eq!(event(Proto::Anthropic, r#"{"type":"error","error":{"message":"boom"}}"#, &mut St::default(), &mut |_| {}), Err("boom".into()));
        assert!(event(Proto::Anthropic, "nope", &mut St::default(), &mut |_| {}).is_err());
    }
    #[test] fn openai_events() {
        let mut st = St::default(); let mut got = String::new();
        for d in [r#"{"choices":[{"delta":{"role":"assistant"}}]}"#, r#"{"choices":[{"delta":{"reasoning_content":"hm"}}]}"#, r#"{"choices":[{"delta":{"content":"ab"}}]}"#, r#"{"choices":[{"delta":{},"finish_reason":"length"}]}"#, "[DONE]"] {
            event(Proto::OpenAi, d, &mut st, &mut |t| got += t).unwrap();
        }
        assert_eq!((got.as_str(), st.stop.as_deref(), st.done), ("ab", Some("length"), true));
        assert_eq!(error_message(&json::parse(r#"[{"error":{"message":"gem"}}]"#).unwrap()), Some("gem".into()));
        assert_eq!(error_message(&json::parse(r#"{"error":"plain"}"#).unwrap()), Some("plain".into()));
    }
    #[test] fn status_texts() {
        let c = Config::default(); let a = prov(&c, "anthropic"); let o = prov(&c, "ollama");
        assert!(status_msg(&a, 401, None, "bad").contains("run agent --login anthropic")); assert!(status_msg(&o, 401, None, "bad").contains("ollama said: bad"));
        assert!(status_msg(&a, 429, Some("7"), "").contains("try again in 7 s")); assert!(status_msg(&a, 529, None, "").contains("overloaded"));
        assert!(status_msg(&a, 502, None, "").contains("server error (502)")); assert!(status_msg(&a, 302, None, "").contains("not followed"));
        assert!(status_msg(&a, 429, Some("a\x1b[2J"), "").contains("a few"));
    }
    #[test] fn sanitizing() { assert_eq!(sanitize("a\x1b[1mb\u{202e}c\r\n\td"), "a[1mbc\n\td"); assert_eq!(clean("a\nb\x1b"), "a b"); }
    #[test] fn private_reads() {
        let d = tmp("rp"); let f = d.join("k"); fs::write(&f, "x").unwrap(); fs::set_permissions(&f, fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(read_private(&f, me(), 10), Ok(Some("x".into()))); assert_eq!(read_private(&d.join("none"), me(), 10), Ok(None));
        assert!(read_private(&f, me() + 1, 10).is_err()); assert!(read_private(&f, me(), 0).is_err());
        let l = d.join("link"); symlink(&f, &l).unwrap(); assert!(read_private(&l, me(), 10).unwrap_err().contains("symlink"));
        let h = d.join("hard"); fs::hard_link(&f, &h).unwrap(); assert!(read_private(&f, me(), 10).is_err()); fs::remove_file(&h).unwrap();
        fs::set_permissions(&f, fs::Permissions::from_mode(0o644)).unwrap(); assert!(read_private(&f, me(), 10).unwrap_err().contains("others"));
        let fifo = d.join("fifo"); let c = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap(); assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        assert!(read_private(&fifo, me(), 10).is_err());   // must not hang
        let _ = fs::remove_dir_all(d);
    }
    #[test] fn private_writes_and_dirs() {
        let h = tmp("pd"); let dir = private_dir(&h, &["keys"]).unwrap();
        assert_eq!(fs::metadata(&dir).unwrap().mode() & 0o777, 0o700);
        let f = dir.join("n"); write_private(&f, "a b\n").unwrap(); assert_eq!(fs::metadata(&f).unwrap().mode() & 0o777, 0o600);
        write_private(&f, "c\n").unwrap(); assert_eq!(read_private(&f, me(), 10), Ok(Some("c\n".into())));
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap(); assert!(private_dir(&h, &["keys"]).is_err());
        let h2 = tmp("pd2"); let real = tmp("pd2real"); symlink(&real, h2.join(".config")).unwrap(); assert!(private_dir(&h2, &[]).is_err());
        for p in [h, h2, real] { let _ = fs::remove_dir_all(p); }
    }
}
