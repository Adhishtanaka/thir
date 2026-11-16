/// Why `rm` must refuse `path`, or None: '/', '.', '..', and paths ending in '/.' or '/..'.
pub fn rm_refuses(path: &str) -> Option<&'static str> {
    let last = path.trim_end_matches('/').rsplit('/').next().unwrap_or("");
    if path.is_empty() { Some("empty path") }
    else if path.bytes().all(|b| b == b'/') { Some("is the root") }
    else if last == "." || last == ".." { Some("'.' and '..' are refused") }
    else { None }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn refused() { for p in ["/", "//", ".", "..", "a/..", "/data/home/x/.", ""] { assert!(rm_refuses(p).is_some(), "{p:?}"); } }
    #[test] fn allowed() { for p in ["a", "a/", "./a", "/data/home/x/docs", "..a"] { assert!(rm_refuses(p).is_none(), "{p:?}"); } }
}
