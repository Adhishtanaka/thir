//! top: CPU, memory and processes, refreshed every second. Parsers live in thir::procinfo.
use std::{collections::HashMap, fs, process::exit, time::{Duration, Instant}};
use thir::{clock, files::printable, line::Decoder, procinfo::*, term::{self, put, Raw, RST}};

struct Proc { pid: u32, user: String, state: char, cpu: f32, rss: u64, cmd: String }
#[derive(Clone, Copy, PartialEq)]
enum Sort { Cpu, Mem, Pid }

fn fit(s: &str, w: usize) -> String { s.chars().take(w).collect() }
fn color(p: f32) -> &'static str { if p < 50.0 { "\x1b[32m" } else if p < 80.0 { "\x1b[33m" } else { "\x1b[31m" } }
fn read(p: &str) -> String { fs::read_to_string(p).unwrap_or_default() }

fn totals(s: &str) -> (u64, u64) { parse_stat(s).iter().fold((0, 0), |a, c| (a.0 + c.busy, a.1 + c.total)) }

struct Sample { cpu: f32, mem: Mem, load: String, up: u64, procs: Vec<Proc> }

struct Sampler { cpu: (u64, u64), ticks: HashMap<u32, u64>, at: Instant, hz: f32, me: u32 }
impl Sampler {
    fn take(&mut self) -> Sample {
        let dt = self.at.elapsed().as_secs_f32().max(0.05); self.at = Instant::now();
        let t = totals(&read("/proc/stat"));
        let cpu = pct(t.0.saturating_sub(self.cpu.0), t.1.saturating_sub(self.cpu.1)); self.cpu = t;
        let user_name = std::env::var("USER").unwrap_or_default();
        let (mut procs, mut ticks) = (vec![], HashMap::new());
        for e in fs::read_dir("/proc").into_iter().flatten().flatten() {
            let Some(pid) = e.file_name().to_str().and_then(|n| n.parse::<u32>().ok()) else { continue };
            let Some((comm, state, t)) = parse_pid_stat(&read(&format!("/proc/{pid}/stat"))) else { continue };   // exited meanwhile
            let (uid, rss) = parse_status(&read(&format!("/proc/{pid}/status")));
            let cmdline = read(&format!("/proc/{pid}/cmdline")).replace('\0', " ");
            let cmd = if cmdline.trim().is_empty() { format!("[{comm}]") } else { cmdline.trim().to_string() };
            let cpu = self.ticks.get(&pid).map_or(0.0, |&p| t.saturating_sub(p) as f32 * 100.0 / (dt * self.hz));
            ticks.insert(pid, t);
            let user = if uid == 0 { "root".into() } else if uid == self.me { user_name.clone() } else { uid.to_string() };
            procs.push(Proc { pid, user: printable(&user), state, cpu, rss, cmd: printable(&cmd) });
        }
        self.ticks = ticks;
        let load = read("/proc/loadavg").split_whitespace().take(3).collect::<Vec<_>>().join(" ");
        let up = read("/proc/uptime").split('.').next().and_then(|s| s.parse().ok()).unwrap_or(0);
        Sample { cpu, mem: parse_meminfo(&read("/proc/meminfo")), load, up, procs }
    }
}

fn frame(s: &Sample, sort: Sort, msg: &str, cols: usize, rows: usize) -> Vec<u8> {
    if rows < 8 || cols < 40 { return b"\x1b[H\x1b[2Jtop: terminal too small".to_vec() }
    let w = cols - 1;
    let l = clock::local(clock::now(), clock::home_tz().unwrap_or(0));
    let head = format!(" top  {} {:02} {} {:02}:{:02}:{:02}  up {}  load {}  {} tasks", clock::WDAY[l.wday as usize], l.day, clock::MONTH[l.month as usize - 1], l.hour, l.min, l.sec, uptime_str(s.up), s.load, s.procs.len());
    let quit = "q quit";
    let mut o = String::from("\x1b[?25l");
    o += &format!("\x1b[1;1H{}{}{}\x1b[K", fit(&head, w.saturating_sub(quit.len() + 1)), " ".repeat(w.saturating_sub(head.chars().count().min(w.saturating_sub(quit.len() + 1)) + quit.len())), quit);
    let bw = cols.saturating_sub(14).clamp(4, 40);
    let used = s.mem.total.saturating_sub(s.mem.avail);
    let mf = if s.mem.total == 0 { 0.0 } else { used as f32 / s.mem.total as f32 };
    o += &format!("\x1b[2;1H CPU  {}{}{RST}  {:>3.0}%\x1b[K", color(s.cpu), bar(s.cpu / 100.0, bw), s.cpu);
    let tail = format!("   {} used / {}   {} cached", size_kb(used), size_kb(s.mem.total), size_kb(s.mem.cached));
    o += &format!("\x1b[3;1H Mem  {}{}{RST}  {:>3.0}%{}\x1b[K", color(mf * 100.0), bar(mf, bw), mf * 100.0, fit(&tail, w.saturating_sub(bw + 12)));
    let name = match sort { Sort::Cpu => "cpu", Sort::Mem => "mem", Sort::Pid => "pid" };
    o += &format!("\x1b[4;1H{}\x1b[K\x1b[5;1H{}\x1b[K", fit(&format!(" sort: {name}   (s cpu  m mem  p pid  k kill)"), w), fit(&printable(msg), w));
    o += &format!("\x1b[6;1H{}{}\x1b[0m\x1b[K", term::REV, fit(&format!("{:>6} {:<9} {}  {:>4} {:>6}  {}", "PID", "USER", "S", "%CPU", "RSS", "COMMAND"), w));
    let mut v: Vec<&Proc> = s.procs.iter().collect();
    match sort {
        Sort::Cpu => v.sort_by(|a, b| b.cpu.total_cmp(&a.cpu).then(a.pid.cmp(&b.pid))),
        Sort::Mem => v.sort_by(|a, b| b.rss.cmp(&a.rss).then(a.pid.cmp(&b.pid))),
        Sort::Pid => v.sort_by_key(|p| p.pid),
    }
    for r in 7..=rows {
        let line = v.get(r - 7).map_or(String::new(), |p| fit(&format!("{:>6} {:<9} {}  {:>4.1} {:>6}  {}", p.pid, fit(&p.user, 9), p.state, p.cpu, size_kb(p.rss), p.cmd), w));
        o += &format!("\x1b[{r};1H{line}\x1b[K");
    }
    o.into_bytes()
}

fn kill(dec: &mut Decoder, rows: usize) -> String {
    let Some(p) = term::prompt(dec, rows, "kill pid: ") else { return String::new() };
    let Some(pid) = p.trim().parse::<i32>().ok().filter(|&n| n > 0) else { return "bad pid".into() };
    match term::prompt(dec, rows, &format!("kill {pid}? (y/n) ")) {
        Some(a) if a.trim().eq_ignore_ascii_case("y") => {
            if unsafe { libc::kill(pid, libc::SIGTERM) } == 0 { return format!("sent SIGTERM to {pid}") }
            match std::io::Error::last_os_error().raw_os_error() { Some(libc::EPERM) => "not your process".into(), Some(libc::ESRCH) => "no such process".into(), _ => "kill failed".into() }
        }
        _ => String::new(),
    }
}

fn main() {
    if unsafe { libc::isatty(0) != 1 || libc::isatty(1) != 1 } { eprintln!("top: not a terminal"); exit(1) }
    let Some(_raw) = Raw::enter() else { eprintln!("top: not a terminal"); exit(1) };
    let mut sm = Sampler { cpu: totals(&read("/proc/stat")), ticks: HashMap::new(), at: Instant::now(), hz: unsafe { libc::sysconf(libc::_SC_CLK_TCK) }.max(1) as f32, me: unsafe { libc::getuid() } };
    sm.take();   // primes the per-process tick table
    let (mut sort, mut msg, mut dec) = (Sort::Cpu, String::new(), Decoder::new());
    std::thread::sleep(Duration::from_millis(300));
    let mut sample = sm.take();
    let mut next = Instant::now() + Duration::from_secs(1);
    loop {
        let (cols, rows) = term::size();
        put(&frame(&sample, sort, &msg, cols, rows));
        let wait = next.saturating_duration_since(Instant::now()).as_millis() as i32;
        let Some(keys) = term::read_keys(&mut dec, wait) else { break };
        for k in keys {
            use thir::line::Key::*;
            msg.clear();
            match k {
                Char('q') | Ctrl(b'c') => { put(b"\x1b[H\x1b[2J"); return }
                Char('s') => sort = Sort::Cpu, Char('m') => sort = Sort::Mem, Char('p') => sort = Sort::Pid,
                Char('k') => { msg = kill(&mut dec, rows); }
                _ => {}
            }
        }
        if Instant::now() >= next { sample = sm.take(); next = Instant::now() + Duration::from_secs(1); }
    }
    put(b"\x1b[H\x1b[2J");
}
