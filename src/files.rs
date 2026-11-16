/// Why `rm` must refuse `path`, or None: '/', '.', '..', and paths ending in '/.' or '/..'.
pub fn rm_refuses(path: &str) -> Option<&'static str> {
    let last = path.trim_end_matches('/').rsplit('/').next().unwrap_or("");
    if path.is_empty() { Some("empty path") }
    else if path.bytes().all(|b| b == b'/') { Some("is the root") }
    else if last == "." || last == ".." { Some("'.' and '..' are refused") }
    else { None }
}

/// Control, bidi/format and non-space whitespace characters: they can send escape sequences or make a name display as something else.
pub fn unsafe_char(c: char) -> bool {
    c.is_control() || (c.is_whitespace() && c != ' ') || matches!(c, '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2060}'..='\u{206f}' | '\u{feff}')
}
/// Unsafe characters as '?', so a hostile file name cannot drive the terminal.
pub fn printable(s: &str) -> String { s.chars().map(|c| if unsafe_char(c) { '?' } else { c }).collect() }

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn printable_hides_control_bytes() { assert_eq!(printable("a\x1b[2Jb\n\u{202e}c\u{2028}"), "a?[2Jb??c?"); }
    #[test] fn refused() { for p in ["/", "//", ".", "..", "a/..", "/data/home/x/.", ""] { assert!(rm_refuses(p).is_some(), "{p:?}"); } }
    #[test] fn allowed() { for p in ["a", "a/", "./a", "/data/home/x/docs", "..a"] { assert!(rm_refuses(p).is_none(), "{p:?}"); } }
}
