//! The surface protocol between den and a pixel app (view, later play and web). One SOCK_SEQPACKET socket at /run/den/sock; a message is `u32 type` + a fixed payload, all little endian.
//! The app owns a sealed memfd of BGRA pixels the size of its pane; den (root) maps it read-only and copies damaged rects onto the screen. den checks everything the app sends (see `Surface::handle`, `peer_tty`).
use std::{io, os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd}};

pub const SOCK: &str = "/run/den/sock";
pub const VER: u32 = 1;
pub const MAX_RECTS: usize = 32;
const MAX_BYTES: u64 = 256 << 20;   // a surface never needs more (a 4096x4096 BGRA image is 64 MiB)

/// A damaged rectangle in surface pixels: x, y, w, h.
pub type R16 = (u16, u16, u16, u16);

/// App to den. MicStart and MicStop are for `rec` only: den checks the program (see mic.rs) and asks the user.
#[derive(Debug, PartialEq, Clone)]
pub enum Up { Hello(u32), Buffer { w: u32, h: u32, stride: u32 }, Commit(Vec<R16>), MicStart, MicStop }
/// Den to app. `Key.value`: 0 release, 1 press, 2 repeat; `code` is the evdev key code. Pointer coordinates are surface pixels (may lie outside while dragging). Scroll: dy > 0 = up.
#[derive(Debug, PartialEq, Clone, Copy)]
pub enum Down { Configure { w: u32, h: u32, scale: u32, focused: bool }, Key { code: u32, value: u32, mods: u32 }, Pointer { x: i32, y: i32 }, Button { code: u32, pressed: bool }, Scroll { dx: i32, dy: i32 }, Done, Mic { state: u32, rate: u32 } }
pub const MOD_SHIFT: u32 = 1; pub const MOD_CTRL: u32 = 2; pub const MOD_CAPS: u32 = 8;

fn word(b: &[u8], i: usize) -> Option<u32> { Some(u32::from_le_bytes(b.get(i * 4..i * 4 + 4)?.try_into().ok()?)) }
fn words(t: u32, v: &[u32]) -> Vec<u8> { std::iter::once(t).chain(v.iter().copied()).flat_map(u32::to_le_bytes).collect() }

impl Up {
    /// None = not a valid message: den drops the client.
    pub fn parse(b: &[u8]) -> Option<Up> {
        Some(match (word(b, 0)?, b.len()) {
            (1, 8) => Up::Hello(word(b, 1)?),
            (2, 16) => Up::Buffer { w: word(b, 1)?, h: word(b, 2)?, stride: word(b, 3)? },
            (3, n) if n >= 8 => {
                let c = word(b, 1)? as usize;
                if c > MAX_RECTS || n != 8 + 8 * c { return None }
                let h = |o: usize| u16::from_le_bytes([b[o], b[o + 1]]);
                Up::Commit((0..c).map(|i| { let o = 8 + 8 * i; (h(o), h(o + 2), h(o + 4), h(o + 6)) }).collect())
            }
            (4, 4) => Up::MicStart, (5, 4) => Up::MicStop,
            _ => return None,
        })
    }
    pub fn encode(&self) -> Vec<u8> {
        match self {
            Up::Hello(v) => words(1, &[*v]), Up::Buffer { w, h, stride } => words(2, &[*w, *h, *stride]),
            Up::Commit(r) => { let mut v = words(3, &[r.len() as u32]); for q in r { for x in [q.0, q.1, q.2, q.3] { v.extend(x.to_le_bytes()); } } v }
            Up::MicStart => words(4, &[]), Up::MicStop => words(5, &[]),
        }
    }
}
impl Down {
    pub fn parse(b: &[u8]) -> Option<Down> {
        let w = |i| word(b, i);
        Some(match (w(0)?, b.len()) {
            (0x101, 20) => Down::Configure { w: w(1)?, h: w(2)?, scale: w(3)?, focused: w(4)? != 0 },
            (0x102, 16) => Down::Key { code: w(1)?, value: w(2)?, mods: w(3)? },
            (0x103, 12) => Down::Pointer { x: w(1)? as i32, y: w(2)? as i32 },
            (0x104, 12) => Down::Button { code: w(1)?, pressed: w(2)? != 0 },
            (0x105, 12) => Down::Scroll { dx: w(1)? as i32, dy: w(2)? as i32 },
            (0x106, 4) => Down::Done,
            (0x107, 12) => Down::Mic { state: w(1)?, rate: w(2)? },
            _ => return None,
        })
    }
    pub fn encode(&self) -> Vec<u8> {
        match *self {
            Down::Configure { w, h, scale, focused } => words(0x101, &[w, h, scale, focused as u32]), Down::Key { code, value, mods } => words(0x102, &[code, value, mods]),
            Down::Pointer { x, y } => words(0x103, &[x as u32, y as u32]), Down::Button { code, pressed } => words(0x104, &[code, pressed as u32]),
            Down::Scroll { dx, dy } => words(0x105, &[dx as u32, dy as u32]), Down::Done => words(0x106, &[]), Down::Mic { state, rate } => words(0x107, &[state, rate]),
        }
    }
}

/// Longest PCM payload in one message.
pub const PCM_MAX: usize = 4096;
/// den to rec: microphone samples (stereo S16_LE), at most PCM_MAX bytes. A different shape from `Down`: a variable length.
pub fn pcm_msg(data: &[u8]) -> Vec<u8> { [&0x108u32.to_le_bytes()[..], &data[..data.len().min(PCM_MAX)]].concat() }

/// The rect cut to a w x h surface as (x, y, w, h); None if nothing is left.
pub fn clamp(r: R16, w: usize, h: usize) -> Option<(usize, usize, usize, usize)> {
    let (x, y) = (r.0 as usize, r.1 as usize);
    if x >= w || y >= h { return None }
    let (rw, rh) = ((r.2 as usize).min(w - x), (r.3 as usize).min(h - y));
    (rw > 0 && rh > 0).then_some((x, y, rw, rh))
}

/// Trust check 3 and 4: the buffer is the size den configured, the memfd cannot shrink (a read of a truncated page would kill den with SIGBUS) and holds all of it.
pub fn check_buffer(w: u32, h: u32, stride: u32, want: (u32, u32), size: u64, seals: i32) -> Result<(), &'static str> {
    if (w, h) != want || w == 0 || h == 0 { return Err("buffer is not the configured size") }
    if stride % 4 != 0 || (stride as u64) < w as u64 * 4 { return Err("bad stride") }
    let need = stride as u64 * h as u64;
    if need > MAX_BYTES { return Err("buffer too big") }
    if seals & libc::F_SEAL_SHRINK == 0 { return Err("memfd is not sealed against shrinking") }
    if size < need { return Err("memfd is smaller than the buffer") }
    Ok(())
}

/// den's read-only view of an app's pixels.
pub struct Buf { map: *const u8, len: usize, pub w: usize, pub h: usize, stride: usize }
impl Buf {
    pub fn attach(fd: OwnedFd, w: u32, h: u32, stride: u32, want: (u32, u32)) -> Result<Buf, &'static str> {
        let (mut st, seals) = (unsafe { std::mem::zeroed::<libc::stat>() }, unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GET_SEALS) });
        if unsafe { libc::fstat(fd.as_raw_fd(), &mut st) } != 0 || seals < 0 || st.st_mode & libc::S_IFMT != libc::S_IFREG { return Err("not a sealable file") }
        check_buffer(w, h, stride, want, st.st_size as u64, seals)?;
        let len = stride as usize * h as usize;
        let map = unsafe { libc::mmap(std::ptr::null_mut(), len, libc::PROT_READ, libc::MAP_SHARED, fd.as_raw_fd(), 0) };
        if map == libc::MAP_FAILED { return Err("mmap failed") }
        Ok(Buf { map: map as *const u8, len, w: w as usize, h: h as usize, stride: stride as usize })   // the mapping keeps the file; the fd closes here
    }
    /// Copies the rect `r` (already clamped to the buffer) to `dst` (`dw` pixels per row) at offset (ox, oy). The alpha byte is dropped.
    pub fn blit(&self, dst: &mut [u32], dw: usize, ox: usize, oy: usize, r: (usize, usize, usize, usize)) {
        let (x, y, w, h) = r;
        if x + w > self.w || y + h > self.h || ox + x + w > dw { return }
        for row in y..y + h {
            let off = (oy + row) * dw + ox + x;
            let Some(d) = dst.get_mut(off..off + w) else { return };
            // SAFETY: the row lies inside the mapping (checked above); the app may write at the same time, which only tears pixels. stride is a multiple of 4 and the map is page aligned.
            unsafe { std::ptr::copy_nonoverlapping(self.map.add(row * self.stride + x * 4) as *const u32, d.as_mut_ptr(), w); }
            for p in d { *p &= 0xFF_FFFF; }
        }
    }
}
impl Drop for Buf { fn drop(&mut self) { unsafe { libc::munmap(self.map as *mut _, self.len); } } }

/// What den should do after a valid message.
#[derive(Debug, PartialEq)]
pub enum Got { Nothing, Hello, Attached, Damage(Vec<(usize, usize, usize, usize)>), MicStart, MicStop }

/// den's end of one app connection.
pub struct Surface { fd: OwnedFd, hello: bool, pub buf: Option<Buf>, pub sent: Option<(u32, u32, bool)>, pub mic: bool }   // mic: the app is rec (it asked for the microphone): a text app, it keeps the keyboard
impl Surface {
    pub fn new(fd: OwnedFd) -> Surface { Surface { fd, hello: false, buf: None, sent: None, mic: false } }
    pub fn raw(&self) -> RawFd { self.fd.as_raw_fd() }
    /// The app has said hello: it gets configure, keys and pointer events.
    pub fn ready(&self) -> bool { self.hello }
    /// Never blocks: a full queue (an app that does not read) loses the event. shortcut: a lost key release can stick a key in that app; fine while the only apps are ours.
    pub fn send(&self, m: Down) { let _ = send_msg(self.raw(), &m.encode(), None, true); }
    /// `want` = the (w, h) den configured. Err = the app broke the protocol: drop it.
    pub fn handle(&mut self, m: Up, fds: Vec<OwnedFd>, want: (u32, u32)) -> Result<Got, &'static str> {
        if !fds.is_empty() && !matches!(m, Up::Buffer { .. }) { return Err("unexpected file descriptor") }
        match m {
            Up::Hello(v) => { if self.hello || v != VER { return Err("bad hello") } self.hello = true; Ok(Got::Hello) }
            _ if !self.hello => Err("no hello"),
            Up::Buffer { w, h, stride } => {
                let mut it = fds.into_iter();
                let (Some(fd), None) = (it.next(), it.next()) else { return Err("buffer needs exactly one fd") };
                self.buf = Some(Buf::attach(fd, w, h, stride, want)?); Ok(Got::Attached)
            }
            Up::Commit(r) => Ok(match &self.buf { None => Got::Nothing, Some(b) => Got::Damage(r.into_iter().filter_map(|q| clamp(q, b.w, b.h)).collect()) }),   // no buffer: a commit raced a resize, ignore
            Up::MicStart => Ok(Got::MicStart), Up::MicStop => Ok(Got::MicStop),
        }
    }
}

/// One datagram out, with an optional fd (SCM_RIGHTS).
pub fn send_msg(sock: RawFd, bytes: &[u8], pass: Option<RawFd>, nonblock: bool) -> io::Result<()> {
    let mut iov = libc::iovec { iov_base: bytes.as_ptr() as *mut _, iov_len: bytes.len() };
    let mut cbuf = [0u64; 4];   // 32 bytes >= CMSG_SPACE(4)
    let mut mh: libc::msghdr = unsafe { std::mem::zeroed() };
    mh.msg_iov = &mut iov; mh.msg_iovlen = 1;
    if let Some(fd) = pass {
        mh.msg_control = cbuf.as_mut_ptr() as *mut _; mh.msg_controllen = unsafe { libc::CMSG_SPACE(4) } as _;
        unsafe {
            let c = libc::CMSG_FIRSTHDR(&mh);
            (*c).cmsg_level = libc::SOL_SOCKET; (*c).cmsg_type = libc::SCM_RIGHTS; (*c).cmsg_len = libc::CMSG_LEN(4) as _;
            std::ptr::write_unaligned(libc::CMSG_DATA(c) as *mut i32, fd);
        }
    }
    let flags = libc::MSG_NOSIGNAL | if nonblock { libc::MSG_DONTWAIT } else { 0 };
    let n = unsafe { libc::sendmsg(sock, &mh, flags) };
    if n < 0 { Err(io::Error::last_os_error()) } else { Ok(()) }
}

/// One datagram in: (length, received fds, damaged). `damaged` = the message or its fds were cut short. Length 0 = the peer closed. WouldBlock when nothing is queued.
pub fn recv_msg(sock: RawFd, buf: &mut [u8], nonblock: bool) -> io::Result<(usize, Vec<OwnedFd>, bool)> {
    let mut iov = libc::iovec { iov_base: buf.as_mut_ptr() as *mut _, iov_len: buf.len() };
    let mut cbuf = [0u64; 8];
    let mut mh: libc::msghdr = unsafe { std::mem::zeroed() };
    mh.msg_iov = &mut iov; mh.msg_iovlen = 1; mh.msg_control = cbuf.as_mut_ptr() as *mut _; mh.msg_controllen = std::mem::size_of_val(&cbuf) as _;
    let n = unsafe { libc::recvmsg(sock, &mut mh, libc::MSG_CMSG_CLOEXEC | if nonblock { libc::MSG_DONTWAIT } else { 0 }) };
    if n < 0 { return Err(io::Error::last_os_error()) }
    let mut fds = vec![];
    unsafe {
        let mut c = libc::CMSG_FIRSTHDR(&mh);
        while !c.is_null() {
            if (*c).cmsg_level == libc::SOL_SOCKET && (*c).cmsg_type == libc::SCM_RIGHTS {
                let data = (*c).cmsg_len as usize - libc::CMSG_LEN(0) as usize;
                for i in 0..data / 4 { fds.push(OwnedFd::from_raw_fd(std::ptr::read_unaligned((libc::CMSG_DATA(c) as *const i32).add(i)))); }
            }
            c = libc::CMSG_NXTHDR(&mh, c);
        }
    }
    Ok((n as usize, fds, mh.msg_flags & (libc::MSG_TRUNC | libc::MSG_CTRUNC) != 0))
}

/// The controlling terminal (`tty_nr`) from the text of /proc/PID/stat. The name field can hold spaces and parentheses: fields count from the last ')'.
pub fn tty_nr(stat: &str) -> Option<u64> {
    let rest = &stat[stat.rfind(')')? + 1..];
    rest.split_whitespace().nth(4)?.parse::<i64>().ok().map(|v| v as u64)   // state ppid pgrp session tty_nr
}

/// The connected process: its credentials and a pidfd that pins it (SO_PEERPIDFD, Linux 6.5: a reused pid fails the check). An old kernel has none: refused, never trusted.
fn peer(sock: RawFd) -> Result<(libc::ucred, OwnedFd), &'static str> {
    let mut cred: libc::ucred = unsafe { std::mem::zeroed() };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    if unsafe { libc::getsockopt(sock, libc::SOL_SOCKET, libc::SO_PEERCRED, &mut cred as *mut _ as *mut _, &mut len) } != 0 { return Err("no peer credentials") }
    const SO_PEERPIDFD: libc::c_int = 77;
    let (mut pfd, mut len) = (-1i32, 4 as libc::socklen_t);
    if unsafe { libc::getsockopt(sock, libc::SOL_SOCKET, SO_PEERPIDFD, &mut pfd as *mut _ as *mut _, &mut len) } != 0 || pfd < 0 { return Err("no peer pidfd") }
    let pidfd = unsafe { OwnedFd::from_raw_fd(pfd) };
    let info = std::fs::read_to_string(format!("/proc/self/fdinfo/{}", pidfd.as_raw_fd())).map_err(|_| "no pidfd info")?;
    let pid = info.lines().find_map(|l| l.strip_prefix("Pid:")?.trim().parse::<i32>().ok());
    if pid != Some(cred.pid) { return Err("pid mismatch") }
    Ok((cred, pidfd))
}
/// The process must still be the one that connected (not exited, its pid not reused) after we read its /proc entries.
fn alive(pidfd: &OwnedFd) -> Result<(), &'static str> {
    if unsafe { libc::syscall(libc::SYS_pidfd_send_signal, pidfd.as_raw_fd() as libc::c_long, 0 as libc::c_long, 0 as libc::c_long, 0 as libc::c_long) } != 0 { return Err("peer is gone") }
    Ok(())
}
/// Trust checks 1 and 2 on a new connection (den runs as root): the peer is `uid`, and the pid the kernel reports now is still the process that connected. Returns the peer's controlling tty;
/// the caller compares it with the pty of a pane.
pub fn peer_tty(sock: RawFd, uid: u32) -> Result<u64, &'static str> {
    let (cred, pidfd) = peer(sock)?;
    if cred.uid != uid { return Err("peer is not the logged-in user") }
    let tty = tty_nr(&std::fs::read_to_string(format!("/proc/{}/stat", cred.pid)).map_err(|_| "peer is gone")?).ok_or("bad stat")?;
    alive(&pidfd)?; Ok(tty)
}
/// What program the peer runs now (/proc/PID/exe), read through the pinned pid. Asked again at MIC_START: a process that changed its program after connecting is not trusted with the microphone.
pub fn peer_exe(sock: RawFd) -> Result<std::path::PathBuf, &'static str> {
    let (cred, pidfd) = peer(sock)?;
    let exe = std::fs::read_link(format!("/proc/{}/exe", cred.pid)).map_err(|_| "no exe")?;
    alive(&pidfd)?; Ok(exe)
}

/// What `poll_ev` returns.
pub enum Ev { Down(Down), Pcm(Vec<u8>) }

/// The app side: connect to den, share a sealed memfd, commit damage, read events.
pub struct Client { sock: OwnedFd, pub w: u32, pub h: u32, pub scale: u32, pub focused: bool, map: *mut u32, map_len: usize }
impl Client {
    pub fn connect() -> io::Result<Client> {
        let s = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC, 0) };
        if s < 0 { return Err(io::Error::last_os_error()) }
        let sock = unsafe { OwnedFd::from_raw_fd(s) };
        let mut a: libc::sockaddr_un = unsafe { std::mem::zeroed() };
        a.sun_family = libc::AF_UNIX as _;
        for (d, b) in a.sun_path.iter_mut().zip(SOCK.bytes()) { *d = b as _; }
        if unsafe { libc::connect(s, &a as *const _ as *const _, std::mem::size_of::<libc::sockaddr_un>() as _) } != 0 { return Err(io::Error::last_os_error()) }
        let mut c = Client { sock, w: 0, h: 0, scale: 1, focused: false, map: std::ptr::null_mut(), map_len: 0 };
        send_msg(s, &Up::Hello(VER).encode(), None, false)?;
        loop {   // den answers with CONFIGURE, or closes the socket when it does not trust us
            match c.poll(3000)? { Some(Down::Configure { .. }) => return Ok(c), Some(_) => {}, None => return Err(io::Error::new(io::ErrorKind::TimedOut, "den did not answer")) }
        }
    }
    /// Waits up to `ms` for one event (-1 = forever); None on timeout. A closed socket is an error.
    pub fn poll(&mut self, ms: i32) -> io::Result<Option<Down>> { Ok(match self.poll_ev(ms)? { Some(Ev::Down(d)) => Some(d), _ => None }) }
    /// Like `poll`, and also gives the microphone samples (only `rec` gets those).
    pub fn poll_ev(&mut self, ms: i32) -> io::Result<Option<Ev>> {
        let mut p = libc::pollfd { fd: self.sock.as_raw_fd(), events: libc::POLLIN, revents: 0 };
        if unsafe { libc::poll(&mut p, 1, ms) } <= 0 { return Ok(None) }
        let mut b = [0u8; 4 + PCM_MAX + 64];
        let (n, _, bad) = recv_msg(self.sock.as_raw_fd(), &mut b, true)?;
        if n == 0 || bad { return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "den closed the connection")) }
        if word(&b[..n], 0) == Some(0x108) { return Ok(Some(Ev::Pcm(b[4..n].to_vec()))) }
        let m = Down::parse(&b[..n]).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "bad message from den"))?;
        if let Down::Configure { w, h, scale, focused } = m { (self.w, self.h, self.scale, self.focused) = (w, h, scale, focused); }
        Ok(Some(Ev::Down(m)))
    }
    /// The socket, for a main loop to watch (read events with `poll`).
    pub fn raw_fd(&self) -> RawFd { self.sock.as_raw_fd() }
    pub fn send(&self, m: &Up) -> io::Result<()> { send_msg(self.sock.as_raw_fd(), &m.encode(), None, false) }
    /// A new sealed buffer of the configured size (call again after a CONFIGURE with a new size).
    pub fn attach(&mut self) -> io::Result<()> {
        let (w, h) = (self.w as usize, self.h as usize);
        if w == 0 || h == 0 { return Err(io::Error::new(io::ErrorKind::InvalidInput, "empty surface")) }
        let len = w * h * 4;
        let fd = unsafe { libc::memfd_create(c"thir-surface".as_ptr(), libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING) };
        if fd < 0 { return Err(io::Error::last_os_error()) }
        let fd = unsafe { OwnedFd::from_raw_fd(fd) };
        if unsafe { libc::ftruncate(fd.as_raw_fd(), len as _) } != 0 || unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_ADD_SEALS, libc::F_SEAL_SHRINK | libc::F_SEAL_GROW | libc::F_SEAL_SEAL) } != 0 { return Err(io::Error::last_os_error()) }
        let map = unsafe { libc::mmap(std::ptr::null_mut(), len, libc::PROT_READ | libc::PROT_WRITE, libc::MAP_SHARED, fd.as_raw_fd(), 0) };
        if map == libc::MAP_FAILED { return Err(io::Error::last_os_error()) }
        send_msg(self.sock.as_raw_fd(), &Up::Buffer { w: self.w, h: self.h, stride: self.w * 4 }.encode(), Some(fd.as_raw_fd()), false)?;
        if !self.map.is_null() { unsafe { libc::munmap(self.map as *mut _, self.map_len); } }
        (self.map, self.map_len) = (map as *mut u32, len);
        Ok(())
    }
    /// The pixels, w * h, one u32 per pixel 0xAARRGGBB (den ignores the alpha byte). Empty before `attach`.
    pub fn pixels(&mut self) -> &mut [u32] { if self.map.is_null() { &mut [] } else { unsafe { std::slice::from_raw_parts_mut(self.map, self.map_len / 4) } } }
    pub fn commit(&self, rects: &[R16]) -> io::Result<()> { send_msg(self.sock.as_raw_fd(), &Up::Commit(rects.iter().take(MAX_RECTS).copied().collect()).encode(), None, false) }
}
impl Drop for Client { fn drop(&mut self) { if !self.map.is_null() { unsafe { libc::munmap(self.map as *mut _, self.map_len); } } } }

#[cfg(test)]
mod tests {
    use super::*;
    fn rng(seed: &mut u64) -> u64 { *seed ^= *seed << 13; *seed ^= *seed >> 7; *seed ^= *seed << 17; *seed }
    #[test] fn messages_round_trip() {
        for m in [Up::Hello(1), Up::Buffer { w: 3, h: 4, stride: 12 }, Up::Commit(vec![]), Up::Commit(vec![(1, 2, 3, 4); 32]), Up::MicStart, Up::MicStop] { assert_eq!(Up::parse(&m.encode()), Some(m.clone())); }
        for m in [Down::Configure { w: 1, h: 2, scale: 2, focused: true }, Down::Key { code: 30, value: 0, mods: 3 }, Down::Pointer { x: -5, y: 7 }, Down::Button { code: 0x110, pressed: true }, Down::Scroll { dx: 0, dy: -2 }, Down::Done, Down::Mic { state: 1, rate: 48000 }] { assert_eq!(Down::parse(&m.encode()), Some(m)); }
    }
    #[test] fn bad_messages_are_rejected() {
        let c = Up::Commit(vec![(0, 0, 1, 1); 2]).encode();
        assert_eq!(Up::parse(&c[..c.len() - 1]), None);                                    // short
        assert_eq!(Up::parse(&[c.clone(), vec![0]].concat()), None);                       // long
        assert_eq!(Up::parse(&Up::Commit(vec![(0, 0, 1, 1); 33]).encode()), None);         // too many rects
        assert_eq!(Up::parse(&words(3, &[u32::MAX])), None);                               // count lies
        assert_eq!(Up::parse(&words(9, &[])), None); assert_eq!(Up::parse(&[]), None); assert_eq!(Up::parse(&[1, 0, 0]), None);
        assert_eq!(Up::parse(&words(1, &[1, 2])), None);                                   // hello with extra payload
        assert_eq!(Down::parse(&words(0x106, &[1])), None); assert_eq!(Down::parse(&words(0x107, &[1])), None);
    }
    #[test] fn pcm_messages_are_capped() {
        let m = pcm_msg(&[7u8; 5000]); assert_eq!(m.len(), 4 + PCM_MAX); assert_eq!(&m[..4], &0x108u32.to_le_bytes()); assert_eq!(pcm_msg(&[1, 2, 3, 4]), [8, 1, 0, 0, 1, 2, 3, 4]);
        assert_eq!(Down::parse(&m), None);   // not a fixed-size message: only rec reads it, through poll_ev
    }
    #[test] fn random_bytes_never_panic() {
        let mut s = 0x9e3779b97f4a7c15u64;
        for i in 0..20000 {
            let n = (rng(&mut s) % 300) as usize;
            let mut b: Vec<u8> = (0..n).map(|_| rng(&mut s) as u8).collect();
            if i % 2 == 0 && n >= 4 { let t = [1u32, 2, 3, 4, 5, 0x101, 0x102, 0x103, 0x104, 0x105, 0x106, 0x107][(rng(&mut s) % 12) as usize]; b[..4].copy_from_slice(&t.to_le_bytes()); }   // a real type: reach the payload checks
            if i % 4 == 0 && n >= 8 { b[4..8].copy_from_slice(&((rng(&mut s) % 40) as u32).to_le_bytes()); }
            let _ = Up::parse(&b); let _ = Down::parse(&b); let _ = tty_nr(&String::from_utf8_lossy(&b));
        }
    }
    #[test] fn rects_are_clamped() {
        assert_eq!(clamp((0, 0, 10, 10), 100, 50), Some((0, 0, 10, 10)));
        assert_eq!(clamp((95, 45, 65535, 65535), 100, 50), Some((95, 45, 5, 5)));
        assert_eq!(clamp((100, 0, 5, 5), 100, 50), None); assert_eq!(clamp((0, 50, 5, 5), 100, 50), None); assert_eq!(clamp((3, 3, 0, 5), 100, 50), None); assert_eq!(clamp((3, 3, 5, 0), 100, 50), None);
    }
    #[test] fn buffer_checks() {
        let ok = |w, h, stride, want, size, seals| check_buffer(w, h, stride, want, size, seals);
        assert_eq!(ok(10, 5, 40, (10, 5), 200, libc::F_SEAL_SHRINK), Ok(()));
        assert!(ok(10, 5, 40, (10, 5), 199, libc::F_SEAL_SHRINK).is_err());                          // file smaller than stride * h: would SIGBUS
        assert!(ok(10, 5, 40, (10, 5), 200, 0).is_err()); assert!(ok(10, 5, 40, (10, 5), 200, libc::F_SEAL_GROW).is_err());   // not sealed against shrinking
        assert!(ok(10, 5, 40, (10, 6), 200, libc::F_SEAL_SHRINK).is_err()); assert!(ok(11, 5, 44, (10, 5), 220, libc::F_SEAL_SHRINK).is_err());   // not the configured size
        assert!(ok(10, 5, 36, (10, 5), 200, libc::F_SEAL_SHRINK).is_err()); assert!(ok(10, 5, 42, (10, 5), 210, libc::F_SEAL_SHRINK).is_err());   // stride too small, not a multiple of 4
        assert!(ok(0, 5, 0, (0, 5), 0, libc::F_SEAL_SHRINK).is_err());
        assert!(ok(10, 5, u32::MAX - 3, (10, 5), u64::MAX, libc::F_SEAL_SHRINK).is_err());           // huge: no overflow, refused
    }
    #[test] fn tty_from_stat() {
        assert_eq!(tty_nr("12 (view) S 1 12 12 34817 12 0 0"), Some(34817));
        assert_eq!(tty_nr("12 (a b) c) S 1 12 12 34818 12"), Some(34818));   // ')' inside the name
        assert_eq!(tty_nr("12 (x) S 1 12 12 0 -1"), Some(0));
        assert_eq!(tty_nr("junk"), None); assert_eq!(tty_nr("1 (x) S 1 2"), None);
    }
    fn pair() -> (OwnedFd, OwnedFd) {
        let mut v = [0; 2];
        assert_eq!(unsafe { libc::socketpair(libc::AF_UNIX, libc::SOCK_SEQPACKET, 0, v.as_mut_ptr()) }, 0);
        unsafe { (OwnedFd::from_raw_fd(v[0]), OwnedFd::from_raw_fd(v[1])) }
    }
    fn memfd(len: usize, seals: i32) -> OwnedFd {
        let fd = unsafe { OwnedFd::from_raw_fd(libc::memfd_create(c"t".as_ptr(), libc::MFD_ALLOW_SEALING)) };
        assert_eq!(unsafe { libc::ftruncate(fd.as_raw_fd(), len as _) }, 0);
        if seals != 0 { assert_eq!(unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_ADD_SEALS, seals) }, 0); }
        fd
    }
    #[test] fn surface_flow_with_real_fds() {
        let (a, b) = pair();
        let mut s = Surface::new(a);
        assert_eq!(s.handle(Up::Commit(vec![]), vec![], (4, 2)), Err("no hello"));
        assert_eq!(s.handle(Up::Hello(VER), vec![], (4, 2)), Ok(Got::Hello));
        assert_eq!(s.handle(Up::Hello(VER), vec![], (4, 2)), Err("bad hello"));
        assert_eq!(s.handle(Up::Commit(vec![(0, 0, 1, 1)]), vec![], (4, 2)), Ok(Got::Nothing));   // no buffer yet: ignored
        assert!(s.handle(Up::Buffer { w: 4, h: 2, stride: 16 }, vec![], (4, 2)).is_err());        // no fd
        assert!(s.handle(Up::Buffer { w: 4, h: 2, stride: 16 }, vec![memfd(32, 0)], (4, 2)).is_err());                 // not sealed
        assert!(s.handle(Up::Buffer { w: 4, h: 2, stride: 16 }, vec![memfd(31, libc::F_SEAL_SHRINK)], (4, 2)).is_err());   // too small
        assert!(s.handle(Up::Buffer { w: 4, h: 2, stride: 16 }, vec![memfd(32, libc::F_SEAL_SHRINK)], (5, 2)).is_err());    // not the configured size
        assert!(s.handle(Up::Commit(vec![]), vec![memfd(1, 0)], (4, 2)).is_err());                // a stray fd
        let m = memfd(32, libc::F_SEAL_SHRINK);
        let px = unsafe { libc::mmap(std::ptr::null_mut(), 32, libc::PROT_READ | libc::PROT_WRITE, libc::MAP_SHARED, m.as_raw_fd(), 0) } as *mut u32;
        for i in 0..8 { unsafe { *px.add(i) = 0xFF00_0000 | i as u32 * 0x010101; } }
        assert_eq!(s.handle(Up::Buffer { w: 4, h: 2, stride: 16 }, vec![m], (4, 2)), Ok(Got::Attached));
        assert_eq!(s.handle(Up::Commit(vec![(2, 1, 100, 100), (9, 9, 1, 1)]), vec![], (4, 2)), Ok(Got::Damage(vec![(2, 1, 2, 1)])));
        let mut dst = vec![7u32; 4 * 2];
        s.buf.as_ref().unwrap().blit(&mut dst, 4, 0, 0, (2, 1, 2, 1)); assert_eq!(dst, [7, 7, 7, 7, 7, 7, 0x060606, 0x070707]);   // alpha dropped
        s.buf.as_ref().unwrap().blit(&mut dst, 4, 3, 0, (2, 1, 2, 1)); assert_eq!(dst[7], 0x070707);   // would run past the row: refused, nothing written
        s.send(Down::Done);
        let mut r = [0u8; 16]; let (n, _, bad) = recv_msg(b.as_raw_fd(), &mut r, true).unwrap(); assert!(!bad); assert_eq!(Down::parse(&r[..n]), Some(Down::Done));
        unsafe { libc::munmap(px as *mut _, 32); }
    }
    #[test] fn recv_flags_truncation_and_close() {
        let (a, b) = pair();
        send_msg(a.as_raw_fd(), &[1; 100], None, true).unwrap();
        let mut small = [0u8; 10]; let (_, _, bad) = recv_msg(b.as_raw_fd(), &mut small, true).unwrap(); assert!(bad);
        assert_eq!(recv_msg(b.as_raw_fd(), &mut small, true).unwrap_err().kind(), io::ErrorKind::WouldBlock);
        let m = memfd(8, 0); send_msg(a.as_raw_fd(), &[1], Some(m.as_raw_fd()), true).unwrap();
        let (n, fds, bad) = recv_msg(b.as_raw_fd(), &mut small, true).unwrap(); assert_eq!((n, fds.len(), bad), (1, 1, false));
        drop(a); assert_eq!(recv_msg(b.as_raw_fd(), &mut small, true).unwrap().0, 0);
    }
    #[test] fn peer_must_be_the_logged_in_user() {
        let (a, _b) = pair();
        let me = unsafe { libc::getuid() };
        assert_eq!(peer_tty(a.as_raw_fd(), me.wrapping_add(1)), Err("peer is not the logged-in user"));
        assert!(peer_tty(a.as_raw_fd(), me).is_ok() || peer_tty(a.as_raw_fd(), me) == Err("no peer pidfd"));   // an old kernel has no SO_PEERPIDFD: refused, never trusted
        match peer_exe(a.as_raw_fd()) { Ok(p) => assert_eq!(p, std::env::current_exe().unwrap().canonicalize().unwrap()), Err(e) => assert_eq!(e, "no peer pidfd") }
    }
    #[test] fn mic_messages_reach_the_broker_only_after_hello() {
        let (a, _b) = pair(); let mut s = Surface::new(a);
        assert_eq!(s.handle(Up::MicStart, vec![], (0, 0)), Err("no hello"));
        s.handle(Up::Hello(VER), vec![], (0, 0)).unwrap();
        assert_eq!(s.handle(Up::MicStart, vec![], (0, 0)), Ok(Got::MicStart)); assert_eq!(s.handle(Up::MicStop, vec![], (0, 0)), Ok(Got::MicStop));
        assert!(s.handle(Up::MicStart, vec![memfd(1, 0)], (0, 0)).is_err());   // a mic message carries no fd
    }
}
