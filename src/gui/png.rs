//! Minimal PNG decoder: 8-bit RGB or RGBA (alpha dropped), not interlaced.
pub struct Image { pub w: usize, pub h: usize, pub rgb: Vec<u8> }

fn be32(b: &[u8]) -> usize { u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize }

fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let (ia, ib, ic) = (a as i32, b as i32, c as i32);
    let p = ia + ib - ic;
    let (pa, pb, pc) = ((p - ia).abs(), (p - ib).abs(), (p - ic).abs());
    if pa <= pb && pa <= pc { a } else if pb <= pc { b } else { c }
}

/// Undo the per-row filters in place. `raw` is h rows of (1 filter byte + stride bytes).
fn unfilter(raw: &[u8], stride: usize, bpp: usize, h: usize) -> Result<Vec<u8>, &'static str> {
    let mut out = vec![0u8; stride * h];
    for y in 0..h {
        let f = raw[y * (stride + 1)];
        let line = &raw[y * (stride + 1) + 1..(y + 1) * (stride + 1)];
        let (prev, cur) = out.split_at_mut(y * stride);
        let prev = if y > 0 { &prev[(y - 1) * stride..] } else { &[][..] };
        let cur = &mut cur[..stride];
        for i in 0..stride {
            let a = if i >= bpp { cur[i - bpp] } else { 0 };
            let b = if y > 0 { prev[i] } else { 0 };
            let c = if y > 0 && i >= bpp { prev[i - bpp] } else { 0 };
            cur[i] = line[i].wrapping_add(match f {
                0 => 0,
                1 => a,
                2 => b,
                3 => ((a as u16 + b as u16) / 2) as u8,
                4 => paeth(a, b, c),
                _ => return Err("bad filter type"),
            });
        }
    }
    Ok(out)
}

pub fn decode(data: &[u8]) -> Result<Image, &'static str> {
    if data.len() < 8 || data[..8] != [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a] { return Err("not a PNG"); }
    let (mut pos, mut hdr, mut idat) = (8, None, Vec::new());
    while pos + 12 <= data.len() {
        let len = be32(&data[pos..]);
        let body = data.get(pos + 8..pos + 8 + len).ok_or("truncated chunk")?;
        match &data[pos + 4..pos + 8] {
            b"IHDR" if len == 13 => hdr = Some((be32(body), be32(&body[4..]), body[8], body[9], body[12])),
            b"IDAT" => idat.extend_from_slice(body),
            b"IEND" => break,
            _ => {}
        }
        pos += 12 + len;
    }
    let (w, h, depth, color, interlace) = hdr.ok_or("no IHDR")?;
    if depth != 8 || interlace != 0 || (color != 2 && color != 6) { return Err("unsupported PNG (need 8-bit RGB/RGBA, not interlaced)"); }
    if w == 0 || h == 0 || w > 16384 || h > 16384 { return Err("bad PNG size"); }
    let bpp = if color == 2 { 3 } else { 4 };
    let raw = miniz_oxide::inflate::decompress_to_vec_zlib(&idat).map_err(|_| "inflate failed")?;
    if raw.len() != h * (w * bpp + 1) { return Err("wrong decoded size"); }
    let px = unfilter(&raw, w * bpp, bpp, h)?;
    let rgb = if bpp == 3 { px } else { px.chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]]).collect() };
    Ok(Image { w, h, rgb })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn adler(d: &[u8]) -> u32 { let (mut a, mut b) = (1u32, 0u32); for &x in d { a = (a + x as u32) % 65521; b = (b + a) % 65521; } b << 16 | a }
    fn chunk(out: &mut Vec<u8>, t: &[u8], body: &[u8]) { out.extend((body.len() as u32).to_be_bytes()); out.extend(t); out.extend(body); out.extend([0; 4]); }
    /// PNG whose zlib stream is one stored (uncompressed) deflate block.
    fn make(w: u32, h: u32, color: u8, raw: &[u8]) -> Vec<u8> {
        let mut z = vec![0x78, 0x01, 1];
        z.extend((raw.len() as u16).to_le_bytes()); z.extend((!(raw.len() as u16)).to_le_bytes()); z.extend(raw); z.extend(adler(raw).to_be_bytes());
        let mut ihdr = Vec::new(); ihdr.extend(w.to_be_bytes()); ihdr.extend(h.to_be_bytes()); ihdr.extend([8, color, 0, 0, 0]);
        let mut p = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
        chunk(&mut p, b"IHDR", &ihdr); chunk(&mut p, b"IDAT", &z); chunk(&mut p, b"IEND", &[]); p
    }
    // 2x5 RGB image, one row per filter type (0..4), plus a repeat of Paeth. Expected pixels are hand-derived below.
    #[test] fn all_filters() {
        // original rows (6 bytes each)
        let rows: [[u8; 6]; 5] = [[10, 20, 30, 40, 50, 60], [11, 22, 33, 44, 55, 66], [90, 80, 70, 60, 50, 40], [5, 200, 7, 9, 100, 11], [250, 1, 128, 3, 255, 0]];
        let mut raw = Vec::new();
        for (y, r) in rows.iter().enumerate() {
            raw.push(y as u8);
            for i in 0..6 {
                let a = if i >= 3 { r[i - 3] } else { 0 };
                let b = if y > 0 { rows[y - 1][i] } else { 0 };
                let c = if y > 0 && i >= 3 { rows[y - 1][i - 3] } else { 0 };
                let pred = match y { 0 => 0, 1 => a, 2 => b, 3 => ((a as u16 + b as u16) / 2) as u8, _ => paeth(a, b, c) };
                raw.push(r[i].wrapping_sub(pred));
            }
        }
        let img = decode(&make(2, 5, 2, &raw)).unwrap();
        assert_eq!((img.w, img.h), (2, 5));
        assert_eq!(img.rgb, rows.concat());
    }
    #[test] fn paeth_known() { assert_eq!(paeth(10, 20, 10), 20); assert_eq!(paeth(0, 0, 0), 0); assert_eq!(paeth(100, 50, 100), 50); }
    #[test] fn rgba_drops_alpha() { let img = decode(&make(2, 1, 6, &[0, 1, 2, 3, 255, 4, 5, 6, 0])).unwrap(); assert_eq!(img.rgb, [1, 2, 3, 4, 5, 6]); }
    #[test] fn rejects() {
        assert!(decode(b"nope").is_err());
        assert!(decode(&make(1, 1, 3, &[0, 0])).is_err());           // palette
        assert!(decode(&make(1, 1, 2, &[9, 1, 2, 3])).is_err());     // filter 9
        assert!(decode(&make(1, 2, 2, &[0, 1, 2, 3])).is_err());     // too little data
    }
}
