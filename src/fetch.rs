//! neofetch-style system summary: a bear logo beside info lines. Pure formatting and parsers, tested on sample text; src/bin/fetch.rs gathers the facts.
use crate::procinfo::size_kb;

pub const LOGO: [&str; 8] = [
    r"  .--.     .--.  ",
    r" /    \---/    \ ",
    r" \             / ",
    r" |   o     o   | ",
    r" |      ^      | ",
    r" |    \___/    | ",
    r"  \           /  ",
    r"   '---._.---'   ",
];
const FUR: &str = "\x1b[33m";
const LABEL: &str = "\x1b[1;33m";
const NAME: &str = "\x1b[1;91m";
const RST: &str = "\x1b[0m";

/// /sys/class/graphics/fb0/virtual_size is `W,H`.
pub fn parse_fb_size(s: &str) -> Option<String> {
    let (w, h) = s.trim().split_once(',')?;
    Some(format!("{}x{}", w.parse::<u32>().ok()?, h.parse::<u32>().ok()?))
}

/// "N x model" when /proc/cpuinfo names one (x86: `model name`; some ARM kernels: `Hardware`/`Model`), else just the count ("2 cores").
pub fn cpu_line(cpuinfo: &str, count: usize) -> String {
    let model = cpuinfo.lines().find_map(|l| {
        let (k, v) = l.split_once(':')?;
        (matches!(k.trim(), "model name" | "Hardware" | "Model") && !v.trim().is_empty()).then(|| v.trim().to_string())
    });
    match model { Some(m) => format!("{count} x {m}"), None => format!("{count} {}", if count == 1 { "core" } else { "cores" }) }
}

/// `used / total (pct%)` from kB values.
pub fn usage_kb(used: u64, total: u64) -> String {
    format!("{} / {} ({}%)", size_kb(used), size_kb(total), if total == 0 { 0 } else { used * 100 / total })
}

pub struct Info { pub user: String, pub host: String, pub rows: Vec<(&'static str, String)> }

/// The picture: logo left (fixed width), info right, `width` columns at most. Ends with a row of the 8 palette colors.
pub fn render(info: &Info, width: usize) -> String {
    let lw = LOGO[0].chars().count() + 2;
    let title = format!("{}@{}", info.user, info.host);
    let mut right = vec![format!("{NAME}{title}{RST}"), "-".repeat(title.chars().count())];
    for (k, v) in &info.rows {
        let room = width.saturating_sub(lw + k.len() + 2).max(1);
        right.push(format!("{LABEL}{k}{RST}: {}", v.chars().take(room).collect::<String>()));
    }
    right.push(String::new());
    right.push((40..48).map(|c| format!("\x1b[{c}m  ")).collect::<String>() + RST);
    let mut o = String::new();
    for i in 0..LOGO.len().max(right.len()) {
        let logo = LOGO.get(i).copied().unwrap_or("");
        o += &format!("{FUR}{logo:<w$}{RST}  {}\n", right.get(i).map_or("", |s| s.as_str()), w = lw - 2);
    }
    o
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn fb_size() {
        assert_eq!(parse_fb_size("2560,1600\n").as_deref(), Some("2560x1600"));
        assert_eq!((parse_fb_size("x,1"), parse_fb_size("5"), parse_fb_size("")), (None, None, None));
    }
    #[test] fn cpu() {
        assert_eq!(cpu_line("processor\t: 0\nmodel name\t: Cortex-A72\n", 2), "2 x Cortex-A72");
        assert_eq!(cpu_line("processor : 0\nBogoMIPS : 50\n", 1), "1 core");
        assert_eq!(cpu_line("", 4), "4 cores");
    }
    #[test] fn usage() { assert_eq!(usage_kb(2048, 8192), "2.0M / 8.0M (25%)"); assert_eq!(usage_kb(1, 0), "1K / 0K (0%)"); }
    #[test] fn render_shape() {
        let i = Info { user: "alice".into(), host: "thir".into(), rows: vec![("OS", "thir 0.1.0".into()), ("Kernel", "6.12.117".into())] };
        let s = render(&i, 80);
        assert!(s.contains("alice@thir") && s.contains("----------") && s.contains("OS\x1b[0m: thir 0.1.0") && s.contains("Kernel"));
        assert_eq!(s.lines().count(), LOGO.len());   // title, rule, 2 rows, blank, colors: 6 < 8
        assert!(LOGO.iter().all(|l| l.chars().count() == LOGO[0].chars().count()));
        let narrow = render(&i, 24);   // long values are cut to the width, never wrapped
        assert!(narrow.contains("OS\x1b[0m: t") && !narrow.contains("0.1.0"));
    }
}
