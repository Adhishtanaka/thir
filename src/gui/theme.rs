//! Every color and size of the GUI ("calm navy and peach", see the design system notes). Colors are 0xRRGGBB.
//! Peach means "here": focus, titles, selection, the cursor and the wordmark. Everything else is cool grey-blue.
pub const WALL: u32 = 0x08152b;       // wallpaper and bar, flat
pub const PANEL: u32 = 0x0b1c36;      // pane fill, opaque
pub const MARK_ALPHA: u32 = 102;      // the wordmark under panes and in the gaps: this much of 255 is peach, the rest wallpaper (the login screen keeps it full)
pub const PANEL_ALPHA: u32 = 191;     // a plain shell pane: this much of 255 is pane fill, the rest the wallpaper. Worst case (over the mark) is dim text 4.6:1, normal text 10.5:1 (WCAG AA needs 4.5)
pub const BAR: u32 = WALL;
pub const SEL: u32 = 0x15294a;        // selected row fill
pub const BORDER_DIM: u32 = 0x1e3050; // unfocused border, rules
pub const ACCENT: u32 = 0xf0b27f;     // focused border, pane title, cursor, header title, selection marker, wordmark
pub const FG: u32 = 0xd4dcec;         // text
pub const DIM: u32 = 0x8192b2;        // hints, labels, column heads, unfocused pane names
pub const ACCENT2: u32 = 0x7fb2f0;    // links, folders, "next" markers only
pub const OK: u32 = 0x9fd08a;         // done, on, secure
pub const WARN: u32 = 0xecc87c;       // waiting, caution
pub const ERR: u32 = 0xf2808b;        // failure, warning words
pub const MARK: u32 = ACCENT;         // the wallpaper wordmark
#[derive(Clone, Copy)]
pub struct Metrics { pub bar: i32, pub gap: i32, pub border: i32, pub padx: i32, pub pady: i32 }
impl Metrics { pub const fn at(s: i32) -> Metrics { Metrics { bar: 28 * s, gap: 12 * s, border: s, padx: 20 * s, pady: 16 * s } } }

/// ANSI colors 0..15 (30-37, 90-97 and their backgrounds). 0 is for backgrounds; 8 equals DIM.
pub const PALETTE: [u32; 16] = [
    0x0f1d36, 0xf2808b, 0x9fd08a, 0xecc87c, 0x7fb2f0, 0xc3a3f0, 0x7fd3d9, 0xc5cee3,
    0x8192b2, 0xff9ea8, 0xb6e4a2, 0xf6dc9e, 0x9fc3ff, 0xd8bcff, 0xa2e7ea, 0xf1f4fb,
];
/// Theme colors a terminal app asks for with a 24-bit SGR (38;2 / 48;2); vt.rs keeps them as cell colors 18.. and den looks them up in `Look`.
pub const SOFT: u32 = 0xf8d9bf;       // lighter peach: the bear's ear insides and muzzle (neofetch only)
pub const SLOT_ACCENT: u8 = 18; pub const SLOT_SEL: u8 = 19; pub const SLOT_WALL: u8 = 20; pub const SLOT_BORDER: u8 = 21; pub const SLOT_PANEL: u8 = 22; pub const SLOT_SOFT: u8 = 23;
pub const SLOTS: [(u8, u32); 6] = [(SLOT_ACCENT, ACCENT), (SLOT_SEL, SEL), (SLOT_WALL, WALL), (SLOT_BORDER, BORDER_DIM), (SLOT_PANEL, PANEL), (SLOT_SOFT, SOFT)];

/// A wallpaper preset: (name, wall and bar, pane fill, selected row). The first is the default and is exactly the design system; the others keep the same structure in another hue. The wordmark stays peach on all of them.
pub const WALLS: [(&str, u32, u32, u32); 5] = [
    ("navy", WALL, PANEL, SEL),
    ("graphite", 0x121315, 0x191a1d, 0x25272b),
    ("forest", 0x08201a, 0x0c2a22, 0x103026),
    ("plum", 0x1a1026, 0x221534, 0x30204a),
    ("ember", 0x1f110c, 0x2a1812, 0x3b241b),
];

/// Accent choices in Settings (name, colour); the first is the default. Every one keeps WCAG AA (4.5:1) as text on every wallpaper preset's wall, pane fill and selected row, and as a fill under wall-coloured text (tests below).
pub const ACCENTS: [(&str, u32); 5] = [("peach", ACCENT), ("blue", 0x7fb2f0), ("mint", 0x8fd9b6), ("lilac", 0xc3a3f0), ("rose", 0xf08fa8)];
/// The look on screen now: the accent and the pane fill. den sets it from the user's settings; apps get it with every Configure message (their strips follow the theme).
static NOW: [std::sync::atomic::AtomicU32; 2] = [std::sync::atomic::AtomicU32::new(ACCENT), std::sync::atomic::AtomicU32::new(PANEL)];
pub fn accent() -> u32 { NOW[0].load(std::sync::atomic::Ordering::Relaxed) }
pub fn panel() -> u32 { NOW[1].load(std::sync::atomic::Ordering::Relaxed) }
pub fn set_look(accent: u32, panel: u32) { NOW[0].store(accent & 0xFF_FFFF, std::sync::atomic::Ordering::Relaxed); NOW[1].store(panel & 0xFF_FFFF, std::sync::atomic::Ordering::Relaxed); }
/// WCAG contrast ratio of two 0xRRGGBB colours.
pub fn contrast(a: u32, b: u32) -> f64 {
    let l = |c: u32| { let f = |v: u32| { let v = v as f64 / 255.0; if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) } }; 0.2126 * f(c >> 16 & 255) + 0.7152 * f(c >> 8 & 255) + 0.0722 * f(c & 255) };
    let (x, y) = (l(a), l(b)); (x.max(y) + 0.05) / (x.min(y) + 0.05)
}

/// WCAG AA for text.
pub const AA: f64 = 4.5;
/// "#rrggbb" or "rrggbb" (any case) as 0xRRGGBB.
pub fn parse_hex(s: &str) -> Option<u32> {
    let t = s.trim(); let t = t.strip_prefix('#').unwrap_or(t);
    if t.len() != 6 || !t.bytes().all(|b| b.is_ascii_hexdigit()) { return None }
    u32::from_str_radix(t, 16).ok()
}
/// The worst contrast of `c` as text on any wallpaper preset's wall, pane fill and selected row (it is also the contrast of wall-coloured text on `c` as a fill).
pub fn accent_ratio(c: u32) -> f64 { WALLS.iter().flat_map(|w| [w.1, w.2, w.3]).map(|bg| contrast(c, bg)).fold(f64::MAX, f64::min) }
/// `c` made lighter (mixed toward white in 2% steps) until it reaches AA on every wallpaper; `c` itself when it already does.
pub fn adjust_accent(c: u32) -> u32 {
    (0..=50).map(|i| { let m = |s: u32| { let v = c >> s & 255; (v + (255 - v) * i * 2 / 100) << s }; m(16) | m(8) | m(0) }).find(|&x| accent_ratio(x) >= AA).unwrap_or(0xffffff)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn hex_codes() {
        assert_eq!(parse_hex("#7fb2f0"), Some(0x7fb2f0)); assert_eq!(parse_hex("7FB2F0"), Some(0x7fb2f0)); assert_eq!(parse_hex(" #000000 "), Some(0));
        for bad in ["", "#fff", "#12345", "#1234567", "#12345g", "red", "##123456", "0x7fb2f0"] { assert_eq!(parse_hex(bad), None, "{bad}"); }
    }
    #[test] fn dark_accents_are_lifted_to_aa_and_good_ones_stay() {
        assert!(accent_ratio(0x1a2a6c) < AA); let a = adjust_accent(0x1a2a6c); assert!(accent_ratio(a) >= AA, "{a:06x}"); assert_ne!(a, 0x1a2a6c);
        for (_, c) in ACCENTS { assert_eq!(adjust_accent(c), c); }
        assert!(accent_ratio(adjust_accent(0)) >= AA); assert!(accent_ratio(adjust_accent(0x333333)) >= AA); assert_eq!(adjust_accent(0xffffff), 0xffffff);
    }
    #[test] fn every_accent_is_readable_on_every_wallpaper() {
        for (an, a) in ACCENTS { for (wn, wall, panel, sel) in WALLS {
            for (what, bg) in [("wall", wall), ("panel", panel), ("selected row", sel)] { assert!(contrast(a, bg) >= 4.5, "{an} on {wn} {what}: {:.2}", contrast(a, bg)); }
        } }
    }
    #[test] fn text_and_dim_stay_readable_on_every_wallpaper() {
        for (wn, wall, panel, sel) in WALLS { for bg in [wall, panel, sel] { assert!(contrast(FG, bg) >= 4.5 && contrast(DIM, bg) >= 4.5, "{wn}: {:.2} {:.2}", contrast(FG, bg), contrast(DIM, bg)); } }
    }
    #[test] fn contrast_basics() { assert!((contrast(0xffffff, 0x000000) - 21.0).abs() < 0.01); assert!((contrast(0x777777, 0x777777) - 1.0).abs() < 1e-9); assert_eq!(ACCENTS[0].1, ACCENT); }
}
