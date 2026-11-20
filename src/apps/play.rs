//! play: music and video from the web add-on's ffmpeg (a sandboxed child: reads only the file and /web, writes nothing, no network).
//! `play FILE|DIR [-s]`. Music (no picture) is a text screen that also works on the serial console; a DIR plays its audio files by name, -s shuffles.
//! Video is a den surface: ffmpeg sends BGRA frames on fd 3 and PCM on fd 4; the sound card is the clock (frames written minus the card's delay), so the picture follows the sound.
//! Keys: space pause, left/right 10 s, n/p next/previous track, + - volume, q quit. Clicking the progress line in the video window seeks.
//! Pause, seek and a window resize restart ffmpeg at the new position (-ss). Animated GIF and WebP are video without sound and loop; `view` shows only their first frame.
//! `play --check FILE` decodes the whole file and prints what it found (used by test/media.sh).
use crate::{files, gui::{draw::{fill, hint_cols, hints, text}, font::Font, layout::Rect, surface::{Client, Down, R16}, theme::*}, line::{Decoder, Key}, sandbox, sound, term, tui};
use std::{fs::File, io::{Read, Write}, os::{fd::{AsRawFd, FromRawFd, OwnedFd}, unix::process::CommandExt}, path::{Path, PathBuf}, process::{Child, Command, Stdio},
    sync::{atomic::{AtomicBool, AtomicI64, AtomicU32, Ordering::Relaxed}, mpsc, Arc}, thread, time::{Duration, Instant}};

const FFMPEG: &str = "/usr/bin/ffmpeg";
const FPS: u32 = 30;
const EXTS: [&str; 8] = ["mp3", "ogg", "oga", "opus", "flac", "wav", "m4a", "aac"];

// ---- pure helpers (tested) ----

/// The audio files to play: DIR's by name (case-insensitive), or just FILE. `seed` shuffles (None = in order).
pub fn playlist(arg: &Path, seed: Option<u64>) -> Vec<PathBuf> {
    let mut v = if arg.is_dir() {
        let mut v: Vec<PathBuf> = std::fs::read_dir(arg).into_iter().flatten().flatten().map(|e| e.path())
            .filter(|p| p.is_file() && p.extension().and_then(|e| e.to_str()).is_some_and(|e| EXTS.contains(&e.to_ascii_lowercase().as_str()))).collect();
        v.sort_by_key(|p| p.file_name().map(|n| n.to_string_lossy().to_lowercase())); v
    } else { vec![arg.to_path_buf()] };
    if let Some(mut s) = seed { s |= 1; for i in (1..v.len()).rev() { s ^= s << 13; s ^= s >> 7; s ^= s << 17; v.swap(i, (s % (i as u64 + 1)) as usize); } }
    v
}
/// Is video frame `k` (0-based, FPS per second) due when `played` frames of a `rate` Hz clock are out? Frame k shows at k/FPS seconds.
pub fn due(k: u64, played: i64, rate: u32) -> bool { played >= 0 && k as i128 * rate as i128 <= played as i128 * FPS as i128 }
/// Milliseconds until frame `k` is due (at least 1).
pub fn wait_ms(k: u64, played: i64, rate: u32) -> i32 { (((k as i128 * rate as i128 / FPS as i128 - played as i128) * 1000 + rate as i128 - 1) / rate.max(1) as i128).clamp(1, 1000) as i32 }
/// The largest size with the video's aspect that fits (aw, ah); it enlarges small videos too.
pub fn fit_box(vw: usize, vh: usize, aw: usize, ah: usize) -> (usize, usize) {
    if vw == 0 || vh == 0 || aw == 0 || ah == 0 { return (1, 1) }
    if vw * ah <= vh * aw { (((vw * ah + vh / 2) / vh).clamp(1, aw), ah) } else { (aw, ((vh * aw + vw / 2) / vw).clamp(1, ah)) }
}
/// 3.7 s -> "0:03", 3725 s -> "62:05".
pub fn clock(s: f64) -> String { let s = s.max(0.0) as u64; format!("{}:{:02}", s / 60, s % 60) }
/// The new position after a seek of `delta` seconds, kept inside the file.
pub fn seek_to(pos: f64, delta: f64, dur: f64) -> f64 { let p = pos + delta; if dur > 0.0 { p.clamp(0.0, (dur - 0.5).max(0.0)) } else { p.max(0.0) } }
/// Volume (0 to 100 percent) applied to little-endian S16 samples in place. A trailing odd byte is left alone.
pub fn scale_s16le(buf: &mut [u8], vol: u32) {
    if vol >= 100 { return }
    for s in buf.chunks_exact_mut(2) { let v = i16::from_le_bytes([s[0], s[1]]) as i32 * vol as i32 / 100; s.copy_from_slice(&(v as i16).to_le_bytes()); }
}
/// A progress bar of `w` cells, `frac` 0 to 1 filled.
pub fn bar(frac: f64, w: usize) -> String { let n = ((frac.clamp(0.0, 1.0) * w as f64).round() as usize).min(w); format!("{}{}", "\u{25ae}".repeat(n), "\u{2500}".repeat(w - n)) }

#[derive(Debug, Default, PartialEq)]
pub struct Info { pub fmt: String, pub dur: f64, pub title: String, pub artist: String, pub video: Option<(String, usize, usize)>, pub audio: Option<(String, u32)> }
/// What `ffmpeg -i FILE` prints on stderr: container, duration, first tags, the first video (not cover art) and audio stream.
pub fn parse_info(t: &str) -> Info {
    let mut i = Info::default();
    for l in t.lines() {
        let s = l.trim();
        if let Some(r) = s.strip_prefix("Input #0, ") { i.fmt = r.split(',').next().unwrap_or("").to_string(); }
        else if let Some(r) = s.strip_prefix("Duration: ") {
            let hms: Vec<f64> = r.split(',').next().unwrap_or("").split(':').filter_map(|x| x.trim().parse().ok()).collect();
            if let [h, m, sec] = hms[..] { i.dur = h * 3600.0 + m * 60.0 + sec; }
        } else if let Some((k, v)) = s.split_once(':').filter(|(k, _)| matches!(k.trim().to_ascii_lowercase().as_str(), "title" | "artist")) {   // the first one wins: the container's tags come before the streams' (Ogg has only the stream's)
            let slot = if k.trim().eq_ignore_ascii_case("title") { &mut i.title } else { &mut i.artist };
            if slot.is_empty() { *slot = files::printable(v.trim()); }
        } else if s.starts_with("Stream #") {
            if let Some((_, r)) = s.split_once(" Video: ") {
                if i.video.is_none() && !s.contains("(attached pic)") { if let Some((w, h)) = super::view::parse_dims(&format!("Video: {r}")) { i.video = Some((r.split(&[' ', ','][..]).next().unwrap_or("").to_string(), w, h)); } }
            } else if let Some((_, r)) = s.split_once(" Audio: ") {
                if i.audio.is_none() { let hz = r.split(", ").find_map(|p| p.strip_suffix(" Hz")?.parse().ok()).unwrap_or(0); i.audio = Some((r.split(&[' ', ','][..]).next().unwrap_or("").to_string(), hz)); }
            }
        }
    }
    i
}

// ---- ffmpeg ----

fn probe(file: &Path) -> Result<Info, String> {
    let rules = sandbox::rules(&[Path::new("/web")], &[file]);
    let mut c = Command::new(FFMPEG);
    c.args(["-hide_banner", "-nostdin", "-i"]).arg(file).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped());
    // SAFETY: only prctl and landlock syscalls run between fork and exec.
    unsafe { c.pre_exec(move || rules.enforce().map(|_| ())); }
    let i = parse_info(&String::from_utf8_lossy(&c.output().map_err(|e| format!("cannot run ffmpeg: {e}"))?.stderr));
    if i.video.is_none() && i.audio.is_none() { Err("not a file ffmpeg can play".into()) } else { Ok(i) }
}
fn pipe() -> std::io::Result<(File, OwnedFd)> {   // (read end, write end moved above fd 9 so the child's dup2 to 3 and 4 cannot clobber it)
    let mut fds = [0; 2];
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } != 0 { return Err(std::io::Error::last_os_error()) }
    let (r, w) = unsafe { (File::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) };
    let hi = unsafe { libc::fcntl(w.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 10) };
    if hi < 0 { return Err(std::io::Error::last_os_error()) }
    Ok((r, unsafe { OwnedFd::from_raw_fd(hi) }))
}

/// The sound card, shared by the audio thread and the clock. `written` counts frames since the last (re)start; `base` frames from before an underrun.
struct Out { pcm: File, rate: u32, vol: AtomicU32, written: AtomicI64, base: AtomicI64, ended: AtomicBool, stop: AtomicBool }
impl Out {
    fn played(&self) -> i64 { self.base.load(Relaxed) + self.written.load(Relaxed) - sound::delay(&self.pcm) }
}
fn audio_thread(mut src: File, out: Arc<Out>) {
    let mut buf = vec![0u8; 16384];
    let mut have = 0;
    'outer: while !out.stop.load(Relaxed) {
        let n = match src.read(&mut buf[have..]) { Ok(0) | Err(_) => break, Ok(n) => n };
        have += n;
        let u = have / 4 * 4;
        scale_s16le(&mut buf[..u], out.vol.load(Relaxed));
        let mut off = 0;
        while off < u {
            match (&out.pcm).write(&buf[off..u]) {
                Ok(w) => { off += w; out.written.fetch_add(w as i64 / 4, Relaxed); }
                Err(e) if e.raw_os_error() == Some(libc::EPIPE) => { out.base.fetch_add(out.written.swap(0, Relaxed), Relaxed); sound::restart(&out.pcm); }   // an underrun: what was written counts as played
                Err(e) if e.raw_os_error() == Some(libc::EINTR) => {}
                Err(_) => break 'outer,
            }
        }
        buf.copy_within(u..have, 0); have -= u;
    }
    if !out.stop.load(Relaxed) { sound::drain(&out.pcm); }
    out.ended.store(true, Relaxed);
}

/// One ffmpeg child and its reader threads, started at `start` seconds.
struct Run { child: Child, frames: Option<mpsc::Receiver<Vec<u8>>>, audio: Option<thread::JoinHandle<()>>, out: Option<Arc<Out>>, t0: Instant, start: f64, k: u64, held: Option<Vec<u8>>, eof: bool }
impl Run {
    fn start(file: &Path, start: f64, video: Option<(usize, usize)>, out: Option<Arc<Out>>) -> std::io::Result<Run> {
        let mut args: Vec<String> = ["-hide_banner", "-nostdin", "-loglevel", "error", "-noautorotate", "-ss"].map(String::from).into();
        args.push(format!("{start:.3}")); args.push("-i".into()); args.push(file.to_string_lossy().into_owned());
        if let Some((w, h)) = video { args.extend(["-map", "0:v:0", "-vf"].map(String::from)); args.push(format!("fps={FPS},scale={w}:{h}")); args.extend(["-pix_fmt", "bgra", "-f", "rawvideo", "pipe:3"].map(String::from)); }
        if let Some(o) = &out { args.extend(["-map", "0:a:0", "-ar"].map(String::from)); args.push(o.rate.to_string()); args.extend(["-ac", "2", "-f", "s16le", "pipe:4"].map(String::from)); }
        let (vr, vwr): (Option<File>, Option<OwnedFd>) = (if video.is_some() { Some(pipe()?) } else { None }).unzip();
        let (ar, awr): (Option<File>, Option<OwnedFd>) = (if out.is_some() { Some(pipe()?) } else { None }).unzip();
        let (vw, aw) = (vwr.as_ref().map_or(-1, |p| p.as_raw_fd()), awr.as_ref().map_or(-1, |p| p.as_raw_fd()));
        let rules = sandbox::rules(&[Path::new("/web")], &[file]);
        let mut c = Command::new(FFMPEG);
        c.args(&args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        // SAFETY: only prctl, landlock and dup2 run between fork and exec.
        unsafe { c.pre_exec(move || { if vw >= 0 { libc::dup2(vw, 3); } if aw >= 0 { libc::dup2(aw, 4); } rules.enforce().map(|_| ()) }); }
        let child = c.spawn()?;
        drop((vwr, awr));   // only the child holds the write ends now: its exit is EOF here
        let frames = vr.map(|mut rd| {
            let size = video.map_or(0, |(w, h)| w * h * 4);
            let (tx, rx) = mpsc::sync_channel(3);
            thread::spawn(move || loop { let mut b = vec![0u8; size]; if rd.read_exact(&mut b).is_err() || tx.send(b).is_err() { break } });
            rx
        });
        let audio = ar.zip(out.clone()).map(|(rd, o)| {
            o.stop.store(false, Relaxed); o.ended.store(false, Relaxed); o.written.store(0, Relaxed); o.base.store(0, Relaxed); sound::restart(&o.pcm);
            thread::spawn(move || audio_thread(rd, o))
        });
        Ok(Run { child, frames, audio, out, t0: Instant::now(), start, k: 0, held: None, eof: false })
    }
    /// How far the clock is: (units played, units per second). The sound card when there is sound, else the wall clock in microseconds.
    fn clock(&self) -> (i64, u32) { match &self.out { Some(o) => (o.played(), o.rate), None => (self.t0.elapsed().as_micros() as i64, 1_000_000) } }
    fn pos(&self) -> f64 { let (p, r) = self.clock(); self.start + p.max(0) as f64 / r as f64 }
    fn ended(&self) -> bool { match &self.out { Some(o) => o.ended.load(Relaxed), None => self.eof && self.held.is_none() } }
    /// The newest frame that is due by now (older due ones are skipped); None if none is.
    fn frame(&mut self) -> Option<Vec<u8>> {
        let played = self.clock();
        let mut latest = None;
        loop {
            let f = match self.held.take() { Some(f) => f, None => match self.frames.as_ref()?.try_recv() { Ok(f) => f, Err(mpsc::TryRecvError::Empty) => break, Err(_) => { self.eof = true; break } } };
            if due(self.k, played.0, played.1) { latest = Some(f); self.k += 1; } else { self.held = Some(f); break }
        }
        latest
    }
    fn next_wait(&self) -> i32 { let (p, r) = self.clock(); if self.held.is_some() { wait_ms(self.k, p, r).min(20) } else { 10 } }
    fn stop(&mut self) {
        if let Some(o) = &self.out { o.stop.store(true, Relaxed); }
        let _ = self.child.kill(); let _ = self.child.wait();
        if let Some(o) = &self.out { sound::restart(&o.pcm); }   // wakes a write that is waiting for room
        if let Some(h) = self.audio.take() { let _ = h.join(); }
    }
}
impl Drop for Run { fn drop(&mut self) { self.stop(); } }

/// A file being played: where it stands, and the run (None = paused or not started).
struct Player { file: PathBuf, info: Info, out: Option<Arc<Out>>, run: Option<Run>, pos: f64, vid: Option<(usize, usize)> }
impl Player {
    fn new(file: PathBuf, info: Info, out: Option<Arc<Out>>) -> Player { Player { file, info, out, run: None, pos: 0.0, vid: None } }
    fn go(&mut self, pos: f64) -> bool { self.go_with(pos, true) }
    /// Starts without sound: the clock is the wall clock (to fetch one still frame).
    fn go_still(&mut self, pos: f64) -> bool { self.go_with(pos, false) }
    fn go_with(&mut self, pos: f64, sound: bool) -> bool {
        self.stop(); self.pos = pos;
        let out = self.out.as_ref().filter(|_| sound && self.info.audio.is_some()).cloned();
        self.run = Run::start(&self.file, pos, self.vid, out).ok(); self.run.is_some()
    }
    fn stop(&mut self) { if let Some(r) = self.run.as_mut() { self.pos = r.pos().min(if self.info.dur > 0.0 { self.info.dur } else { f64::MAX }); r.stop(); } self.run = None; }
    fn now(&self) -> f64 { self.run.as_ref().map_or(self.pos, |r| r.pos().min(if self.info.dur > 0.0 { self.info.dur } else { f64::MAX })) }
    fn playing(&self) -> bool { self.run.is_some() }
    fn name(&self) -> String { if !self.info.title.is_empty() { self.info.title.clone() } else { files::printable(&self.file.file_name().map_or(String::new(), |n| n.to_string_lossy().into_owned())) } }
}

fn open_sound() -> std::io::Result<Arc<Out>> {
    let (pcm, rate) = sound::retry_busy(|| sound::open_buffered(200), Duration::from_secs(2), Duration::from_millis(100))?;
    let vol = sound::config(&crate::store::read_settings()).1;
    Ok(Arc::new(Out { pcm, rate, vol: AtomicU32::new(vol), written: AtomicI64::new(0), base: AtomicI64::new(0), ended: AtomicBool::new(false), stop: AtomicBool::new(false) }))
}
fn volume_step(out: &Out, d: i32) { out.vol.store((out.vol.load(Relaxed) as i32 + d).clamp(0, 100) as u32, Relaxed); }

// ---- music: the text screen ----

fn music_lines(p: &Player, n: (usize, usize), vol: u32, shuffle: bool, cols: usize, rows: usize, paused: bool) -> Vec<String> {
    let w = tui::width(cols);
    let (pos, dur) = (p.now(), p.info.dur);
    let (acc, dim, fg, r) = (tui::ACC, tui::DIM, tui::FG, tui::R);
    let mut v = vec![tui::head("play", &if n.1 > 1 { format!("{}/{}", n.0 + 1, n.1) } else { String::new() }, w), String::new()];
    v.push(format!(" {acc}\u{25cf}{r} {fg}{}{r}", tui::fit(&p.name(), w.saturating_sub(2))));
    v.push(format!("   {dim}{}{r}", tui::fit(&p.info.artist, w.saturating_sub(3))));
    v.push(String::new());
    v.push(format!(" {dim}{}{r}  {fg}{} / {}{r}", if paused { "paused " } else { "playing" }, clock(pos), if dur > 0.0 { clock(dur) } else { "?:??".into() }));
    let b = bar(if dur > 0.0 { pos / dur } else { 0.0 }, w);
    let k = b.chars().filter(|&c| c == '\u{25ae}').count();
    v.push(format!(" {acc}{}{}{}{r}", "\u{25ae}".repeat(k), tui::BDIM, "\u{2500}".repeat(w - k)));
    v.push(String::new());
    v.push(tui::dots(&[format!("{dim}volume {vol}%{r}"), format!("{dim}{}{r}", if shuffle { "shuffle" } else { "in order" })]));
    while v.len() + 2 < rows { v.push(String::new()); }
    v.push(String::new());
    v.push(tui::hints(&[("space", "pause"), ("\u{2190}/\u{2192}", "10 s"), ("n/p", "track"), ("+/-", "volume"), ("q", "quit")]));
    v
}

fn music(list: Vec<PathBuf>, shuffle: bool) -> i32 {
    let Some(_raw) = term::Raw::enter() else { eprintln!("play: needs a terminal"); return 1 };
    let out = match open_sound() { Ok(o) => o, Err(e) => { drop(_raw); eprintln!("play: no sound ({e})"); return 1 } };
    let (mut dec, mut i, mut played_any) = (Decoder::new(), 0usize, false);
    term::cursor_hide();
    let _title = crate::gui::draw::title("play");
    let mut dir = 1isize;
    while i < list.len() {
        let info = match probe(&list[i]) { Ok(x) if x.audio.is_some() => x, _ => { i = (i as isize + dir).max(0) as usize; if dir < 0 && i == 0 { dir = 1; } continue } };
        let mut p = Player::new(list[i].clone(), info, Some(out.clone()));
        played_any |= p.go(0.0);
        let mut next = 1isize;
        loop {
            let paused = !p.playing();
            tui::draw(&music_lines(&p, (i, list.len()), out.vol.load(Relaxed), shuffle, term::size().0, term::size().1, paused));
            let Some(keys) = term::read_keys(&mut dec, 250) else { return 0 };
            let mut quit = false;
            for k in keys {
                match k {
                    Key::Char(' ') => if p.playing() { p.stop() } else { p.go(p.pos); },
                    Key::Left | Key::Right => { let was = p.playing(); let to = seek_to(p.now(), if k == Key::Left { -10.0 } else { 10.0 }, p.info.dur); if was { p.go(to) } else { p.pos = to; true }; }
                    Key::Char('n') => { next = 1; p.stop(); p.pos = f64::MAX; }
                    Key::Char('p') => { next = -1; p.stop(); p.pos = f64::MAX; }
                    Key::Char('+') | Key::Char('=') => volume_step(&out, 5), Key::Char('-') | Key::Char('_') => volume_step(&out, -5),
                    Key::Char('q') | Key::Ctrl(3) => quit = true,
                    _ => {}
                }
            }
            if quit { p.stop(); term::cursor_show(); term::put(b"\x1b[2J\x1b[H"); return 0 }
            if p.pos == f64::MAX || p.run.as_ref().is_some_and(|r| r.ended()) { break }
        }
        drop(p);
        i = (i as isize + next).clamp(0, list.len() as isize) as usize;
        dir = next;
        if i == 0 && next < 0 { dir = 1; }
    }
    term::cursor_show(); term::put(b"\x1b[2J\x1b[H");
    if played_any { 0 } else { eprintln!("play: nothing to play"); 1 }
}

// ---- video: a den surface ----

const PLAY_HINTS: [(&str, &str); 4] = [("space", "pause"), ("</>", "seek"), ("+/-", "vol"), ("q", "quit")];
fn strip_h(font: &Font, scale: u32) -> usize { font.h + 10 * scale.max(1) as usize }
/// Draws the control strip at the bottom of the surface; its rect.
fn paint_strip(c: &mut Client, font: &Font, p: &Player, vol: Option<u32>, note: &str) -> R16 {
    let (w, h, sc, sh) = (c.w as usize, c.h as usize, c.scale.max(1) as usize, strip_h(font, c.scale));
    let y0 = h.saturating_sub(sh);
    let px = c.pixels();
    if px.len() < w * h { return (0, 0, 0, 0) }
    fill(px, w, Rect { x: 0, y: y0 as i32, w: w as i32, h: sh as i32 }, PANEL);
    let frac = if p.info.dur > 0.0 { (p.now() / p.info.dur).clamp(0.0, 1.0) } else { 0.0 };
    fill(px, w, Rect { x: 0, y: y0 as i32, w: w as i32, h: 3 * sc as i32 }, BORDER_DIM);
    fill(px, w, Rect { x: 0, y: y0 as i32, w: (frac * w as f64) as i32, h: 3 * sc as i32 }, ACCENT);
    let y = (y0 + 6 * sc) as i32;
    let mut x = font.w as i32;
    x += text(px, w, font, x, y, if p.playing() { "\u{25cf} " } else { "\u{25cb} " }, if p.playing() { ACCENT } else { DIM });
    let room = (w / font.w).saturating_sub(4);
    x += text(px, w, font, x, y, &p.name().chars().take(room.min(40)).collect::<String>(), FG) + 2 * font.w as i32;
    let times = format!("{} / {}{}", clock(p.now()), if p.info.dur > 0.0 { clock(p.info.dur) } else { "?:??".into() }, vol.map_or(String::new(), |v| format!("   vol {v}")));
    x += text(px, w, font, x, y, &times, DIM);
    if !note.is_empty() { x += text(px, w, font, x + 2 * font.w as i32, y, &note.chars().take(room.min(30)).collect::<String>(), WARN); }
    let hw = hint_cols(&PLAY_HINTS) * font.w;
    if x as usize + hw + 6 * font.w < w { hints(px, w, font, (w - hw - font.w) as i32, y, &PLAY_HINTS); }
    (0, y0 as u16, w.min(65535) as u16, sh as u16)
}

fn video(file: PathBuf, info: Info) -> i32 {
    let mut c = match Client::connect() { Ok(c) => c, Err(e) => { eprintln!("play: video needs the desktop: run it in a den pane ({e})"); return 1 } };
    let font = std::fs::read(if c.scale >= 2 { "/share/font32.psf" } else { "/share/font16.psf" }).ok().and_then(|b| Font::parse(&b).ok());
    let Some(font) = font else { eprintln!("play: no font"); return 1 };
    let _title = crate::gui::draw::title("play");
    let (vw, vh) = info.video.as_ref().map_or((1, 1), |v| (v.1, v.2));
    let (out, mut note) = match open_sound() { Ok(o) if info.audio.is_some() => (Some(o), String::new()), Ok(_) => (None, String::new()), Err(e) => (None, format!("no sound ({e})")) };
    let mut p = Player::new(file, info, out.clone());
    if c.attach().is_err() { return 1 }   // a new surface is all black
    let mut attached = (c.w, c.h);
    let (mut dirty, mut waiting, mut ptr, mut last_sec) = (true, false, (0i32, 0i32), -1i64);
    let (mut shown, mut ended, mut hold): (Option<Vec<u8>>, bool, bool) = (None, false, false);   // hold: a silent run that only fetches one frame to show while paused
    let layout = |c: &Client| { let (aw, ah) = (c.w as usize, (c.h as usize).saturating_sub(strip_h(&font, c.scale)).max(1)); let (ow, oh) = fit_box(vw, vh, aw, ah); (ow, oh, (aw - ow) / 2, (ah - oh) / 2) };
    // (re)start at `pos` for the current window size; `still`: show the frame at `pos` and stay paused
    let begin = |p: &mut Player, c: &Client, pos: f64, still: bool| { let (ow, oh, _, _) = layout(c); p.vid = Some((ow, oh)); if still { p.go_still(pos) } else { p.go(pos) } };
    begin(&mut p, &c, 0.0, false);
    loop {
        let wait = if let Some(r) = p.run.as_ref() { r.next_wait() } else { 250 };
        let mut ev = match c.poll(if dirty && !waiting { 0 } else { wait }) { Ok(e) => e, Err(_) => return 0 };
        while let Some(e) = ev {
            let dur = p.info.dur;
            let seek_by = |p: &mut Player, c: &Client, to: f64, hold: &mut bool| { let still = !p.playing(); begin(p, c, to, still); *hold = still; };
            match e {
                Down::Done => waiting = false,
                Down::Configure { .. } => if (c.w, c.h) != attached {
                    if c.attach().is_err() { return 1 }
                    attached = (c.w, c.h);
                    let to = seek_to(p.now(), 0.0, dur); seek_by(&mut p, &c, to, &mut hold); dirty = true;
                },
                Down::Key { code, value, .. } if value != 0 => {
                    match code {
                        16 | 1 => return 0,
                        57 if value == 1 => if p.playing() && !hold { p.stop(); } else { let pos = if ended { 0.0 } else { p.pos }; if ended { note.clear(); } ended = false; hold = false; begin(&mut p, &c, pos, false); },
                        105 | 106 => { let to = seek_to(p.now(), if code == 105 { -10.0 } else { 10.0 }, dur); ended = false; seek_by(&mut p, &c, to, &mut hold); }
                        13 | 78 => if let Some(o) = &out { volume_step(o, 5) }, 12 | 74 => if let Some(o) = &out { volume_step(o, -5) },
                        _ => {}
                    }
                    dirty = true;
                }
                Down::Pointer { x, y } => ptr = (x, y),
                Down::Button { code: 0x110, pressed: true } if ptr.1 >= c.h as i32 - strip_h(&font, c.scale) as i32 && dur > 0.0 => {
                    let to = (ptr.0.clamp(0, c.w as i32) as f64 / c.w as f64 * dur).min(dur - 0.5).max(0.0);
                    ended = false; seek_by(&mut p, &c, to, &mut hold); dirty = true;
                }
                _ => {}
            }
            ev = c.poll(0).unwrap_or(None);
        }
        let (mut got, mut over) = (false, false);
        if let Some(r) = p.run.as_mut() { if let Some(f) = r.frame() { shown = Some(f); got = true; } over = r.ended(); }
        if got { dirty = true; if hold { p.stop(); hold = false; over = false; } }
        if over {
            if hold || (p.info.audio.is_none() && dur_short(&p)) { hold = false; begin(&mut p, &c, 0.0, false); }   // a picture-only clip (GIF, WebP) loops
            else { p.stop(); p.pos = p.info.dur; ended = true; note = "ended: space plays again".into(); dirty = true; }
        }
        let sec = (p.now() * 2.0) as i64;
        if sec != last_sec { last_sec = sec; dirty = true; }
        if dirty && !waiting {
            let (ow, oh, ox, oy) = layout(&c);
            let (w, h) = (c.w as usize, c.h as usize);
            let mut r = vec![];
            if let Some(f) = shown.take() {
                let px = c.pixels();
                if px.len() >= w * h && f.len() == ow * oh * 4 {
                    // SAFETY: the surface is u32 pixels; the rows below stay inside the video area.
                    let bytes = unsafe { std::slice::from_raw_parts_mut(px.as_mut_ptr() as *mut u8, px.len() * 4) };
                    for y in 0..oh { bytes[((oy + y) * w + ox) * 4..((oy + y) * w + ox + ow) * 4].copy_from_slice(&f[y * ow * 4..(y + 1) * ow * 4]); }
                    r.push((ox as u16, oy as u16, ow as u16, oh as u16));
                }
            }
            if r.is_empty() && p.run.is_none() { r.push((0, 0, c.w.min(65535) as u16, c.h.min(65535) as u16)); }   // a resize: repaint the whole surface
            r.push(paint_strip(&mut c, &font, &p, out.as_ref().map(|o| o.vol.load(Relaxed)), &note));
            if c.commit(&r).is_err() { return 0 }
            (dirty, waiting) = (false, true);
        }
    }
}
fn dur_short(p: &Player) -> bool { p.info.dur < 60.0 }

// ---- --check ----

/// Decodes everything in `file` (no sound device, no den) and prints one line: container, codecs, size and how many frames and samples came out.
fn check(file: &Path) -> i32 {
    let info = match probe(file) { Ok(i) => i, Err(e) => { println!("play: {}: {e}", file.display()); return 1 } };
    let run = |args: &[&str]| -> Option<usize> {
        let rules = sandbox::rules(&[Path::new("/web")], &[file]);
        let mut c = Command::new(FFMPEG);
        c.args(["-hide_banner", "-nostdin", "-loglevel", "error", "-noautorotate", "-i"]).arg(file).args(args).stdin(Stdio::null()).stderr(Stdio::null());
        // SAFETY: only prctl and landlock syscalls run between fork and exec.
        unsafe { c.pre_exec(move || rules.enforce().map(|_| ())); }
        c.output().ok().filter(|o| o.status.success()).map(|o| o.stdout.len())
    };
    let v = info.video.as_ref().map(|(c, w, h)| (c.clone(), *w, *h, run(&["-map", "0:v:0", "-fps_mode", "passthrough", "-vf", "scale=8:8", "-pix_fmt", "gray", "-f", "rawvideo", "pipe:1"]).map(|n| n / 64)));
    let a = info.audio.as_ref().map(|(c, hz)| (c.clone(), *hz, run(&["-map", "0:a:0", "-ac", "1", "-f", "s16le", "pipe:1"]).map(|n| n / 2)));
    let n = |x: Option<usize>| x.map_or("failed".to_string(), |n| n.to_string());
    println!("play: {} fmt={} dur={:.2} video={} audio={}", file.display(), info.fmt, info.dur,
        v.as_ref().map_or("none".into(), |(c, w, h, f)| format!("{c} {w}x{h} frames={}", n(*f))), a.as_ref().map_or("none".into(), |(c, hz, s)| format!("{c} {hz}Hz samples={}", n(*s))));
    if v.as_ref().is_some_and(|x| x.3.is_none()) || a.as_ref().is_some_and(|x| x.2.is_none()) { 1 } else { 0 }
}

pub fn run(args: &[String]) -> i32 {
    let shuffle = args.iter().any(|a| a == "-s");
    let rest: Vec<&String> = args.iter().filter(|a| *a != "-s").collect();
    if !Path::new(FFMPEG).exists() { eprintln!("play needs the web add-on"); return 1 }
    match rest[..] {
        [f, a] if f == "--check" => check(Path::new(a)),
        [a] if !a.starts_with('-') => {
            let seed = shuffle.then(|| std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_nanos() as u64));
            let list = playlist(Path::new(a), seed);
            if list.is_empty() { eprintln!("play: no audio files in {a}"); return 1 }
            if Path::new(a).is_dir() { return music(list, shuffle) }
            match probe(&list[0]) {
                Err(e) => { eprintln!("play: {a}: {e}"); 1 }
                Ok(info) if info.video.is_some() => video(list[0].clone(), info),
                Ok(_) => music(list, shuffle),
            }
        }
        _ => { eprintln!("usage: play FILE|DIR [-s]   (music in the text screen, video in a den pane)"); 2 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const MP4: &str = "Input #0, mov,mp4,m4a,3gp,3g2,mj2, from 'a.mp4':\n  Metadata:\n    major_brand     : isom\n    title           : Quadrants\n    artist          : thir\n  Duration: 00:00:02.04, start: 0.000000, bitrate: 57 kb/s\n  Stream #0:0[0x1](und): Video: h264 (High) (avc1 / 0x31637661), yuv420p(progressive), 160x120 [SAR 1:1 DAR 4:3], 30 fps, 30 tbr\n    Metadata:\n      handler_name    : VideoHandler\n      title           : not this one\n  Stream #0:1[0x2](und): Audio: aac (LC) (mp4a / 0x6134706D), 44100 Hz, stereo, fltp, 69 kb/s\n";
    #[test] fn info_from_ffmpeg_text() {
        let i = parse_info(MP4);
        assert_eq!((i.fmt.as_str(), i.dur, i.title.as_str(), i.artist.as_str()), ("mov", 2.04, "Quadrants", "thir"));
        assert_eq!(i.video, Some(("h264".into(), 160, 120))); assert_eq!(i.audio, Some(("aac".into(), 44100)));
        let m = parse_info("Input #0, mp3, from 'a.mp3':\n  Duration: 01:02:03.50, start: 0.025057, bitrate: 128 kb/s\n  Stream #0:0: Audio: mp3 (mp3float), 44100 Hz, stereo, fltp, 128 kb/s\n  Stream #0:1: Video: mjpeg (Baseline), yuvj420p(pc, bt470bg/unknown/unknown), 300x300 [SAR 1:1 DAR 1:1], 90k tbr (attached pic)\n");
        assert_eq!((m.video.is_none(), m.dur), (true, 3723.5)); assert_eq!(m.audio, Some(("mp3".into(), 44100)));   // cover art is not a video
        assert_eq!(parse_info("Duration: N/A, bitrate: N/A").dur, 0.0); assert_eq!(parse_info(""), Info::default());
        assert_eq!(parse_info("  Stream #0:0: Audio: vorbis, 44100 Hz\n      Metadata:\n        title           : Tone B\n").title, "Tone B");
        assert_eq!(parse_info("  Metadata:\n    TITLE           : Loud\n    ARTIST          : Band\n").title, "Loud");   // Vorbis comments are upper case   // only the top-level tag lines count
        assert!(!parse_info("Input #0, flac, from 'x':\n  Metadata:\n    title           : a\u{1b}[31mb\n").title.contains('\u{1b}'));
    }
    #[test] fn scheduler_follows_the_audio_clock() {
        // 48 kHz: frame k is due at k/30 s = 1600 k samples
        assert!(due(0, 0, 48000) && !due(1, 1599, 48000) && due(1, 1600, 48000) && due(30, 48000, 48000) && !due(31, 48000, 48000));
        assert!(!due(0, -1, 48000)); assert!(due(0, 5, 44100));   // before the card started playing nothing is due (negative = still in the buffer)
        assert!(due(1, 33334, 1_000_000) && !due(1, 33333, 1_000_000));   // the wall clock in microseconds
        assert_eq!(wait_ms(1, 0, 48000), 34); assert_eq!(wait_ms(1, 1600, 48000), 1); assert_eq!(wait_ms(1, 800, 48000), 17); assert_eq!(wait_ms(100000, 0, 48000), 1000);
        let _ = due(u64::MAX, i64::MAX, u32::MAX);   // no overflow
    }
    #[test] fn video_fits_the_pane_keeping_its_shape() {
        assert_eq!(fit_box(160, 120, 1258, 700), (933, 700)); assert_eq!(fit_box(1920, 1080, 1258, 700), (1244, 700)); assert_eq!(fit_box(1920, 1080, 640, 700), (640, 360));
        assert_eq!(fit_box(100, 1000, 50, 50), (5, 50)); assert_eq!(fit_box(0, 5, 10, 10), (1, 1)); assert_eq!(fit_box(1000, 1, 10, 10), (10, 1));
    }
    #[test] fn s16_volume_scaling() {
        let mut b: Vec<u8> = [1000i16, -1000, i16::MAX, i16::MIN].iter().flat_map(|s| s.to_le_bytes()).collect();
        scale_s16le(&mut b, 100); assert_eq!(&b[..2], &1000i16.to_le_bytes());
        scale_s16le(&mut b, 50);
        let s: Vec<i16> = b.chunks(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect(); assert_eq!(s, [500, -500, 16383, -16384]);
        scale_s16le(&mut b, 0); assert!(b.iter().all(|&x| x == 0));
        let mut odd = vec![7u8, 0, 9]; scale_s16le(&mut odd, 0); assert_eq!(odd, [0, 0, 9]);   // the half sample at the end is kept for the next read
    }
    #[test] fn seek_stays_inside_the_file() {
        assert_eq!(seek_to(5.0, 10.0, 12.0), 11.5); assert_eq!(seek_to(5.0, -10.0, 12.0), 0.0); assert_eq!(seek_to(5.0, 10.0, 0.0), 15.0); assert_eq!(seek_to(0.0, -10.0, 0.0), 0.0); assert_eq!(seek_to(1.0, 10.0, 0.2), 0.0);
        assert_eq!((clock(3.7), clock(3725.0), clock(-4.0)), ("0:03".into(), "62:05".into(), "0:00".into()));
    }
    #[test] fn progress_bar() { assert_eq!(bar(0.5, 4), "\u{25ae}\u{25ae}\u{2500}\u{2500}"); assert_eq!(bar(2.0, 3), "\u{25ae}\u{25ae}\u{25ae}"); assert_eq!(bar(-1.0, 2), "\u{2500}\u{2500}"); assert_eq!(bar(0.5, 0), ""); }
    #[test] fn playlist_sorted_filtered_and_shuffled() {
        let d = std::env::temp_dir().join(format!("thir-play-{}", std::process::id())); std::fs::create_dir_all(&d).unwrap();
        for n in ["b.MP3", "a.flac", "c.txt", "d.wav", "e.mp4", "f.ogg"] { std::fs::write(d.join(n), "x").unwrap(); }
        std::fs::create_dir(d.join("g.mp3")).unwrap();
        let names = |v: &[PathBuf]| v.iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect::<Vec<_>>();
        assert_eq!(names(&playlist(&d, None)), ["a.flac", "b.MP3", "d.wav", "f.ogg"]);
        let (s1, s2) = (names(&playlist(&d, Some(1))), names(&playlist(&d, Some(1))));
        assert_eq!(s1, s2);   // the same seed, the same order
        let mut sorted = s1.clone(); sorted.sort(); assert_eq!(sorted, ["a.flac", "b.MP3", "d.wav", "f.ogg"]);   // nothing lost or doubled
        assert!((2..40).any(|s| names(&playlist(&d, Some(s))) != names(&playlist(&d, None))));   // and it does shuffle
        assert_eq!(names(&playlist(&d.join("c.txt"), None)), ["c.txt"]);   // a file is played as given
        assert!(playlist(&d.join("none"), Some(5)).len() == 1);
        std::fs::remove_dir_all(&d).unwrap();
    }
    #[test] fn music_screen_shows_title_artist_and_progress() {
        let mut p = Player::new("/x/song.mp3".into(), Info { dur: 100.0, title: "Song".into(), artist: "Me".into(), audio: Some(("mp3".into(), 44100)), ..Default::default() }, None);
        p.pos = 25.0;
        let l = music_lines(&p, (1, 3), 70, true, 80, 24, true).join("\n");
        for want in ["Song", "Me", "2/3", "paused", "0:25 / 1:40", "volume 70%", "shuffle"] { assert!(l.contains(want), "{want}: {l}"); }
        assert!(l.contains('\u{25ae}') && l.contains('\u{2500}'));
    }
}
