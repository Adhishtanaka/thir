//! Where the noise of helper programs goes (WebKit, Mesa, glib, fontconfig, ffmpeg): ~/.cache/thir-web/log, 0600, never the pane.
//! shortcut: a log over CAP bytes starts again from empty at the next open; keep a rotated copy if the old lines are ever needed.
use std::{fs::{DirBuilder, File, OpenOptions}, io, os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt}, path::{Path, PathBuf}};

pub const CAP: u64 = 256 << 10;

pub fn path(home: &Path) -> PathBuf { home.join(".cache/thir-web/log") }

/// Opens `p` for appending (made 0600, in a 0700 folder; a symlink is refused). A file over `cap` bytes is emptied first.
pub fn open_in(p: &Path, cap: u64) -> io::Result<File> {
    if let Some(d) = p.parent() { DirBuilder::new().recursive(true).mode(0o700).create(d)?; }
    let f = OpenOptions::new().append(true).create(true).mode(0o600).custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC).open(p)?;
    if f.metadata()?.size() > cap { f.set_len(0)?; }
    Ok(f)
}

/// The log of `home`; None when it cannot be opened (the caller then sends the noise to /dev/null, never to the pane).
pub fn open(home: &Path) -> Option<File> { open_in(&path(home), CAP).ok() }

/// Points this process's stderr (and so its children's) at the log, or at /dev/null.
pub fn redirect_stderr(home: &Path) {
    let f = open(home).or_else(|| File::options().write(true).open("/dev/null").ok());
    if let Some(f) = f { use std::os::fd::AsRawFd; unsafe { libc::dup2(f.as_raw_fd(), 2); } }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn private_capped_and_no_symlinks() {
        let d = std::env::temp_dir().join(format!("thir-log-{}", std::process::id())); let _ = std::fs::remove_dir_all(&d);
        let p = d.join("a/log");
        { use std::io::Write; let mut f = open_in(&p, 10).unwrap(); f.write_all(b"0123456789abc").unwrap(); }
        assert_eq!(std::fs::metadata(&p).unwrap().mode() & 0o777, 0o600); assert_eq!(std::fs::metadata(p.parent().unwrap()).unwrap().mode() & 0o777, 0o700);
        open_in(&p, 10).unwrap(); assert_eq!(std::fs::metadata(&p).unwrap().len(), 0, "over the cap: emptied");
        let l = d.join("a/link"); std::os::unix::fs::symlink("/dev/null", &l).unwrap(); assert!(open_in(&l, 10).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }
}
