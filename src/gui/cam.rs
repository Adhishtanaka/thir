//! The camera device (den's side, root): V4L2 capture with memory-mapped buffers. The device node is root-only; only den opens it, after the user said yes (see mic.rs for the question),
//! and relays the frames. YUYV (what every UVC camera offers) is asked for first, then MJPEG. The structs are the 64-bit Linux ABI (both thir architectures).
use std::{fs::OpenOptions, os::{fd::{AsRawFd, OwnedFd}, unix::fs::OpenOptionsExt}};

pub const YUYV: u32 = 0x5659_5559; pub const MJPG: u32 = 0x4750_4a4d;
const CAPTURE: u32 = 1; const MMAP: u32 = 1;
const QUERYCAP: u64 = 0x8068_5600; const S_FMT: u64 = 0xC0D0_5605; const REQBUFS: u64 = 0xC014_5608; const QUERYBUF: u64 = 0xC058_5609;
const QBUF: u64 = 0xC058_560F; const DQBUF: u64 = 0xC058_5611; const STREAMON: u64 = 0x4004_5612; const STREAMOFF: u64 = 0x4004_5613;
/// One frame slot in the shared memory (two slots): room for 1280x960 YUYV.
pub const SLOT: usize = 4 << 20;
/// Is there a /dev/video* node at all?
pub fn exists() -> bool { std::fs::read_dir("/dev").is_ok_and(|d| d.flatten().any(|e| e.file_name().to_string_lossy().starts_with("video"))) }
/// The frame memory den fills and the app maps: a memfd of two slots, sealed so that the app cannot shrink it.
pub fn share_buffer() -> Result<OwnedFd, String> {
    let fd = unsafe { libc::memfd_create(c"thir-camera".as_ptr(), libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING) };
    if fd < 0 { return Err(std::io::Error::last_os_error().to_string()) }
    let fd: OwnedFd = unsafe { std::os::fd::FromRawFd::from_raw_fd(fd) }; let raw = fd.as_raw_fd();
    if unsafe { libc::ftruncate(raw, 2 * SLOT as i64) } != 0 || unsafe { libc::fcntl(raw, libc::F_ADD_SEALS, libc::F_SEAL_SHRINK | libc::F_SEAL_GROW | libc::F_SEAL_SEAL) } != 0 { return Err(std::io::Error::last_os_error().to_string()) }
    Ok(fd)
}
/// The width and height asked for: small, and every camera has it.
pub const WANT: (u32, u32) = (640, 480);

pub struct Cam { fd: OwnedFd, bufs: Vec<(*mut u8, usize)>, pub w: u32, pub h: u32, pub fmt: u32, pub size: usize, held: Option<u32> }

fn ioctl(fd: i32, req: u64, arg: *mut u8) -> Result<(), String> {
    loop {
        if unsafe { libc::ioctl(fd, req as _, arg) } == 0 { return Ok(()) }
        let e = std::io::Error::last_os_error();
        if e.raw_os_error() != Some(libc::EINTR) { return Err(e.to_string()) }
    }
}
fn put(b: &mut [u8], at: usize, v: u32) { b[at..at + 4].copy_from_slice(&v.to_ne_bytes()); }
fn get(b: &[u8], at: usize) -> u32 { u32::from_ne_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]]) }

/// A capture node: the capabilities say video capture with streaming (not a metadata node, not an output).
pub fn is_capture(caps: u32, device_caps: u32) -> bool { let c = if caps & 0x8000_0000 != 0 { device_caps } else { caps }; c & 1 != 0 && c & 0x0400_0000 != 0 }

impl Cam {
    /// The first camera in /dev/video*: Err says why not ("no camera found" when there is none).
    pub fn open() -> Result<Cam, String> {
        let mut nodes: Vec<(u32, std::path::PathBuf)> = std::fs::read_dir("/dev").map_err(|e| e.to_string())?.flatten()
            .filter_map(|e| { let n = e.file_name().to_string_lossy().into_owned(); Some((n.strip_prefix("video")?.parse().ok()?, e.path())) }).collect();
        nodes.sort();
        let mut last = "no camera found".to_string();
        for (_, p) in nodes { match Cam::open_node(&p) { Ok(c) => return Ok(c), Err(e) if e != "not a camera" => last = e, Err(_) => {} } }
        Err(last)
    }
    fn open_node(p: &std::path::Path) -> Result<Cam, String> {
        let f = OpenOptions::new().read(true).write(true).custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC).open(p).map_err(|e| format!("{}: {e}", p.display()))?;
        let fd = f.as_raw_fd();
        let mut cap = [0u8; 104]; ioctl(fd, QUERYCAP, cap.as_mut_ptr())?;
        if !is_capture(get(&cap, 84), get(&cap, 88)) { return Err("not a camera".into()) }
        let mut got = None;
        for want in [YUYV, MJPG] {
            let mut f = [0u8; 208]; put(&mut f, 0, CAPTURE); put(&mut f, 8, WANT.0); put(&mut f, 12, WANT.1); put(&mut f, 16, want);
            if ioctl(fd, S_FMT, f.as_mut_ptr()).is_ok() && get(&f, 16) == want { got = Some((get(&f, 8), get(&f, 12), want, get(&f, 28) as usize)); break }
        }
        let Some((w, h, fmt, size)) = got else { return Err("this camera gives neither YUYV nor MJPEG".into()) };
        if w == 0 || h == 0 || size == 0 || size > SLOT { return Err("the camera gave a strange picture size".into()) }
        let mut rb = [0u8; 20]; put(&mut rb, 0, 3); put(&mut rb, 4, CAPTURE); put(&mut rb, 8, MMAP);
        ioctl(fd, REQBUFS, rb.as_mut_ptr())?;
        let n = get(&rb, 0);
        let mut cam = Cam { fd: OwnedFd::from(f), bufs: vec![], w, h, fmt, size, held: None };
        for i in 0..n {
            let mut b = [0u8; 88]; put(&mut b, 0, i); put(&mut b, 4, CAPTURE); put(&mut b, 60, MMAP);
            ioctl(fd, QUERYBUF, b.as_mut_ptr())?;
            let (len, off) = (get(&b, 72) as usize, get(&b, 64) as libc::off_t);
            let m = unsafe { libc::mmap(std::ptr::null_mut(), len, libc::PROT_READ, libc::MAP_SHARED, fd, off) };
            if m == libc::MAP_FAILED { return Err("cannot map a camera buffer".into()) }
            cam.bufs.push((m as *mut u8, len));
            ioctl(fd, QBUF, b.as_mut_ptr())?;
        }
        if cam.bufs.is_empty() { return Err("the camera gave no buffers".into()) }
        let mut t = CAPTURE; ioctl(fd, STREAMON, &mut t as *mut u32 as *mut u8)?;
        Ok(cam)
    }
    pub fn raw(&self) -> i32 { self.fd.as_raw_fd() }
    /// The next frame, if one is ready (None = not yet, or the device failed: Err). The buffer is given back to the driver at the next call.
    pub fn frame(&mut self) -> Result<Option<&[u8]>, String> {
        let fd = self.raw();
        if let Some(i) = self.held.take() { let mut b = [0u8; 88]; put(&mut b, 0, i); put(&mut b, 4, CAPTURE); put(&mut b, 60, MMAP); let _ = ioctl(fd, QBUF, b.as_mut_ptr()); }
        let mut b = [0u8; 88]; put(&mut b, 4, CAPTURE); put(&mut b, 60, MMAP);
        match ioctl(fd, DQBUF, b.as_mut_ptr()) {
            Ok(()) => {
                let (i, used) = (get(&b, 0) as usize, get(&b, 8) as usize);
                let Some(&(p, len)) = self.bufs.get(i) else { return Err("bad buffer from the camera".into()) };
                self.held = Some(i as u32);
                Ok(Some(unsafe { std::slice::from_raw_parts(p, used.min(len)) }))
            }
            Err(e) if e.contains("temporarily") || e.contains("os error 11") => Ok(None),
            Err(e) => Err(e),
        }
    }
}
impl Drop for Cam {
    fn drop(&mut self) {
        let mut t = CAPTURE; let _ = ioctl(self.raw(), STREAMOFF, &mut t as *mut u32 as *mut u8);
        for &(p, len) in &self.bufs { unsafe { libc::munmap(p as *mut _, len); } }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn capture_nodes() {
        assert!(is_capture(0x8500_0001 | 0x0400_0000, 0x0400_0001));        // caps with DEVICE_CAPS: the device caps decide
        assert!(!is_capture(0x8500_0001, 0x0400_0000 | 0x0000_0000));       // device caps without capture
        assert!(!is_capture(0x8000_0000, 0x0400_0001 & !1));                // a metadata node: no capture bit
        assert!(is_capture(0x0400_0001, 0));                                // old drivers: caps itself
        assert!(!is_capture(0x0000_0001, 0));                               // capture but no streaming
    }
    #[test] fn struct_poking() { let mut b = [0u8; 12]; put(&mut b, 4, 0x01020304); assert_eq!(get(&b, 4), 0x01020304); assert_eq!(&b[..4], &[0; 4]); }
}
