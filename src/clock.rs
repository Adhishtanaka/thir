//! Local time from a UTC offset: pure date math, the ~/.tz reader, and the formats den, `date` and top show.
use crate::files::read_small;

pub struct Local { pub year: i64, pub month: u32, pub day: u32, pub wday: u32 /* 0 = Sun */, pub hour: u32, pub min: u32, pub sec: u32 }
pub const WDAY: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
pub const MONTH: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/// Days since 1970-01-01 -> (year, month 1-12, day 1-31). Howard Hinnant's algorithm.
pub fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + (m <= 2) as i64, m, d)
}

pub fn local(utc: i64, off_min: i32) -> Local {
    let t = utc + off_min as i64 * 60;
    let days = t.div_euclid(86400);
    let s = t.rem_euclid(86400) as u32;
    let (year, month, day) = civil_from_days(days);
    Local { year, month, day, wday: (days + 4).rem_euclid(7) as u32, hour: s / 3600, min: s / 60 % 60, sec: s % 60 }
}

/// Exactly `[+-]HH:MM`, minutes < 60, at most 14:00.
pub fn parse_offset(s: &str) -> Option<i32> {
    let b = s.as_bytes();
    if b.len() != 6 || !matches!(b[0], b'+' | b'-') || b[3] != b':' || ![1, 2, 4, 5].iter().all(|&i| b[i].is_ascii_digit()) { return None }
    let n = |i: usize| (b[i] - b'0') as i32 * 10 + (b[i + 1] - b'0') as i32;
    let v = n(1) * 60 + n(4);
    if n(4) >= 60 || v > 14 * 60 { return None }
    Some(if b[0] == b'-' { -v } else { v })
}

pub fn format_offset(m: i32) -> String { format!("{}{:02}:{:02}", if m < 0 { '-' } else { '+' }, m.abs() / 60, m.abs() % 60) }

pub fn date_line(utc: i64, off: i32) -> String {
    let l = local(utc, off);
    format!("{} {:02} {} {} {:02}:{:02}:{:02} {}", WDAY[l.wday as usize], l.day, MONTH[l.month as usize - 1], l.year, l.hour, l.min, l.sec, format_offset(off))
}

/// The den bar: `Tue 17 Nov  14:32`, with ` UTC` appended when no offset is known.
pub fn bar_line(utc: i64, off: Option<i32>) -> String {
    let l = local(utc, off.unwrap_or(0));
    let s = format!("{} {:02} {}  {:02}:{:02}", WDAY[l.wday as usize], l.day, MONTH[l.month as usize - 1], l.hour, l.min);
    if off.is_none() { s + " UTC" } else { s }
}

pub const WDAY_FULL: [&str; 7] = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
/// The den bar centre: `Friday 19:35` (24 hour, no seconds), with ` UTC` appended when no offset is known.
pub fn day_time(utc: i64, off: Option<i32>) -> String {
    let l = local(utc, off.unwrap_or(0));
    let s = format!("{} {:02}:{:02}", WDAY_FULL[l.wday as usize], l.hour, l.min);
    if off.is_none() { s + " UTC" } else { s }
}

/// The offset in a user's `.tz`. den (root) calls this every second on a user-owned path, so: never follow a symlink, never block on a FIFO,
/// accept regular files only, read at most 64 bytes, and show nothing but the parsed number (see files::read_small).
pub fn read_tz(path: &str, owner: u32) -> Option<i32> { read_small(path, owner, 64).and_then(|s| parse_offset(s.trim())) }

/// The build host's offset (build.sh passes THIR_TZ), used until a user sets ~/.tz.
pub fn default_offset() -> Option<i32> { option_env!("THIR_TZ").and_then(parse_offset) }

pub fn home_tz() -> Option<i32> {
    std::env::var("HOME").ok().and_then(|h| read_tz(&format!("{h}/.tz"), unsafe { libc::getuid() })).or_else(default_offset)
}

pub fn now() -> i64 { unsafe { libc::time(std::ptr::null_mut()) as i64 } }

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn bar_day_time() { assert_eq!(day_time(1794906125, Some(330)), "Tuesday 14:32"); assert_eq!(day_time(1794906125, None), "Tuesday 09:02 UTC"); }
    #[test] fn dates() {
        assert_eq!(date_line(0, 0), "Thu 01 Jan 1970 00:00:00 +00:00");
        assert!(date_line(951782400, 0).starts_with("Tue 29 Feb 2000"));
        assert!(date_line(1709164800, 0).starts_with("Thu 29 Feb 2024"));
        assert_eq!(date_line(1700000000, 0), "Tue 14 Nov 2023 22:13:20 +00:00");
        assert_eq!(date_line(1700000000, 330), "Wed 15 Nov 2023 03:43:20 +05:30");
        assert_eq!(date_line(100, -60), "Wed 31 Dec 1969 23:01:40 -01:00");
        assert!(date_line(1794906125, 0).starts_with("Tue 17 Nov 2026 09:02:05"));
    }
    #[test] fn bar() {
        assert_eq!(bar_line(1794906125, Some(330)), "Tue 17 Nov  14:32");
        assert!(bar_line(1794906125, None).ends_with(" UTC"));
    }
    #[test] fn offsets() {
        assert_eq!((parse_offset("+05:30"), parse_offset("-03:00"), parse_offset("+00:00"), parse_offset("+14:00")), (Some(330), Some(-180), Some(0), Some(840)));
        for bad in ["5:30", "+5:30", "+05:60", "+15:00", "", "+05:30 IST", "+05-30", "+0a:30", "+٠٥:٣٠"] { assert_eq!(parse_offset(bad), None, "{bad:?}"); }
        assert_eq!(format_offset(-180), "-03:00");
    }
    fn tmp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("thir-clock-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d); std::fs::create_dir_all(&d).unwrap(); d
    }
    #[test] fn read_tz_regular_file_only() {
        let d = tmp("reg");
        let f = d.join("tz"); std::fs::write(&f, "+05:30\n").unwrap();
        assert_eq!(read_tz(f.to_str().unwrap(), me()), Some(330));
        std::fs::write(&f, "junk").unwrap(); assert_eq!(read_tz(f.to_str().unwrap(), me()), None);
        std::fs::write(&f, "+05:30".repeat(100)).unwrap(); assert_eq!(read_tz(f.to_str().unwrap(), me()), None);
        assert_eq!(read_tz(d.join("missing").to_str().unwrap(), me()), None);
        let _ = std::fs::remove_dir_all(d);
    }
    fn me() -> u32 { unsafe { libc::getuid() } }
    #[test] fn read_tz_refuses_other_owner_and_hard_links() {
        let d = tmp("own");
        let f = d.join("tz"); std::fs::write(&f, "+05:30\n").unwrap();
        assert_eq!(read_tz(f.to_str().unwrap(), me() + 1), None);
        let l = d.join("hard"); std::fs::hard_link(&f, &l).unwrap();
        assert_eq!(read_tz(f.to_str().unwrap(), me()), None);   // nlink 2
        let _ = std::fs::remove_dir_all(d);
    }
    #[test] fn read_tz_refuses_fifo_and_symlink() {
        let d = tmp("fifo");
        let fifo = d.join("fifo");
        let c = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        assert_eq!(read_tz(fifo.to_str().unwrap(), me()), None);   // would hang here if it blocked
        let real = d.join("real"); std::fs::write(&real, "+05:30").unwrap();
        let link = d.join("link"); std::os::unix::fs::symlink(&real, &link).unwrap();
        assert_eq!(read_tz(link.to_str().unwrap(), me()), None);
        let _ = std::fs::remove_dir_all(d);
    }
}
