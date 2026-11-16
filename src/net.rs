use std::{fs, io, net::Ipv4Addr};

pub struct Iface { pub name: String, pub mac: String, pub up: bool, pub ipv4: Option<Ipv4Addr> }

impl Iface { pub fn line(&self) -> String { format!("{:<8} {:<17} {:<4} {}", self.name, self.mac, if self.up {"up"} else {"down"}, self.ipv4.map_or("-".into(), |a| a.to_string())) } }

fn ifreq(name: &str) -> libc::ifreq {
    let mut r: libc::ifreq = unsafe { std::mem::zeroed() };
    for (d, s) in r.ifr_name.iter_mut().zip(name.bytes().take(libc::IFNAMSIZ - 1)) { *d = s as libc::c_char; }
    r
}
struct Sock(libc::c_int);           // AF_INET/SOCK_DGRAM control socket; Drop closes it
impl Sock { fn new() -> io::Result<Self> { let fd = unsafe { libc::socket(libc::AF_INET, libc::SOCK_DGRAM, 0) }; if fd < 0 { Err(io::Error::last_os_error()) } else { Ok(Sock(fd)) } } }
impl Drop for Sock { fn drop(&mut self) { unsafe { libc::close(self.0); } } }

fn ioctl(s: &Sock, req: libc::c_ulong, r: &mut libc::ifreq) -> io::Result<()> {
    // `as _`: the request is c_ulong on glibc but c_int on musl
    if unsafe { libc::ioctl(s.0, req as _, r) } < 0 { Err(io::Error::last_os_error()) } else { Ok(()) }
}

pub fn set_up(name: &str) -> io::Result<()> {
    let s = Sock::new()?; let mut r = ifreq(name);
    ioctl(&s, libc::SIOCGIFFLAGS, &mut r)?;
    unsafe { r.ifr_ifru.ifru_flags |= libc::IFF_UP as libc::c_short; }
    ioctl(&s, libc::SIOCSIFFLAGS, &mut r)
}

/// Names from /sys/class/net (sorted), MAC from .../address, flags + IPv4 via ioctl.
pub fn interfaces() -> io::Result<Vec<Iface>> {
    let s = Sock::new()?;
    let mut names: Vec<String> = fs::read_dir("/sys/class/net")?.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    Ok(names.into_iter().map(|name| {
        let mac = fs::read_to_string(format!("/sys/class/net/{name}/address")).map(|s| s.trim().to_string()).unwrap_or_default();
        let mut r = ifreq(&name);
        let up = ioctl(&s, libc::SIOCGIFFLAGS, &mut r).is_ok() && unsafe { r.ifr_ifru.ifru_flags } & libc::IFF_UP as libc::c_short != 0;
        let ipv4 = ioctl(&s, libc::SIOCGIFADDR, &mut r).ok().map(|_| {
            let sa = unsafe { &*(&r.ifr_ifru.ifru_addr as *const libc::sockaddr as *const libc::sockaddr_in) };
            Ipv4Addr::from(u32::from_be(sa.sin_addr.s_addr))
        });
        Iface { name, mac, up, ipv4 }
    }).collect())
}
