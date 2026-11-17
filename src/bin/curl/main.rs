//! curl: thir's small HTTP/HTTPS client. Run as `agent` (a symlink to this binary) it is the AI chat instead.
mod agent;
mod conn;

use conn::{body_err, exchange, NetErr};
use std::{fs::{File, OpenOptions}, io::{self, Read, Write}, os::unix::fs::OpenOptionsExt, process::exit, time::{Duration, Instant}};
use thir::{files::printable, http::{self, Body, Url}};

const USAGE: &str = "usage: curl [options] URL        thir's small curl: HTTP and HTTPS (HTTP/1.1)
  -o FILE          save the body in FILE       -O   save it under the URL's name
  -L               follow redirects (max 10, never from https to http)
  -I               headers only (HEAD)         -i   headers, then the body
  -f               exit 22 on HTTP errors, print no body
  -s               no progress line            -X METHOD   request method
  -H 'Name: value' add a header (repeatable)   -d DATA     send DATA (POST)
  --data-binary @FILE  send a file            --max-time N  give up after N s
  --cacert FILE    trust only the CAs in FILE
Certificates are always checked. -k is not supported.
";
const MAX_DATA: u64 = 16 << 20;

pub fn die(code: i32, msg: &str) -> ! { eprintln!("curl: {}", printable(msg)); exit(code) }

#[derive(Default)]
struct Opts {
    url: Option<String>, out: Option<String>, remote_name: bool, follow: bool, head: bool, include: bool, fail: bool, silent: bool,
    method: Option<String>, headers: Vec<(String, String)>, data: Option<Vec<u8>>, max_time: Option<u64>, cacert: Option<String>,
}

fn parse_args(args: &[String]) -> Opts {
    let mut o = Opts::default();
    let mut it = args.iter();
    let mut only_urls = false;
    while let Some(a) = it.next() {
        if only_urls || a == "-" || !a.starts_with('-') { set_url(&mut o, a); continue; }
        if a == "--" { only_urls = true; continue; }
        if let Some(l) = a.strip_prefix("--") {
            let mut val = |name: &str| it.next().cloned().unwrap_or_else(|| die(2, &format!("{name} needs a value (curl -h)")));
            match l {
                "help" => { print!("{USAGE}"); exit(0) }
                "insecure" => no_k(),
                "output" => o.out = Some(val("--output")), "remote-name" => o.remote_name = true, "location" => o.follow = true,
                "head" => o.head = true, "include" => o.include = true, "fail" => o.fail = true, "silent" | "show-error" => o.silent |= l == "silent",
                "request" => o.method = Some(val("--request")), "header" => add_header(&mut o, &val("--header")), "data" => set_data(&mut o, &val("--data"), false),
                "data-binary" => set_data(&mut o, &val("--data-binary"), true),
                "max-time" => o.max_time = Some(val("--max-time").parse().unwrap_or_else(|_| die(2, "--max-time needs a number of seconds"))),
                "cacert" => o.cacert = Some(val("--cacert")),
                _ => die(2, &format!("unknown option --{l} (curl -h)")),
            }
            continue;
        }
        let cs: Vec<char> = a[1..].chars().collect();
        for (i, &c) in cs.iter().enumerate() {
            let mut take = |name: &str| { let rest: String = cs[i + 1..].iter().collect(); if rest.is_empty() { it.next().cloned().unwrap_or_else(|| die(2, &format!("-{name} needs a value (curl -h)"))) } else { rest } };
            match c {
                'k' => no_k(),
                'h' => { print!("{USAGE}"); exit(0) }
                'O' => o.remote_name = true, 'L' => o.follow = true, 'I' => o.head = true, 'i' => o.include = true, 'f' => o.fail = true, 's' => o.silent = true, 'S' => {}
                'o' | 'X' | 'H' | 'd' => {
                    let v = take(&c.to_string());
                    match c { 'o' => o.out = Some(v), 'X' => o.method = Some(v), 'H' => add_header(&mut o, &v), _ => set_data(&mut o, &v, false) }
                    break;
                }
                _ => die(2, &format!("unknown option -{c} (curl -h)")),
            }
        }
    }
    if let Some(m) = &o.method { if !http::token(m) { die(2, "-X needs a method name such as GET or POST"); } }
    o
}

fn no_k() -> ! { die(2, "-k is not supported: thir always checks certificates (use --cacert FILE for a private CA)") }
fn set_url(o: &mut Opts, a: &str) { if o.url.replace(a.to_string()).is_some() { die(2, "only one URL at a time"); } }
fn add_header(o: &mut Opts, h: &str) {
    let Some((n, v)) = h.split_once(':') else { die(2, "-H needs 'Name: value'") };
    let (n, v) = (n.trim(), v.trim());
    if !http::token(n) || !http::value_ok(v) { die(2, "bad header"); }
    if ["host", "content-length", "connection", "transfer-encoding"].contains(&n.to_ascii_lowercase().as_str()) { die(2, &format!("-H {n}: curl sets this header itself")); }
    o.headers.push((n.to_string(), v.to_string()));
}
fn set_data(o: &mut Opts, v: &str, binary: bool) {
    if !binary {
        if v.starts_with('@') { die(2, "-d @FILE is not supported: use --data-binary @FILE"); }
        o.data = Some(v.as_bytes().to_vec());
    } else {
        let Some(path) = v.strip_prefix('@') else { die(2, "--data-binary needs @FILE") };
        let r = File::open(path).and_then(|f| { let mut b = vec![]; f.take(MAX_DATA + 1).read_to_end(&mut b).map(|_| b) });
        match r { Ok(b) if b.len() as u64 <= MAX_DATA => o.data = Some(b), Ok(_) => die(26, &format!("{path}: more than 16 MiB")), Err(e) => die(26, &format!("{path}: {e}")) }
    }
}

fn net_die(e: NetErr) -> ! { die(e.code(), &e.msg) }

/// The file name for -O: the last path segment, without the query.
fn remote_name(u: &Url) -> Option<String> {
    let n = u.path.split('?').next().unwrap_or("").rsplit('/').next().unwrap_or("");
    if n.is_empty() || n == "." || n == ".." || printable(n) != n { None } else { Some(n.to_string()) }
}

fn human(n: u64) -> String { if n >= 1 << 20 { format!("{:.1} MiB", n as f64 / 1048576.0) } else if n >= 1024 { format!("{:.1} KiB", n as f64 / 1024.0) } else { format!("{n} B") } }

fn main() {
    let mut args: Vec<String> = std::env::args().collect();
    let prog = args.remove(0);
    if prog.rsplit('/').next() == Some("agent") { exit(agent::main(args)); }
    let o = parse_args(&args);
    let Some(first) = o.url.clone() else { eprint!("{USAGE}"); exit(2) };
    let mut url = Url::parse(&first).unwrap_or_else(|e| die(3, &e));
    let deadline = o.max_time.map(|s| Instant::now() + Duration::from_secs(s));
    let mut method = o.method.clone().unwrap_or_else(|| if o.head { "HEAD".into() } else if o.data.is_some() { "POST".into() } else { "GET".into() });
    let mut body = o.data.clone();
    let mut headers: Vec<(String, String)> = vec![("User-Agent".into(), "thir-curl/0.3".into()), ("Accept".into(), "*/*".into())];
    headers.extend(o.headers.iter().cloned());
    if body.is_some() && !headers.iter().any(|(n, _)| n.eq_ignore_ascii_case("content-type")) { headers.push(("Content-Type".into(), "application/x-www-form-urlencoded".into())); }
    let mut hops = 0;
    let (head, framing, reader) = loop {
        let (head, framing, reader) = exchange(&url, &method, &headers, body.as_deref(), deadline, None, o.cacert.as_deref()).unwrap_or_else(|e| net_die(e));
        let loc = head.get("location").map(str::to_string);
        if let (true, 301 | 302 | 303 | 307 | 308, Some(loc)) = (o.follow, head.status, loc) {
            hops += 1;
            if hops > 10 { die(47, "too many redirects (maximum is 10)"); }
            let next = url.join(&loc).unwrap_or_else(|e| die(1, &format!("bad redirect: {e}")));
            if url.https && !next.https { die(1, "refusing to follow a redirect from https to http"); }
            if matches!(head.status, 301 | 302 | 303) && method == "POST" { method = "GET".into(); body = None; headers.retain(|(n, _)| !n.eq_ignore_ascii_case("content-type")); }
            if !url.same_origin(&next) { headers.retain(|(n, _)| !["authorization", "cookie", "proxy-authorization"].contains(&n.to_ascii_lowercase().as_str())); }
            url = next;
            continue;
        }
        break (head, framing, reader);
    };
    let tls = reader.get_ref().is_tls();
    if o.fail && head.status >= 400 { die(22, &format!("the server answered {}", printable(head.lines[0].split_once(' ').map_or("", |x| x.1)))); }
    let mut out: Box<dyn Write> = match (&o.out, o.remote_name) {
        (None, false) => Box::new(io::stdout()),
        (path, _) => {
            let p = match path { Some(p) => p.clone(), None => remote_name(&url).unwrap_or_else(|| die(23, "cannot make a file name from this URL; use -o FILE")) };
            match OpenOptions::new().write(true).create(true).truncate(true).custom_flags(libc::O_NOFOLLOW).open(&p) {
                Ok(f) => Box::new(f), Err(e) => die(23, &format!("{p}: {e}")),
            }
        }
    };
    let to_file = o.out.is_some() || o.remote_name;
    let tty_out = !to_file && unsafe { libc::isatty(1) } == 1;
    if o.include || o.head {
        for l in &head.lines { let _ = writeln!(out, "{}", printable(l)); }
        let _ = writeln!(out);
    }
    let total = if let http::Framing::Length(n) = framing { Some(n) } else { None };
    let progress = to_file && !o.silent && unsafe { libc::isatty(2) } == 1;
    let mut body_r = Body::new(reader, framing);
    let (mut buf, mut done, mut first) = (vec![0u8; 16384], 0u64, true);
    let (t0, mut last) = (Instant::now(), Instant::now());
    loop {
        let n = match body_r.read(&mut buf) { Ok(0) => break, Ok(n) => n, Err(e) => { if progress { eprintln!(); } net_die(body_err(&e, framing, tls)) } };
        if first && tty_out && buf[..n].contains(&0) { die(23, "binary output can mess up your terminal; use -o FILE"); }
        first = false;
        if out.write_all(&buf[..n]).is_err() { die(23, "write error"); }
        done += n as u64;
        if progress && last.elapsed() >= Duration::from_millis(250) {
            last = Instant::now();
            let rate = (done as f64 / t0.elapsed().as_secs_f64().max(0.001)) as u64;
            eprint!("\r{} of {} ({}%) {}/s\x1b[K", human(done), total.map_or("?".into(), human), total.map_or("?".into(), |t| (done * 100 / t.max(1)).to_string()), human(rate));
        }
    }
    if out.flush().is_err() { die(23, "write error"); }
    if progress { eprintln!(); }
}
