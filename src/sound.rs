//! Sound: PCM playback straight on /dev/snd/pcmC0D0p with raw ioctls (no alsa-lib), and a few synthesised tones.
//! Kernel side is Intel HDA (QEMU `intel-hda` + `hda-duplex`, VirtualBox "Intel HD Audio"). Capture (the microphone) is opened only by den, as root, after the user said yes on den's own bar. Whether sound is on and how loud comes from the settings file.
use std::{fs::{File, OpenOptions}, io::{self, Write}, os::{fd::AsRawFd, unix::fs::{FileTypeExt, OpenOptionsExt}}, time::{Duration, Instant}};

pub const RATE: u32 = 48000;
const DEV: &str = "/dev/snd/pcmC0D0p";
const CAP: &str = "/dev/snd/pcmC0D0c";

#[repr(C)] #[derive(Clone, Copy)] struct Iv { min: u32, max: u32, flags: u32 }
#[repr(C)] struct HwParams { flags: u32, masks: [[u32; 8]; 3], mres: [[u32; 8]; 5], ivs: [Iv; 12], ires: [Iv; 9], rmask: u32, cmask: u32, info: u32, msbits: u32, rate_num: u32, rate_den: u32, fifo: libc::c_ulong, reserved: [u8; 64] }
// _IOWR('A', 0x11, struct snd_pcm_hw_params), _IO('A', 0x40 prepare), _IO('A', 0x44 drain), _IO('A', 0x43 drop)
const HW_PARAMS: libc::c_ulong = 0xC000_0000 | (std::mem::size_of::<HwParams>() as libc::c_ulong) << 16 | 0x41 << 8 | 0x11;
const PREPARE: libc::c_ulong = 0x4140; const START: libc::c_ulong = 0x4142; const DROP: libc::c_ulong = 0x4143; const DRAIN: libc::c_ulong = 0x4144;
// _IOR('A', 0x21, snd_pcm_sframes_t): frames still queued before the speaker (a C long)
const DELAY: libc::c_ulong = 0x8000_0000 | (std::mem::size_of::<libc::c_long>() as libc::c_ulong) << 16 | 0x4121;

fn bit(m: &mut [u32; 8], n: usize) { *m = [0; 8]; m[n / 32] = 1 << (n % 32); }
/// `ivs` slots: 2 = channels, 3 = rate, 4 = period time (us), 8 = buffer time (us); 0 = any. The kernel picks the smallest period it may when none is asked for: a few frames, an interrupt every millisecond or less, which a virtual HDA (VirtualBox) cannot keep up with.
fn params(rate: u32, buf_us: u32, per_us: u32) -> HwParams {
    let mut p: HwParams = unsafe { std::mem::zeroed() };
    for m in &mut p.masks { *m = [u32::MAX; 8]; }
    for i in &mut p.ivs { *i = Iv { min: 0, max: u32::MAX, flags: 0 }; }
    bit(&mut p.masks[0], 3);   // access: RW interleaved
    bit(&mut p.masks[1], 2);   // format: S16_LE
    bit(&mut p.masks[2], 0);   // subformat: standard
    p.ivs[2] = Iv { min: 2, max: 2, flags: 0 }; p.ivs[3] = Iv { min: rate, max: rate, flags: 0 };
    if buf_us > 0 { p.ivs[8].min = buf_us; }
    if per_us > 0 { p.ivs[4].min = per_us; }
    p.rmask = u32::MAX; p
}

/// init (root) calls this once when the card is up; users never get the control node.
/// The HDA codec starts with its outputs and inputs muted or at zero. Turn every playback and capture switch on and every volume up to the top (the volume setting is in the samples; the capture node itself stays root-only).
/// Raw `snd_ctl_elem_*` ioctls on /dev/snd/controlC0; offsets are those of the 64-bit kernel structs (id 64 bytes, info 272, value 1224). Failures are ignored: no mixer, no change.
pub fn unmute() {
    let Ok(f) = OpenOptions::new().read(true).write(true).open("/dev/snd/controlC0") else { return };
    let fd = f.as_raw_fd();
    let (list, info, write) = (0xC050_5510 as libc::c_ulong, 0xC110_5511 as libc::c_ulong, 0xC4C8_5513 as libc::c_ulong);   // _IOWR('U', 0x10 | 0x11 | 0x13, ...)
    let mut l = [0u8; 80];
    if unsafe { libc::ioctl(fd, list as _, l.as_mut_ptr()) } < 0 { return }
    let n = u32::from_ne_bytes([l[12], l[13], l[14], l[15]]).min(512) as usize;
    let mut ids = vec![0u8; 64 * n.max(1)];
    l[4..8].copy_from_slice(&(n as u32).to_ne_bytes()); l[16..24].copy_from_slice(&(ids.as_mut_ptr() as usize as u64).to_ne_bytes());
    if unsafe { libc::ioctl(fd, list as _, l.as_mut_ptr()) } < 0 { return }
    for id in ids.chunks(64).take(n) {
        let name = String::from_utf8_lossy(&id[16..60]).trim_end_matches('\0').to_string();
        if !(name.contains("Playback") || name.contains("Capture")) || !(name.ends_with("Switch") || name.ends_with("Volume")) { continue }
        let (mut i, mut v) = ([0u8; 272], [0u8; 1224]);
        i[..64].copy_from_slice(id);
        if unsafe { libc::ioctl(fd, info as _, i.as_mut_ptr()) } < 0 { continue }
        let (ty, count, max) = (u32::from_ne_bytes([i[64], i[65], i[66], i[67]]), u32::from_ne_bytes([i[72], i[73], i[74], i[75]]) as usize, i64::from_ne_bytes(i[88..96].try_into().unwrap_or([0; 8])));
        v[..64].copy_from_slice(id);
        for k in 0..count.min(128) { v[72 + 8 * k..80 + 8 * k].copy_from_slice(&(if ty == 1 { 1 } else { max }).to_ne_bytes()); }   // 1 = boolean, 2 = integer
        unsafe { libc::ioctl(fd, write as _, v.as_mut_ptr()); }
    }
}

/// Who owns a /dev/snd node and its mode while `user` (a uid, or None = nobody logged in) is logged in. Only the playback stream goes to the user, mode 0600;
/// the mixer control node goes with it (alsa-lib, which the browser's GStreamer uses, opens it to play: it moves levels, it carries no audio); capture (the microphone) and the timer stay root 0600 always.
pub fn node_policy(name: &str, user: Option<u32>) -> (u32, u32) {
    if (name.starts_with("pcm") && name.ends_with('p')) || name.starts_with("control") { (user.unwrap_or(0), 0o600) } else { (0, 0o600) }
}
/// Where login records who holds the playback stream (a uid), so root can re-apply it when the card appears late. No file = nobody.
const STATE: &str = "/run/snd-user";
/// What a node needs: (change owner, change mode) for a node with owner `uid`:`gid` and mode `mode` while `user` is logged in.
pub fn drift(name: &str, user: Option<u32>, uid: u32, gid: u32, mode: u32) -> (bool, bool) {
    let (want, m) = node_policy(name, user);
    (uid != want || gid != want, mode & 0o7777 != m)
}
/// Applies node_policy to every device node of `dir`, changing only what differs.
pub fn apply(dir: &str, user: Option<u32>) {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let Ok(m) = std::fs::symlink_metadata(e.path()) else { continue };
        if !m.file_type().is_char_device() { continue }
        let (own, mode) = drift(&name, user, m.uid(), m.gid(), m.mode());
        let (uid, want) = node_policy(&name, user);
        if own { let _ = std::os::unix::fs::chown(e.path(), Some(uid), Some(uid)); }
        if mode { let _ = std::fs::set_permissions(e.path(), std::fs::Permissions::from_mode(want)); }
    }
}
/// login (root, before it drops privileges) with the user, and again with None at a new login prompt: records the holder and applies it.
pub fn grant(user: Option<u32>) {
    match user { Some(u) => { let _ = std::fs::write(STATE, format!("{u}\n")); } None => { let _ = std::fs::remove_file(STATE); } }
    apply("/dev/snd", user);
}
/// init (root) calls this every second: the recorded holder gets the playback node, also when the sound card showed up after the login.
pub fn sync() { apply("/dev/snd", holder(&std::fs::read_to_string(STATE).unwrap_or_default())); }
/// The uid in the state file; anything else (empty, garbage, 0) is nobody.
pub fn holder(state: &str) -> Option<u32> { state.trim().parse::<u32>().ok().filter(|&u| u != 0) }

/// Sample rates to try, best first. The VirtualBox HDA codec stops at 44.1 kHz and refuses 48 kHz (hw_params: EINVAL); QEMU takes 48 kHz.
const RATES: [u32; 4] = [RATE, 44100, 32000, 16000];

/// Stereo S16 samples made at RATE, played at `to` (linear interpolation).
fn resample(pcm: &[i16], to: u32) -> Vec<i16> {
    if to == RATE { return pcm.to_vec(); }
    let frames = pcm.len() / 2;
    (0..frames * to as usize / RATE as usize).flat_map(|i| {
        let pos = i as f64 * RATE as f64 / to as f64;
        let (a, t) = (pos as usize, pos.fract());
        let b = (a + 1).min(frames.saturating_sub(1));
        (0..2).map(move |c| (pcm[a * 2 + c] as f64 * (1.0 - t) + pcm[b * 2 + c] as f64 * t) as i16)
    }).collect()
}

/// Opens the playback stream: stereo S16_LE at the first of RATES the card takes, ready to write. The rate is returned.
/// Err keeps the OS error: EBUSY = another program is playing (one substream only).
pub fn open() -> io::Result<(File, u32)> { open_buffered(0) }
/// Like `open`, with a buffer of at least `buf_ms` where the card allows it (a player that streams from another process wants room against underruns; the chimes keep the card's default).
pub fn open_buffered(buf_ms: u32) -> io::Result<(File, u32)> { open_dev(OpenOptions::new().write(true), DEV, buf_ms, 0) }
/// Opens the capture stream (root only: den): stereo S16_LE at the first of RATES the card takes, started, non-blocking. The rate is returned.
pub fn open_capture() -> io::Result<(File, u32)> {
    let (f, rate) = open_dev(OpenOptions::new().read(true).custom_flags(libc::O_NONBLOCK), CAP, CAP_BUF_MS, CAP_PERIOD_MS)?;
    unsafe { libc::ioctl(f.as_raw_fd(), START as _); }
    Ok((f, rate))
}
/// Capture buffer and period: a wake-up every 20 ms is 50 a second, however the card is made.
const CAP_BUF_MS: u32 = 200; const CAP_PERIOD_MS: u32 = 20;
/// After an overrun (EPIPE from read): start the capture again.
pub fn capture_restart(f: &File) { restart(f); unsafe { libc::ioctl(f.as_raw_fd(), START as _); } }
fn open_dev(o: &mut OpenOptions, path: &str, buf_ms: u32, per_ms: u32) -> io::Result<(File, u32)> {
    let f = o.open(path)?;
    let fd = f.as_raw_fd();
    let bad = |what: &str| io::Error::new(io::ErrorKind::Other, format!("{what}: {}", io::Error::last_os_error()));
    let rate = RATES.iter().flat_map(|&r| [(r, buf_ms * 1000, per_ms * 1000), (r, 0, per_ms * 1000), (r, 0, 0)]).find(|&(r, b, q)| { let mut p = params(r, b, q); unsafe { libc::ioctl(fd, HW_PARAMS as _, &mut p) == 0 } }).map(|(r, _, _)| r);
    let Some(rate) = rate else { return Err(bad("hw_params")) };
    if unsafe { libc::ioctl(fd, PREPARE as _) } < 0 { return Err(bad("prepare")); }
    Ok((f, rate))
}
/// Throws away what is queued and gets the stream ready to start again (a seek, a pause, an underrun).
pub fn restart(f: &File) { unsafe { libc::ioctl(f.as_raw_fd(), DROP as _); libc::ioctl(f.as_raw_fd(), PREPARE as _); } }
/// Waits until everything written is out of the speaker.
pub fn drain(f: &File) { unsafe { libc::ioctl(f.as_raw_fd(), DRAIN as _); } }
/// Frames written but not yet played; 0 when the stream is not running.
pub fn delay(f: &File) -> i64 { let mut d: libc::c_long = 0; if unsafe { libc::ioctl(f.as_raw_fd(), DELAY as _, &mut d) } == 0 { d as i64 } else { 0 } }

/// Runs `f` again every `step` while it fails with EBUSY, for up to `wait`.
pub fn retry_busy<T>(mut f: impl FnMut() -> io::Result<T>, wait: Duration, step: Duration) -> io::Result<T> {
    let t = Instant::now();
    loop {
        match f() { Err(e) if e.raw_os_error() == Some(libc::EBUSY) && t.elapsed() < wait => std::thread::sleep(step), r => return r }
    }
}

/// Plays stereo S16 samples (interleaved, 48 kHz) and waits until they are out. Err = no sound device or it refused every rate.
pub fn play(pcm: &[i16]) -> io::Result<()> {
    let (mut f, rate) = open()?;
    let bytes: Vec<u8> = resample(pcm, rate).iter().flat_map(|s| s.to_le_bytes()).collect();
    let r = f.write_all(&bytes);
    if r.is_ok() { drain(&f); } else { unsafe { libc::ioctl(f.as_raw_fd(), DROP as _); } }
    r
}

/// A note: sine at `hz` for `ms`, a quick attack and an exponential decay, both channels. `gain` 0.0 to 1.0.
pub fn tone(hz: f32, ms: u32, gain: f32) -> Vec<i16> {
    let n = (RATE * ms / 1000) as usize;
    (0..n).flat_map(|i| {
        let t = i as f32 / RATE as f32;
        let env = (1.0 - (-t * 400.0).exp()) * (-t * 5000.0 / ms as f32).exp();
        let v = ((t * hz * std::f32::consts::TAU).sin() * env * gain * 30000.0) as i16;
        [v, v]
    }).collect()
}
/// The alarm chime: E5 G5 C6 and a longer C6, with a pause. `volume` 0 to 100.
pub fn chime(volume: u32) -> Vec<i16> {
    let g = volume.min(100) as f32 / 100.0;
    let mut v = vec![];
    for (hz, ms) in [(659.3, 180), (784.0, 180), (1046.5, 420)] { v.extend(tone(hz, ms, g)); }
    v.extend(std::iter::repeat(0).take((RATE as usize / 4) * 2));
    v
}

/// What the settings say: (sound on, volume 0 to 100).
pub fn config(settings: &str) -> (bool, u32) { (crate::apps::prefs::get(settings, "sound") != "off", crate::apps::prefs::get(settings, "volume").parse::<u32>().unwrap_or(70).min(100)) }

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn only_playback_goes_to_the_user() {
        assert_eq!(node_policy("pcmC0D0p", Some(1000)), (1000, 0o600)); assert_eq!(node_policy("pcmC0D0p", None), (0, 0o600));
        assert_eq!(node_policy("controlC0", Some(1000)), (1000, 0o600)); assert_eq!(node_policy("controlC0", None), (0, 0o600));
        for n in ["pcmC0D0c", "timer", "seq", "pcmC0D1c"] { assert_eq!(node_policy(n, Some(1000)), (0, 0o600), "{n}"); }
    }
    #[test] fn holder_is_a_plain_uid() { assert_eq!(holder("1000\n"), Some(1000)); for b in ["", "0", "-1", "x", "1000 5"] { assert_eq!(holder(b), None, "{b:?}"); } }
    #[test] fn a_late_card_gets_fixed_and_capture_never_moves() {
        assert_eq!(drift("pcmC0D0p", Some(1000), 0, 0, 0o20620), (true, true));      // appeared after the login: root-owned, mode from devtmpfs
        assert_eq!(drift("pcmC0D0p", Some(1000), 1000, 1000, 0o20600), (false, false));
        assert_eq!(drift("pcmC0D0p", None, 1000, 1000, 0o20600), (true, false));    // logout: back to root
        for n in ["pcmC0D0c"] { assert_eq!(drift(n, Some(1000), 0, 0, 0o20600), (false, false), "{n}"); assert_eq!(drift(n, Some(1000), 1000, 1000, 0o20666), (true, true), "{n}"); }   // anything else is pulled back to root 0600
    }
    #[test] fn busy_is_retried_for_a_while_and_other_errors_are_not() {
        let mut n = 0;
        let r = retry_busy(|| { n += 1; if n < 4 { Err(io::Error::from_raw_os_error(libc::EBUSY)) } else { Ok(n) } }, Duration::from_secs(5), Duration::from_millis(1));
        assert_eq!(r.unwrap(), 4);
        let mut n = 0;
        assert!(retry_busy(|| -> io::Result<()> { n += 1; Err(io::Error::from_raw_os_error(libc::ENOENT)) }, Duration::from_secs(5), Duration::from_millis(1)).is_err()); assert_eq!(n, 1);
        let mut n = 0;
        assert!(retry_busy(|| -> io::Result<()> { n += 1; Err(io::Error::from_raw_os_error(libc::EBUSY)) }, Duration::from_millis(20), Duration::from_millis(5)).is_err()); assert!(n >= 2);   // gives up after `wait`
    }
    #[test] fn buffer_time_is_asked_for_only_when_wanted() {
        assert_eq!(params(44100, 0, 0).ivs[8].min, 0); assert_eq!(params(44100, 200_000, 0).ivs[8].min, 200_000); assert_eq!(params(44100, 200_000, 0).ivs[3].min, 44100);
        assert_eq!(params(44100, 0, 0).ivs[4].min, 0); assert_eq!(params(44100, 200_000, 20_000).ivs[4].min, 20_000);   // capture asks for a 20 ms period: never the smallest one
        assert!(CAP_PERIOD_MS >= 10 && CAP_BUF_MS >= 4 * CAP_PERIOD_MS);
        if cfg!(target_pointer_width = "64") { assert_eq!(DELAY, 0x8008_4121); }
    }
    #[test] fn struct_matches_the_kernel_abi() { assert_eq!(std::mem::size_of::<HwParams>(), if cfg!(target_pointer_width = "64") { 608 } else { 600 }); assert_eq!(HW_PARAMS & 0xffff, 0x4111); }
    #[test] fn tone_is_stereo_bounded_and_fades() {
        let t = tone(440.0, 100, 1.0);
        assert_eq!(t.len(), (RATE / 10 * 2) as usize);
        assert!(t.iter().any(|&s| s.abs() > 10000)); assert!(t.chunks(2).all(|c| c[0] == c[1]));
        assert!(t[t.len() - 20..].iter().all(|&s| s.abs() < 600));   // decayed
    }
    #[test] fn resample_keeps_the_length_ratio_and_the_shape() {
        let t = tone(440.0, 100, 1.0);
        assert_eq!(resample(&t, RATE), t);
        let r = resample(&t, 44100); assert_eq!(r.len(), t.len() / 2 * 44100 / 48000 * 2);
        assert!(r.iter().any(|&s| s.abs() > 10000)); assert!(r.chunks(2).all(|c| c[0] == c[1]));
        assert!(resample(&[], 44100).is_empty());
    }
    #[test] fn volume_scales_and_zero_is_silent() {
        let loud = chime(100).iter().map(|s| s.unsigned_abs() as u32).max().unwrap();
        let soft = chime(20).iter().map(|s| s.unsigned_abs() as u32).max().unwrap();
        assert!(loud > 20000 && soft < loud / 3); assert!(chime(0).iter().all(|&s| s == 0));
    }
    #[test] fn config_defaults() { assert_eq!(config(""), (true, 70)); assert_eq!(config("sound=off\nvolume=30\n"), (false, 30)); assert_eq!(config("volume=900"), (true, 100)); }
}
