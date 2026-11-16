/// Split a command line into argv. Whitespace separates; "..." and '...' group;
/// backslash escapes the next char outside single quotes. Unterminated quote => Err.
pub fn parse_line(line: &str) -> Result<Vec<String>, &'static str> {
    let (mut out, mut cur) = (Vec::new(), String::new());
    let (mut in_single, mut in_double, mut have_token) = (false, false, false);
    let mut chars = line.chars();
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
        else if c.is_whitespace() {
            if have_token { out.push(std::mem::take(&mut cur)); have_token = false }
        } else { cur.push(c); have_token = true }
    }
    if in_single || in_double { return Err("unterminated quote"); }
    if have_token { out.push(cur); }
    Ok(out)
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
}
