//! fetch (also: neofetch): a bear and facts about this system.
use std::{fs, ffi::CStr};
use thir::{clock, fetch::*, files::printable, net, procinfo::{parse_meminfo, uptime_str}, term};

fn read(p: &str) -> String { fs::read_to_string(p).unwrap_or_default() }

fn kernel() -> String {
    let mut u: libc::utsname = unsafe { std::mem::zeroed() };
    if unsafe { libc::uname(&mut u) } != 0 { return "?".into() }
    unsafe { CStr::from_ptr(u.release.as_ptr()) }.to_string_lossy().into_owned()
}

fn main() {
    let user = std::env::var("USER").unwrap_or_else(|_| "?".into());
    let host = read("/proc/sys/kernel/hostname");
    let m = parse_meminfo(&read("/proc/meminfo"));
    let (cols, rows) = term::size();
    let cpus = unsafe { libc::sysconf(libc::_SC_NPROCESSORS_ONLN) }.max(1) as usize;
    let ip = net::interfaces().ok().and_then(|v| v.into_iter().find(|i| i.name != "lo" && i.ipv4.is_some()).map(|i| format!("{} ({})", i.ipv4.unwrap(), i.name)));
    let off = clock::home_tz().unwrap_or(0);
    let up = read("/proc/uptime").split('.').next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let info = Info {
        user: printable(&user), host: printable(host.trim()),
        rows: vec![
            ("OS", format!("thir {}", env!("CARGO_PKG_VERSION"))),
            ("Kernel", printable(&kernel())),
            ("Uptime", uptime_str(up)),
            ("Shell", "nsh".into()),
            ("Resolution", parse_fb_size(&read("/sys/class/graphics/fb0/virtual_size")).unwrap_or_else(|| "serial".into())),
            ("Terminal", format!("{cols}x{rows}")),
            ("CPU", printable(&cpu_line(&read("/proc/cpuinfo"), cpus))),
            ("Memory", usage_kb(m.total.saturating_sub(m.avail), m.total)),
            ("Disk /data", thir::disk::usage_kb("/data").map_or("n/a".into(), |(u, t)| usage_kb(u, t))),
            ("Local IP", ip.unwrap_or_else(|| "none".into())),
            ("Date", clock::date_line(clock::now(), off)),
        ],
    };
    print!("{}", render(&info, cols));
}
