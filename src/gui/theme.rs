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
    ("forest", 0x08201a, 0x0c2a22, 0x143a2f),
    ("plum", 0x1a1026, 0x221534, 0x30204a),
    ("ember", 0x1f110c, 0x2a1812, 0x3b241b),
];
