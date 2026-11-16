//! Accounts: /data/etc/users, one line per user: name:uid:argon2id:m_kib:t:p:salt_hex:hash_hex (gid = uid).
//! Passwords are hashed with Argon2id (RustCrypto `argon2`); hashes are compared in constant time.
use std::{fs::{self, File, OpenOptions}, io::{self, Write}, os::unix::fs::OpenOptionsExt};
use argon2::{Algorithm, Argon2, Params as A2Params, Version};
use subtle::ConstantTimeEq;

pub const USERS: &str = "/data/etc/users";
pub const HOME_ROOT: &str = "/data/home";
pub const FIRST_UID: u32 = 1000;
pub const MIN_PASSWORD: usize = 5;
pub const MAX_PASSWORD: usize = 128; // bytes

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Params { pub m_kib: u32, pub t: u32, pub p: u32 }
pub const DEFAULT: Params = Params { m_kib: 19456, t: 2, p: 1 }; // OWASP minimum for Argon2id
#[cfg(test)]
pub const FAST: Params = Params { m_kib: 8, t: 1, p: 1 };

#[derive(Debug, Clone, PartialEq)]
pub struct User { pub name: String, pub uid: u32, pub params: Params, pub salt: [u8; 16], pub hash: [u8; 32] }

/// ^[a-z_][a-z0-9_-]{0,31}$ and not "root"
pub fn valid_name(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty() && b.len() <= 32 && !s.eq_ignore_ascii_case("root") && (b[0].is_ascii_alphabetic() || b[0] == b'_')
        && b[1..].iter().all(|c| c.is_ascii_alphanumeric() || *c == b'_' || *c == b'-')
}

pub fn hash(password: &str, salt: &[u8; 16], p: &Params) -> Result<[u8; 32], String> {
    let params = A2Params::new(p.m_kib, p.t, p.p, Some(32)).map_err(|e| e.to_string())?;
    let mut out = [0u8; 32];
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params).hash_password_into(password.as_bytes(), salt, &mut out).map_err(|e| e.to_string())?;
    Ok(out)
}

pub fn new(name: &str, uid: u32, password: &str, salt: [u8; 16], p: &Params) -> Result<User, String> {
    if !valid_name(name) { return Err("invalid name".into()); }
    if password.len() < MIN_PASSWORD { return Err(format!("password too short (min {MIN_PASSWORD})")); }
    if password.len() > MAX_PASSWORD { return Err(format!("password too long (max {MAX_PASSWORD})")); }
    Ok(User { name: name.into(), uid, params: *p, salt, hash: hash(password, &salt, p)? })
}

pub fn create(name: &str, uid: u32, password: &str) -> Result<User, String> {
    let mut salt = [0u8; 16];
    random(&mut salt).map_err(|e| format!("no random numbers: {e}"))?;
    new(name, uid, password, salt, &DEFAULT)
}

fn hex(b: &[u8]) -> String { b.iter().map(|x| format!("{x:02x}")).collect() }
fn unhex<const N: usize>(s: &str) -> Option<[u8; N]> {
    if s.len() != 2 * N || !s.is_ascii() { return None; }
    let mut out = [0u8; N];
    for (i, o) in out.iter_mut().enumerate() { *o = u8::from_str_radix(&s[2 * i..2 * i + 2], 16).ok()?; }
    Some(out)
}

pub fn parse(text: &str) -> Result<Vec<User>, String> {
    let mut users: Vec<User> = Vec::new();
    for (n, line) in text.lines().enumerate().filter(|(_, l)| !l.is_empty()) {
        let bad = |what: &str| format!("users line {}: {what}", n + 1);
        let f: Vec<&str> = line.split(':').collect();
        if f.len() != 8 { return Err(bad("expected 8 fields")); }
        if !valid_name(f[0]) { return Err(bad("bad name")); }
        let uid: u32 = f[1].parse().map_err(|_| bad("bad uid"))?;
        if uid < FIRST_UID { return Err(bad("uid too low")); }
        if f[2] != "argon2id" { return Err(bad("unknown algorithm")); }
        let num = |s: &str| s.parse::<u32>().map_err(|_| bad("bad parameter"));
        let params = Params { m_kib: num(f[3])?, t: num(f[4])?, p: num(f[5])? };
        if !(8..=1 << 20).contains(&params.m_kib) || !(1..=64).contains(&params.t) || !(1..=16).contains(&params.p) { return Err(bad("parameter out of range")); }
        let salt = unhex::<16>(f[6]).ok_or_else(|| bad("bad salt"))?;
        let hash = unhex::<32>(f[7]).ok_or_else(|| bad("bad hash"))?;
        if users.iter().any(|u| u.name == f[0] || u.uid == uid) { return Err(bad("duplicate user")); }
        users.push(User { name: f[0].into(), uid, params, salt, hash });
    }
    Ok(users)
}

impl User {
    pub fn line(&self) -> String {
        format!("{}:{}:argon2id:{}:{}:{}:{}:{}", self.name, self.uid, self.params.m_kib, self.params.t, self.params.p, hex(&self.salt), hex(&self.hash))
    }
    pub fn verify(&self, password: &str) -> bool {
        if password.len() > MAX_PASSWORD { return false; }
        hash(password, &self.salt, &self.params).map_or(false, |h| h.ct_eq(&self.hash).into())
    }
    pub fn home(&self) -> String { format!("{HOME_ROOT}/{}", self.name) }
}

pub fn load() -> io::Result<Vec<User>> {
    parse(&fs::read_to_string(USERS)?).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

pub fn save(users: &[User]) -> io::Result<()> {
    let tmp = format!("{USERS}.tmp");
    let mut f = OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?;
    for u in users { writeln!(f, "{}", u.line())?; }
    f.sync_all()?;
    fs::rename(&tmp, USERS)?;
    File::open("/data/etc")?.sync_all()
}

// libc::getrandom (the musl wrapper) jumps to address 0 in static x86_64 builds; the raw syscall works on both arches
pub fn random(buf: &mut [u8]) -> io::Result<()> {
    let mut done = 0;
    while done < buf.len() {
        let n = unsafe { libc::syscall(libc::SYS_getrandom, buf[done..].as_mut_ptr(), buf.len() - done, 0) };
        if n < 0 {
            let e = io::Error::last_os_error();
            if e.kind() != io::ErrorKind::Interrupted { return Err(e); }
        } else { done += n as usize; }
    }
    Ok(())
}

/// Drop root for good: clear groups, set gid then uid, then check nothing of root is left.
pub fn drop_to(uid: u32, gid: u32) -> io::Result<()> {
    let fail = |what: &str| Err(io::Error::new(io::ErrorKind::Other, format!("{what}: {}", io::Error::last_os_error())));
    unsafe {
        if libc::setgroups(0, std::ptr::null()) != 0 { return fail("setgroups"); }
        if libc::setgid(gid) != 0 { return fail("setgid"); }
        if libc::setuid(uid) != 0 { return fail("setuid"); }
        if libc::getuid() != uid || libc::geteuid() != uid || libc::getgid() != gid || libc::getegid() != gid {
            return Err(io::Error::new(io::ErrorKind::Other, "ids not dropped"));
        }
        if uid != 0 && libc::setuid(0) == 0 { return Err(io::Error::new(io::ErrorKind::Other, "regained root")); }
    }
    Ok(())
}

pub fn read_line(prompt: &str) -> io::Result<String> {
    print!("{prompt}");
    io::stdout().flush()?;
    let mut s = String::new();
    if io::stdin().read_line(&mut s)? == 0 { return Err(io::ErrorKind::UnexpectedEof.into()); }
    Ok(s.trim_end_matches(['\n', '\r']).to_string())
}

/// Like read_line with the tty's ECHO off (Err if fd 0 is not a tty). ECHO is restored on every path.
pub fn read_secret(prompt: &str) -> io::Result<String> {
    let mut old: libc::termios = unsafe { std::mem::zeroed() };
    if unsafe { libc::tcgetattr(0, &mut old) } != 0 { return Err(io::Error::last_os_error()); }
    let mut quiet = old;
    quiet.c_lflag &= !libc::ECHO;
    if unsafe { libc::tcsetattr(0, libc::TCSAFLUSH, &quiet) } != 0 { return Err(io::Error::last_os_error()); }
    let r = read_line(prompt);
    unsafe { libc::tcsetattr(0, libc::TCSAFLUSH, &old); }
    println!();
    r
}

/// An unknown user costs the same as a wrong password.
pub fn dummy_verify(password: &str) { let _ = hash(password, &[0u8; 16], &DEFAULT); }

#[cfg(test)]
mod tests {
    use super::*;
    fn user() -> User { new("ann", 1000, "correct horse", [7; 16], &FAST).unwrap() }
    #[test] fn round_trip() { let u = user(); assert_eq!(parse(&u.line()).unwrap(), vec![u]); }
    #[test] fn verify_cases() { let u = user(); assert!(u.verify("correct horse")); assert!(!u.verify("wrong horse!")); assert!(!u.verify("")); }
    #[test] fn parse_rejects() {
        let l = user().line();
        let f: Vec<&str> = l.split(':').collect();
        let with = |i: usize, v: &str| { let mut g = f.clone(); g[i] = v; g.join(":") };
        assert!(parse(&f[..7].join(":")).is_err());
        assert!(parse(&with(1, "0")).is_err());
        assert!(parse(&with(6, "abcd")).is_err());
        assert!(parse(&with(3, "x")).is_err());
        assert!(parse(&with(2, "scrypt")).is_err());
        assert!(parse(&with(0, "root")).is_err());
        assert!(parse(&format!("{l}\n{l}")).is_err());
    }
    #[test] fn names() {
        for ok in ["a", "_x", "ann-1_b", "Ann", "Adhishtanaka", &"a".repeat(32)] { assert!(valid_name(ok), "{ok}"); }
        for bad in ["", "root", "Root", "1a", "-a", "a b", "a:b", &"a".repeat(33)] { assert!(!valid_name(bad), "{bad}"); }
    }
    #[test] fn password_length() {
        assert!(new("ann", 1000, "abcd", [0; 16], &FAST).is_err());   // 4 bytes, below MIN_PASSWORD
        assert!(new("ann", 1000, &"x".repeat(129), [0; 16], &FAST).is_err());
        assert!(new("ann", 1000, &"x".repeat(128), [0; 16], &FAST).is_ok());
    }
}
