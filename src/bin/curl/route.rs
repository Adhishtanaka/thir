//! `!ask "TEXT"`: plain words typed at the shell. The AI picks ONE action from a fixed list; thir shows it and does it only after the user agrees.
//! Security rules: the model's reply is untrusted. It can only choose a verb (it never writes a command, a path or code that runs); an app name must pass
//! app_name_ok and exist; a fact passes memory::add; a time passes parse_when. BUILD, EDIT, FIX and DO get the line the user typed, not the model's words.
//! There is no delete or run-a-program verb. This file starts no programs: actions are calls into make, act, memory and schedule.
use crate::{agent::{clean, ctx_for, load_config, sanitize, stream}, context, make, memory, progress::{self, fit, ACC, CYAN, DIM, R}, schedule};
use thir::{clock, files::{app_name_ok, printable}};

const PROMPT: &str = include_str!("prompts/route.txt");
const MAX_LINE: usize = 1000;
const MAX_FACT: usize = 120;

#[derive(Debug, PartialEq)]
pub enum Pick { Answer(String), Build, Run(String), Edit(String), Fix(String), Do, Remember(String), Remind(String) }

/// The first non-empty line is `KEYWORD [argument]`; ANSWER's text is the rest. Extra lines after any other verb are ignored.
pub fn parse_pick(text: &str) -> Result<Pick, String> {
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim_start().starts_with("```")).collect();
    let at = lines.iter().position(|l| !l.trim().is_empty()).ok_or("the AI sent an empty answer")?;
    let (kw, arg) = lines[at].trim().split_once(char::is_whitespace).map_or((lines[at].trim(), ""), |(k, a)| (k, a.trim()));
    let app = |a: &str| if app_name_ok(a) { Ok(a.to_string()) } else { Err(format!("\"{}\" is not an app name", clean(a))) };
    match kw {
        "ANSWER" => {
            let body = std::iter::once(arg).chain(lines[at + 1..].iter().copied()).collect::<Vec<_>>().join("\n");
            let body = sanitize(body.trim());
            if body.is_empty() { Err("the AI sent an empty answer".into()) } else { Ok(Pick::Answer(body)) }
        }
        "BUILD" => Ok(Pick::Build),
        "DO" => Ok(Pick::Do),
        "RUN" => app(arg).map(Pick::Run),
        "EDIT" => app(arg).map(Pick::Edit),
        "FIX" => app(arg).map(Pick::Fix),
        "REMEMBER" | "REMIND" => {
            let a = clean(arg).trim().to_string();
            if a.is_empty() { return Err(format!("{kw} has no text")); }
            if a.chars().count() > MAX_FACT { return Err(format!("{kw} text is too long")); }
            Ok(if kw == "REMEMBER" { Pick::Remember(a) } else { Pick::Remind(a) })
        }
        _ => Err(format!("unknown action {}", clean(&kw.chars().take(20).collect::<String>()))),
    }
}

/// Printed as the answer streams in: two spaces at the start of every line.
struct Indent { bol: bool }
impl Indent {
    fn put(&mut self, t: &str) {
        let mut o = String::new();
        for c in t.chars() { if self.bol && c != '\n' { o.push_str("  "); } self.bol = c == '\n'; o.push(c); }
        print!("{o}");
        let _ = std::io::Write::flush(&mut std::io::stdout());
    }
}

fn fail(m: &str) -> i32 { eprintln!("{m}"); 1 }
fn ask(label: &str, what: &str, cost: &str) -> bool {
    make::ask_yes(&format!("  {CYAN}\u{2192}{R} {ACC}{label}{R} {what}{}  ok? {DIM}[Y/n]{R} ", if cost.is_empty() { String::new() } else { format!(" {DIM}\u{b7} {cost}{R}") }))
}

pub fn command(args: &[String]) -> i32 {
    let cli = match make::parse_cli(args) { Ok(c) => c, Err(m) => return fail(&m) };
    let line = cli.words.join(" ").trim().to_string();
    if line.is_empty() { eprintln!("usage: !ask \"TEXT\""); return 2; }
    if line.chars().count() > MAX_LINE { return fail("agent: that line is too long"); }
    let home = match make::home() { Ok(h) => h, Err(m) => return fail(&m) };
    let facts = match memory::load(&home) { Ok(f) => f, Err(m) => return fail(&m) };
    let cfg = match load_config(&home) { Ok((c, _)) => c, Err(m) => return fail(&m) };
    let ch = match ctx_for(&home, &cfg, cli.p.as_deref(), cli.m.as_deref(), cli.e.as_deref(), "low") { Ok(c) => c, Err((code, m)) => { eprintln!("{m}"); return code } };
    let msg = format!("{}{}===LINE===\n{line}", memory::block(&facts), context::block(&home, &std::env::current_dir().unwrap_or_else(|_| home.clone())));
    let (mut think, mut head, mut streaming, mut out) = (progress::Think::start(), String::new(), false, Indent { bol: true });
    let r = stream(&ch.ctx(), PROMPT, &[("user", msg)], &mut |t| {
        if streaming { out.put(t); return; }
        head.push_str(t);
        if let Some(i) = head.find('\n') {
            if head[..i].trim() == "ANSWER" { think.clear(); streaming = true; println!("  {DIM}\u{2192} answer{R}"); let rest = head[i + 1..].to_string(); out.put(&rest); }
        }
    });
    think.clear();
    let d = match r { Ok(d) => d, Err(m) => return fail(&m) };
    if d.stop == "refusal" { return fail(&format!("agent: Claude declined this request (category: {})", clean(d.category.as_deref().unwrap_or("unknown")))); }
    if d.stop == "content_filter" { return fail("agent: the provider declined this request"); }
    if streaming { if !d.text.ends_with('\n') { println!(); } return 0; }
    let pick = match parse_pick(&d.text) { Ok(p) => p, Err(e) => return fail(&format!("agent: could not tell what to do ({e}). Nothing was done. Try other words, or start with a command.")) };
    act(&cli.flags(), &home, &line, pick)
}

impl make::Cli {
    /// The -p, -m and -e the user gave, to pass on to the command that does the work.
    fn flags(&self) -> Vec<String> {
        [("-p", &self.p), ("-m", &self.m), ("-e", &self.e)].into_iter().filter_map(|(f, v)| v.as_ref().map(|v| [f.to_string(), v.clone()])).flatten().collect()
    }
}

/// Shows the chosen action and, after a yes, does it. The user's own line goes to the commands that take text.
fn act(flags: &[String], home: &std::path::Path, line: &str, pick: Pick) -> i32 {
    let what = fit(&printable(line), 50);
    let with = |extra: &[&str]| flags.iter().cloned().chain(["--".to_string()]).chain(extra.iter().map(|s| s.to_string())).collect::<Vec<_>>();
    let has_app = |a: &str| if make::apps(home).iter().any(|x| x.0 == a) { Ok(()) } else { Err(format!("agent: no app called {a} (!list shows your apps)")) };
    let no = || { println!("  nothing done."); 0 };
    match pick {
        Pick::Answer(t) => { println!("  {DIM}\u{2192} answer{R}"); Indent { bol: true }.put(&format!("{t}\n")); 0 }
        Pick::Build => if ask("build:", &what, "2 to 5 API calls") { make::command("build", &with(&[line])) } else { no() },
        Pick::Run(a) => { if let Err(m) = has_app(&a) { return fail(&m); } if ask("open:", &a, "") { make::run(&[a]) } else { no() } }
        Pick::Edit(a) => { if let Err(m) = has_app(&a) { return fail(&m); } if ask(&format!("edit {a}:"), &what, "about 2 API calls") { make::command("edit", &with(&[&a, line])) } else { no() } }
        Pick::Fix(a) => { if let Err(m) = has_app(&a) { return fail(&m); } if ask(&format!("fix {a}:"), &what, "about 2 API calls") { make::command("fix", &with(&[&a, line])) } else { no() } }
        Pick::Do => if ask("do:", &what, "1 or 2 API calls, then you see the plan") { crate::act::command(&with(&[line])) } else { no() },
        Pick::Remember(f) => {
            let facts = match memory::load(home) { Ok(f) => f, Err(m) => return fail(&m) };
            if let Err(m) = memory::add(&facts, &f) { return fail(&m); }
            if ask("remember:", &f, "") { memory::command("remember", &[f]) } else { no() }
        }
        Pick::Remind(s) => {
            let (now, off) = (clock::now(), clock::home_tz().unwrap_or(0));
            let w: Vec<&str> = s.split_whitespace().collect();
            let (when, n) = match schedule::parse_when(&w, now, off) { Ok(r) => r, Err(m) => return fail(&m) };
            let text = match schedule::clean_text(&w[n..].join(" ")) { Ok(t) => t, Err(m) => return fail(&m) };
            if ask("remind:", &format!("{} \u{b7} {text}", schedule::describe(when, now, off)), "") { schedule::command("remind", &w.iter().map(|s| s.to_string()).collect::<Vec<_>>()) } else { no() }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn s(x: &str) -> String { x.to_string() }
    #[test] fn every_verb_parses() {
        assert_eq!(parse_pick("BUILD"), Ok(Pick::Build)); assert_eq!(parse_pick("  DO \nthe rest is ignored"), Ok(Pick::Do));
        assert_eq!(parse_pick("RUN calc"), Ok(Pick::Run(s("calc")))); assert_eq!(parse_pick("EDIT todo\nmore"), Ok(Pick::Edit(s("todo")))); assert_eq!(parse_pick("FIX weather"), Ok(Pick::Fix(s("weather"))));
        assert_eq!(parse_pick("REMEMBER city: Colombo"), Ok(Pick::Remember(s("city: Colombo"))));
        assert_eq!(parse_pick("REMIND daily 07:00 open weather"), Ok(Pick::Remind(s("daily 07:00 open weather"))));
        assert_eq!(parse_pick("ANSWER\nline one\nline two"), Ok(Pick::Answer(s("line one\nline two")))); assert_eq!(parse_pick("ANSWER mock: x"), Ok(Pick::Answer(s("mock: x"))));
        assert_eq!(parse_pick("```\nRUN calc\n```"), Ok(Pick::Run(s("calc"))));   // a fence is not part of the answer
        assert_eq!(parse_pick("ANSWER\na\x1b[2Jb"), Ok(Pick::Answer(s("a[2Jb"))));
    }
    #[test] fn bad_replies_are_refused() {
        for t in ["RUN ../x", "RUN calc; rm -rf ~", "RUN ../../bin/sh", "RUN", "RUN Calc", "EDIT /bin/sh", "FIX a b", "EXEC /bin/sh", "run calc", "REMEMBER", "REMIND  ", "ANSWER", "ANSWER\n \n", "", "  \n", "sure! BUILD", &format!("REMEMBER {}", "x".repeat(121))] {
            assert!(parse_pick(t).is_err(), "{t:?}");
        }
        assert_eq!(parse_pick("EXEC /bin/sh"), Err(s("unknown action EXEC")));
    }
    #[test] fn flags_pass_on() {
        let c = make::parse_cli(&[s("-p"), s("mockb"), s("-e"), s("high"), s("--"), s("-x")]).unwrap();
        assert_eq!(c.flags(), ["-p", "mockb", "-e", "high"]); assert_eq!(c.words, ["-x"]);
    }
}
