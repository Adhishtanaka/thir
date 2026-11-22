//! camera: a webcam preview in a den pane, and photos. It never opens the camera itself: den (root) asks the user on its own bar and relays the frames through shared memory (see gui/cam.rs, gui/mic.rs).
//! Only den panes can do that, so `camera` on the serial console says so and stops. Photos go to ~/Pictures/photo-YYYY-MM-DD-HHMMSS.png (folder 0700, file 0600, never overwritten); a camera that only speaks MJPEG saves .jpg.
//! shortcut: photos only. Video recording is not here (it would need an encoder in the sandbox); preview needs YUYV, which every UVC camera has.
//! Keys: space takes a photo, q stops. Alt+R (den) also stops it.
use crate::{clock, gui::{cam::{SLOT, YUYV}, draw::{fill, hint_cols, hints, text}, font::Font, layout::Rect, surface::{Client, Down, Up}, theme::*}};
use std::{fs::DirBuilder, io::Write, os::{fd::AsRawFd, unix::fs::{DirBuilderExt, MetadataExt}}, path::{Path, PathBuf}, time::{Duration, Instant}};

/// One limited-range BT.601 YUV sample to 0xRRGGBB.
pub fn yuv(y: u8, u: u8, v: u8) -> u32 {
    let (c, d, e) = (y as i32 - 16, u as i32 - 128, v as i32 - 128);
    let cl = |x: i32| (x >> 8).clamp(0, 255) as u32;
    cl(298 * c + 409 * e + 128) << 16 | cl(298 * c - 100 * d - 208 * e + 128) << 8 | cl(298 * c + 516 * d + 128)
}
/// The pixel (x, y) of a YUYV frame `w` wide (two pixels share their U and V).
pub fn yuyv_at(f: &[u8], w: usize, x: usize, y: usize) -> u32 {
    let o = (y * w + (x & !1)) * 2;
    match f.get(o..o + 4) { Some(&[y0, u, y1, v]) => yuv(if x & 1 == 0 { y0 } else { y1 }, u, v), _ => 0 }
}
/// The whole frame as RGB bytes (for a photo).
pub fn yuyv_rgb(f: &[u8], w: usize, h: usize) -> Vec<u8> {
    let mut o = Vec::with_capacity(w * h * 3);
    for y in 0..h { for x in 0..w { let p = yuyv_at(f, w, x, y); o.extend([(p >> 16) as u8, (p >> 8) as u8, p as u8]); } }
    o
}
/// The frame (fw x fh) drawn into the area (aw x ah) of `px` (`pw` per row), as large as fits, centred, nearest pixel. The rest of the area is the pane colour.
pub fn draw_yuyv(px: &mut [u32], pw: usize, aw: usize, ah: usize, f: &[u8], fw: usize, fh: usize) {
    fill(px, pw, Rect { x: 0, y: 0, w: aw as i32, h: ah as i32 }, panel());
    if fw == 0 || fh == 0 || aw == 0 || ah == 0 { return }
    let (dw, dh) = if aw * fh <= ah * fw { (aw, (aw * fh / fw).max(1)) } else { ((ah * fw / fh).max(1), ah) };
    let (ox, oy) = ((aw - dw) / 2, (ah - dh) / 2);
    for dy in 0..dh { let sy = dy * fh / dh; for dx in 0..dw { px[(oy + dy) * pw + ox + dx] = yuyv_at(f, fw, dx * fw / dw, sy); } }
}

fn crc32(data: &[u8], mut c: u32) -> u32 {
    c = !c;
    for &b in data { c ^= b as u32; for _ in 0..8 { c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 }; } }
    !c
}
fn adler32(data: &[u8]) -> u32 { let (mut a, mut b) = (1u32, 0u32); for &x in data { a = (a + x as u32) % 65521; b = (b + a) % 65521; } b << 16 | a }
/// An RGB picture as a PNG file. The pixels are stored, not compressed (a 640x480 photo is about 0.9 MB): no encoder is needed and every viewer reads it.
pub fn png(w: usize, h: usize, rgb: &[u8]) -> Vec<u8> {
    let mut raw = Vec::with_capacity(h * (w * 3 + 1));
    for row in rgb.chunks(w * 3).take(h) { raw.push(0); raw.extend_from_slice(row); }
    let mut z = vec![0x78, 0x01];
    let mut blocks = raw.chunks(65535).peekable();
    if blocks.peek().is_none() { z.extend([1, 0, 0, 0xff, 0xff]); }
    while let Some(b) = blocks.next() { z.push(blocks.peek().is_none() as u8); z.extend((b.len() as u16).to_le_bytes()); z.extend((!(b.len() as u16)).to_le_bytes()); z.extend_from_slice(b); }
    z.extend(adler32(&raw).to_be_bytes());
    let mut o = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut chunk = |kind: &[u8; 4], data: &[u8]| { o.extend((data.len() as u32).to_be_bytes()); o.extend(kind); o.extend(data); o.extend(crc32(data, crc32(kind, 0)).to_be_bytes()); };
    let mut ihdr = Vec::new(); ihdr.extend((w as u32).to_be_bytes()); ihdr.extend((h as u32).to_be_bytes()); ihdr.extend([8, 2, 0, 0, 0]);
    chunk(b"IHDR", &ihdr); chunk(b"IDAT", &z); chunk(b"IEND", &[]);
    o
}
/// "photo-2026-11-20-143205" from a local time.
pub fn file_name(l: &clock::Local) -> String { format!("photo-{:04}-{:02}-{:02}-{:02}{:02}{:02}", l.year, l.month, l.day, l.hour, l.min, l.sec) }

/// ~/Pictures, made 0700 when missing. A link or a folder of someone else is refused.
fn pictures_dir() -> Result<PathBuf, String> {
    let home = std::env::var("HOME").map_err(|_| "no home folder".to_string())?;
    let d = Path::new(&home).join("Pictures");
    match DirBuilder::new().mode(0o700).create(&d) { Ok(()) => {} Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {} Err(e) => return Err(format!("{}: {e}", d.display())) }
    let m = std::fs::symlink_metadata(&d).map_err(|e| e.to_string())?;
    if !m.is_dir() || m.uid() != unsafe { libc::getuid() } { return Err(format!("{} is not your folder", d.display())) }
    Ok(d)
}

/// The frame memory den shares: two slots, mapped read-only.
struct Slots(*const u8);
impl Slots {
    fn map(fd: &std::os::fd::OwnedFd) -> Option<Slots> {
        let m = unsafe { libc::mmap(std::ptr::null_mut(), 2 * SLOT, libc::PROT_READ, libc::MAP_SHARED, fd.as_raw_fd(), 0) };
        (m != libc::MAP_FAILED).then_some(Slots(m as *const u8))
    }
    /// A copy of the frame in `slot` (den may write the other slot meanwhile).
    fn frame(&self, slot: u32, len: u32) -> Vec<u8> { let n = (len as usize).min(SLOT); unsafe { std::slice::from_raw_parts(self.0.add((slot as usize & 1) * SLOT), n) }.to_vec() }
}
impl Drop for Slots { fn drop(&mut self) { unsafe { libc::munmap(self.0 as *mut _, 2 * SLOT); } } }

struct Cam { w: usize, h: usize, fmt: u32, frame: Vec<u8>, note: (String, bool, Instant), frames: u64 }   // note: text, is an error, when

fn paint(c: &mut Client, font: &Font, cam: Option<&Cam>, waiting: bool) {
    let (w, h, sc) = (c.w as usize, c.h as usize, c.scale.max(1) as i32);
    let bar = font.h + 6 * sc as usize; let ah = h.saturating_sub(bar).max(1);
    let px = c.pixels(); if px.len() < w * h { return }
    match cam {
        Some(m) if m.fmt == YUYV && !m.frame.is_empty() => draw_yuyv(px, w, w, ah, &m.frame, m.w, m.h),
        _ => fill(px, w, Rect { x: 0, y: 0, w: w as i32, h: ah as i32 }, panel()),
    }
    fill(px, w, Rect { x: 0, y: ah as i32, w: w as i32, h: bar as i32 }, panel());
    fill(px, w, Rect { x: 0, y: ah as i32, w: w as i32, h: 1 }, BORDER_DIM);
    let y = (ah + 3 * sc as usize) as i32; let mut x = font.w as i32;
    let room = w / font.w;
    if waiting {
        let mid = (ah as i32 - font.h as i32) / 2;
        let t = "\u{25d0} waiting for permission"; text(px, w, font, (w as i32 - t.chars().count() as i32 * font.w as i32) / 2, mid, t, accent());
        let t = "The question is in the top bar. Answer there to start the camera."; text(px, w, font, (w as i32 - t.chars().count() as i32 * font.w as i32).max(0) / 2, mid + 2 * font.h as i32, t, DIM);
        hints(px, w, font, x, y, &[("y", "allow"), ("n", "deny"), ("q", "quit")]);
        return
    }
    if let Some(m) = cam {
        if m.fmt != YUYV { let t = "This camera only gives MJPEG: no preview, but space still takes photos."; text(px, w, font, font.w as i32, ah as i32 / 2, t, DIM); }
        else if m.frame.is_empty() { let t = "\u{25d0} waiting for the first picture"; text(px, w, font, (w as i32 - t.chars().count() as i32 * font.w as i32) / 2, ah as i32 / 2, t, accent()); }
        let (n, bad, at) = &m.note;
        let info = if !n.is_empty() && at.elapsed() < Duration::from_secs(4) { (n.clone(), *bad) } else { (format!("live  {}x{}", m.w, m.h), false) };
        x += text(px, w, font, x, y, &info.0.chars().take(room.saturating_sub(40)).collect::<String>(), if info.1 { ERR } else if !n.is_empty() && at.elapsed() < Duration::from_secs(4) { OK } else { DIM }) + 2 * font.w as i32;
        let help = [("space", "photo"), ("q", "quit")];
        let hw = hint_cols(&help) * font.w;
        if (x as usize) + hw + 4 * font.w < w { hints(px, w, font, (w - hw - font.w) as i32, y, &help); }
    }
}

pub fn run(args: &[String]) -> i32 {
    if !args.is_empty() { eprintln!("usage: camera   (shows your webcam; space takes a photo into ~/Pictures; den asks you first)"); return 2 }
    let mut c = match Client::connect() {
        Ok(c) => c,
        Err(_) => { eprintln!("camera: the camera can only be used from a den pane, where the screen asks you first. It is not available on the serial console."); return 1 }
    };
    let dir = match pictures_dir() { Ok(d) => d, Err(e) => { eprintln!("camera: {e}"); return 1 } };
    let font = std::fs::read(if c.scale >= 2 { "/share/font32.psf" } else { "/share/font16.psf" }).ok().and_then(|b| Font::parse(&b).ok());
    let Some(font) = font else { eprintln!("camera: no font"); return 1 };
    let _title = crate::gui::draw::title("camera");
    if c.attach().is_err() { return 1 }
    let mut attached = (c.w, c.h);
    let whole = |c: &Client| [(0u16, 0u16, c.w.min(65535) as u16, c.h.min(65535) as u16)];
    paint(&mut c, &font, None, true); if c.commit(&whole(&c)).is_err() { return 1 }
    if c.send(&Up::CamStart).is_err() { eprintln!("camera: den went away"); return 1 }
    let (mut cam, mut slots): (Option<Cam>, Option<Slots>) = (None, None);
    let (mut dirty, mut waiting) = (true, true);   // the first commit's DONE is still to come
    let t0 = Instant::now(); let mut said = String::new();
    'main: loop {
        let mut ev = match c.poll(if dirty && !waiting { 0 } else { 100 }) { Ok(e) => e, Err(_) => break };   // den closed the socket: leave quietly
        if cam.is_none() && t0.elapsed() > Duration::from_secs(40) { said = "camera: no answer".into(); break }
        while let Some(e) = ev {
            match e {
                Down::Done => waiting = false,
                Down::Configure { .. } => { dirty = true; if (c.w, c.h) != attached { if c.attach().is_err() { return 1 } attached = (c.w, c.h); } }
                Down::Cam { state: 1, w, h, fmt } => {
                    let Some(fd) = c.fds.pop() else { said = "camera: no picture memory from den".into(); break 'main };
                    let Some(s) = Slots::map(&fd) else { said = "camera: cannot map the pictures".into(); break 'main };
                    slots = Some(s); cam = Some(Cam { w: w as usize, h: h as usize, fmt, frame: vec![], note: (String::new(), false, Instant::now()), frames: 0 }); dirty = true;
                }
                Down::Cam { state: 4, .. } => { said = "camera: no camera found. Plug one in and try again.".into(); break 'main }
                Down::Cam { state: 5, .. } => { said = "camera: the camera would not open (it may be in use). See ~/.cache or try again.".into(); break 'main }
                Down::Cam { state: 2 | 3, .. } => { if cam.is_none() { said = "camera: the camera was not allowed.".into() } break 'main }   // Alt+R after it ran is a normal end
                Down::CamFrame { slot, len } => if let (Some(m), Some(s)) = (cam.as_mut(), slots.as_ref()) {
                    let f = s.frame(slot, len);
                    if m.fmt != YUYV || f.len() >= m.w * m.h * 2 { m.frame = f; m.frames += 1; dirty = m.fmt == YUYV || m.frames == 1; }
                },
                Down::Key { code, value: 1, .. } => match code {
                    16 => break 'main,   // q
                    57 => if let Some(m) = cam.as_mut().filter(|m| !m.frame.is_empty()) {   // space: a photo of this very picture
                        let (data, ext) = if m.fmt == YUYV { (png(m.w, m.h, &yuyv_rgb(&m.frame, m.w, m.h)), "png") } else { (m.frame.clone(), "jpg") };
                        let base = file_name(&clock::local(clock::now(), clock::home_tz().unwrap_or(0)));
                        m.note = match crate::apps::rec::create_new_ext(&dir, &base, ext).and_then(|(mut f, p)| f.write_all(&data).and_then(|_| f.sync_all()).map(|_| p)) {
                            Ok(p) => (format!("saved ~/Pictures/{}", p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()), false, Instant::now()),
                            Err(e) => (format!("could not save the photo: {e}"), true, Instant::now()),
                        };
                        dirty = true;
                    },
                    _ => {}
                },
                _ => {}
            }
            ev = c.poll(0).unwrap_or(None);
        }
        if dirty && !waiting {
            paint(&mut c, &font, cam.as_ref(), cam.is_none());
            if c.commit(&whole(&c)).is_err() { break }
            (dirty, waiting) = (false, true);
        }
    }
    let _ = c.send(&Up::CamStop);
    drop(_title);
    if !said.is_empty() { eprintln!("{said}"); return 1 }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn yuv_ends_and_greys() {
        assert_eq!(yuv(16, 128, 128), 0x000000); assert_eq!(yuv(235, 128, 128), 0xffffff); assert_eq!(yuv(126, 128, 128), 0x808080);   // mid grey
        assert_eq!(yuv(0, 128, 128), 0); assert_eq!(yuv(255, 128, 128), 0xffffff);   // out of range: clamped
    }
    #[test] fn yuv_primaries() {
        let red = yuv(81, 90, 240); assert!((red >> 16) > 240 && ((red >> 8) & 255) < 20 && (red & 255) < 20, "{red:06x}");
        let blue = yuv(41, 240, 110); assert!((blue & 255) > 240 && (blue >> 16) < 25, "{blue:06x}");
        let green = yuv(145, 54, 34); assert!(((green >> 8) & 255) > 240 && (green >> 16) < 20, "{green:06x}");
    }
    #[test] fn yuyv_pairs_share_chroma() {
        let f = [16u8, 128, 235, 128, 81, 90, 81, 240];   // two pixels black/white, then two reds with different V
        assert_eq!((yuyv_at(&f, 4, 0, 0), yuyv_at(&f, 4, 1, 0)), (0x000000, 0xffffff)); assert_eq!(yuyv_at(&f, 4, 2, 0), yuyv_at(&f, 4, 3, 0)); assert_eq!(yuyv_at(&f, 4, 9, 0), 0, "outside the frame");
        let rgb = yuyv_rgb(&f, 4, 1); assert_eq!(rgb.len(), 12); assert_eq!(&rgb[..6], &[0, 0, 0, 255, 255, 255]);
    }
    #[test] fn preview_fits_and_centres() {
        let f: Vec<u8> = [235u8, 128].iter().cycle().take(4 * 2 * 2).copied().collect();   // 4x2 white
        let mut px = vec![1u32; 8 * 8]; draw_yuyv(&mut px, 8, 8, 8, &f, 4, 2);
        assert_eq!(px[0], panel()); assert_eq!(px[2 * 8 + 3], 0xffffff); assert_eq!(px[5 * 8 + 3], 0xffffff); assert_eq!(px[6 * 8 + 3], panel());   // 8x4 picture in rows 2..6
        draw_yuyv(&mut px, 8, 0, 0, &f, 4, 2); draw_yuyv(&mut px, 8, 8, 8, &[], 4, 2);   // empty things never panic
    }
    #[test] fn checksums() { assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398); assert_eq!(crc32(b"IEND", 0), 0xAE42_6082); assert_eq!(crc32(b"", 0), 0); }
    #[test] fn png_is_well_formed() {
        let (w, h) = (3usize, 2usize); let rgb: Vec<u8> = (0..w * h * 3).map(|i| (i * 7) as u8).collect();
        let p = png(w, h, &rgb);
        assert_eq!(&p[..8], b"\x89PNG\r\n\x1a\n");
        let (mut at, mut idat, mut kinds) = (8, vec![], vec![]);
        while at < p.len() {
            let len = u32::from_be_bytes(p[at..at + 4].try_into().unwrap()) as usize; let kind = &p[at + 4..at + 8]; let data = &p[at + 8..at + 8 + len];
            assert_eq!(u32::from_be_bytes(p[at + 8 + len..at + 12 + len].try_into().unwrap()), crc32(data, crc32(kind, 0)), "crc of {:?}", kind);
            if kind == b"IDAT" { idat = data.to_vec() } if kind == b"IHDR" { assert_eq!((u32::from_be_bytes(data[..4].try_into().unwrap()), u32::from_be_bytes(data[4..8].try_into().unwrap()), &data[8..]), (3, 2, &[8u8, 2, 0, 0, 0][..])); }
            kinds.push(kind.to_vec()); at += 12 + len;
        }
        assert_eq!(kinds, [b"IHDR".to_vec(), b"IDAT".to_vec(), b"IEND".to_vec()]);
        assert_eq!(&idat[..2], &[0x78, 0x01]); assert_eq!(idat[2], 1);   // one final stored block
        let n = u16::from_le_bytes([idat[3], idat[4]]) as usize; assert_eq!(n, h * (w * 3 + 1)); assert_eq!(u16::from_le_bytes([idat[5], idat[6]]), !(n as u16));
        let raw = &idat[7..7 + n]; assert_eq!(&raw[1..10], &rgb[..9]); assert_eq!(raw[0], 0); assert_eq!(raw[10], 0);
        assert_eq!(u32::from_be_bytes(idat[7 + n..].try_into().unwrap()), adler32(raw));
    }
    #[test] fn big_photo_uses_several_stored_blocks() { let p = png(640, 480, &vec![9u8; 640 * 480 * 3]); assert!(p.len() > 640 * 480 * 3 && p.len() < 640 * 480 * 3 + 1000); }
    #[test] fn photo_names_are_dated() { assert_eq!(file_name(&clock::local(0, 0)), "photo-1970-01-01-000000"); }
}
