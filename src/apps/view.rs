//! view: an image viewer for a den pane. ffmpeg (from the web add-on, run in a sandbox) decodes the picture to BGRA; view fits, zooms and pans it and steps through a folder.
//! An animated GIF or WebP shows its first frame here; `play FILE` plays it.
//! Keys: n/p next and previous, s slideshow (5 s), r rotate, f fit, 1 actual size, + - or the wheel zoom, arrows or a drag pan, q quit.
use crate::{gui::{draw::{fill, hint_cols, hints, text}, font::Font, layout::Rect, surface::{Client, Down}, theme::*}, sandbox};
use std::{os::unix::process::CommandExt, path::{Path, PathBuf}, process::{Command, Stdio}, time::{Duration, Instant}};

const FFMPEG: &str = "/usr/bin/ffmpeg";
const CAP: usize = 4096;   // longest side kept after decoding
const EXTS: [&str; 6] = ["png", "jpg", "jpeg", "gif", "webp", "bmp"];
const SLIDE: Duration = Duration::from_secs(5);

pub struct Img { pub w: usize, pub h: usize, pub px: Vec<u32> }   // 0xAARRGGBB

/// The pictures to show: DIR's pictures by name, or the pictures next to FILE (and which one FILE is).
pub fn list(arg: &Path) -> (Vec<PathBuf>, usize) {
    let is_img = |p: &Path| p.extension().and_then(|e| e.to_str()).is_some_and(|e| EXTS.contains(&e.to_ascii_lowercase().as_str()));
    let pics = |dir: &Path| { let mut v: Vec<PathBuf> = std::fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| is_img(p) && p.is_file()).collect();
        v.sort_by_key(|p| p.file_name().map(|n| n.to_string_lossy().to_lowercase())); v };
    if arg.is_dir() { return (pics(arg), 0) }
    let dir = arg.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let v = pics(dir);
    match v.iter().position(|p| p.file_name() == arg.file_name()) { Some(i) => (v, i), None => (vec![arg.to_path_buf()], 0) }
}

/// The picture size from the text `ffmpeg -i` prints: the first WxH after "Video:".
pub fn parse_dims(info: &str) -> Option<(usize, usize)> {
    let line = info.lines().find(|l| l.contains("Video:"))?;
    line.split(|c: char| c == ',' || c == ' ' || c == '[' || c == ']' || c == '(' || c == ')').find_map(|t| {
        let (a, b) = t.split_once('x')?;
        let (w, h) = (a.parse::<usize>().ok()?, b.parse::<usize>().ok()?);
        ((1..=100_000).contains(&w) && (1..=100_000).contains(&h) && a.bytes().all(|c| c.is_ascii_digit()) && b.bytes().all(|c| c.is_ascii_digit())).then_some((w, h))
    })
}

/// The ffmpeg -vf chain and the output size for a (w, h) picture turned `rot` quarter turns clockwise, longest side at most CAP.
pub fn filters(w: usize, h: usize, rot: u8) -> (String, usize, usize) {
    let (tw, th) = if w.max(h) > CAP { ((w * CAP / w.max(h)).max(1), (h * CAP / w.max(h)).max(1)) } else { (w, h) };
    let mut f = vec![];
    if (tw, th) != (w, h) { f.push(format!("scale={tw}:{th}")); }
    f.extend(match rot % 4 { 1 => vec!["transpose=1"], 2 => vec!["transpose=1", "transpose=1"], 3 => vec!["transpose=2"], _ => vec![] }.into_iter().map(String::from));
    let (ow, oh) = if rot % 2 == 1 { (th, tw) } else { (tw, th) };
    (f.join(","), ow, oh)
}

/// Runs ffmpeg on `file` in the sandbox: no_new_privs, and Landlock lets it read only /web (itself and its libraries) and that one file.
fn ffmpeg(file: &Path, args: &[&str]) -> Result<std::process::Output, String> {
    let rules = sandbox::rules(&[Path::new("/web")], &[file]);
    let mut c = Command::new(FFMPEG);
    c.args(["-hide_banner", "-nostdin", "-noautorotate", "-i"]).arg(file).args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    // SAFETY: only prctl and landlock syscalls run between fork and exec.
    unsafe { c.pre_exec(move || rules.enforce().map(|_| ())); }
    c.output().map_err(|e| format!("cannot run ffmpeg: {e}"))
}

/// Decodes the first frame of `file`, turned `rot` quarter turns. shortcut: EXIF orientation is not applied (-noautorotate keeps the size predictable); press r.
pub fn decode(file: &Path, rot: u8) -> Result<Img, String> {
    let probe = ffmpeg(file, &[])?;
    let (w, h) = parse_dims(&String::from_utf8_lossy(&probe.stderr)).ok_or("not a picture ffmpeg can read")?;
    let (vf, ow, oh) = filters(w, h, rot);
    let mut args = vec!["-loglevel", "error", "-frames:v", "1"];
    if !vf.is_empty() { args.extend(["-vf", &vf]); }
    args.extend(["-pix_fmt", "bgra", "-f", "rawvideo", "pipe:1"]);
    let o = ffmpeg(file, &args)?;
    if !o.status.success() || o.stdout.len() != ow * oh * 4 { return Err("ffmpeg could not decode it".into()) }
    Ok(Img { w: ow, h: oh, px: o.stdout.chunks_exact(4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect() })
}

/// Zoom and position. `zoom` None = fit the window; (cx, cy) = the picture point at the middle of the window.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cam { pub zoom: Option<f32>, pub cx: f32, pub cy: f32 }
impl Cam {
    pub fn fit(iw: usize, ih: usize, aw: usize, ah: usize) -> f32 { (aw as f32 / iw as f32).min(ah as f32 / ih as f32).min(1.0) }   // never enlarges past 1:1
    pub fn scale(&self, iw: usize, ih: usize, aw: usize, ah: usize) -> f32 { self.zoom.unwrap_or_else(|| Cam::fit(iw, ih, aw, ah)) }
    /// A picture smaller than the window is centred; a bigger one cannot be panned past its edges.
    pub fn clamp(&mut self, iw: usize, ih: usize, aw: usize, ah: usize) {
        let s = self.scale(iw, ih, aw, ah);
        for (c, i, a) in [(&mut self.cx, iw, aw), (&mut self.cy, ih, ah)] {
            let half = a as f32 / s / 2.0;
            *c = if (i as f32) <= 2.0 * half { i as f32 / 2.0 } else { c.clamp(half, i as f32 - half) };
        }
    }
    /// Zooms by `f` keeping the picture point under window point (px, py) in place.
    pub fn zoom_at(&mut self, f: f32, px: f32, py: f32, iw: usize, ih: usize, aw: usize, ah: usize) {
        let s = self.scale(iw, ih, aw, ah); let n = (s * f).clamp(0.05, 16.0);
        let (ix, iy) = (self.cx + (px - aw as f32 / 2.0) / s, self.cy + (py - ah as f32 / 2.0) / s);
        *self = Cam { zoom: Some(n), cx: ix - (px - aw as f32 / 2.0) / n, cy: iy - (py - ah as f32 / 2.0) / n };
        self.clamp(iw, ih, aw, ah);
    }
}

/// Paints the picture into the top `ah` rows of `px` (`w` per row) at scale `s` around (cx, cy); the rest of the area is PANEL. Shrinking averages up to 4x4 samples per pixel.
pub fn draw_image(px: &mut [u32], w: usize, ah: usize, img: &Img, s: f32, cx: f32, cy: f32) {
    let n = if s < 1.0 { (1.0 / s).ceil().min(4.0) as usize } else { 1 };
    let (lx, ly) = (w as f32 / 2.0 - cx * s, ah as f32 / 2.0 - cy * s);
    let (rx, ry) = (lx + img.w as f32 * s, ly + img.h as f32 * s);
    for dy in 0..ah {
        let row = &mut px[dy * w..(dy + 1) * w];
        if (dy as f32) < ly - 1.0 || dy as f32 > ry + 1.0 { row.fill(PANEL); continue }
        for (dx, out) in row.iter_mut().enumerate() {
            if (dx as f32) < lx - 1.0 || dx as f32 > rx + 1.0 { *out = PANEL; continue }
            let mut sum = [0u32; 3];
            for j in 0..n { for i in 0..n {
                let sx = cx + (dx as f32 + (i as f32 + 0.5) / n as f32 - w as f32 / 2.0) / s;
                let sy = cy + (dy as f32 + (j as f32 + 0.5) / n as f32 - ah as f32 / 2.0) / s;
                let c = if sx >= 0.0 && sy >= 0.0 && (sx as usize) < img.w && (sy as usize) < img.h {
                    let p = img.px[sy as usize * img.w + sx as usize];
                    if p >> 24 == 255 { p } else { crate::gui::wall::blend(p & 0xFF_FFFF, PANEL, p >> 24) }   // transparent parts show the pane colour
                } else { PANEL };
                sum[0] += c >> 16 & 255; sum[1] += c >> 8 & 255; sum[2] += c & 255;
            } }
            let d = (n * n) as u32;
            *out = (sum[0] / d) << 16 | (sum[1] / d) << 8 | sum[2] / d;
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum Cmd { Quit, Next, Prev, Slide, Rotate, Fit, One, In, Out, Pan(i32, i32) }
/// The command for an evdev key code (US layout).
pub fn command(code: u32) -> Option<Cmd> {
    Some(match code {
        16 | 1 => Cmd::Quit, 49 | 109 | 57 => Cmd::Next, 25 | 104 => Cmd::Prev, 31 => Cmd::Slide, 19 => Cmd::Rotate, 33 | 11 => Cmd::Fit, 2 => Cmd::One,
        13 | 78 => Cmd::In, 12 | 74 => Cmd::Out, 103 => Cmd::Pan(0, -1), 108 => Cmd::Pan(0, 1), 105 => Cmd::Pan(-1, 0), 106 => Cmd::Pan(1, 0),
        _ => return None,
    })
}

struct View { files: Vec<PathBuf>, idx: usize, rot: u8, img: Option<Img>, cam: Cam, note: String, slide: Option<Instant>, drag: Option<(i32, i32)>, ptr: (i32, i32) }

impl View {
    fn name(&self) -> String { self.files[self.idx].file_name().map_or(String::new(), |n| crate::files::printable(&n.to_string_lossy())) }
    fn load(&mut self) {
        match decode(&self.files[self.idx], self.rot) {
            Ok(i) => { self.img = Some(i); self.note.clear(); }
            Err(e) => { self.img = None; self.note = e; }
        }
        self.cam = Cam { zoom: None, cx: 0.0, cy: 0.0 };
        if let Some(i) = &self.img { self.cam.cx = i.w as f32 / 2.0; self.cam.cy = i.h as f32 / 2.0; }
        if self.slide.is_some() { self.slide = Some(Instant::now() + SLIDE); }
    }
    fn step(&mut self, d: isize) { self.idx = (self.idx as isize + d).rem_euclid(self.files.len() as isize) as usize; self.rot = 0; self.load(); }
    fn paint(&mut self, c: &mut Client, font: &Font, busy: bool) {
        let (w, h, sc) = (c.w as usize, c.h as usize, c.scale.max(1) as i32);
        let bar = font.h + 6 * sc as usize;
        let ah = h.saturating_sub(bar).max(1);
        let px = c.pixels();
        if px.len() < w * h { return }
        match &self.img {
            Some(i) => { self.cam.clamp(i.w, i.h, w, ah); let s = self.cam.scale(i.w, i.h, w, ah); draw_image(px, w, ah, i, s, self.cam.cx, self.cam.cy); }
            None => fill(px, w, Rect { x: 0, y: 0, w: w as i32, h: ah as i32 }, PANEL),
        }
        fill(px, w, Rect { x: 0, y: ah as i32, w: w as i32, h: bar as i32 }, PANEL);
        fill(px, w, Rect { x: 0, y: ah as i32, w: w as i32, h: 1 }, BORDER_DIM);
        let (info, bad) = match (&self.img, busy) {
            (_, true) => ("\u{25d0} loading picture".to_string(), false),
            (Some(i), _) => (format!("{}x{}  {}%{}", i.w, i.h, (self.cam.scale(i.w, i.h, w, ah) * 100.0).round(), if self.cam.zoom.is_none() { " fit" } else { "" }), false),
            (None, _) => (self.note.clone(), true),
        };
        let room = w / font.w;
        let y = (ah + 3 * sc as usize) as i32;
        let mut x = font.w as i32;
        x += text(px, w, font, x, y, &self.name().chars().take(room / 3).collect::<String>(), FG) + 2 * font.w as i32;
        x += text(px, w, font, x, y, &format!("{}/{}", self.idx + 1, self.files.len()), DIM) + 2 * font.w as i32;
        let rest: String = format!("{info}{}", if self.slide.is_some() { "  slideshow" } else { "" }).chars().take(room.saturating_sub(x as usize / font.w + 2)).collect();
        let lw = x as usize + text(px, w, font, x, y, &rest, if bad { ERR } else if busy { ACCENT } else { DIM }) as usize;
        let help = [("n/p", "next"), ("s", "slide"), ("r", "rotate"), ("f", "fit"), ("1", "1:1"), ("q", "quit")];
        let hw = hint_cols(&help) * font.w;
        if lw + hw + 6 * font.w < w { hints(px, w, font, (w - hw - font.w) as i32, y, &help); }
    }
    /// Applies a command; false = quit.
    fn run(&mut self, cmd: Cmd, area: (usize, usize)) -> bool {
        let (aw, ah) = area; let Some((iw, ih)) = self.img.as_ref().map(|i| (i.w, i.h)) else { return self.run_nopic(cmd) };
        match cmd {
            Cmd::Quit => return false,
            Cmd::Next => self.step(1), Cmd::Prev => self.step(-1),
            Cmd::Slide => self.slide = if self.slide.is_some() { None } else { Some(Instant::now() + SLIDE) },
            Cmd::Rotate => { self.rot = (self.rot + 1) % 4; self.load(); }
            Cmd::Fit => self.cam = Cam { zoom: None, cx: iw as f32 / 2.0, cy: ih as f32 / 2.0 },
            Cmd::One => { let (px, py) = (aw as f32 / 2.0, ah as f32 / 2.0); let s = self.cam.scale(iw, ih, aw, ah); self.cam.zoom_at(1.0 / s, px, py, iw, ih, aw, ah); }
            Cmd::In => self.cam.zoom_at(1.25, aw as f32 / 2.0, ah as f32 / 2.0, iw, ih, aw, ah),
            Cmd::Out => self.cam.zoom_at(0.8, aw as f32 / 2.0, ah as f32 / 2.0, iw, ih, aw, ah),
            Cmd::Pan(x, y) => { let s = self.cam.scale(iw, ih, aw, ah); if self.cam.zoom.is_none() { self.cam.zoom = Some(s); } self.cam.cx += x as f32 * 64.0 / s; self.cam.cy += y as f32 * 64.0 / s; }
        }
        true
    }
    fn run_nopic(&mut self, cmd: Cmd) -> bool {   // nothing decoded: only moving on, rotating again and quitting make sense
        match cmd { Cmd::Quit => return false, Cmd::Next => self.step(1), Cmd::Prev => self.step(-1), Cmd::Rotate => { self.rot = (self.rot + 1) % 4; self.load(); } _ => {} }
        true
    }
}

/// `view --selftest`: locks this process into the sandbox and checks that reading a file outside the rules fails.
fn selftest() -> i32 {
    match sandbox::rules(&[Path::new("/web")], &[]).enforce() {
        Ok(true) => if std::fs::File::open("/proc/self/stat").is_ok() { println!("sandbox: Landlock did not block a read"); 1 } else { println!("sandbox ok"); 0 },
        Ok(false) => { println!("sandbox: no Landlock in this kernel, only no_new_privs"); 1 }
        Err(e) => { println!("sandbox: {e}"); 1 }
    }
}

pub fn run(args: &[String]) -> i32 {
    if args == ["--selftest"] { return selftest() }
    let [arg] = args else { eprintln!("usage: view FILE|DIR"); return 2 };
    let (files, idx) = list(Path::new(arg));
    if files.is_empty() { eprintln!("view: no pictures in {arg}"); return 1 }
    if !Path::new(FFMPEG).exists() { eprintln!("view needs the web add-on"); return 1 }
    let mut c = match Client::connect() {
        Ok(c) => c,
        Err(e) => { eprintln!("view needs the desktop: run it in a den pane ({e})"); return 1 }
    };
    let font = std::fs::read(if c.scale >= 2 { "/share/font32.psf" } else { "/share/font16.psf" }).ok().and_then(|b| Font::parse(&b).ok());
    let Some(font) = font else { eprintln!("view: no font"); return 1 };
    let _title = crate::gui::draw::title("view");
    let mut v = View { files, idx, rot: 0, img: None, cam: Cam { zoom: None, cx: 0.0, cy: 0.0 }, note: String::new(), slide: None, drag: None, ptr: (0, 0) };
    let area = |c: &Client| (c.w as usize, (c.h as usize).saturating_sub(font.h + 6 * c.scale.max(1) as usize).max(1));
    if c.attach().is_err() { return 1 }
    let mut attached = (c.w, c.h);
    v.paint(&mut c, &font, true); if c.commit(&[(0, 0, c.w.min(65535) as u16, c.h.min(65535) as u16)]).is_err() { return 1 }
    v.load();
    let (mut dirty, mut waiting) = (true, true);   // the first commit's DONE is still to come
    loop {
        let wait = v.slide.map_or(-1, |t| t.saturating_duration_since(Instant::now()).as_millis().min(i32::MAX as u128) as i32);
        let mut ev = match c.poll(if dirty && !waiting { 0 } else { wait }) { Ok(e) => e, Err(_) => return 0 };   // den closed the socket: leave quietly
        if ev.is_none() && v.slide.is_some_and(|t| t <= Instant::now()) { v.step(1); dirty = true; }
        while let Some(e) = ev {
            let (aw, ah) = area(&c);
            match e {
                Down::Done => waiting = false,
                Down::Configure { .. } => { dirty = true; if (c.w, c.h) != attached { if c.attach().is_err() { return 1 } attached = (c.w, c.h); } }   // a new size: den dropped the old buffer
                Down::Key { code, value, .. } if value != 0 => if let Some(cmd) = command(code) {
                    let repeats = matches!(cmd, Cmd::Pan(..) | Cmd::In | Cmd::Out);
                    if value == 1 || repeats { if !v.run(cmd, (aw, ah)) { return 0 } dirty = true; }
                },
                Down::Button { code: 0x110, pressed } => v.drag = pressed.then_some(v.ptr),
                Down::Pointer { x, y } => {
                    if let (Some((lx, ly)), Some(i)) = (v.drag, v.img.as_ref()) {
                        let s = v.cam.scale(i.w, i.h, aw, ah);
                        if v.cam.zoom.is_none() { v.cam.zoom = Some(s); }
                        v.cam.cx -= (x - lx) as f32 / s; v.cam.cy -= (y - ly) as f32 / s;
                        v.drag = Some((x, y)); dirty = true;
                    }
                    v.ptr = (x, y);
                }
                Down::Scroll { dy, .. } if dy != 0 => if let Some((iw, ih)) = v.img.as_ref().map(|i| (i.w, i.h)) {
                    v.cam.zoom_at(if dy > 0 { 1.25 } else { 0.8 }, v.ptr.0 as f32, v.ptr.1 as f32, iw, ih, aw, ah); dirty = true;
                },
                _ => {}
            }
            ev = c.poll(0).unwrap_or(None);
        }
        if dirty && !waiting {
            v.paint(&mut c, &font, false);
            if c.commit(&[(0, 0, c.w.min(65535) as u16, c.h.min(65535) as u16)]).is_err() { return 0 }
            (dirty, waiting) = (false, true);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn dims_from_ffmpeg_text() {
        assert_eq!(parse_dims("Input #0, png_pipe\n  Stream #0:0: Video: png, rgba(pc), 400x300, 25 fps"), Some((400, 300)));
        assert_eq!(parse_dims("Stream #0:0[0x1]: Video: h264 (High) (avc1 / 0x31637661), yuv420p(progressive), 1920x1080 [SAR 1:1 DAR 16:9], 25 fps"), Some((1920, 1080)));
        assert_eq!(parse_dims("Stream #0:0: Audio: mp3, 44100 Hz"), None); assert_eq!(parse_dims("Video: x, 0x0, 5"), None); assert_eq!(parse_dims(""), None);
    }
    #[test] fn filter_chain_and_size() {
        assert_eq!(filters(400, 300, 0), (String::new(), 400, 300));
        assert_eq!(filters(400, 300, 1), ("transpose=1".into(), 300, 400));
        assert_eq!(filters(400, 300, 2), ("transpose=1,transpose=1".into(), 400, 300));
        assert_eq!(filters(400, 300, 3), ("transpose=2".into(), 300, 400));
        assert_eq!(filters(8000, 2000, 0), ("scale=4096:1024".into(), 4096, 1024));
        assert_eq!(filters(2000, 8000, 1), ("scale=1024:4096,transpose=1".into(), 4096, 1024));
        assert_eq!(filters(10000, 1, 0).0, "scale=4096:1");
    }
    #[test] fn folder_listing() {
        let d = std::env::temp_dir().join(format!("thir-view-{}", std::process::id())); std::fs::create_dir_all(&d).unwrap();
        for n in ["b.PNG", "a.jpg", "c.txt", "d.gif"] { std::fs::write(d.join(n), "x").unwrap(); }
        std::fs::create_dir(d.join("e.png")).unwrap();   // a directory named like a picture
        let names = |v: &[PathBuf]| v.iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect::<Vec<_>>();
        let (v, i) = list(&d); assert_eq!((names(&v), i), (vec!["a.jpg".into(), "b.PNG".into(), "d.gif".into()], 0));
        let (v, i) = list(&d.join("b.PNG")); assert_eq!((v.len(), i), (3, 1));
        let (v, i) = list(&d.join("c.txt")); assert_eq!((names(&v), i), (vec!["c.txt".into()], 0));   // not a picture name: shown alone
        std::fs::remove_dir_all(&d).unwrap();
    }
    #[test] fn fit_never_enlarges() {
        assert_eq!(Cam::fit(400, 300, 1258, 700), 1.0); assert!((Cam::fit(2000, 1000, 1000, 700) - 0.5).abs() < 1e-6); assert!((Cam::fit(1000, 2000, 1000, 500) - 0.25).abs() < 1e-6);
    }
    #[test] fn small_pictures_centre_and_big_ones_stay_covered() {
        let mut c = Cam { zoom: None, cx: 7.0, cy: 7.0 }; c.clamp(400, 300, 1258, 700); assert_eq!((c.cx, c.cy), (200.0, 150.0));
        let mut c = Cam { zoom: Some(1.0), cx: -50.0, cy: 99999.0 }; c.clamp(2000, 1500, 1000, 600); assert_eq!((c.cx, c.cy), (500.0, 1200.0));
    }
    #[test] fn zoom_keeps_the_point_under_the_pointer() {
        let mut c = Cam { zoom: Some(1.0), cx: 1000.0, cy: 750.0 };
        let before = (c.cx + (300.0 - 500.0), c.cy + (200.0 - 300.0));   // picture point under window point (300, 200)
        c.zoom_at(2.0, 300.0, 200.0, 2000, 1500, 1000, 600);
        assert_eq!(c.zoom, Some(2.0));
        let after = (c.cx + (300.0 - 500.0) / 2.0, c.cy + (200.0 - 300.0) / 2.0); assert!((after.0 - before.0).abs() < 1e-3 && (after.1 - before.1).abs() < 1e-3);
        c.zoom_at(1000.0, 0.0, 0.0, 2000, 1500, 1000, 600); assert_eq!(c.zoom, Some(16.0)); c.zoom_at(1e-9, 0.0, 0.0, 2000, 1500, 1000, 600); assert_eq!(c.zoom, Some(0.05));
    }
    #[test] fn one_to_one_is_exact_and_shrinking_averages() {
        let img = Img { w: 4, h: 2, px: vec![0xFFFF0000, 0xFF00FF00, 0xFF0000FF, 0xFFFFFF00, 0xFF000000, 0xFF000000, 0xFFFFFFFF, 0xFFFFFFFF] };
        let mut px = vec![0u32; 8 * 4];   // 8x4 window, picture centred at 1:1: it covers x 2..6, y 1..3
        draw_image(&mut px, 8, 4, &img, 1.0, 2.0, 1.0);
        assert_eq!((px[8 + 2], px[8 + 3], px[8 + 5], px[2 * 8 + 4]), (0xFF0000, 0x00FF00, 0xFFFF00, 0xFFFFFF)); assert_eq!((px[0], px[8 + 1], px[8 + 6]), (PANEL, PANEL, PANEL));
        let mut px = vec![0u32; 2 * 1];   // all 4x2 shrunk into one 2x1 window: each pixel averages a 2x2 block of picture
        draw_image(&mut px, 2, 1, &img, 0.5, 2.0, 1.0);
        assert_eq!(px[0], (255 / 4) << 16 | (255 / 4) << 8 | 0);   // (red + green + black + black) / 4 per channel
    }
    #[test] fn alpha_shows_the_pane_colour() {
        let img = Img { w: 1, h: 1, px: vec![0x00FFFFFF] };   // fully transparent white
        let mut px = vec![0u32; 1]; draw_image(&mut px, 1, 1, &img, 1.0, 0.5, 0.5); assert_eq!(px[0], PANEL);
    }
    #[test] fn keys() {
        assert_eq!(command(16), Some(Cmd::Quit)); assert_eq!(command(49), Some(Cmd::Next)); assert_eq!(command(25), Some(Cmd::Prev)); assert_eq!(command(19), Some(Cmd::Rotate));
        assert_eq!(command(103), Some(Cmd::Pan(0, -1))); assert_eq!(command(106), Some(Cmd::Pan(1, 0))); assert_eq!(command(30), None); assert_eq!(command(99999), None);
    }
}
