//! `dhcp [iface...]`: get an IPv4 lease, set address, mask, default route and /etc/resolv.conf.
//! No args = every ethernet interface, one after another (they share UDP port 68).
//! shortcut: no lease renewal; fine while sessions are shorter than the lease (QEMU gives 24 h). Add renew before long-running use.
use thir::{dhcp, net};
use std::{env, fs, io, net::{Ipv4Addr, UdpSocket}, os::fd::AsRawFd, process::exit, time::Duration};

fn mac(iface: &str) -> [u8; 6] {
    let mut m = [0u8; 6];
    let s = fs::read_to_string(format!("/sys/class/net/{iface}/address")).unwrap_or_default();
    for (d, h) in m.iter_mut().zip(s.trim().split(':')) { *d = u8::from_str_radix(h, 16).unwrap_or(0); }
    m
}

fn socket(iface: &str) -> io::Result<UdpSocket> {
    let s = UdpSocket::bind("0.0.0.0:68")?;
    s.set_broadcast(true)?;
    s.set_read_timeout(Some(Duration::from_secs(2)))?;
    // the interface has no address yet, so pin send and receive to it
    if unsafe { libc::setsockopt(s.as_raw_fd(), libc::SOL_SOCKET, libc::SO_BINDTODEVICE, iface.as_ptr() as *const _, iface.len() as _) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(s)
}

/// Send `msg` up to 3 times; return the first reply of kind `want` (or a NAK).
/// With `server` set, replies carrying another server id are ignored.
fn exchange(s: &UdpSocket, msg: &[u8], xid: u32, mac: [u8; 6], want: u8, server: Option<Ipv4Addr>) -> Option<dhcp::Lease> {
    let mut buf = [0u8; 1500];
    for _ in 0..3 {
        let _ = s.send_to(msg, (Ipv4Addr::BROADCAST, 67));   // a down link fails here; keep retrying
        while let Ok(n) = s.recv(&mut buf) {
            let Some(l) = dhcp::parse(&buf[..n], xid, mac) else { continue };
            if server.is_some() && l.server != server { continue; }
            if l.kind == want || l.kind == dhcp::NAK { return Some(l); }
        }
    }
    None
}

/// Unpredictable xid, so an off-path host cannot guess it and answer first. No weak fallback.
fn xid() -> Result<u32, String> {
    let mut b = [0u8; 4];
    thir::user::random(&mut b).map_err(|e| format!("getrandom: {e}"))?;   // shared helper: raw syscall, see user.rs
    Ok(u32::from_ne_bytes(b))
}

fn configure(iface: &str) -> Result<String, String> {
    net::set_up(iface).map_err(|e| format!("up: {e}"))?;
    let s = socket(iface).map_err(|e| format!("socket: {e}"))?;
    let mac = mac(iface);
    let xid = xid()?;
    let offer = exchange(&s, &dhcp::packet(dhcp::DISCOVER, xid, mac, None), xid, mac, dhcp::OFFER, None).ok_or("no offer")?;
    let server = offer.server.ok_or("offer without server id")?;
    let ack = exchange(&s, &dhcp::packet(dhcp::REQUEST, xid, mac, Some((offer.ip, server))), xid, mac, dhcp::ACK, Some(server)).ok_or("no ack")?;
    if ack.kind == dhcp::NAK { return Err("server sent NAK".into()); }
    let mask = ack.mask.unwrap_or(Ipv4Addr::new(255, 255, 255, 0));
    dhcp::check(ack.ip, mask, ack.router, ack.dns).map_err(|e| format!("rejected lease {}: {e}", ack.ip))?;
    net::set_addr(iface, ack.ip, mask).map_err(|e| format!("set address: {e}"))?;
    if let Some(gw) = ack.router { net::add_default_route(gw).map_err(|e| format!("route: {e}"))?; }
    if let Some(dns) = ack.dns { fs::write("/etc/resolv.conf", format!("nameserver {dns}\n")).map_err(|e| format!("resolv.conf: {e}"))?; }
    let opt = |a: Option<Ipv4Addr>| a.map_or("-".into(), |a| a.to_string());
    Ok(format!("{} mask {mask} gw {} dns {}", ack.ip, opt(ack.router), opt(ack.dns)))
}

fn main() {
    let mut ifaces: Vec<String> = env::args().skip(1).collect();
    if ifaces.is_empty() {
        // ARPHRD_ETHER = 1; this skips lo and tunnels
        ifaces = fs::read_dir("/sys/class/net").map(|d| d.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| fs::read_to_string(format!("/sys/class/net/{n}/type")).is_ok_and(|t| t.trim() == "1")).collect()).unwrap_or_default();
        ifaces.sort();
    }
    let mut failed = false;
    for i in &ifaces {
        match configure(i) { Ok(m) => println!("dhcp: {i} {m}"), Err(e) => { eprintln!("dhcp: {i}: {e}"); failed = true; } }
    }
    exit(failed as i32);
}
