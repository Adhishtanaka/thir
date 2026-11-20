//! Finding the data filesystem without udev or blkid: the ext4 superblock label.

/// Label from the 1024-byte ext2/3/4 superblock (the bytes at offset 1024 of the device); None if the magic is missing.
pub fn ext_label(sb: &[u8]) -> Option<&str> {
    if sb.len() < 0x88 || sb[0x38..0x3a] != [0x53, 0xef] { return None; }
    let l = &sb[0x78..0x88];
    std::str::from_utf8(&l[..l.iter().position(|&b| b == 0).unwrap_or(16)]).ok()
}

/// (used, total) kB of the filesystem at `path`.
pub fn usage_kb(path: &str) -> Option<(u64, u64)> {
    let p = std::ffi::CString::new(path).ok()?;
    let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(p.as_ptr(), &mut s) } != 0 { return None }
    let unit = s.f_frsize as u64;
    let total = s.f_blocks as u64 * unit / 1024;
    Some((total.saturating_sub(s.f_bfree as u64 * unit / 1024), total))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sb(label: &[u8]) -> Vec<u8> {
        let mut v = vec![0u8; 1024];
        v[0x38] = 0x53; v[0x39] = 0xef; v[0x78..0x78 + label.len()].copy_from_slice(label); v
    }
    #[test] fn label_with_nul() { assert_eq!(ext_label(&sb(b"thirdata\0")), Some("thirdata")); }
    #[test] fn no_magic() { let mut v = sb(b"thirdata"); v[0x38] = 0; assert_eq!(ext_label(&v), None); }
    #[test] fn full_16_byte_label() { assert_eq!(ext_label(&sb(b"0123456789abcdef")), Some("0123456789abcdef")); }
    #[test] fn short_buffer() { assert_eq!(ext_label(&[0x53; 0x80]), None); }
}
