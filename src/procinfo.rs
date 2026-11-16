//! Parsers for /proc text that `top` shows, plus its small formatters. Pure, tested on sample text.
pub struct Cpu { pub busy: u64, pub total: u64 }
pub struct Mem { pub total: u64, pub avail: u64, pub cached: u64 }   // kB

/// One entry per `cpuN` line (the aggregate `cpu ` line is skipped). idle = idle + iowait.
pub fn parse_stat(s: &str) -> Vec<Cpu> {
    s.lines().filter(|l| l.starts_with("cpu") && l.as_bytes().get(3).is_some_and(u8::is_ascii_digit)).map(|l| {
        let v: Vec<u64> = l.split_whitespace().skip(1).map(|x| x.parse().unwrap_or(0)).collect();
        let total: u64 = v.iter().take(8).sum();   // user..steal; guest is already inside user
        let idle = v.get(3).copied().unwrap_or(0) + v.get(4).copied().unwrap_or(0);
        Cpu { busy: total.saturating_sub(idle), total }
    }).collect()
}

pub fn parse_meminfo(s: &str) -> Mem {
    let get = |k: &str| s.lines().find_map(|l| l.strip_prefix(k)?.strip_prefix(':')?.split_whitespace().next()?.parse().ok()).unwrap_or(0);
    Mem { total: get("MemTotal"), avail: get("MemAvailable"), cached: get("Cached") }
}

/// (comm, state, utime + stime ticks). comm sits between the first '(' and the LAST ')', so names with spaces or parens work.
pub fn parse_pid_stat(s: &str) -> Option<(String, char, u64)> {
    let (a, b) = (s.find('(')?, s.rfind(')')?);
    if b < a { return None }
    let f: Vec<&str> = s[b + 1..].split_whitespace().collect();   // f[0] = state (field 3); utime = field 14 = f[11]
    Some((s[a + 1..b].to_string(), f.first()?.chars().next()?, f.get(11)?.parse::<u64>().ok()? + f.get(12)?.parse::<u64>().ok()?))
}

/// (real uid, VmRSS in kB; 0 when absent, as for kernel threads)
pub fn parse_status(s: &str) -> (u32, u64) {
    let num = |k: &str| s.lines().find_map(|l| l.strip_prefix(k)?.split_whitespace().next()?.parse::<u64>().ok());
    (num("Uid:").unwrap_or(u32::MAX as u64) as u32, num("VmRSS:").unwrap_or(0))
}

pub fn uptime_str(secs: u64) -> String {
    let (d, h, m) = (secs / 86400, secs / 3600 % 24, secs / 60 % 60);
    if d > 0 { format!("{d}d {h:02}:{m:02}") } else { format!("{h:02}:{m:02}") }
}
pub fn pct(dbusy: u64, dtotal: u64) -> f32 { if dtotal == 0 { 0.0 } else { (dbusy as f32 * 100.0 / dtotal as f32).clamp(0.0, 100.0) } }
pub fn bar(frac: f32, width: usize) -> String {
    let n = ((frac.clamp(0.0, 1.0) * width as f32).round() as usize).min(width);
    format!("[{}{}]", "|".repeat(n), " ".repeat(width - n))
}
pub fn size_kb(kb: u64) -> String {
    if kb >= 1 << 20 { format!("{:.1}G", kb as f32 / (1 << 20) as f32) } else if kb >= 1024 { format!("{:.1}M", kb as f32 / 1024.0) } else { format!("{kb}K") }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn stat_two_cores() {
        let s = "cpu  300 0 100 1000 100 0 0 0 0 0\ncpu0 100 0 50 500 50 0 0 0 0 0\ncpu1 200 0 50 500 50 0 0 0 0 0\nintr 1\n";
        let c = parse_stat(s);
        assert_eq!(c.len(), 2);
        assert_eq!((c[0].busy, c[0].total), (150, 700));
        assert_eq!((c[1].busy, c[1].total), (250, 800));
    }
    #[test] fn meminfo() {
        let m = parse_meminfo("MemTotal:  250000 kB\nMemFree: 1 kB\nMemAvailable:  200000 kB\nCached:   30000 kB\n");
        assert_eq!((m.total, m.avail, m.cached), (250000, 200000, 30000));
        assert_eq!(parse_meminfo("").total, 0);
    }
    #[test] fn pid_stat_with_parens() {
        let s = "42 (a (b) c) S 1 42 42 0 -1 4194560 100 0 0 0 7 5 0 0 20 0 1 0 100 1000 50 18446744073709551615";
        assert_eq!(parse_pid_stat(s), Some(("a (b) c".into(), 'S', 12)));
        assert_eq!(parse_pid_stat("garbage"), None);
        assert_eq!(parse_pid_stat("1 (x) S 1"), None);
    }
    #[test] fn status() {
        assert_eq!(parse_status("Name:\tx\nUid:\t1000\t1000\t1000\t1000\nVmRSS:\t   1234 kB\n"), (1000, 1234));
        assert_eq!(parse_status("Name:\tkthreadd\nUid:\t0\t0\t0\t0\n"), (0, 0));
    }
    #[test] fn formats() {
        assert_eq!(pct(50, 100), 50.0); assert_eq!(pct(1, 0), 0.0);
        assert_eq!(bar(0.5, 10), "[|||||     ]"); assert_eq!(bar(2.0, 4), "[||||]");
        assert_eq!(uptime_str(93785), "1d 02:03"); assert_eq!(uptime_str(7980), "02:13");
        assert_eq!((size_kb(512).as_str(), size_kb(1536).as_str()), ("512K", "1.5M"));
    }
}
