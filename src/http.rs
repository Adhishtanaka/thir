//! HTTP/1.1 client pieces for curl and ask: URLs, request bytes, response head, body framing, SSE. Pure code over Read/BufRead; no TLS here.
use std::io::{self, BufRead, Read};

pub const MAX_LINE: usize = 8 * 1024;
pub const MAX_HEADERS: usize = 100;
pub const MAX_SSE_DATA: usize = 1 << 20;

#[derive(Debug, Clone, PartialEq)]
pub struct Url { pub https: bool, pub host: String, pub port: u16, pub path: String }

impl Url {
    pub fn parse(s: &str) -> Result<Url, String> {
        if s.bytes().any(|b| b <= 0x20 || b >= 0x7f) { return Err("URL has spaces, control or non-ASCII characters".into()); }
        let s = s.split('#').next().unwrap_or("");
        let lower = s.to_ascii_lowercase();
        let (https, rest) = if lower.starts_with("https://") { (true, &s[8..]) } else if lower.starts_with("http://") { (false, &s[7..]) } else { return Err("only http and https URLs are supported".into()) };
        let end = rest.find(['/', '?']).unwrap_or(rest.len());
        let (auth, tail) = rest.split_at(end);
        if auth.contains('@') { return Err("user names in URLs are not supported".into()); }
        let port_of = |p: &str| p.parse::<u16>().map_err(|_| "bad port".to_string());
        let (host, port) = if let Some(r) = auth.strip_prefix('[') {
            let (h, p) = r.split_once(']').ok_or("bad IPv6 address")?;
            if h.is_empty() || !h.bytes().all(|b| b.is_ascii_hexdigit() || b == b':' || b == b'.') { return Err("bad IPv6 address".into()); }
            (h.to_string(), if p.is_empty() { None } else { Some(port_of(p.strip_prefix(':').ok_or("bad port")?)?) })
        } else {
            let (h, p) = match auth.rsplit_once(':') { Some((h, p)) => (h, Some(p)), None => (auth, None) };
            if h.is_empty() || !h.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-' || b == b'_') { return Err("bad host name".into()); }
            (h.to_string(), match p { Some(p) => Some(port_of(p)?), None => None })
        };
        if port == Some(0) { return Err("bad port".into()); }
        let path = if tail.is_empty() { "/".to_string() } else if tail.starts_with('?') { format!("/{tail}") } else { tail.to_string() };
        Ok(Url { https, host: host.to_ascii_lowercase(), port: port.unwrap_or(if https { 443 } else { 80 }), path })
    }
    fn base(&self) -> String { format!("{}://{}", if self.https { "https" } else { "http" }, self.host_header()) }
    /// Resolves a Location value against this URL. shortcut: no dot-segment removal, so a relative "../x" stays as written; fine for redirects.
    pub fn join(&self, loc: &str) -> Result<Url, String> {
        let l = loc.to_ascii_lowercase();
        if l.starts_with("http://") || l.starts_with("https://") { return Url::parse(loc); }
        if loc.split(['/', '?']).next().is_some_and(|f| f.contains(':')) { return Err("only http and https URLs are supported".into()); }
        let scheme = if self.https { "https" } else { "http" };
        if let Some(r) = loc.strip_prefix("//") { return Url::parse(&format!("{scheme}://{r}")); }
        let p = self.path.split('?').next().unwrap_or("/");
        let full = if loc.starts_with('/') { format!("{}{loc}", self.base()) }
            else if loc.starts_with('?') { format!("{}{p}{loc}", self.base()) }
            else { format!("{}{}{loc}", self.base(), &p[..p.rfind('/').map_or(0, |i| i + 1)]) };
        Url::parse(&full)
    }
    pub fn same_origin(&self, o: &Url) -> bool { self.https == o.https && self.host == o.host && self.port == o.port }
    pub fn host_header(&self) -> String {
        let h = if self.host.contains(':') { format!("[{}]", self.host) } else { self.host.clone() };
        if self.port == if self.https { 443 } else { 80 } { h } else { format!("{h}:{}", self.port) }
    }
}

/// RFC 7230 token: header names and methods.
pub fn token(s: &str) -> bool { !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b)) }
/// A header value that cannot split the request.
pub fn value_ok(s: &str) -> bool { !s.bytes().any(|b| b == b'\r' || b == b'\n' || b == 0) }

/// The request head. Callers check method/header names with `token` and values with `value_ok` first.
pub fn request(method: &str, url: &Url, headers: &[(String, String)], body_len: Option<usize>) -> Vec<u8> {
    let mut r = format!("{method} {} HTTP/1.1\r\nHost: {}\r\n", url.path, url.host_header());
    for (n, v) in headers { r += &format!("{n}: {v}\r\n"); }
    r += "Connection: close\r\n";
    if let Some(n) = body_len { r += &format!("Content-Length: {n}\r\n"); }
    r += "\r\n";
    r.into_bytes()
}

#[derive(Debug)]
pub struct Head { pub status: u16, pub lines: Vec<String>, pub headers: Vec<(String, String)> }
impl Head {
    pub fn get(&self, name: &str) -> Option<&str> { self.headers.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str()) }
    fn all<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a str> { self.headers.iter().filter(move |(h, _)| h.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str()) }
}

fn bad(m: &str) -> io::Error { io::Error::new(io::ErrorKind::InvalidData, m.to_string()) }

/// One line without its line end, at most `max` bytes. None = end of input before any byte.
fn read_line<R: BufRead>(r: &mut R, max: usize) -> io::Result<Option<Vec<u8>>> {
    let mut l = vec![];
    loop {
        let buf = r.fill_buf()?;
        if buf.is_empty() { return if l.is_empty() { Ok(None) } else { Err(io::ErrorKind::UnexpectedEof.into()) }; }
        let (take, found) = match buf.iter().position(|&b| b == b'\n') { Some(i) => (i + 1, true), None => (buf.len(), false) };
        l.extend_from_slice(&buf[..take]); r.consume(take);
        if l.len() > max + 2 { return Err(bad("line too long")); }
        if found { l.pop(); if l.last() == Some(&b'\r') { l.pop(); } if l.len() > max { return Err(bad("line too long")); } return Ok(Some(l)); }
    }
}

/// Reads the status line and headers; interim 1xx replies are skipped.
pub fn read_head<R: BufRead>(r: &mut R) -> io::Result<Head> {
    loop {
        let st = String::from_utf8_lossy(&read_line(r, MAX_LINE)?.ok_or_else(|| bad("connection closed before a reply"))?).into_owned();
        let code = st.strip_prefix("HTTP/1.1 ").or_else(|| st.strip_prefix("HTTP/1.0 ")).ok_or_else(|| bad("not an HTTP/1.x reply"))?;
        let status: u16 = code.get(..3).and_then(|c| c.parse().ok()).filter(|_| code.len() == 3 || code.as_bytes()[3] == b' ').ok_or_else(|| bad("bad status line"))?;
        let (mut lines, mut headers) = (vec![st], vec![]);
        loop {
            let l = read_line(r, MAX_LINE)?.ok_or(io::ErrorKind::UnexpectedEof)?;
            if l.is_empty() { break; }
            if headers.len() >= MAX_HEADERS { return Err(bad("too many headers")); }
            let t = String::from_utf8_lossy(&l).into_owned();
            let (n, v) = t.split_once(':').ok_or_else(|| bad("bad header line"))?;
            if !token(n) { return Err(bad("bad header name")); }
            headers.push((n.to_string(), v.trim().to_string())); lines.push(t);
        }
        if (100..200).contains(&status) { continue; }
        return Ok(Head { status, lines, headers });
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Framing { None, Length(u64), Chunked, Close }

pub fn framing(h: &Head, head_req: bool) -> Result<Framing, String> {
    if head_req || h.status == 204 || h.status == 304 || (100..200).contains(&h.status) { return Ok(Framing::None); }
    let te: Vec<&str> = h.all("transfer-encoding").flat_map(|v| v.split(',')).map(str::trim).collect();
    let cl: Vec<&str> = h.all("content-length").flat_map(|v| v.split(',')).map(str::trim).collect();
    if !te.is_empty() {
        if !cl.is_empty() { return Err("reply has both Transfer-Encoding and Content-Length".into()); }
        return if te.last().is_some_and(|c| c.eq_ignore_ascii_case("chunked")) { Ok(Framing::Chunked) } else { Err("unsupported Transfer-Encoding".into()) };
    }
    let mut n = None;
    for c in cl {
        if c.is_empty() || !c.bytes().all(|b| b.is_ascii_digit()) { return Err("bad Content-Length".into()); }
        let v: u64 = c.parse().map_err(|_| "bad Content-Length".to_string())?;
        if n.is_some_and(|p| p != v) { return Err("conflicting Content-Length headers".into()); }
        n = Some(v);
    }
    Ok(n.map_or(Framing::Close, Framing::Length))
}

pub struct Body<R> { r: R, f: Framing, left: u64, done: bool }
impl<R: BufRead> Body<R> {
    pub fn new(r: R, f: Framing) -> Self { Body { r, f, left: if let Framing::Length(n) = f { n } else { 0 }, done: matches!(f, Framing::None | Framing::Length(0)) } }
    fn chunk_head(&mut self) -> io::Result<()> {
        let l = read_line(&mut self.r, MAX_LINE)?.ok_or(io::ErrorKind::UnexpectedEof)?;
        let t = String::from_utf8_lossy(&l); let h = t.split(';').next().unwrap_or("").trim();
        if h.is_empty() || h.len() > 16 || !h.bytes().all(|b| b.is_ascii_hexdigit()) { return Err(bad("bad chunk size")); }
        self.left = u64::from_str_radix(h, 16).map_err(|_| bad("bad chunk size"))?;
        if self.left == 0 {   // trailers are read and dropped
            for _ in 0..=MAX_HEADERS { if read_line(&mut self.r, MAX_LINE)?.ok_or(io::ErrorKind::UnexpectedEof)?.is_empty() { self.done = true; return Ok(()); } }
            return Err(bad("too many trailers"));
        }
        Ok(())
    }
}
impl<R: BufRead> Read for Body<R> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if self.done || out.is_empty() { return Ok(0); }
        if self.f == Framing::Close { return self.r.read(out); }
        if self.f == Framing::Chunked && self.left == 0 {
            self.chunk_head()?;
            if self.done { return Ok(0); }
        }
        let want = out.len().min(self.left.min(usize::MAX as u64) as usize);
        let n = self.r.read(&mut out[..want])?;
        if n == 0 { return Err(io::ErrorKind::UnexpectedEof.into()); }
        self.left -= n as u64;
        if self.left == 0 {
            if self.f == Framing::Chunked { if !read_line(&mut self.r, MAX_LINE)?.ok_or(io::ErrorKind::UnexpectedEof)?.is_empty() { return Err(bad("bad chunk end")); } }
            else { self.done = true; }
        }
        Ok(n)
    }
}

/// Server-sent events, fed with whatever bytes arrive.
#[derive(Default)]
pub struct Sse { buf: Vec<u8>, event: String, data: String, has_data: bool }
impl Sse {
    pub fn feed(&mut self, bytes: &[u8], out: &mut Vec<(String, String)>) -> Result<(), String> {
        for &b in bytes {
            if b != b'\n' { self.buf.push(b); if self.buf.len() > MAX_SSE_DATA + 64 { return Err("event line too long".into()); } continue; }
            if self.buf.last() == Some(&b'\r') { self.buf.pop(); }
            let line = String::from_utf8_lossy(&std::mem::take(&mut self.buf)).into_owned();
            if line.is_empty() {
                if self.has_data { out.push((if self.event.is_empty() { "message".into() } else { std::mem::take(&mut self.event) }, std::mem::take(&mut self.data))); }
                self.event.clear(); self.data.clear(); self.has_data = false;
            } else if !line.starts_with(':') {
                let (f, v) = line.split_once(':').map_or((line.as_str(), ""), |(f, v)| (f, v.strip_prefix(' ').unwrap_or(v)));
                match f {
                    "event" => self.event = v.to_string(),
                    "data" => {
                        if self.data.len() + v.len() + 1 > MAX_SSE_DATA { return Err("event data too large".into()); }
                        if self.has_data { self.data.push('\n'); }
                        self.data.push_str(v); self.has_data = true;
                    }
                    _ => {}
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufReader;
    struct One<'a>(&'a [u8]);   // one byte per read
    impl Read for One<'_> { fn read(&mut self, b: &mut [u8]) -> io::Result<usize> { if self.0.is_empty() || b.is_empty() { return Ok(0); } b[0] = self.0[0]; self.0 = &self.0[1..]; Ok(1) } }
    fn u(s: &str) -> Url { Url::parse(s).unwrap() }
    fn head(raw: &str) -> Head { read_head(&mut raw.as_bytes()).unwrap() }
    fn body(raw: &str, f: Framing) -> io::Result<Vec<u8>> { let mut v = vec![]; Body::new(BufReader::with_capacity(1, One(raw.as_bytes())), f).read_to_end(&mut v).map(|_| v) }

    #[test] fn url_forms() {
        assert_eq!(u("HTTP://Example.COM"), Url { https: false, host: "example.com".into(), port: 80, path: "/".into() });
        assert_eq!(u("https://a.b:8443/x?y=1#frag").path, "/x?y=1"); assert_eq!(u("https://a.b:8443/x").port, 8443);
        assert_eq!(u("https://a.b?q=1").path, "/?q=1");
        let v6 = u("http://[::1]:11434/v1"); assert_eq!((v6.host.as_str(), v6.port), ("::1", 11434)); assert_eq!(v6.host_header(), "[::1]:11434");
        assert_eq!(u("https://10.0.2.2:18443/").host_header(), "10.0.2.2:18443"); assert_eq!(u("https://x.org/").host_header(), "x.org");
        for bad in ["ftp://x/", "x.org", "http://u@x/", "http://x /", "http://x/\u{e9}", "http://x:0/", "http://x:99999/", "http:///p", "http://[::1/", "http://x/\x7f"] { assert!(Url::parse(bad).is_err(), "{bad}"); }
    }
    #[test] fn join_forms() {
        let b = u("https://a.org/dir/page?x=1");
        assert_eq!(b.join("https://c.org/z").unwrap().host, "c.org");
        assert_eq!(b.join("//c.org/z").unwrap(), u("https://c.org/z"));
        assert_eq!(b.join("/abs").unwrap().path, "/abs");
        assert_eq!(b.join("?q=2").unwrap().path, "/dir/page?q=2");
        assert_eq!(b.join("other").unwrap().path, "/dir/other");
        assert_eq!(u("http://a.org").join("o").unwrap().path, "/o");
        assert!(b.join("ftp://x/").is_err()); assert!(b.join("mailto:x").is_err());
        assert!(b.same_origin(&u("https://a.org:443/q"))); assert!(!b.same_origin(&u("http://a.org/")));
    }
    #[test] fn request_bytes() {
        let r = request("POST", &u("http://10.0.2.2:18080/echo?a=1"), &[("X-A".into(), "b".into())], Some(3));
        assert_eq!(r, b"POST /echo?a=1 HTTP/1.1\r\nHost: 10.0.2.2:18080\r\nX-A: b\r\nConnection: close\r\nContent-Length: 3\r\n\r\n");
        assert_eq!(request("GET", &u("https://x.org/"), &[], None), b"GET / HTTP/1.1\r\nHost: x.org\r\nConnection: close\r\n\r\n");
        assert!(token("X-Api_Key") && !token("a b") && !token("") && !token("a:b")); assert!(value_ok("x y") && !value_ok("a\r\nb") && !value_ok("a\0"));
    }
    #[test] fn head_parsing() {
        let h = head("HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 200 OK\r\nContent-Length: 5\r\nX-A:  b \r\n\r\n");
        assert_eq!(h.status, 200); assert_eq!(h.get("x-a"), Some("b")); assert_eq!(h.lines[0], "HTTP/1.1 200 OK");
        assert_eq!(head("HTTP/1.0 404\r\n\r\n").status, 404);
        let many = format!("HTTP/1.1 200 OK\r\n{}\r\n", "A: b\r\n".repeat(101));
        assert!(read_head(&mut many.as_bytes()).is_err());
        assert!(read_head(&mut "HTTP/2 200\r\n\r\n".as_bytes()).is_err()); assert!(read_head(&mut "".as_bytes()).is_err());
        assert!(read_head(&mut format!("HTTP/1.1 200 {}\r\n\r\n", "x".repeat(9000)).as_bytes()).is_err());
        assert!(read_head(&mut "HTTP/1.1 200 OK\r\nBad Name: x\r\n\r\n".as_bytes()).is_err());
    }
    #[test] fn framing_rules() {
        let f = |raw: &str, hr: bool| framing(&head(raw), hr);
        assert_eq!(f("HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\n", false), Ok(Framing::Length(5)));
        assert_eq!(f("HTTP/1.1 200 OK\r\nContent-Length: 5\r\nContent-Length: 5\r\n\r\n", false), Ok(Framing::Length(5)));
        assert!(f("HTTP/1.1 200 OK\r\nContent-Length: 5\r\nContent-Length: 6\r\n\r\n", false).is_err());
        assert!(f("HTTP/1.1 200 OK\r\nContent-Length: -1\r\n\r\n", false).is_err());
        assert!(f("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nContent-Length: 5\r\n\r\n", false).is_err());
        assert!(f("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked, gzip\r\n\r\n", false).is_err());
        assert_eq!(f("HTTP/1.1 200 OK\r\nTransfer-Encoding: gzip, Chunked\r\n\r\n", false), Ok(Framing::Chunked));
        assert_eq!(f("HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\n", true), Ok(Framing::None));
        assert_eq!(f("HTTP/1.1 204 No Content\r\n\r\n", false), Ok(Framing::None)); assert_eq!(f("HTTP/1.1 304 X\r\n\r\n", false), Ok(Framing::None));
        assert_eq!(f("HTTP/1.1 200 OK\r\n\r\n", false), Ok(Framing::Close));
    }
    #[test] fn chunked_ext_trailers_one_byte_reads() {
        let raw = "5;ext=1\r\nhello\r\n1\r\n \r\n6\r\nworld!\r\n0\r\nX-T: 1\r\n\r\nNEXT";
        assert_eq!(body(raw, Framing::Chunked).unwrap(), b"hello world!");
        assert!(body("zz\r\nx", Framing::Chunked).is_err());
        assert!(body("11111111111111111\r\nx", Framing::Chunked).is_err());
        assert!(body("5\r\nhelloXX\r\n", Framing::Chunked).is_err());
    }
    #[test] fn truncated_is_error() {
        assert_eq!(body("hello", Framing::Length(5)).unwrap(), b"hello");
        assert_eq!(body("hello", Framing::Length(3)).unwrap(), b"hel");
        assert_eq!(body("hel", Framing::Length(5)).unwrap_err().kind(), io::ErrorKind::UnexpectedEof);
        assert_eq!(body("5\r\nhel", Framing::Chunked).unwrap_err().kind(), io::ErrorKind::UnexpectedEof);
        assert_eq!(body("5\r\nhello\r\n", Framing::Chunked).unwrap_err().kind(), io::ErrorKind::UnexpectedEof);
        assert_eq!(body("abc", Framing::Close).unwrap(), b"abc"); assert_eq!(body("abc", Framing::None).unwrap(), b"");
    }
    const STREAM: &str = ": c\r\nevent: message_start\r\ndata: {\"a\":1}\r\n\r\nevent: ping\ndata: x\ndata: y\n\ndata: only\n\nevent: lone\n\nevent: e\r\ndata: z\r\n\r\n";
    fn want() -> Vec<(String, String)> { [("message_start", "{\"a\":1}"), ("ping", "x\ny"), ("message", "only"), ("e", "z")].iter().map(|(a, b)| (a.to_string(), b.to_string())).collect() }
    #[test] fn sse_whole_bytewise_and_every_split() {
        let mut o = vec![]; Sse::default().feed(STREAM.as_bytes(), &mut o).unwrap(); assert_eq!(o, want());
        let mut s = Sse::default(); let mut o = vec![]; for b in STREAM.as_bytes() { s.feed(&[*b], &mut o).unwrap(); } assert_eq!(o, want());
        for i in 0..=STREAM.len() { let mut s = Sse::default(); let mut o = vec![]; s.feed(&STREAM.as_bytes()[..i], &mut o).unwrap(); s.feed(&STREAM.as_bytes()[i..], &mut o).unwrap(); assert_eq!(o, want(), "split {i}"); }
    }
    #[test] fn sse_limits() {
        let mut o = vec![]; assert!(Sse::default().feed(&vec![b'a'; MAX_SSE_DATA + 100], &mut o).is_err());
        let mut s = Sse::default(); let line = format!("data: {}\n", "a".repeat(600_000));
        assert!(s.feed(line.as_bytes(), &mut o).is_ok()); assert!(s.feed(line.as_bytes(), &mut o).is_err());
    }
}
