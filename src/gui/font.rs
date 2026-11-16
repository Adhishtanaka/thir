//! PSF1/PSF2 bitmap fonts. A glyph is `h` rows of `(w+7)/8` bytes, MSB first.
use std::collections::HashMap;

pub struct Font { pub w: usize, pub h: usize, row: usize, glyphs: Vec<u8>, map: HashMap<char, usize> }

fn u32le(b: &[u8], i: usize) -> Option<usize> { Some(u32::from_le_bytes(b.get(i..i + 4)?.try_into().ok()?) as usize) }

impl Font {
    pub fn parse(d: &[u8]) -> Result<Font, &'static str> {
        let (w, h, n, start, uni) = if d.starts_with(&[0x72, 0xb5, 0x4a, 0x86]) {
            let (hs, flags, n, cs, h, w) = (u32le(d, 8).ok_or("short")?, u32le(d, 12).ok_or("short")?, u32le(d, 16).ok_or("short")?, u32le(d, 20).ok_or("short")?, u32le(d, 24).ok_or("short")?, u32le(d, 28).ok_or("short")?);
            if cs != h * w.div_ceil(8) { return Err("bad PSF2 charsize"); }
            (w, h, n, hs, flags & 1 != 0)
        } else if d.starts_with(&[0x36, 0x04]) && d.len() > 4 {
            (8, d[3] as usize, if d[2] & 1 != 0 { 512 } else { 256 }, 4, d[2] & 6 != 0)
        } else { return Err("not a PSF font"); };
        if w == 0 || h == 0 || w > 64 || h > 64 || n == 0 { return Err("bad PSF header"); }
        let row = w.div_ceil(8);
        let end = start + n * h * row;
        let glyphs = d.get(start..end).ok_or("truncated glyphs")?.to_vec();
        let mut map = HashMap::new();
        if uni {
            let t = &d[end..];
            if d[0] == 0x36 {   // PSF1: u16 LE codepoints, 0xFFFE starts a sequence (skipped), 0xFFFF ends the glyph
                let (mut g, mut skip) = (0, false);
                for p in t.chunks_exact(2) {
                    match u16::from_le_bytes([p[0], p[1]]) {
                        0xFFFF => { g += 1; skip = false; if g >= n { break } }
                        0xFFFE => skip = true,
                        c if !skip => { if let Some(c) = char::from_u32(c as u32) { map.entry(c).or_insert(g); } }
                        _ => {}
                    }
                }
            } else {            // PSF2: UTF-8, 0xFF ends the glyph, 0xFE starts a sequence (skipped)
                for (g, e) in t.split(|&b| b == 0xFF).take(n).enumerate() {
                    let e = e.split(|&b| b == 0xFE).next().unwrap_or(&[]);
                    for c in String::from_utf8_lossy(e).chars() { map.entry(c).or_insert(g); }
                }
            }
        } else { for g in 0..n.min(256) { map.insert(char::from_u32(g as u32).unwrap_or('?'), g); } }
        Ok(Font { w, h, row, glyphs, map })
    }

    /// Bitmap of `c`, or of '?' (or glyph 0) when the font lacks it.
    pub fn glyph(&self, c: char) -> &[u8] {
        let g = self.map.get(&c).or_else(|| self.map.get(&'?')).copied().unwrap_or(0);
        &self.glyphs[g * self.h * self.row..(g + 1) * self.h * self.row]
    }
    pub fn row_bytes(&self) -> usize { self.row }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn psf1(mode: u8, h: u8, glyphs: &[u8], table: &[u8]) -> Vec<u8> { let mut v = vec![0x36, 0x04, mode, h]; v.extend(glyphs); v.extend(table); v }
    #[test] fn psf1_with_unicode() {
        let mut g = vec![0u8; 256 * 2]; g[2 * 65] = 0xAA; g[2 * 65 + 1] = 0x55; g[2 * 63] = 0xFF;   // glyph 65 = 'A' art, 63 = '?'
        let mut t = Vec::new();
        for i in 0..256u16 { if i == 65 { t.extend([0x41, 0, 0xFE, 0xFF, 0x42, 0, 0xFF, 0xFF]) } else { t.extend((if i == 63 { 0x3f } else { 0x1000 + i }).to_le_bytes()); t.extend([0xFF, 0xFF]) } }
        let f = Font::parse(&psf1(2, 2, &g, &t)).unwrap();
        assert_eq!((f.w, f.h), (8, 2));
        assert_eq!(f.glyph('A'), &[0xAA, 0x55]);
        assert_ne!(f.glyph('B'), &[0xAA, 0x55]);   // after 0xFFFE: a sequence, not a mapping
        assert_eq!(f.glyph('\u{2603}'), &g[2 * 63..2 * 63 + 2]);   // missing => '?'
    }
    #[test] fn psf2_no_table_uses_ascii_index() {
        let mut v = vec![0x72, 0xb5, 0x4a, 0x86]; for x in [0u32, 32, 0, 128, 2, 2, 8] { v.extend(x.to_le_bytes()); }
        let mut g = vec![0u8; 128 * 2]; g[2 * 72] = 0x81; v.extend(g);
        let f = Font::parse(&v).unwrap();
        assert_eq!(f.glyph('H')[0], 0x81);
    }
    #[test] fn psf2_unicode_table() {
        let mut v = vec![0x72, 0xb5, 0x4a, 0x86]; for x in [0u32, 32, 1, 2, 1, 1, 8] { v.extend(x.to_le_bytes()); }
        v.extend([0x11, 0x22]); v.extend("é".bytes()); v.push(0xFF); v.extend(b"x"); v.push(0xFF);
        let f = Font::parse(&v).unwrap();
        assert_eq!(f.glyph('é'), &[0x11]); assert_eq!(f.glyph('x'), &[0x22]);
    }
    #[test] fn rejects() { assert!(Font::parse(b"junk").is_err()); assert!(Font::parse(&[0x36, 0x04, 0, 16]).is_err()); }
    #[test] fn bundled_font_parses() {
        let f = Font::parse(include_bytes!("../../assets/fonts/Uni3-Terminus16.psf")).unwrap();
        assert_eq!((f.w, f.h), (8, 16)); assert_ne!(f.glyph('A'), f.glyph(' '));
    }
}
