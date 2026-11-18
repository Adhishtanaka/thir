//! agent memory: facts the user asked agent to keep, one per line in ~/.config/thir/memory (0600, in a 0700 folder).
//! The whole list goes to the user's AI provider inside chat and build prompts. Nothing is saved unless the user types `agent remember`.
//! Apps never see it, and these commands never read a key or the provider config.
//! shortcut: the whole list is sent; split into an index + files when facts get longer than one line.
use crate::{agent::{clean, private_dir, read_private, write_private}, make};
use std::path::{Path, PathBuf};

const MAX_FACTS: usize = 30;
const MAX_LEN: usize = 120;
const MAX_FILE: u64 = 8192;
const HEADER: &str = "# thir memory: one fact per line. Sent to your AI provider when it helps.\n";
const FILLER: [&str; 14] = ["the", "and", "for", "with", "use", "uses", "from", "that", "this", "are", "not", "you", "your", "please"];

fn path(home: &Path) -> PathBuf { home.join(".config/thir/memory") }

/// Facts from the file text: comments and blank lines skipped, each cleaned and cut to 120 characters, at most 30.
pub fn parse(text: &str) -> Vec<String> {
    text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).map(|l| clean(l).trim().chars().take(MAX_LEN).collect::<String>()).filter(|l| !l.is_empty()).take(MAX_FACTS).collect()
}

/// The saved facts (none when the file is missing). Err = the file is refused.
pub fn load(home: &Path) -> Result<Vec<String>, String> {
    let p = path(home);
    read_private(&p, unsafe { libc::getuid() }, MAX_FILE).map(|t| parse(&t.unwrap_or_default())).map_err(|w| format!("agent: refusing to read {}: it {w}. Delete it (rm) to forget everything.", p.display()))
}

pub fn save(home: &Path, facts: &[String]) -> Result<(), String> {
    let dir = private_dir(home, &[])?;
    let text = facts.iter().fold(HEADER.to_string(), |t, f| t + f + "\n");
    write_private(&dir.join("memory"), &text).map_err(|e| format!("agent: cannot save the memory: {e}"))
}

/// The part before ':' of "key: value" (1 to 24 letters, digits, spaces or -), lowercase.
fn key(f: &str) -> Option<String> {
    let k = f.split_once(':')?.0.trim();
    ((1..=24).contains(&k.chars().count()) && k.chars().all(|c| c.is_ascii_alphanumeric() || c == ' ' || c == '-')).then(|| k.to_lowercase())
}

/// A guard, not a guarantee: text that looks like a password or a key is never kept.
pub fn looks_secret(f: &str) -> bool {
    let l = f.to_lowercase();
    ["password", "passwd", "passphrase", "api key", "api_key", "apikey", "secret", "private key", "token"].iter().any(|w| l.contains(w))
        || l.split_whitespace().any(|w| {
            let w = w.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_');
            ["sk-", "ghp_", "gho_", "akia", "aiza"].iter().any(|p| w.starts_with(p)) || (w.len() >= 20 && w.chars().any(|c| c.is_ascii_alphabetic()) && w.chars().any(|c| c.is_ascii_digit()))
        })
}

/// The new fact list and the message to show. Same key as an old fact: replaced.
pub fn add(facts: &[String], new: &str) -> Result<(Vec<String>, String), String> {
    let n = clean(new).trim().to_string();
    if n.is_empty() { return Err("agent: nothing to remember".into()); }
    if n.chars().count() > MAX_LEN { return Err(format!("agent: keep a fact under {MAX_LEN} characters")); }
    if looks_secret(&n) { return Err("agent: that looks like a password or key; agent never remembers those (keys go in agent --login)".into()); }
    let mut v = facts.to_vec();
    if v.iter().any(|f| f.to_lowercase() == n.to_lowercase()) { return Ok((v, format!("already remembered: {n}"))); }
    if let Some(i) = key(&n).and_then(|k| v.iter().position(|f| key(f).as_deref() == Some(&k))) {
        let old = std::mem::replace(&mut v[i], n.clone());
        return Ok((v, format!("updated: {n} (was: {old})")));
    }
    if v.len() >= MAX_FACTS { return Err(format!("agent: memory is full ({MAX_FACTS} facts). Remove one first: agent forget N")); }
    v.push(n.clone());
    Ok((v, format!("remembered: {n}")))
}

/// Without the fact chosen by number, or by a word it contains ("all" empties the list). Returns the rest and the removed text.
pub fn remove(facts: &[String], sel: &str) -> Result<(Vec<String>, String), String> {
    let sel = sel.trim();
    if sel == "all" { return Ok((vec![], "everything".into())); }
    let hits: Vec<usize> = match sel.parse::<usize>() {
        Ok(n) => (1..=facts.len()).filter(|&i| i == n).map(|i| i - 1).collect(),
        Err(_) => facts.iter().enumerate().filter(|(_, f)| f.to_lowercase().contains(&sel.to_lowercase())).map(|(i, _)| i).collect(),
    };
    match hits.as_slice() {
        [] => Err(format!("agent: no fact matches \"{}\" (agent memory lists them)", clean(sel))),
        [i] => { let mut v = facts.to_vec(); let f = v.remove(*i); Ok((v, f)) }
        many => Err(format!("more than one fact has \"{}\":\n{}\nRemove one by number: agent forget N", clean(sel), many.iter().map(|&i| format!("  {:<2} {}", i + 1, facts[i])).collect::<Vec<_>>().join("\n"))),
    }
}

/// The block put before a request: "" when there are no facts.
pub fn block(facts: &[String]) -> String {
    if facts.is_empty() { return String::new(); }
    format!("===ABOUT THE USER===\nFacts the user asked thir to remember. Use one only when it helps.\n{}", facts.iter().map(|f| format!("- {f}\n")).collect::<String>())
}

fn words(s: &str) -> Vec<String> { s.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_lowercase).collect() }

/// The facts that the new text used: every significant word (3+ letters, not filler) of the fact is a whole word of `after`, and `before` did not already have them all.
pub fn used(facts: &[String], before: &str, after: &str) -> Vec<String> {
    let (b, a) = (words(before), words(after));
    facts.iter().filter(|f| {
        let w: Vec<String> = words(f).into_iter().filter(|w| w.chars().count() >= 3 && !FILLER.contains(&w.as_str())).collect();
        !w.is_empty() && w.iter().all(|x| a.contains(x)) && !w.iter().all(|x| b.contains(x))
    }).cloned().collect()
}

fn numbered(facts: &[String]) -> String { facts.iter().enumerate().map(|(i, f)| format!("  {:<2} {f}\n", i + 1)).collect() }

/// `agent remember | memory | forget`: no key and no provider config are read here.
pub fn command(c: &str, args: &[String]) -> i32 {
    let fail = |m: &str| { eprintln!("{m}"); 1 };
    let home = match make::home() { Ok(h) => h, Err(m) => return fail(&m) };
    let facts = match load(&home) { Ok(f) => f, Err(m) => return fail(&m) };
    let text = args.join(" ").trim().to_string();
    match c {
        "memory" => {
            if !args.is_empty() { eprintln!("usage: agent memory"); return 2; }
            if facts.is_empty() { println!("agent remembers nothing yet. Try: agent remember \"city: Colombo\""); return 0; }
            print!("agent remembers (only you can read this):\n{}agent forget N removes one. To change them all: edit ~/.config/thir/memory\n", numbered(&facts));
            0
        }
        "remember" => {
            let text = if !text.is_empty() { text } else if make::tty() { match make::ask_line("What should agent remember? ") { Some(l) if !l.is_empty() => l, _ => return 1 } } else { eprintln!("usage: agent remember \"FACT\""); return 2 };
            let (v, msg) = match add(&facts, &text) { Ok(r) => r, Err(m) => return fail(&m) };
            if v != facts { if let Err(m) = save(&home, &v) { return fail(&m); } }
            println!("{msg}{}", if facts.is_empty() && msg.starts_with("remembered") { "\n(kept in ~/.config/thir/memory; sent to your AI provider when it helps)" } else { "" });
            0
        }
        _ => {
            if text.is_empty() { eprintln!("usage: agent forget N|WORD|all"); return 2; }
            if facts.is_empty() { println!("agent remembers nothing yet."); return 0; }
            if text == "all" && !make::ask(&format!("Forget all {} facts? (y/N) ", facts.len())) { println!("kept everything"); return 1; }
            match remove(&facts, &text) {
                Ok((v, gone)) => { if let Err(m) = save(&home, &v) { return fail(&m); } println!("{}", if text == "all" { "forgot everything".to_string() } else { format!("forgot: {gone}") }); 0 }
                Err(m) => fail(&m),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::{symlink, MetadataExt, PermissionsExt}};
    fn tmp(n: &str) -> PathBuf { let d = std::env::temp_dir().join(format!("thir-mem-{}-{n}", std::process::id())); let _ = fs::remove_dir_all(&d); fs::create_dir_all(&d).unwrap(); d }
    fn s(v: &[&str]) -> Vec<String> { v.iter().map(|x| x.to_string()).collect() }

    #[test] fn parse_skips_comments_cuts_and_cleans() {
        assert_eq!(parse("# h\n\ncity: Colombo\n  units: metric  \n"), s(&["city: Colombo", "units: metric"]));
        assert_eq!(parse(&(0..31).map(|i| format!("f{i}\n")).collect::<String>()).len(), 30);
        assert_eq!(parse(&"x".repeat(200)).remove(0).len(), 120);
        assert_eq!(parse("a\x1b[2Jb"), s(&["a[2Jb"]));
    }
    #[test] fn add_appends_replaces_and_refuses() {
        let (v, m) = add(&[], "city: Colombo").unwrap(); assert_eq!((v.clone(), m.as_str()), (s(&["city: Colombo"]), "remembered: city: Colombo"));
        let (v2, m) = add(&v, "City: Kandy").unwrap(); assert_eq!((v2, m.as_str()), (s(&["City: Kandy"]), "updated: City: Kandy (was: city: Colombo)"));
        assert_eq!(add(&v, "CITY: colombo").unwrap().1, "already remembered: CITY: colombo");
        let (v3, _) = add(&v, "short answers, please").unwrap(); assert_eq!(v3.len(), 2);
        let full: Vec<String> = (0..30).map(|i| format!("k{i}: v")).collect();
        assert!(add(&full, "new thing").unwrap_err().contains("memory is full")); assert!(add(&full, "k3: changed").unwrap().1.starts_with("updated"));
        assert!(add(&[], &"x".repeat(121)).unwrap_err().contains("under 120"));
        assert!(add(&[], "my password is hunter2").unwrap_err().contains("never remembers"));
    }
    #[test] fn remove_by_number_word_all() {
        let f = s(&["city: Colombo", "units: metric", "city of birth: Galle"]);
        assert_eq!(remove(&f, "2").unwrap(), (s(&["city: Colombo", "city of birth: Galle"]), "units: metric".into()));
        assert_eq!(remove(&f, "metric").unwrap().1, "units: metric");
        let e = remove(&f, "city").unwrap_err(); assert!(e.contains("more than one fact has \"city\"") && e.contains("  1  city: Colombo") && e.contains("agent forget N"), "{e}");
        assert!(remove(&f, "zzz").unwrap_err().contains("no fact matches")); assert!(remove(&f, "4").unwrap_err().contains("no fact matches"));
        assert_eq!(remove(&f, "all").unwrap().0.len(), 0);
    }
    #[test] fn secrets_are_spotted() {
        for t in ["sk-ant-thir-smoke-not-a-key", "my password is hunter2", "ghp_abcdefghij", "AKIAIOSFODNN7EXAMPLE", "key abcdefghij0123456789xyz", "my api key is x", "AIzaSyD"] { assert!(looks_secret(t), "{t}"); }
        for t in ["city: Colombo", "units: metric", "short answers, please"] { assert!(!looks_secret(t), "{t}"); }
    }
    #[test] fn block_text() {
        assert_eq!(block(&[]), "");
        assert_eq!(block(&s(&["a: b"])), "===ABOUT THE USER===\nFacts the user asked thir to remember. Use one only when it helps.\n- a: b\n");
    }
    #[test] fn used_facts() {
        let f = s(&["city: Colombo", "units: metric", "name: Adhish", "short answers, please", "to be"]);
        let plan = "name: calc\nstarts with the city Colombo; c changes it\nmetric units";
        assert_eq!(used(&f, "", plan), s(&["city: Colombo", "units: metric"]));
        assert!(used(&f, "", "name: calc\nbasic maths").is_empty());
        assert!(used(&f, "local city = store.get(\"city\", \"Colombo\")", "local city = store.get(\"city\", \"Colombo\")").is_empty());   // already in the old code
        assert_eq!(used(&s(&["city: Colombo"]), "x", "city Colombo"), s(&["city: Colombo"]));
    }
    #[test] fn file_is_private_and_refused_when_not() {
        let h = tmp("perm");
        assert_eq!(load(&h), Ok(vec![]));
        save(&h, &s(&["city: Colombo"])).unwrap();
        let f = h.join(".config/thir/memory");
        assert_eq!(fs::metadata(&f).unwrap().mode() & 0o777, 0o600); assert_eq!(fs::metadata(h.join(".config/thir")).unwrap().mode() & 0o777, 0o700);
        assert_eq!(load(&h), Ok(s(&["city: Colombo"])));
        fs::set_permissions(&f, fs::Permissions::from_mode(0o644)).unwrap(); assert!(load(&h).unwrap_err().contains("readable by others"));
        fs::remove_file(&f).unwrap(); let real = h.join("real"); fs::write(&real, "x: y\n").unwrap(); fs::set_permissions(&real, fs::Permissions::from_mode(0o600)).unwrap();
        symlink(&real, &f).unwrap(); assert!(load(&h).unwrap_err().contains("symlink"));
    }
}
