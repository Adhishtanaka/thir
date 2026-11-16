//! Every color and size of the GUI. Colors are 0xRRGGBB. Tuned for the "plain brown" wallpaper.
pub const PANEL: u32 = 0x141113;      // pane fill, blended over the wallpaper
pub const PANEL_ALPHA: u32 = 217;     // 0..=255, about 0.85
pub const BAR: u32 = 0x141113;
pub const BAR_ALPHA: u32 = 230;
pub const FG: u32 = 0xecd6cd;         // default text
pub const DIM: u32 = 0x807370;        // rgba(236,214,205,0.5) over PANEL
pub const ACCENT: u32 = 0xe0b8a8;     // focused border, cursor, bar highlights
pub const BORDER_DIM: u32 = 0x3a3134; // unfocused border
#[derive(Clone, Copy)]
pub struct Metrics { pub bar: i32, pub gap: i32, pub border: i32, pub pad: i32, pub foot: i32 }
impl Metrics { pub const fn at(s: i32) -> Metrics { Metrics { bar: 28 * s, gap: 12 * s, border: s, pad: 8 * s, foot: 44 * s } } }
pub const MARK: u32 = 0x9c8c86;   // rgba(236,214,205,0.62) over the wallpaper, as in wallpaper.html variant D
pub const WALL: [(f32, u32); 3] = [(0.0, 0x1a1518), (0.5, 0x110e10), (1.0, 0x09080a)];   // radial stops, centre bottom-middle
pub const HELP: &str = "alt|cmd + enter split \u{b7} arrows focus \u{b7} q close \u{b7} f full";

/// ANSI colors 0..15 (30-37, 90-97 and their backgrounds).
pub const PALETTE: [u32; 16] = [
    0x2a2326, 0xe07a72, 0x9cc78a, 0xe0c080, 0x8aa8d8, 0xc89ac0, 0x86c8c0, 0xecd6cd,
    0x5c5155, 0xf0a098, 0xb8e0a8, 0xf0d8a0, 0xa8c4ec, 0xe0b8d8, 0xa8e0d8, 0xffffff,
];
