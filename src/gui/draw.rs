//! Pixel helpers shared by den and the surface apps. Back buffers hold 0xRRGGBB pixels, `w` pixels per row.
use super::{font::Font, layout::Rect};

pub fn fill(back: &mut [u32], w: usize, r: Rect, c: u32) {
    for y in r.y.max(0)..(r.y + r.h).max(0) { for x in r.x.max(0)..(r.x + r.w).min(w as i32) { if let Some(p) = back.get_mut(y as usize * w + x as usize) { *p = c; } } }
}
pub fn glyph(back: &mut [u32], w: usize, font: &Font, x: i32, y: i32, ch: char, c: u32) {
    let rb = font.row_bytes();
    for (gy, row) in font.glyph(ch).chunks(rb).enumerate() {
        let py = y + gy as i32; if py < 0 { continue }
        for gx in 0..font.w {
            let px = x + gx as i32;
            if px >= 0 && (px as usize) < w && row[gx / 8] & (0x80 >> (gx % 8)) != 0 { if let Some(p) = back.get_mut(py as usize * w + px as usize) { *p = c; } }
        }
    }
}
/// `s` at (x, y), one glyph cell per char; the width drawn.
pub fn text(back: &mut [u32], w: usize, font: &Font, x: i32, y: i32, s: &str, c: u32) -> i32 {
    for (k, ch) in s.chars().enumerate() { glyph(back, w, font, x + (k * font.w) as i32, y, ch, c); }
    (s.chars().count() * font.w) as i32
}
/// Width in glyph cells of a key-hint row (see `hints`).
pub fn hint_cols(h: &[(&str, &str)]) -> usize { h.iter().map(|(k, d)| k.chars().count() + 1 + d.chars().count()).sum::<usize>() + 3 * h.len().saturating_sub(1) }
/// A key-hint row: key in FG, label in DIM, three spaces between pairs; the width drawn.
pub fn hints(back: &mut [u32], w: usize, font: &Font, x: i32, y: i32, h: &[(&str, &str)]) -> i32 {
    let mut x2 = x;
    for (i, (k, d)) in h.iter().enumerate() {
        if i > 0 { x2 += 3 * font.w as i32; }
        x2 += text(back, w, font, x2, y, k, super::theme::FG) + font.w as i32;
        x2 += text(back, w, font, x2, y, d, super::theme::DIM);
    }
    x2 - x
}
/// Names the den pane (OSC 2) after an app; dropping the guard gives the name back to the shell.
pub struct Title;
pub fn title(name: &str) -> Title { print!("\x1b]2;{name}\x07"); let _ = std::io::Write::flush(&mut std::io::stdout()); Title }
impl Drop for Title { fn drop(&mut self) { print!("\x1b]2;\x07"); let _ = std::io::Write::flush(&mut std::io::stdout()); } }
