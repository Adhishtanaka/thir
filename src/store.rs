//! Where the apps keep their data: ~/.config/thir/<app>/ (dirs 0700, files 0600), one record per line, fields split by TAB.
//! TAB, newline and backslash inside a field are escaped, so any text is safe. A line that does not parse is skipped, never fatal.
use crate::files::{read_small, save};
use std::{fs::DirBuilder, io, os::unix::fs::DirBuilderExt, path::PathBuf};

const MAX_FILE: usize = 1 << 20;

pub fn esc(s: &str) -> String { s.replace('\\', "\\\\").replace('\t', "\\t").replace('\n', "\\n").replace('\r', "") }
pub fn unesc(s: &str) -> String {
    let (mut o, mut it) = (String::new(), s.chars());
    while let Some(c) = it.next() { if c != '\\' { o.push(c); continue }; match it.next() { Some('t') => o.push('\t'), Some('n') => o.push('\n'), Some(x) => o.push(x), None => {} } }
    o
}
/// One record line from fields.
pub fn join(f: &[&str]) -> String { f.iter().map(|x| esc(x)).collect::<Vec<_>>().join("\t") }
/// The fields of a record line (`n` at least, else None).
pub fn split(line: &str, n: usize) -> Option<Vec<String>> { let v: Vec<String> = line.split('\t').map(unesc).collect(); (v.len() >= n).then_some(v) }

pub fn home() -> String { std::env::var("HOME").unwrap_or_else(|_| "/".into()) }
/// ~/.config/thir/<app>, made when missing (0700).
pub fn dir(app: &str) -> io::Result<PathBuf> {
    let mut p = PathBuf::from(home());
    for part in [".config", "thir", app] { p.push(part); if !p.is_dir() { DirBuilder::new().mode(0o700).create(&p)?; } }
    Ok(p)
}
/// The text of a data file; empty when missing, foreign or oversized.
pub fn read(app: &str, file: &str) -> String { read_small(&format!("{}/.config/thir/{app}/{file}", home()), unsafe { libc::getuid() }, MAX_FILE).unwrap_or_default() }
pub fn write(app: &str, file: &str, text: &str) -> io::Result<()> { save(&dir(app)?.join(file), text.as_bytes()) }
/// Settings are one file for all apps: ~/.config/thir/settings (key=value lines), read by den too.
pub fn settings_path(home: &str) -> String { format!("{home}/.config/thir/settings") }
pub fn read_settings() -> String { read_small(&settings_path(&home()), unsafe { libc::getuid() }, 4096).unwrap_or_default() }
pub fn write_settings(text: &str) -> io::Result<()> { dir("")?; save(std::path::Path::new(&settings_path(&home())), text.as_bytes()) }

/// A record file's next free id.
pub fn next_id(ids: impl Iterator<Item = u32>) -> u32 { ids.max().unwrap_or(0) + 1 }

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn fields_survive_tabs_newlines_and_backslashes() {
        let f = ["a\tb", "line1\nline2", "back\\slash", "", "end\\"];
        let l = join(&f);
        assert!(!l.contains('\n') && l.matches('\t').count() == 4);
        assert_eq!(split(&l, 5).unwrap(), f);
    }
    #[test] fn short_lines_are_refused() { assert!(split("a\tb", 3).is_none()); assert!(split("", 1).is_some()); }
    #[test] fn ids() { assert_eq!(next_id([3, 9, 2].into_iter()), 10); assert_eq!(next_id(std::iter::empty()), 1); }
    #[test] fn files_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let d = std::env::temp_dir().join(format!("thir-store-{}", std::process::id())); let _ = std::fs::remove_dir_all(&d); std::fs::create_dir_all(&d).unwrap();
        std::env::set_var("HOME", &d);
        write("x", "f", "a\n").unwrap();
        assert_eq!(read("x", "f"), "a\n"); assert_eq!(read("x", "none"), "");
        let m = |p: &str| std::fs::metadata(d.join(p)).unwrap().permissions().mode() & 0o777;
        assert_eq!((m(".config/thir/x"), m(".config/thir/x/f")), (0o700, 0o600));
        write_settings("sound=on\n").unwrap(); assert_eq!(read_settings(), "sound=on\n");
        let _ = std::fs::remove_dir_all(d);
    }
}
