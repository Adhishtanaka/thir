//! Connections for curl and agent: TCP, TLS (the only place rustls is used), timeouts, and one request/reply exchange.
use rustls::{pki_types::{pem::PemObject, CertificateDer, ServerName}, ClientConfig, ClientConnection, RootCertStore, StreamOwned};
use std::{io::{self, BufReader, Read, Write}, net::{TcpStream, ToSocketAddrs}, sync::Arc, time::{Duration, Instant}};
use thir::{clock, http::{self, Framing, Head, Url}};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind { Resolve, Connect, Head, Short, Timeout, Tls, Cert, Ca, Io }
#[derive(Debug)]
pub struct NetErr { pub kind: Kind, pub msg: String }
impl NetErr {
    pub fn new(kind: Kind, msg: impl Into<String>) -> Self { NetErr { kind, msg: msg.into() } }
    /// curl's exit code for this error.
    pub fn code(&self) -> i32 { match self.kind { Kind::Resolve => 6, Kind::Connect => 7, Kind::Head => 8, Kind::Short => 18, Kind::Timeout => 28, Kind::Tls => 35, Kind::Ca => 77, Kind::Cert => 60, Kind::Io => 56 } }
}

/// Maps an io error from the socket or from rustls (which hides its error inside an io error).
pub fn from_io(e: &io::Error) -> NetErr {
    if let Some(r) = e.get_ref().and_then(|i| i.downcast_ref::<rustls::Error>()) { return from_tls(r); }
    match e.kind() {
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock => NetErr::new(Kind::Timeout, "timed out"),
        io::ErrorKind::InvalidData => NetErr::new(Kind::Head, e.to_string()),
        _ => NetErr::new(Kind::Io, e.to_string()),
    }
}

fn from_tls(r: &rustls::Error) -> NetErr {
    use rustls::{CertificateError as C, Error};
    match r {
        Error::InvalidCertificate(c) => match c {
            C::Expired | C::ExpiredContext { .. } | C::NotValidYet | C::NotValidYetContext { .. } => NetErr::new(Kind::Cert,
                format!("certificate expired or not yet valid. The clock says {}. A wrong clock causes this; check `date`.", clock::date_line(clock::now(), 0))),
            C::UnknownIssuer => NetErr::new(Kind::Cert, "certificate not trusted"),
            C::NotValidForName | C::NotValidForNameContext { .. } => NetErr::new(Kind::Cert, "certificate is for another name"),
            o => NetErr::new(Kind::Cert, format!("certificate problem: {o:?}")),
        },
        o => NetErr::new(Kind::Tls, format!("TLS error: {o}")),
    }
}

/// Sets the socket timeouts to the time left: the smaller of the whole-job deadline and the idle limit. Err(TimedOut) when none is left.
fn arm(t: &TcpStream, deadline: Option<Instant>, idle: Option<Duration>) -> io::Result<()> {
    let left = deadline.map(|d| d.saturating_duration_since(Instant::now()));
    let w = match (left, idle) { (Some(a), Some(b)) => Some(a.min(b)), (a, b) => a.or(b) };
    if w == Some(Duration::ZERO) { return Err(io::ErrorKind::TimedOut.into()); }
    t.set_read_timeout(w)?; t.set_write_timeout(w)
}

enum Stream { Plain(TcpStream), Tls(Box<StreamOwned<ClientConnection, TcpStream>>) }

pub struct Conn { s: Stream, ctl: TcpStream, deadline: Option<Instant>, idle: Option<Duration> }
impl Conn { pub fn is_tls(&self) -> bool { matches!(self.s, Stream::Tls(_)) } }
impl Read for Conn {
    fn read(&mut self, b: &mut [u8]) -> io::Result<usize> {
        arm(&self.ctl, self.deadline, self.idle)?;
        match &mut self.s { Stream::Plain(s) => s.read(b), Stream::Tls(s) => s.read(b) }
    }
}
impl Write for Conn {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        arm(&self.ctl, self.deadline, self.idle)?;
        match &mut self.s { Stream::Plain(s) => s.write(b), Stream::Tls(s) => s.write(b) }
    }
    fn flush(&mut self) -> io::Result<()> { match &mut self.s { Stream::Plain(s) => s.flush(), Stream::Tls(s) => s.flush() } }
}

fn config(cacert: Option<&str>) -> Result<Arc<ClientConfig>, NetErr> {
    let mut roots = RootCertStore::empty();
    match cacert {
        None => roots.roots = webpki_roots::TLS_SERVER_ROOTS.to_vec(),   // Mozilla's roots, compiled in
        Some(path) => {   // replaces the roots for this call; the certificate is still checked
            let it = CertificateDer::pem_file_iter(path).map_err(|e| NetErr::new(Kind::Ca, format!("cannot read {path}: {e}")))?;
            for c in it { let c = c.map_err(|e| NetErr::new(Kind::Ca, format!("bad certificate in {path}: {e}")))?; roots.add(c).map_err(|e| NetErr::new(Kind::Ca, format!("bad certificate in {path}: {e}")))?; }
            if roots.is_empty() { return Err(NetErr::new(Kind::Ca, format!("no certificates found in {path}"))); }
        }
    }
    let mut c = ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions().map_err(|e| NetErr::new(Kind::Tls, e.to_string()))?
        .with_root_certificates(roots).with_no_client_auth();
    c.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(Arc::new(c))
}

pub fn connect(url: &Url, deadline: Option<Instant>, idle: Option<Duration>, cacert: Option<&str>) -> Result<Conn, NetErr> {
    let cfg = if url.https { Some(config(cacert)?) } else { None };   // a bad --cacert fails before any network traffic
    let mut addrs: Vec<_> = (url.host.as_str(), url.port).to_socket_addrs()
        .map_err(|_| NetErr::new(Kind::Resolve, format!("could not resolve host {} (is the network up? see ip)", url.host)))?.collect();
    addrs.sort_by_key(|a| a.is_ipv6());   // IPv4 first
    if addrs.is_empty() { return Err(NetErr::new(Kind::Resolve, format!("could not resolve host {} (is the network up? see ip)", url.host))); }
    let (mut last, mut tcp) = (String::new(), None);
    let budget = deadline.unwrap_or_else(|| Instant::now() + Duration::from_secs(30));   // without --max-time: 30 s to connect, in all
    for a in addrs {
        let t = Duration::from_secs(10).min(budget.saturating_duration_since(Instant::now()));
        if t.is_zero() { return Err(NetErr::new(Kind::Timeout, "timed out")); }
        match TcpStream::connect_timeout(&a, t) {
            Ok(s) => { tcp = Some(s); break; }
            Err(e) if e.kind() == io::ErrorKind::TimedOut => last = "timed out".into(),
            Err(e) => last = e.to_string(),
        }
    }
    let Some(tcp) = tcp else { return Err(NetErr::new(Kind::Connect, format!("could not connect to {}:{} ({last})", url.host, url.port))) };
    let ctl = tcp.try_clone().map_err(|e| from_io(&e))?;
    let s = match cfg {
        None => Stream::Plain(tcp),
        Some(cfg) => {
            let name = ServerName::try_from(url.host.clone()).map_err(|_| NetErr::new(Kind::Tls, "bad host name"))?;
            let mut tls = ClientConnection::new(cfg, name).map_err(|e| from_tls(&e))?;
            let mut tcp = tcp;
            while tls.is_handshaking() {
                arm(&tcp, deadline, idle).map_err(|e| from_io(&e))?;
                tls.complete_io(&mut tcp).map_err(|e| from_io(&e))?;
            }
            Stream::Tls(Box::new(StreamOwned::new(tls, tcp)))
        }
    };
    Ok(Conn { s, ctl, deadline, idle })
}

/// One request and its reply head. The body is still unread in the returned reader.
pub fn exchange(url: &Url, method: &str, headers: &[(String, String)], body: Option<&[u8]>, deadline: Option<Instant>, idle: Option<Duration>, cacert: Option<&str>)
    -> Result<(Head, Framing, BufReader<Conn>), NetErr> {
    let mut c = connect(url, deadline, idle, cacert)?;
    c.write_all(&http::request(method, url, headers, body.map(<[u8]>::len))).map_err(|e| from_io(&e))?;
    if let Some(b) = body { c.write_all(b).map_err(|e| from_io(&e))?; }
    c.flush().map_err(|e| from_io(&e))?;
    let mut r = BufReader::new(c);
    let head = http::read_head(&mut r).map_err(|e| from_io(&e))?;
    let f = http::framing(&head, method == "HEAD").map_err(|m| NetErr::new(Kind::Head, m))?;
    Ok((head, f, r))
}

/// A body read error as a NetErr. A TLS connection that ends without close_notify is only an error for bodies that end at the close.
pub fn body_err(e: &io::Error, f: Framing, tls: bool) -> NetErr {
    if e.kind() == io::ErrorKind::UnexpectedEof {
        return if f == Framing::Close && tls { NetErr::new(Kind::Io, "connection closed without TLS close_notify; body may be cut off") }
            else { NetErr::new(Kind::Short, "the reply ended before all of the body arrived") };
    }
    from_io(e)
}
