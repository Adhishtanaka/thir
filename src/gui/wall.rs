//! The wallpaper, drawn in code: one flat colour and the pixel wordmark "thir" (32 x 12 cells, 3-cell stems), no bear.

pub fn blend(fg: u32, bg: u32, a: u32) -> u32 {
    let ch = |s: u32| ((fg >> s & 255) * a + (bg >> s & 255) * (255 - a)) / 255 << s;
    ch(16) | ch(8) | ch(0)
}

/// The wordmark rows (X = filled cell), 32 columns by 12 rows.
pub const MARK: [&str; 12] = [
    "..........XXX.......XXX.........",
    "..XXX.....XXX.......XXX.........",
    "..XXX.....XXX.......XXX.........",
    "..XXX.....XXX...................",
    "XXXXXXX...XXXXXXX...XXX..XXXXXX.",
    "XXXXXXX...XXXXXXXX..XXX..XXXXXXX",
    "..XXX.....XXX..XXX..XXX..XXX....",
    "..XXX.....XXX..XXX..XXX..XXX....",
    "..XXX.....XXX..XXX..XXX..XXX....",
    "..XXX.....XXX..XXX..XXX..XXX....",
    "..XXX.....XXX..XXX..XXX..XXX....",
    "..XXXXXX..XXX..XXX..XXX..XXX....",
];
pub const MARK_COLS: i32 = 32;
pub const MARK_ROWS: i32 = 12;

/// The wordmark with its top-left corner at (x0, y0), `cell` pixels per cell; each row is drawn as runs of filled rectangles.
pub fn wordmark(px: &mut [u32], w: usize, x0: i32, y0: i32, cell: i32, c: u32) {
    for (r, row) in MARK.iter().enumerate() {
        let b = row.as_bytes();
        let mut x = 0;
        while x < b.len() {
            if b[x] != b'X' { x += 1; continue }
            let s = x; while x < b.len() && b[x] == b'X' { x += 1; }
            for y in (y0 + r as i32 * cell).max(0)..(y0 + (r as i32 + 1) * cell).max(0) {
                for xx in (x0 + s as i32 * cell).max(0)..(x0 + x as i32 * cell).clamp(0, w as i32) { if let Some(p) = px.get_mut(y as usize * w + xx as usize) { *p = c; } }
            }
        }
    }
}

/// Cell size of the desktop wordmark: 1/72 of the screen width (20 px at 1440), at least 8; the login mark is half of it.
pub fn cell_for(w: usize) -> i32 { (w as i32 / 72).max(8) }

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn mark_shape() {
        assert!(MARK.iter().all(|r| r.len() == MARK_COLS as usize) && MARK.len() == MARK_ROWS as usize);
        let mut px = vec![0u32; 64 * 64];
        wordmark(&mut px, 64, 0, 0, 2, 7);
        assert_eq!(px[0], 0); assert_eq!(px[8 * 64 + 2], 7);   // row 4 starts filled at column 0: y 8
        assert_eq!(px.iter().filter(|&&p| p == 7).count(), MARK.iter().map(|r| r.matches('X').count()).sum::<usize>() * 4);
        assert_eq!(cell_for(1440), 20); assert_eq!(cell_for(400), 8);
    }
    #[test] fn blend_mixes() { assert_eq!(blend(0xffffff, 0, 255), 0xffffff); assert_eq!(blend(0xffffff, 0x000000, 0), 0); }
}
