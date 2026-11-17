//! Apps on disk (~/apps/NAME/main.lua + app.json) and the commands `agent run` and `agent list`.
//! `run` never loads an API key or the agent config: it only reads the app's own files.
use crate::{agent::{read_private, write_private}, app};
use std::{fs, io::BufRead, os::unix::fs::MetadataExt, path::{Path, PathBuf}};
use thir::{files::{app_name_ok, printable}, http::Url, json::{self, Json}};

const MAX_CODE: u64 = 64 << 10;
const MAX_MANIFEST: u64 = 16 << 10;
const MAX_NET: usize = 8;

#[derive(Debug, PartialEq)]
pub struct Manifest { pub name: String, pub title: String, pub about: String, pub version: i64, pub net: Vec<String>, pub approved: Vec<String>, pub raw: Json }

/// An origin the manifest may list: http or https, a host and an optional port, nothing else.
pub fn origin_ok(s: &str) -> Result<Url, String> {
    let u = Url::parse(s).map_err(|e| format!("{s}: {e}"))?;
    if u.path != "/" || s.contains(['?', '#']) { return Err(format!("{s}: an origin has no path or query")); }
    Ok(u)
}

fn origins(j: &Json, key: &str) -> Result<Vec<String>, String> {
    let Some(v) = j.get(key) else { return Ok(vec![]) };
    let Json::Arr(a) = v else { return Err(format!("{key} must be a list")) };
    if a.len() > MAX_NET { return Err(format!("{key} lists more than {MAX_NET} origins")); }
    a.iter().map(|o| { let s = o.str().ok_or_else(|| format!("{key} must list text"))?; origin_ok(s)?; Ok(s.to_string()) }).collect()
}

pub fn parse_manifest(text: &str) -> Result<Manifest, String> {
    let j = json::parse(text).map_err(|e| format!("app.json is not valid JSON ({e})"))?;
    if !matches!(j, Json::Obj(_)) { return Err("app.json must be an object".into()); }
    let name = j.get("name").and_then(Json::str).filter(|n| app_name_ok(n)).ok_or("app.json has no valid name")?.to_string();
    let short = |k: &str, max: usize| printable(j.get(k).and_then(Json::str).unwrap_or("")).chars().take(max).collect::<String>();
    let title = short("title", 30);
    Ok(Manifest { title: if title.is_empty() { name.clone() } else { title }, about: short("about", 120), name, version: j.get("version").and_then(Json::num).unwrap_or(1.0) as i64,
        net: origins(&j, "net")?, approved: origins(&j, "approved")?, raw: j })
}

fn same(a: &str, b: &str) -> bool { matches!((Url::parse(a), Url::parse(b)), (Ok(x), Ok(y)) if x.same_origin(&y)) }

/// The net origins the user has not approved yet.
pub fn unapproved(m: &Manifest) -> Vec<String> { m.net.iter().filter(|n| !m.approved.iter().any(|a| same(a, n))).cloned().collect() }

/// The question before an app first connects (or connects somewhere new).
pub fn approval_text(m: &Manifest, new: &[String]) -> String {
    let hosts: Vec<String> = new.iter().map(|o| Url::parse(o).map_or(printable(o), |u| format!("{}{}", u.host_header(), if u.https { "" } else { " (not encrypted)" }))).collect();
    let (name, also) = (&m.name, if m.approved.is_empty() { "wants to" } else { "now also wants to" });
    format!("{name} {also}:\n  \u{b7} connect to {}\n  \u{b7} keep its own data in ~/apps/{name}/data\nIt cannot read your other files or keys, or run programs.\nAllow and run? (y/N) ", hosts.join(", "))
}

/// app.json with `approved` set to the net list.
pub fn with_approved(m: &Manifest) -> String {
    let Json::Obj(o) = &m.raw else { return String::new() };
    let mut o = o.clone();
    o.retain(|(k, _)| k != "approved");
    o.push(("approved".into(), Json::Arr(m.net.iter().cloned().map(Json::Str).collect())));
    json::dump(&Json::Obj(o))
}

fn home() -> Result<PathBuf, String> {
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

fn ask(question: &str) -> bool {
    print!("{question}");
    let _ = std::io::Write::flush(&mut std::io::stdout());
    let mut l = String::new();
    std::io::stdin().lock().read_line(&mut l).is_ok_and(|n| n > 0) && matches!(l.trim(), "y" | "Y" | "yes")
}

/// `agent run APP [ARGS]`.
pub fn run(args: &[String]) -> i32 {
    let fail = |m: String| { eprintln!("{m}"); 1 };
    let Some(name) = args.first() else { eprintln!("usage: agent run APP [ARGS]"); return 2 };
    if !app_name_ok(name) { return fail(format!("agent: no app called {} (names use a-z, 0-9 and -; agent list shows your apps)", printable(name))); }
    let home = match home() { Ok(h) => h, Err(m) => return fail(m) };
    let dir = home.join("apps").join(name);
    if !fs::symlink_metadata(&dir).is_ok() { return fail(format!("agent: no app called {name} (agent list shows your apps)")); }
    if let Err(m) = trusted_dir(&home.join("apps")).and_then(|_| trusted_dir(&dir)) { return fail(m); }
    let me = unsafe { libc::getuid() };
    let refuse = |f: &str, why: String| fail(format!("agent: refusing to read {}: it {why}", dir.join(f).display()));
    let src = match read_private(&dir.join("main.lua"), me, MAX_CODE) { Ok(Some(s)) => s, Ok(None) => return fail(format!("agent: {name} has no main.lua")), Err(w) => return refuse("main.lua", w) };
    let text = match read_private(&dir.join("app.json"), me, MAX_MANIFEST) { Ok(Some(s)) => s, Ok(None) => return fail(format!("agent: {name} has no app.json")), Err(w) => return refuse("app.json", w) };
    let m = match parse_manifest(&text) { Ok(m) if m.name == *name => m, Ok(_) => return fail(format!("agent: {name}/app.json belongs to another app")), Err(e) => return fail(format!("agent: {name}: {e}")) };
    let new = unapproved(&m);
    if !new.is_empty() {
        if unsafe { libc::isatty(0) } != 1 { return fail(format!("agent: {name} must be approved first; run it in a terminal")); }
        if !ask(&approval_text(&m, &new)) { println!("not allowed; {name} did not run"); return 1; }
        if let Err(e) = write_private(&dir.join("app.json"), &with_approved(&m)) { return fail(format!("agent: cannot save the approval: {e}")); }
    }
    let data = dir.join("data");
    if fs::symlink_metadata(&data).is_err() { if let Err(e) = std::os::unix::fs::DirBuilderExt::mode(&mut fs::DirBuilder::new(), 0o700).create(&data) { return fail(format!("agent: cannot make {}: {e}", data.display())); } }
    if let Err(m) = trusted_dir(&data) { return fail(m); }
    let allowed = m.net.iter().filter_map(|o| Url::parse(o).ok()).collect();
    app::run(app::Launch { name: m.name, title: m.title, args: args[1..].to_vec(), allowed, dir, src })
}

/// `agent list`: one line per app.
pub fn list(args: &[String]) -> i32 {
    if !args.is_empty() { eprintln!("usage: agent list"); return 2; }
    let home = match home() { Ok(h) => h, Err(m) => { eprintln!("{m}"); return 1 } };
    let dir = home.join("apps");
    let mut names: Vec<String> = fs::read_dir(&dir).map(|rd| rd.flatten().filter_map(|e| e.file_name().into_string().ok()).filter(|n| app_name_ok(n)).collect()).unwrap_or_default();
    names.sort();
    let me = unsafe { libc::getuid() };
    let mut shown = 0;
    for n in names {
        if !fs::symlink_metadata(dir.join(&n)).is_ok_and(|m| m.is_dir()) { continue; }
        shown += 1;
        match read_private(&dir.join(&n).join("app.json"), me, MAX_MANIFEST).map_err(|w| format!("app.json {w}")).and_then(|t| t.ok_or("no app.json".to_string())).and_then(|t| parse_manifest(&t)) {
            Ok(m) => {
                let net = if m.net.is_empty() { "none".to_string() } else { m.net.iter().map(|o| Url::parse(o).map_or(String::new(), |u| u.host_header())).collect::<Vec<_>>().join(", ") };
                println!("{:<9} v{:<2} {:<40} net: {net}", n, m.version, m.about.chars().take(40).collect::<String>());
            }
            Err(e) => println!("{n:<9} (broken: {})", printable(&e)),
        }
    }
    if shown == 0 { println!("no apps yet (they live in ~/apps)"); }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    const OLD: &str = r#"{"name":"weather","title":"Weather","about":"Forecast","version":2,"created":1763380000,"net":["https://api.open-meteo.com","http://10.0.2.2:18080"],"approved":["https://api.open-meteo.com"],"history":[{"v":1,"kind":"build"}]}"#;

    #[test] fn manifest_reads_and_keeps_everything_on_approval() {
        let m = parse_manifest(OLD).unwrap();
        assert_eq!((m.name.as_str(), m.title.as_str(), m.version, m.net.len(), m.approved.len()), ("weather", "Weather", 2, 2, 1));
        assert_eq!(unapproved(&m), ["http://10.0.2.2:18080"]);
        let back = parse_manifest(&with_approved(&m)).unwrap();
        assert!(unapproved(&back).is_empty());
        assert_eq!(back.raw.get("history"), m.raw.get("history")); assert_eq!(back.raw.get("created"), m.raw.get("created"));
    }
    #[test] fn manifest_rejects_bad_input() {
        for bad in ["[]", "{}", r#"{"name":"A"}"#, r#"{"name":"a","net":["ftp://x.com"]}"#, r#"{"name":"a","net":["https://x.com/path"]}"#, r#"{"name":"a","net":["https://x.com?q=1"]}"#,
            r#"{"name":"a","net":["https://u@x.com"]}"#, r#"{"name":"a","net":"https://x.com"}"#, r#"{"name":"a","approved":[1]}"#, "not json"] {
            assert!(parse_manifest(bad).is_err(), "{bad}");
        }
        let nine = format!(r#"{{"name":"a","net":[{}]}}"#, vec![r#""https://x.com""#; 9].join(","));
        assert!(parse_manifest(&nine).is_err());
        assert!(parse_manifest(r#"{"name":"a","net":["https://x.com/","http://h:8080"]}"#).is_ok());
    }
    #[test] fn approval_diff_and_text() {
        let m = parse_manifest(r#"{"name":"hello","net":["http://10.0.2.2:18080","https://a.example"],"approved":[]}"#).unwrap();
        assert_eq!(unapproved(&m).len(), 2);
        let t = approval_text(&m, &unapproved(&m));
        assert!(t.starts_with("hello wants to:\n") && t.contains("connect to 10.0.2.2:18080 (not encrypted), a.example\n") && t.ends_with("Allow and run? (y/N) "), "{t}");
        let m = parse_manifest(OLD).unwrap();
        let t = approval_text(&m, &unapproved(&m));
        assert!(t.starts_with("weather now also wants to:") && !t.contains("open-meteo"), "{t}");
        let calc = parse_manifest(r#"{"name":"calc","net":[]}"#).unwrap(); assert!(unapproved(&calc).is_empty());
        let shrunk = parse_manifest(r#"{"name":"a","net":["https://a.example"],"approved":["https://a.example/","https://b.example"]}"#).unwrap(); assert!(unapproved(&shrunk).is_empty());
    }
    #[test] fn canned_manifests_are_valid() {
        for (n, t) in [("calc", include_str!("../../../test/apps/calc.json")), ("hello", include_str!("../../../test/apps/hello.json")), ("crash", include_str!("../../../test/apps/crash.json"))] {
            assert_eq!(parse_manifest(t).unwrap().name, n);
        }
    }
    #[test] fn usage_lines_fit_80_cols() { for l in crate::agent::USAGE.lines() { assert!(l.chars().count() <= 80, "{l}"); } }
}
