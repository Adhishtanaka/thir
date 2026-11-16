//! DHCPv4 packet build/parse (RFC 2131). Socket work is in src/bin/dhcp.rs.
use std::net::Ipv4Addr;

pub const DISCOVER: u8 = 1;
pub const OFFER: u8 = 2;
pub const REQUEST: u8 = 3;
pub const ACK: u8 = 5;
pub const NAK: u8 = 6;
const MAGIC: [u8; 4] = [99, 130, 83, 99];

#[derive(Debug, PartialEq)]
pub struct Lease { pub kind: u8, pub ip: Ipv4Addr, pub server: Option<Ipv4Addr>, pub mask: Option<Ipv4Addr>, pub router: Option<Ipv4Addr>, pub dns: Option<Ipv4Addr> }

/// `req` = (offered ip, server id), sent only in REQUEST.
pub fn packet(kind: u8, xid: u32, mac: [u8; 6], req: Option<(Ipv4Addr, Ipv4Addr)>) -> Vec<u8> {
    let mut p = vec![0u8; 240];
    p[0] = 1; p[1] = 1; p[2] = 6;                       // BOOTREQUEST, ethernet, MAC length
    p[4..8].copy_from_slice(&xid.to_be_bytes());
    p[10] = 0x80;                                       // broadcast flag: we have no address yet, so the reply must be broadcast
    p[28..34].copy_from_slice(&mac);
    p[236..240].copy_from_slice(&MAGIC);
    p.extend([53, 1, kind]);
    if let Some((ip, server)) = req { p.extend([50, 4]); p.extend(ip.octets()); p.extend([54, 4]); p.extend(server.octets()); }
    p.extend([55, 3, 1, 3, 6, 255]);                    // ask for mask, router, dns; end
    p.resize(300, 0);
    p
}

/// Addresses no DHCP server should hand out: 0/8, loopback, link-local, multicast, 240/4 and broadcast.
fn unusable(ip: Ipv4Addr) -> bool {
    ip.octets()[0] == 0 || ip.is_loopback() || ip.is_link_local() || ip.is_multicast() || ip.octets()[0] >= 240
}

/// Reject a lease that would install a nonsense address, gateway or resolver.
/// The resolver may be off-subnet (e.g. a public DNS), so it only gets the `unusable` check.
pub fn check(ip: Ipv4Addr, mask: Ipv4Addr, router: Option<Ipv4Addr>, dns: Option<Ipv4Addr>) -> Result<(), &'static str> {
    let (a, m) = (u32::from(ip), u32::from(mask));
    if !m & (!m).wrapping_add(1) != 0 || !(8..=30).contains(&m.count_ones()) { return Err("bad netmask"); }   // contiguous, /8../30
    if unusable(ip) || a & !m == 0 || a & !m == !m { return Err("bad address"); }                         // not network/broadcast
    if let Some(gw) = router { if gw == ip || unusable(gw) || u32::from(gw) & m != a & m { return Err("gateway outside subnet"); } }
    if dns.is_some_and(unusable) { return Err("bad dns server"); }
    Ok(())
}

/// None for anything that is not a well-formed reply to `xid` for `mac`.
pub fn parse(p: &[u8], xid: u32, mac: [u8; 6]) -> Option<Lease> {
    if p.len() < 240 || p[0] != 2 || p[4..8] != xid.to_be_bytes() || p[28..34] != mac || p[236..240] != MAGIC { return None; }
    let ip4 = |b: &[u8]| (b.len() >= 4).then(|| Ipv4Addr::new(b[0], b[1], b[2], b[3]));
    let mut l = Lease { kind: 0, ip: ip4(&p[16..20])?, server: None, mask: None, router: None, dns: None };
    let mut i = 240;
    while i < p.len() {
        match p[i] { 255 => break, 0 => { i += 1; continue; } _ => {} }
        let len = *p.get(i + 1)? as usize;
        let v = p.get(i + 2..i + 2 + len)?;
        match p[i] { 53 => l.kind = *v.first()?, 54 => l.server = ip4(v), 1 => l.mask = ip4(v), 3 => l.router = ip4(v), 6 => l.dns = ip4(v), _ => {} }
        i += 2 + len;
    }
    (l.kind != 0).then_some(l)
}

#[cfg(test)]
mod tests {
    use super::*;
    const MAC: [u8; 6] = [0x52, 0x54, 0, 0x12, 0x34, 0x56];

    fn reply(xid: u32, opts: &[u8]) -> Vec<u8> {
        let mut p = packet(DISCOVER, xid, MAC, None);
        p.truncate(240);
        p[0] = 2;
        p[16..20].copy_from_slice(&[10, 0, 2, 15]);
        p.extend(opts);
        p
    }

    #[test] fn request_has_header_and_options() {
        let p = packet(REQUEST, 7, MAC, Some((Ipv4Addr::new(10, 0, 2, 15), Ipv4Addr::new(10, 0, 2, 2))));
        assert_eq!((p.len(), p[0], &p[4..8], &p[28..34]), (300, 1, &7u32.to_be_bytes()[..], &MAC[..]));
        assert_eq!(&p[240..254], &[53, 1, 3, 50, 4, 10, 0, 2, 15, 54, 4, 10, 0, 2][..]);
    }
    #[test] fn parses_ack() {
        let l = parse(&reply(7, &[53, 1, 5, 1, 4, 255, 255, 255, 0, 3, 4, 10, 0, 2, 2, 6, 4, 10, 0, 2, 3, 255]), 7, MAC).unwrap();
        assert_eq!(l, Lease { kind: ACK, ip: Ipv4Addr::new(10, 0, 2, 15), server: None, mask: Some(Ipv4Addr::new(255, 255, 255, 0)),
                              router: Some(Ipv4Addr::new(10, 0, 2, 2)), dns: Some(Ipv4Addr::new(10, 0, 2, 3)) });
    }
    #[test] fn rejects_wrong_xid_and_own_request() {
        assert_eq!(parse(&reply(7, &[53, 1, 2, 255]), 8, MAC), None);
        assert_eq!(parse(&packet(DISCOVER, 7, MAC, None), 7, MAC), None);
    }
    #[test] fn rejects_reply_for_other_mac() {
        assert_eq!(parse(&reply(7, &[53, 1, 2, 255]), 7, [0; 6]), None);
    }
    #[test] fn checks_lease_sanity() {
        let ip = |a, b, c, d| Ipv4Addr::new(a, b, c, d);
        let m24 = ip(255, 255, 255, 0);
        assert_eq!(check(ip(10, 0, 2, 15), m24, Some(ip(10, 0, 2, 2)), None), Ok(()));
        assert_eq!(check(ip(10, 0, 2, 15), ip(255, 0, 255, 0), None, None), Err("bad netmask"));
        assert_eq!(check(ip(10, 0, 2, 15), ip(0, 0, 0, 0), None, None), Err("bad netmask"));
        assert_eq!(check(ip(127, 0, 0, 1), m24, None, None), Err("bad address"));
        assert_eq!(check(ip(10, 0, 2, 255), m24, None, None), Err("bad address"));
        assert_eq!(check(ip(10, 0, 2, 15), m24, Some(ip(8, 8, 8, 8)), None), Err("gateway outside subnet"));
        assert_eq!(check(ip(10, 0, 2, 15), ip(255, 254, 0, 0), None, None), Ok(()));
        assert_eq!(check(ip(10, 0, 2, 15), ip(254, 0, 0, 0), None, None), Err("bad netmask"));
        assert_eq!(check(ip(169, 254, 1, 9), ip(255, 255, 0, 0), None, None), Err("bad address"));
        assert_eq!(check(ip(10, 0, 2, 15), m24, None, Some(ip(8, 8, 8, 8))), Ok(()));
        assert_eq!(check(ip(10, 0, 2, 15), m24, None, Some(ip(127, 0, 0, 53))), Err("bad dns server"));
    }
    #[test] fn rejects_truncated_option() {
        assert_eq!(parse(&reply(7, &[53, 1, 2, 6, 4, 10, 0]), 7, MAC), None);
    }
}
