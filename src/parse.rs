#[derive(Debug, PartialEq, Default)]
pub struct Cmd { pub argv: Vec<String>, pub redirect: Option<(String, bool)> }   // (path, append)

pub fn parse_line(line: &str) -> Result<Vec<String>, &'static str> { parse_cmd(line).map(|c| c.argv) }

/// Split a command line into argv and an optional `>`/`>>` target. Whitespace separates; "..." and '...' group;
/// backslash escapes the next char outside single quotes; an unquoted `>` ends the token. Unterminated quote => Err.
pub fn parse_cmd(line: &str) -> Result<Cmd, &'static str> {
    let mut cmd = Cmd::default();
    let (mut cur, mut pending) = (String::new(), None::<bool>);   // pending: a `>` waits for its target (append?)
    let (mut in_single, mut in_double, mut have_token) = (false, false, false);
    let mut chars = line.chars().peekable();
    fn done(cmd: &mut Cmd, pending: &mut Option<bool>, tok: String) {
        match pending.take() { Some(a) => cmd.redirect = Some((tok, a)), None => cmd.argv.push(tok) }
    }
    while let Some(c) = chars.next() {
        if in_single {
            if c == '\'' { in_single = false } else { cur.push(c) }
        } else if c == '\\' {
            cur.push(chars.next().ok_or("trailing backslash")?);
            have_token = true;
        } else if in_double {
            if c == '"' { in_double = false } else { cur.push(c) }
        } else if c == '\'' { in_single = true; have_token = true }
        else if c == '"' { in_double = true; have_token = true }
        else if c == '>' {
            if have_token { done(&mut cmd, &mut pending, std::mem::take(&mut cur)); have_token = false }
            if cmd.redirect.is_some() { return Err("multiple redirects"); }
            if pending.is_some() { return Err("missing redirect target"); }
            let append = chars.peek() == Some(&'>');
            if append { chars.next(); }
            pending = Some(append);
        }
        else if c.is_whitespace() {
            if have_token { done(&mut cmd, &mut pending, std::mem::take(&mut cur)); have_token = false }
        } else { cur.push(c); have_token = true }
    }
    if in_single || in_double { return Err("unterminated quote"); }
    if have_token { done(&mut cmd, &mut pending, cur); }
    if pending.is_some() { return Err("missing redirect target"); }
    Ok(cmd)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn v(a: &[&str]) -> Vec<String> { a.iter().map(|s| s.to_string()).collect() }
    #[test] fn empty() { assert_eq!(parse_line("").unwrap(), v(&[])); assert_eq!(parse_line("  \t\n").unwrap(), v(&[])); }
    #[test] fn plain() { assert_eq!(parse_line("  ls  -l /  ").unwrap(), v(&["ls", "-l", "/"])); }
    #[test] fn quotes() { assert_eq!(parse_line("cat \"a b\" 'c d'").unwrap(), v(&["cat", "a b", "c d"])); }
    #[test] fn escape() { assert_eq!(parse_line("echo a\\ b").unwrap(), v(&["echo", "a b"])); }
    #[test] fn unterminated() { assert!(parse_line("\"unterminated").is_err()); }
    fn r(l: &str) -> Cmd { parse_cmd(l).unwrap() }
    #[test] fn redirect() { let c = r("echo hi > f"); assert_eq!(c.argv, v(&["echo", "hi"])); assert_eq!(c.redirect, Some(("f".into(), false))); }
    #[test] fn append() { assert_eq!(r("echo hi >> f").redirect, Some(("f".into(), true))); }
    #[test] fn no_spaces() { let c = r("echo hi>f"); assert_eq!(c.argv, v(&["echo", "hi"])); assert_eq!(c.redirect, Some(("f".into(), false))); }
    #[test] fn quoted_gt() { let c = r("echo \">\""); assert_eq!(c.argv, v(&["echo", ">"])); assert_eq!(c.redirect, None); }
    #[test] fn target_mid_line() { let c = r("cat a > b c"); assert_eq!(c.argv, v(&["cat", "a", "c"])); assert_eq!(c.redirect, Some(("b".into(), false))); }
    #[test] fn missing_target() { assert!(parse_cmd("echo >").is_err()); assert!(parse_cmd("echo > > x").is_err()); }
    #[test] fn multiple() { assert_eq!(parse_cmd("echo > a > b"), Err("multiple redirects")); }
    #[test] fn quoted_target() { assert_eq!(r("echo 'x' > \"my file\"").redirect, Some(("my file".into(), false))); }
}
