//! neofetch-style system summary: a bear logo beside info lines. Pure formatting and parsers, tested on sample text; src/bin/fetch.rs gathers the facts.
use crate::procinfo::size_kb;

/// The soft bear of assets/logo.svg as a 24 x 16 pixel map (H head, S lighter ear insides and muzzle, D eyes and nose); two pixel rows per text row.
pub const BEAR: [&str; 16] = [
    "...HHH............HHH...",
    "..HHHHH..........HHHHH..",
    ".HHSSSHHHHHHHHHHHHSSSHH.",
    ".HHSSSHHHHHHHHHHHHSSSHH.",
    ".HHSSHHHHHHHHHHHHHHSSHH.",
    "..HHHHHHHHHHHHHHHHHHHH..",
    "...HHHHDHHHHHHHHDHHHH...",
    "...HHHHDDHHHHHHDDHHHH...",
    "..HHHHHHHHHHHHHHHHHHHH..",
    "..HHHHHHHHSDDSHHHHHHHH..",
    "...HHHHHHSDDDDSHHHHHH...",
    "...HHHHHSSSSSSSSHHHHH...",
    "....HHHHSSSSSSSSHHHH....",
    ".....HHHHSSSSSSHHHH.....",
    "......HHHHHSSHHHHH......",
    "........HHHHHHHH........",
];
pub const LOGO_COLS: usize = 24;
const RST: &str = "\x1b[0m";
const DIM: &str = "\x1b[90m";
const PEACH: &str = "\x1b[38;2;240;178;127m";
const FG: &str = "\x1b[39m";

/// 38 (foreground) or 48 (background) SGR of a bear pixel: head peach, soft lighter peach, dark the wall blue (the theme slots of den).
fn pix(c: u8, base: u8) -> String { let (r, g, b) = match c { b'H' => (240, 178, 127), b'S' => (248, 217, 191), _ => (8, 21, 43) }; format!("\x1b[{base};2;{r};{g};{b}m") }
/// Text row `r` (0..8) of the bear in half blocks: ▀ takes the upper pixel as its foreground and the lower as its background.
fn bear_row(r: usize) -> String {
    let (top, bot) = (BEAR[2 * r].as_bytes(), BEAR[2 * r + 1].as_bytes());
    let mut o = String::new();
    for (&t, &b) in top.iter().zip(bot) {
        o += &match (t, b) {
            (b'.', b'.') => format!("{RST} "),
            (t, b'.') => format!("{RST}{}\u{2580}", pix(t, 38)),
            (b'.', b) => format!("{RST}{}\u{2584}", pix(b, 38)),
            (t, b) if t == b => format!("{RST}{}\u{2588}", pix(t, 38)),
            (t, b) => format!("{RST}{}{}\u{2580}", pix(t, 38), pix(b, 48)),
        };
    }
    o + RST
}

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

/// The picture: bear left (fixed width), info right, `width` columns at most. Ends with two rows of the 16 palette colors.
pub fn render(info: &Info, width: usize) -> String {
    let lw = LOGO_COLS + 2;
    let title = format!("{}@{}", info.user, info.host);
    let mut right = vec![format!("{PEACH}{title}{RST}"), format!("{}{}{RST}", DIM, "\u{2500}".repeat(title.chars().count()))];
    for (k, v) in &info.rows {
        let k = k.to_lowercase();
        let room = width.saturating_sub(lw + 12).max(1);
        right.push(format!("{DIM}{k:<12}{RST}{FG}{}{RST}", v.chars().take(room).collect::<String>()));
    }
    right.push(String::new());
    right.push((40..48).map(|c| format!("\x1b[{c}m  ")).collect::<String>() + RST);
    right.push((100..108).map(|c| format!("\x1b[{c}m  ")).collect::<String>() + RST);
    // a blank row above and below, and the bear centred against the info block when that is taller (it is, with a full list)
    let rows = BEAR.len() / 2;
    let off = right.len().saturating_sub(rows) / 2;
    let mut o = String::from("\n");
    for i in 0..(rows + off).max(right.len()) {
        let logo = if (off..off + rows).contains(&i) { bear_row(i - off) } else { " ".repeat(LOGO_COLS) };
        o += &format!("{logo}  {}\n", right.get(i).map_or("", |s| s.as_str()));
    }
    o.push('\n');
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
        assert!(s.contains("alice@thir") && s.contains("──────────") && s.contains("os          \x1b[0m\x1b[39mthir 0.1.0") && s.contains("kernel"));
        assert_eq!(s.lines().count(), 10);   // a blank row, then 8 (the bear's text rows: title, rule, 2 rows, blank, two colour rows is only 7), then a blank row
        assert!(s.starts_with('\n') && s.ends_with("\n\n"));
        let tall = Info { user: "a".into(), host: "b".into(), rows: (0..11).map(|_| ("K", "v".into())).collect() };   // 16 info rows: the bear starts 4 rows down, centred
        let t = render(&tall, 80); let l: Vec<&str> = t.lines().collect();
        assert!(!l[1].contains('\u{2584}') && !l[4].contains('\u{2584}') && l[5].contains('\u{2584}') && l.len() == 18);
        assert!(BEAR.len() == 16 && BEAR.iter().all(|l| l.len() == LOGO_COLS));
        let plain = |l: &str| -> usize { let mut esc = false; l.chars().filter(|&c| { if c == '\x1b' { esc = true; false } else if esc { esc = !c.is_ascii_alphabetic(); false } else { true } }).count() };
        assert!((0..8).all(|r| plain(&bear_row(r)) == LOGO_COLS));
        assert!(bear_row(1).contains('\u{2588}') && bear_row(0).contains('\u{2584}') && bear_row(7).contains('\u{2580}'));
        let narrow = render(&i, 24);   // long values are cut to the width, never wrapped
        assert!(!narrow.contains("0.1.0"));
    }
}
