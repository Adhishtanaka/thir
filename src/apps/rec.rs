//! rec: the sound recorder. It never touches the microphone itself: den (root) asks the user on its own bar and relays the samples (see gui/mic.rs). Only den panes can do that,
//! so `rec` on the serial console says so and stops. The samples are mixed down to mono and written to ~/Recordings/rec-YYYY-MM-DD-HHMMSS.wav (folder 0700, file 0600, never overwritten).
//! Keys: q or Enter stop and save. Alt+R (den) also stops it.
use crate::{clock, gui::surface::{Client, Down, Ev, Up}, line::Key, term, tui};
use std::{fs::{DirBuilder, File, OpenOptions}, io::{Seek, SeekFrom, Write}, os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt}, path::{Path, PathBuf}, time::{Duration, Instant}};

/// The 44-byte header of a mono S16_LE WAV with `data` bytes of samples.
pub fn wav_header(data: u32, rate: u32) -> [u8; 44] {
    let mut h = [0u8; 44];
    let mut put = |at: usize, b: &[u8]| h[at..at + b.len()].copy_from_slice(b);
    put(0, b"RIFF"); put(4, &data.saturating_add(36).to_le_bytes()); put(8, b"WAVEfmt "); put(16, &16u32.to_le_bytes());
    put(20, &1u16.to_le_bytes()); put(22, &1u16.to_le_bytes()); put(24, &rate.to_le_bytes()); put(28, &(rate * 2).to_le_bytes()); put(32, &2u16.to_le_bytes()); put(34, &16u16.to_le_bytes());
    put(36, b"data"); put(40, &data.to_le_bytes());
    h
}
/// Stereo S16_LE bytes -> mono samples (the two channels averaged). A trailing partial frame is dropped.
pub fn downmix(stereo: &[u8]) -> Vec<i16> {
    stereo.chunks_exact(4).map(|f| ((i16::from_le_bytes([f[0], f[1]]) as i32 + i16::from_le_bytes([f[2], f[3]]) as i32) / 2) as i16).collect()
}
/// "rec-2026-11-20-143205.wav" from a local time.
pub fn file_name(l: &clock::Local) -> String { format!("rec-{:04}-{:02}-{:02}-{:02}{:02}{:02}", l.year, l.month, l.day, l.hour, l.min, l.sec) }
/// A new file `base`.wav in `dir`, or `base`-2.wav and so on: created exclusively (0600), so an existing recording is never touched.
pub fn create_new(dir: &Path, base: &str) -> std::io::Result<(File, PathBuf)> { create_new_ext(dir, base, "wav") }
/// The same for any extension (the camera's photos use it).
pub fn create_new_ext(dir: &Path, base: &str, ext: &str) -> std::io::Result<(File, PathBuf)> {
    for n in 1..100 {
        let p = dir.join(if n == 1 { format!("{base}.{ext}") } else { format!("{base}-{n}.{ext}") });
        match OpenOptions::new().write(true).create_new(true).mode(0o600).open(&p) { Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue, r => return r.map(|f| (f, p)) }
    }
    Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists, "too many files with this name"))
}
/// ~/Recordings, made 0700 when missing. A link or a folder of someone else is refused.
fn recordings_dir() -> Result<PathBuf, String> {
    let home = std::env::var("HOME").map_err(|_| "no home folder".to_string())?;
    let d = Path::new(&home).join("Recordings");
    match DirBuilder::new().mode(0o700).create(&d) { Ok(()) => {} Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {} Err(e) => return Err(format!("{}: {e}", d.display())) }
    let m = std::fs::symlink_metadata(&d).map_err(|e| e.to_string())?;
    if !m.is_dir() || m.uid() != unsafe { libc::getuid() } { return Err(format!("{} is not your folder", d.display())) }
    Ok(d)
}
/// 0 to 1 for the level meter: the peak of a block, stretched so that quiet sounds show.
pub fn level(samples: &[i16]) -> f64 { (samples.iter().map(|s| s.unsigned_abs()).max().unwrap_or(0) as f64 / 32768.0).sqrt() }

fn lines(rate: u32, secs: f64, lvl: f64, file: &str, note: &str, cols: usize, rows: usize) -> Vec<String> {
    let w = tui::width(cols);
    let (acc, dim, fg, r) = (tui::ACC, tui::DIM, tui::FG, tui::R);
    let waiting = !note.is_empty();
    let mut v = vec![tui::head("rec", "audio recorder", w), String::new()];
    v.push(format!(" {dim}state{r}  {}", if waiting { format!("{acc}\u{25d0}{r} {acc}{note}{r}") } else { format!("{acc}\u{25cf}{r} {fg}recording{r}") }));
    v.push(format!(" {dim}time{r}   {fg}{}{r}", super::play::clock(secs)));
    let n = ((lvl.clamp(0.0, 1.0) * (w - 8) as f64).round() as usize).min(w - 8);
    v.push(format!(" {dim}level{r}  {acc}{}{}{}{r}", "\u{25ae}".repeat(n), tui::BDIM, "\u{2500}".repeat(w - 8 - n)));
    v.push(String::new());
    if waiting { v.push(format!(" {fg}The question is in the top bar.{r}")); v.push(format!(" {dim}Answer there to start recording.{r}")); }
    else { v.push(format!(" {dim}{} \u{b7} {} Hz mono{r}", tui::fit(file, w.saturating_sub(16)), rate)); }
    while v.len() + 2 < rows { v.push(String::new()); }
    v.push(String::new());
    v.push(tui::hints(&if waiting { vec![("y", "allow"), ("n", "deny"), ("q", "quit")] } else { vec![("q / enter", "stop and save"), ("alt+r", "stop (den)")] }));
    v
}

pub fn run(args: &[String]) -> i32 {
    if !args.is_empty() { eprintln!("usage: rec   (records the microphone to ~/Recordings; den asks you first)"); return 2 }
    let mut c = match Client::connect() {
        Ok(c) => c,
        Err(_) => { eprintln!("rec: the microphone can only be used from a den pane, where the screen asks you first. It is not available on the serial console."); return 1 }
    };
    let dir = match recordings_dir() { Ok(d) => d, Err(e) => { eprintln!("rec: {e}"); return 1 } };
    let Some(mut scr) = tui::Screen::start("rec") else { return 1 };
    if c.send(&Up::MicStart).is_err() { drop(scr); eprintln!("rec: den went away"); return 1 }
    // 1. wait for the answer: den shows the question in its own bar and takes the y or n itself
    let t0 = Instant::now();
    let rate = loop {
        tui::draw(&lines(0, 0.0, 0.0, "", "waiting for permission", term::size().0, term::size().1));
        match c.poll(250) {
            Ok(Some(Down::Mic { state: 1, rate })) => break rate,
            Ok(Some(Down::Mic { state: 2, .. })) | Ok(Some(Down::Mic { state: 3, .. })) => { drop(scr); println!("rec: the microphone was not allowed."); return 1 }
            Ok(_) => {}
            Err(_) => { drop(scr); eprintln!("rec: den refused (only the real rec may use the microphone, in a den pane)"); return 1 }
        }
        match scr.keys(0) { Some(k) if k.iter().any(|k| matches!(k, Key::Ctrl(_) | Key::Char('q'))) => { let _ = c.send(&Up::MicStop); drop(scr); println!("rec: cancelled."); return 1 } None => return 1, _ => {} }
        if t0.elapsed() > Duration::from_secs(40) { drop(scr); eprintln!("rec: no answer"); return 1 }
    };
    // 2. record
    let (mut f, path) = match create_new(&dir, &file_name(&clock::local(clock::now(), clock::home_tz().unwrap_or(0)))) {
        Ok(x) => x, Err(e) => { let _ = c.send(&Up::MicStop); drop(scr); eprintln!("rec: cannot make the file: {e}"); return 1 }
    };
    let name = path.display().to_string();
    let (mut bytes, mut lvl, mut last, mut hdr) = (0u64, 0.0f64, Instant::now() - Duration::from_secs(1), Instant::now());
    let mut ok = f.write_all(&wav_header(0, rate)).is_ok();
    let mut asked_stop: Option<Instant> = None;
    'rec: while ok {
        match c.poll_ev(40) {
            Ok(Some(Ev::Pcm(p))) => { let m = downmix(&p); lvl = lvl * 0.6 + level(&m) * 0.4; let b: Vec<u8> = m.iter().flat_map(|s| s.to_le_bytes()).collect(); ok = f.write_all(&b).is_ok(); bytes += b.len() as u64; }
            Ok(Some(Ev::Down(Down::Mic { state: 3, .. }))) | Ok(Some(Ev::Down(Down::Mic { state: 2, .. }))) | Err(_) => break 'rec,   // Alt+R, or den went away
            Ok(_) => {}
        }
        if asked_stop.is_none() {
            let Some(keys) = scr.keys(0) else { break };
            if keys.iter().any(|k| matches!(k, Key::Char('q') | Key::Enter | Key::Ctrl(_))) {
                let _ = c.send(&Up::MicStop); asked_stop = Some(Instant::now());   // den closes the device, then says so: the samples already on the way are still saved
            }
        }
        if hdr.elapsed() >= Duration::from_secs(1) {   // the file stays a valid WAV if rec is killed (pane closed)
            hdr = Instant::now();
            ok = ok && f.seek(SeekFrom::Start(0)).and_then(|_| f.write_all(&wav_header(bytes.min(u32::MAX as u64 - 36) as u32, rate))).and_then(|_| f.seek(SeekFrom::End(0))).is_ok();
        }
        if asked_stop.is_some_and(|t| t.elapsed() > Duration::from_secs(2)) { break }   // den did not answer: save what we have
        if last.elapsed() >= Duration::from_millis(100) { last = Instant::now(); tui::draw(&lines(rate, bytes as f64 / 2.0 / rate as f64, lvl, &name, "", term::size().0, term::size().1)); }
    }
    let secs = bytes as f64 / 2.0 / rate as f64;
    let done = ok && f.seek(SeekFrom::Start(0)).and_then(|_| f.write_all(&wav_header(bytes.min(u32::MAX as u64 - 36) as u32, rate))).is_ok();
    drop(scr);
    if !done { eprintln!("rec: could not write {name}"); return 1 }
    println!("rec: saved {name} ({})", super::play::clock(secs));
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn wav_header_is_a_mono_s16_file() {
        let h = wav_header(96000, 48000);
        assert_eq!(&h[..4], b"RIFF"); assert_eq!(u32::from_le_bytes(h[4..8].try_into().unwrap()), 96036); assert_eq!(&h[8..16], b"WAVEfmt ");
        assert_eq!(u32::from_le_bytes(h[16..20].try_into().unwrap()), 16); assert_eq!(u16::from_le_bytes([h[20], h[21]]), 1); assert_eq!(u16::from_le_bytes([h[22], h[23]]), 1);
        assert_eq!(u32::from_le_bytes(h[24..28].try_into().unwrap()), 48000); assert_eq!(u32::from_le_bytes(h[28..32].try_into().unwrap()), 96000);
        assert_eq!(u16::from_le_bytes([h[32], h[33]]), 2); assert_eq!(u16::from_le_bytes([h[34], h[35]]), 16);
        assert_eq!(&h[36..40], b"data"); assert_eq!(u32::from_le_bytes(h[40..44].try_into().unwrap()), 96000);
        assert_eq!(u32::from_le_bytes(wav_header(u32::MAX, 8000)[4..8].try_into().unwrap()), u32::MAX);   // no overflow
    }
    #[test] fn stereo_is_averaged_to_mono() {
        let st: Vec<u8> = [1000i16, 3000, -2000, -2000, 32767, 32767, 5, 6].iter().flat_map(|s| s.to_le_bytes()).collect();
        assert_eq!(downmix(&st), [2000, -2000, 32767, 5]);
        assert_eq!(downmix(&st[..7]), [2000]); assert!(downmix(&[1, 2, 3]).is_empty());   // a partial frame is dropped
    }
    #[test] fn level_follows_the_peak() { assert_eq!(level(&[]), 0.0); assert_eq!(level(&[0, 0]), 0.0); assert!((level(&[i16::MIN]) - 1.0).abs() < 1e-9); assert!(level(&[1000, -2000]) > level(&[1000])); }
    #[test] fn file_names_are_dated() {
        let l = clock::local(1_795_273_925, 330); assert_eq!(file_name(&l), format!("rec-{:04}-{:02}-{:02}-{:02}{:02}{:02}", l.year, l.month, l.day, l.hour, l.min, l.sec));
        assert_eq!(file_name(&clock::local(0, 0)), "rec-1970-01-01-000000");
    }
    #[test] fn an_existing_recording_is_never_overwritten() {
        use std::os::unix::fs::PermissionsExt;
        let d = std::env::temp_dir().join(format!("thir-rec-{}", std::process::id())); std::fs::create_dir_all(&d).unwrap();
        let (mut a, pa) = create_new(&d, "rec-x").unwrap(); a.write_all(b"first").unwrap();
        let (_, pb) = create_new(&d, "rec-x").unwrap(); let (_, pc) = create_new(&d, "rec-x").unwrap();
        assert_eq!([pa.file_name().unwrap(), pb.file_name().unwrap(), pc.file_name().unwrap()], ["rec-x.wav", "rec-x-2.wav", "rec-x-3.wav"]);
        assert_eq!(std::fs::read(&pa).unwrap(), b"first"); assert_eq!(std::fs::metadata(&pa).unwrap().permissions().mode() & 0o777, 0o600);
        let l = d.join("rec-y.wav"); std::os::unix::fs::symlink(d.join("elsewhere"), &l).unwrap();   // a planted link is not followed either
        assert_eq!(create_new(&d, "rec-y").unwrap().1.file_name().unwrap(), "rec-y-2.wav"); assert!(!d.join("elsewhere").exists());
        std::fs::remove_dir_all(&d).unwrap();
    }
    #[test] fn screen_shows_waiting_then_level_and_file() {
        let w = lines(0, 0.0, 0.0, "", "waiting for permission", 80, 24).join("\n"); assert!(w.contains("rec") && w.contains("waiting for permission") && w.contains("top bar"));
        let r = lines(48000, 12.0, 0.5, "/home/a/Recordings/rec.wav", "", 80, 24); assert_eq!(r.len(), 24);
        let r = r.join("\n"); assert!(r.contains("recording") && r.contains("0:12") && r.contains("48000 Hz mono") && r.contains("stop and save") && r.contains('\u{25ae}'));
    }
}
