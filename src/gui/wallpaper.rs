//! A picture wallpaper. Settings (running as the user) decodes the picture in the sandbox to raw BGRA at the screen size and writes ~/.config/thir/wallpaper.raw (0600) plus a one-line header
//! ~/.config/thir/wallpaper. den (root) never parses a picture format: it only reads those two files, with the same care as the settings file, and falls back to the colour wallpaper on any doubt.
use std::{fs::File, io::Read, os::unix::fs::{MetadataExt, OpenOptionsExt}};
use super::wall::blend;

pub const HEADER: &str = ".config/thir/wallpaper"; pub const RAW: &str = ".config/thir/wallpaper.raw";
/// Most pixels a wallpaper may have (an 8K screen); anything bigger is refused before any memory is used.
const MAX_PIXELS: usize = 7680 * 4320;
/// Under a plain shell pane the picture is dimmed to 45% and then covered by the pane colour at this alpha (of 255): the worst case (a white picture) keeps text at AA (tests).
pub const PHOTO_DIM: u32 = 115; pub const PHOTO_ALPHA: u32 = 245;

pub fn header(w: usize, h: usize) -> String { format!("thir-wallpaper 1 {w} {h} {}\n", w * 4) }
/// (w, h) from a header line: version 1 and a stride of exactly w*4, sizes sane.
pub fn parse_header(t: &str) -> Option<(usize, usize)> {
    let v: Vec<&str> = t.split_whitespace().collect();
    let [magic, ver, w, h, stride] = v[..] else { return None };
    let (w, h, stride): (usize, usize, usize) = (w.parse().ok()?, h.parse().ok()?, stride.parse().ok()?);
    (magic == "thir-wallpaper" && ver == "1" && w >= 16 && h >= 16 && w.checked_mul(h)? <= MAX_PIXELS && stride == w * 4).then_some((w, h))
}
/// A file of exactly `len` bytes that is a plain file owned by `owner`, with one link, not writable by others; opened without following a link. None on any doubt.
pub fn read_exact_private(path: &str, owner: u32, len: usize) -> Option<Vec<u8>> {
    let f = File::options().read(true).custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC).open(path).ok()?;
    let m = f.metadata().ok()?;
    if !m.file_type().is_file() || m.uid() != owner || m.nlink() != 1 || m.mode() & 0o022 != 0 || m.len() != len as u64 { return None }
    let mut b = vec![0u8; len];
    f.take(len as u64).read_exact(&mut b).ok()?;
    Some(b)
}
/// The user's picture as 0xAARRGGBB pixels, if both files are valid and the picture is exactly the screen's size (`sw` x `sh`).
pub fn load(home: &str, owner: u32, sw: usize, sh: usize) -> Option<Vec<u32>> {
    let (w, h) = parse_header(&crate::files::read_small(&format!("{home}/{HEADER}"), owner, 256)?)?;
    if (w, h) != (sw, sh) { return None }
    let b = read_exact_private(&format!("{home}/{RAW}"), owner, w * h * 4)?;
    Some(b.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], 0]) & 0xFF_FFFF).collect())
}
/// What a plain shell pane shows over the picture: the picture dimmed, then the pane colour on top.
pub fn tint(pic: &[u32], panel: u32) -> Vec<u32> { pic.iter().map(|&p| blend(blend(p, 0, PHOTO_DIM), panel, 255 - PHOTO_ALPHA)).collect() }
/// "W,H" as the framebuffer's virtual_size file has it.
pub fn parse_size(t: &str) -> Option<(usize, usize)> { let (w, h) = t.trim().split_once(',')?; Some((w.parse().ok()?, h.parse().ok()?)).filter(|&(w, h)| w >= 16 && h >= 16) }

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{symlink, PermissionsExt};
    fn dir(n: &str) -> std::path::PathBuf { let d = std::env::temp_dir().join(format!("thir-wp-{n}-{}", std::process::id())); let _ = std::fs::remove_dir_all(&d); std::fs::create_dir_all(d.join(".config/thir")).unwrap(); d }
    fn put(d: &std::path::Path, w: usize, h: usize, bytes: usize) {
        std::fs::write(d.join(HEADER), header(w, h)).unwrap(); let r = d.join(RAW); std::fs::write(&r, vec![0x40u8; bytes]).unwrap(); std::fs::set_permissions(&r, std::fs::Permissions::from_mode(0o600)).unwrap();
        std::fs::set_permissions(d.join(HEADER), std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    #[test] fn header_round_trip_and_junk() {
        assert_eq!(parse_header(&header(1280, 800)), Some((1280, 800)));
        for bad in ["", "thir-wallpaper 2 10 10 40", "thir-wallpaper 1 1280 800 5000", "thir-wallpaper 1 8 8 32", "thir-wallpaper 1 99999 99999 399996", "x 1 1280 800 5120", "thir-wallpaper 1 1280 800", "thir-wallpaper 1 -1280 800 5120"] { assert_eq!(parse_header(bad), None, "{bad:?}"); }
    }
    #[test] fn a_good_picture_loads_and_a_wrong_screen_does_not() {
        let d = dir("good"); put(&d, 32, 20, 32 * 20 * 4); let me = unsafe { libc::getuid() }; let h = d.to_str().unwrap();
        let p = load(h, me, 32, 20).unwrap(); assert_eq!(p.len(), 640); assert_eq!(p[0], 0x404040);
        assert!(load(h, me, 64, 20).is_none());   // another screen size: the colour wallpaper
        std::fs::remove_dir_all(&d).unwrap();
    }
    #[test] fn wrong_size_owner_link_and_mode_are_refused() {
        let d = dir("bad"); let me = unsafe { libc::getuid() }; let h = d.to_str().unwrap();
        put(&d, 32, 20, 32 * 20 * 4 - 4); assert!(load(h, me, 32, 20).is_none());   // short
        put(&d, 32, 20, 32 * 20 * 4 + 4); assert!(load(h, me, 32, 20).is_none());   // long
        put(&d, 32, 20, 32 * 20 * 4); assert!(load(h, me.wrapping_add(1), 32, 20).is_none());   // not the logged-in user's
        let raw = d.join(RAW); std::fs::set_permissions(&raw, std::fs::Permissions::from_mode(0o666)).unwrap(); assert!(load(h, me, 32, 20).is_none());   // others can write it
        std::fs::set_permissions(&raw, std::fs::Permissions::from_mode(0o600)).unwrap(); assert!(load(h, me, 32, 20).is_some());
        std::fs::hard_link(&raw, d.join("other")).unwrap(); assert!(load(h, me, 32, 20).is_none());   // a second link
        std::fs::remove_file(d.join("other")).unwrap();
        let target = d.join("target"); std::fs::rename(&raw, &target).unwrap(); symlink(&target, &raw).unwrap(); assert!(load(h, me, 32, 20).is_none());   // a symlink
        std::fs::remove_file(&raw).unwrap(); std::fs::create_dir(&raw).unwrap(); assert!(load(h, me, 32, 20).is_none());   // not a file
        std::fs::remove_dir_all(&d).unwrap();
    }
    #[test] fn size_text() { assert_eq!(parse_size("2940,1912\n"), Some((2940, 1912))); assert_eq!(parse_size("x"), None); assert_eq!(parse_size("0,0"), None); }
    #[test] fn text_stays_readable_over_the_worst_picture() {
        use crate::gui::theme::*;
        for (n, _, panel, sel) in WALLS { for white in [0xffffffu32, 0xff0000, 0x00ff00, 0xffff00] {
            let t = tint(&[white], panel)[0];
            for (what, fg) in [("fg", FG), ("dim", DIM), ("accent", ACCENT)] { assert!(contrast(fg, t) >= AA, "{n} {what} over {white:06x}: {:.2}", contrast(fg, t)); }
            let _ = sel;
        } }
    }
}
