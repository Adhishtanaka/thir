//! The wallpaper, drawn in code: assets/wallpaper.html variant D (warm charcoal radial gradient, small bear head).

pub fn blend(fg: u32, bg: u32, a: u32) -> u32 {
    let ch = |s: u32| ((fg >> s & 255) * a + (bg >> s & 255) * (255 - a)) / 255 << s;
    ch(16) | ch(8) | ch(0)
}

/// Radial gradient from (w/2, h), radius 0.7*hypot(w,h), piecewise-linear `stops`, 4x4 Bayer dither (±0.5 level) so 8-bit never bands.
pub fn gradient(w: usize, h: usize, stops: &[(f32, u32)]) -> Vec<u32> {
    const BAYER: [[f32; 4]; 4] = [[0., 8., 2., 10.], [12., 4., 14., 6.], [3., 11., 1., 9.], [15., 7., 13., 5.]];
    let rad = 0.7 * (w as f32).hypot(h as f32);
    let mut out = Vec::with_capacity(w * h);
    for y in 0..h { for x in 0..w {
        let t = ((x as f32 - w as f32 / 2.0).hypot(y as f32 - h as f32) / rad).clamp(0.0, 1.0);
        let i = stops.iter().position(|s| s.0 >= t).unwrap_or(stops.len() - 1).max(1);
        let ((t0, c0), (t1, c1)) = (stops[i - 1], stops[i]);
        let f = if t1 > t0 { ((t - t0) / (t1 - t0)).clamp(0.0, 1.0) } else { 0.0 };
        let d = (BAYER[y & 3][x & 3] + 0.5) / 16.0 - 0.5;
        let mut v = 0u32;
        for s in [16, 8, 0] {
            let (a, b) = ((c0 >> s & 255) as f32, (c1 >> s & 255) as f32);
            v = v << 8 | (a + (b - a) * f + 0.5 + d).clamp(0.0, 255.0) as u32;
        }
        out.push(v);
    } }
    out
}

/// Bear head from wallpaper.html `bearHead`, anti-aliased with signed-distance coverage. Only touches pixels inside cx±1.5r, cy±1.5r.
pub fn bear(px: &mut [u32], w: usize, cx: f32, cy: f32, r: f32, stroke: u32, fill: u32, lw: f32) {
    let cov = |d: f32| (0.5 - d).clamp(0.0, 1.0);
    let line = |d: f32| (0.5 - (d.abs() - lw / 2.0)).clamp(0.0, 1.0);
    let circle = |x: f32, y: f32, ox: f32, oy: f32, rr: f32| (x - ox).hypot(y - oy) - rr;
    let ell = |x: f32, y: f32, oy: f32, rx: f32, ry: f32| ((x - cx) / rx).hypot((y - oy) / ry).abs() - 1.0;
    let ell = |x: f32, y: f32, oy: f32, rx: f32, ry: f32| ell(x, y, oy, rx, ry) * rx.min(ry);
    let (x0, x1) = ((cx - 1.5 * r).floor().max(0.0) as usize, ((cx + 1.5 * r).ceil() as usize).min(w));
    let (y0, y1) = ((cy - 1.5 * r).floor().max(0.0) as usize, (cy + 1.5 * r).ceil() as usize);
    for y in y0..y1 { for x in x0..x1 {
        let Some(p) = px.get_mut(y * w + x) else { continue };
        let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
        let mut c = *p;
        let mut paint = |col: u32, a: f32| c = blend(col, c, (a * 255.0 + 0.5) as u32);
        for s in [-1.0, 1.0] {   // ears, then head: fill and outline
            let d = circle(fx, fy, cx + s * 0.7 * r, cy - 0.72 * r, 0.36 * r); paint(fill, cov(d)); paint(stroke, line(d));
        }
        let d = circle(fx, fy, cx, cy, r); paint(fill, cov(d)); paint(stroke, line(d));
        paint(stroke, line(ell(fx, fy, cy + 0.34 * r, 0.42 * r, 0.3 * r)));
        paint(stroke, cov(ell(fx, fy, cy + 0.24 * r, 0.13 * r, 0.09 * r)));
        for s in [-1.0, 1.0] { paint(stroke, cov(circle(fx, fy, cx + s * 0.36 * r, cy - 0.12 * r, 0.06 * r))); }
        *p = c;
    } }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui::theme::WALL;
    #[test] fn gradient_shape() {
        let g = gradient(64, 64, &WALL);
        let near = |a: u32, b: u32| [16, 8, 0].iter().all(|s| ((a >> s & 255) as i32 - (b >> s & 255) as i32).abs() <= 1);
        assert!(near(g[63 * 64 + 32], 0x1d3557));
        let (c, k) = (g[32 + 63 * 64], g[0]);
        assert!([16, 8, 0].iter().all(|s| (k >> s & 255) < (c >> s & 255)));
        assert!(g.iter().all(|p| [16, 8, 0].iter().all(|s| (p >> s & 255) <= (0x1d3557 >> s & 255))));
    }
    #[test] fn bear_paints_inside_box_only() {
        let mut px = vec![0u32; 64 * 64];
        bear(&mut px, 64, 32.0, 32.0, 10.0, 0x9c8c86, 0x141113, 1.6);
        assert_eq!(px[27 * 64 + 32], 0x141113);   // forehead
        assert!(px.iter().filter(|&&p| p != 0).count() >= 50);
        for y in 0..64 { for x in 0..64 { if (x as f32 - 32.0).abs() > 16.0 || (y as f32 - 32.0).abs() > 16.0 { assert_eq!(px[y * 64 + x], 0); } } }
    }
}
