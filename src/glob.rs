//! `find -name` patterns: `*` (any run, also empty), `?` (one char), everything else literal.
pub fn glob(pat: &str, name: &str) -> bool {
    let (p, n): (Vec<char>, Vec<char>) = (pat.chars().collect(), name.chars().collect());
    let (mut pi, mut ni, mut star) = (0, 0, None);   // star = (pattern index after '*', name index it restarts from)
    while ni < n.len() {
        if pi < p.len() && p[pi] == '*' { star = Some((pi + 1, ni)); pi += 1; }
        else if pi < p.len() && (p[pi] == '?' || p[pi] == n[ni]) { pi += 1; ni += 1; }
        else if let Some((sp, sn)) = star { pi = sp; ni = sn + 1; star = Some((sp, sn + 1)); }
        else { return false }
    }
    p[pi..].iter().all(|&c| c == '*')
}

#[cfg(test)]
mod tests {
    use super::glob;
    #[test] fn star() { assert!(glob("*", "")); assert!(glob("*", "anything")); assert!(glob("*.txt", "a.txt")); assert!(!glob("*.txt", "a.txt2")); assert!(glob("**", "x")); }
    #[test] fn multi() { assert!(glob("a*b*c", "aXbYc")); assert!(glob("a*b*c", "abc")); assert!(!glob("a*b*c", "acb")); assert!(glob("*a*a", "banana")); }
    #[test] fn question() { assert!(glob("?", "é")); assert!(!glob("?", "")); assert!(!glob("?", "ab")); assert!(glob("a?c", "abc")); }
    #[test] fn edge() { assert!(glob("", "")); assert!(!glob("", "a")); assert!(glob("*.rs", ".hidden.rs")); assert!(glob("*", ".bashrc")); assert!(glob("[a]", "[a]")); assert!(!glob("[a]", "a")); }
}
