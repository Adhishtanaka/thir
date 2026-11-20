//! `!setup`: choose "no AI", or the AI provider and key. Login runs `!setup --if-needed` before every shell: it passes when the AI is off or a provider works.
//! "no AI" is ~/.config/thir/ai = `off`; a finished provider setup removes it.
//! Nothing is saved before the test call passes, or the user keeps the setup untested (not offered after the provider rejected the key).
//! The key goes only through agent::key_for and agent::save_key: 0600, never over plain http, never to another base_url.
use crate::agent::{self, Chosen, Config, Proto, Provider};
use crate::{make, progress::{self, CYAN, DIM, R}};
use thir::user;

const CHOICES: [(&str, &str); 6] = [
    ("anthropic", "Claude \u{b7} needs an API key"), ("openai", "ChatGPT \u{b7} needs an API key"), ("gemini", "Gemini \u{b7} needs an API key"),
    ("deepseek", "DeepSeek \u{b7} needs an API key"), ("ollama", "a local model at 10.0.2.2:11434 \u{b7} no key"), ("custom", "your own URL, anthropic or openai style"),
];

pub fn command(args: &[String]) -> i32 {
    let home = match make::home() { Ok(h) => h, Err(m) => { eprintln!("{m}"); return 1 } };
    match args {
        [] => {}
        [a] if a == "--if-needed" => { if agent::ai_off(&home) || agent::configured(&home).is_ok() { return 0; } }
        _ => { eprintln!("usage: !setup"); return 2; }
    }
    if !make::tty() { eprintln!("agent: setup needs a terminal"); return 1; }
    let text = match agent::load_config(&home) {
        Ok((_, t)) => t,
        Err(m) => {   // a broken file must not lock the user out: keep it as agent.old and start again
            eprintln!("{m}");
            let from = home.join(".config/thir/agent");
            if std::fs::rename(&from, from.with_extension("old")).is_err() { return 1; }
            println!("  the old file is now ~/.config/thir/agent.old");
            String::new()
        }
    };
    run(&home, &text)
}

fn say(s: &str) { println!("  {s}"); }

fn run(home: &std::path::Path, text: &str) -> i32 {
    'pick: loop {
        let w = progress::tw();
        println!("\n{}\n  thir works with or without an AI. API use costs money;\n  a local ollama is free.\n", progress::head("SET UP YOUR AI", "", w));
        println!("{}\n{}", progress::hdr("#  PROVIDER   NOTE"), progress::rule(w));
        println!("  {CYAN}0{R}  {:<9}  {DIM}use thir without AI (turn it on later in settings){R}", "no AI");
        for (i, (n, note)) in CHOICES.iter().enumerate() { println!("  {CYAN}{}{R}  {n:<9}  {DIM}{note}{R}", i + 1); }
        println!("{}", progress::rule(w));
        let n = loop {
            let Some(c) = make::ask_line("  choice [0]: ") else { return 1 };
            match if c.is_empty() { Some(0) } else { c.parse::<usize>().ok().filter(|n| *n <= 6) } { Some(n) => break n, None => say("type a number from 0 to 6") }
        };
        if n == 0 {
            if let Err(m) = agent::set_ai_off(home, true) { eprintln!("{m}"); return 1; }
            println!("{}\n{}", progress::mark(true, "the AI is off. Turn it on any time in settings. Saved keys stay until you remove them."), progress::rule(w));
            return 0;
        }
        let (name, custom) = if n < 6 { (CHOICES[n - 1].0.to_string(), None) } else {
            let name = loop {
                let Some(l) = make::ask_line("  name [mine]: ") else { return 1 };
                let l = if l.is_empty() { "mine".to_string() } else { l };
                if agent::valid_name(&l) && !agent::providers(&Config::default()).iter().any(|p| p.name == l) { break l; }
                say("use a-z, 0-9, _ and -, not a built-in name");
            };
            let proto = loop {
                let Some(l) = make::ask_line("  protocol, anthropic or openai [openai]: ") else { return 1 };
                match l.as_str() { "anthropic" => break Proto::Anthropic, "" | "openai" => break Proto::OpenAi, _ => say("type anthropic or openai") }
            };
            let url = loop {
                let Some(u) = make::ask_line("  URL: ") else { return 1 };
                match agent::setup_text(text, Some((&name, proto, &u)), &name, None) { Ok(_) => break u, Err(e) => say(&e) }
            };
            (name, Some((proto, url)))
        };
        let custom = custom.as_ref().map(|(p, u)| (name.as_str(), *p, u.as_str()));
        let Ok(new) = agent::setup_text(text, custom, &name, None) else { return 1 };
        let cfg = agent::parse_config(&new).unwrap_or_default();
        let Some(p) = agent::providers(&cfg).into_iter().find(|p| p.name == name) else { return 1 };
        let model = loop {
            let def = (p.proto == Proto::Anthropic).then(|| p.model.clone().unwrap_or_else(|| "claude-opus-5-5".into()));
            let Some(l) = make::ask_line(&match &def { Some(d) => format!("  model [{d}]: "), None => "  model (see the provider's model list): ".into() }) else { return 1 };
            let m = if l.is_empty() { def.unwrap_or_default() } else { l };
            if agent::valid_model(&m) { break m; }
            say("that model name has characters agent does not accept");
        };
        let Some(mut key) = ask_key(&p) else { return 1 };
        loop {
            match test(&p, &model, key.as_deref()) {
                Ok(()) => { println!("{}", progress::mark(true, &format!("{name} answered"))); break; }
                Err(msg) => {
                    println!("{}", progress::mark(false, msg.strip_prefix("agent: ").unwrap_or(&msg)));
                    let rej = agent::rejected(&msg);
                    println!("  [r] try again   [c] choose another provider{}", if rej { "" } else { "   [k] keep it untested" });
                    let act = loop {
                        let Some(a) = make::ask_line("  \u{203a} ") else { return 1 };
                        match a.as_str() { "r" | "c" => break a, "k" if !rej => break a, _ => {} }
                    };
                    match act.as_str() {
                        "c" => continue 'pick,
                        "k" => { say("saved without a test; agent says so if it fails later"); break; }
                        _ => { if key.is_some() { match ask_key(&p) { Some(k) => key = k, None => return 1 } } }
                    }
                }
            }
        }
        if let Some(k) = &key {
            if let Err(m) = agent::save_key(home, &p, k) { eprintln!("{m}"); return 1; }
            println!("{}", progress::mark(true, "key saved in ~/.config/thir/keys (only you can read it)"));
        }
        let saved = agent::setup_text(text, custom, &name, Some(&model)).and_then(|t| agent::set_default(home, &t)).and_then(|_| agent::set_ai_off(home, false));
        if let Err(m) = saved { eprintln!("{m}"); return 1; }
        println!("{}\n{}", progress::mark(true, &format!("agent will use {name} ({model}). Change it any time: type ! change my AI provider")), progress::rule(w));
        return 0;
    }
}

/// Some(None) = this provider needs no key. None = no input any more.
fn ask_key(p: &Provider) -> Option<Option<String>> {
    if !p.needs_key() { return Some(None); }
    loop {
        let k = user::read_secret(&format!("  API key for {} (typing is hidden): ", p.name)).ok()?;
        if agent::valid_key(&k) { return Some(Some(k)); }
        say("that does not look like an API key (printable characters, no spaces, 8 to 512 long)");
    }
}

/// One small call: Ok when the provider answers.
fn test(p: &Provider, model: &str, key: Option<&str>) -> Result<(), String> {
    let key = agent::key_for(p, &|_| None, key.map(|k| format!("{} {k}", p.base)).as_deref())?;
    let ch = Chosen { p: p.clone(), model: model.to_string(), key, effort: "low".into() };
    println!("  testing {} ({model})\u{2026}", p.name);
    let d = agent::stream(&ch.ctx(), "You check that a connection works. Reply with one word: ok", &[("user", "ok?".into())], &mut |_| {})?;
    if d.text.trim().is_empty() { Err(format!("agent: {} sent an empty answer", p.name)) } else { Ok(()) }
}
