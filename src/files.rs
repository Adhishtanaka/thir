use std::{fs::{self, File, OpenOptions}, io::{self, Read, Write}, os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt}, path::Path};

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

/// A small text file in a user's home that root (den) or the user reads on a timer. Never follow a symlink, never block on a FIFO, regular files only,
/// `owner` must own it with one link (a hard link to someone else's file is refused), at most `max` bytes. None = refused or unreadable.
pub fn read_small(path: &str, owner: u32, max: usize) -> Option<String> {
    let f = File::options().read(true).custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC).open(path).ok()?;
    let m = f.metadata().ok()?;
    if !m.file_type().is_file() || m.uid() != owner || m.nlink() != 1 || m.len() > max as u64 { return None }
    let mut b = vec![];
    f.take(max as u64).read_to_end(&mut b).ok()?;
    String::from_utf8(b).ok()
}

/// Temp file in the same directory, then rename: a crash never leaves half a file. The old mode is kept.
pub fn save(p: &Path, data: &[u8]) -> io::Result<()> {
    let dir = p.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
    // a fresh, unguessable name opened with O_EXCL: a file or link planted at the temp path is never written through
    let (tmp, mut f) = loop {
        let mut r = [0u8; 8];
        crate::user::random(&mut r)?;
        let t = dir.join(format!(".{}.{:016x}.edit-tmp", p.file_name().map_or("file".into(), |n| n.to_string_lossy()), u64::from_ne_bytes(r)));
        match OpenOptions::new().write(true).create_new(true).mode(0o600).custom_flags(libc::O_NOFOLLOW).open(&t) {
            Ok(f) => break (t, f),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    };
    let r = (|| {
        // keep only rwx bits: setuid/setgid/sticky are never carried to the new file
        // only from a regular file: a symlink's own mode is 0777
        if let Ok(m) = fs::symlink_metadata(p) { if m.file_type().is_file() { f.set_permissions(fs::Permissions::from_mode(m.permissions().mode() & 0o777))?; } }
        f.write_all(data)?; f.sync_all()?;
        fs::rename(&tmp, p)?;
        File::open(dir)?.sync_all()
    })();
    if r.is_err() { let _ = fs::remove_file(&tmp); }
    r
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn printable_hides_control_bytes() { assert_eq!(printable("a\x1b[2Jb\n\u{202e}c\u{2028}"), "a?[2Jb??c?"); }
    #[test] fn save_replaces_content_and_keeps_the_mode() {
        let d = std::env::temp_dir().join(format!("thir-files-{}", std::process::id())); let _ = fs::remove_dir_all(&d); fs::create_dir_all(&d).unwrap();
        let p = d.join("f"); fs::write(&p, "old").unwrap(); fs::set_permissions(&p, fs::Permissions::from_mode(0o640)).unwrap();
        save(&p, b"new\n").unwrap();
        assert_eq!(fs::read_to_string(&p).unwrap(), "new\n"); assert_eq!(fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o640);
        assert_eq!(fs::read_dir(&d).unwrap().count(), 1);
        let _ = fs::remove_dir_all(&d);
    }
    #[test] fn refused() { for p in ["/", "//", ".", "..", "a/..", "/data/home/x/.", ""] { assert!(rm_refuses(p).is_some(), "{p:?}"); } }
    #[test] fn allowed() { for p in ["a", "a/", "./a", "/data/home/x/docs", "..a"] { assert!(rm_refuses(p).is_none(), "{p:?}"); } }
}
