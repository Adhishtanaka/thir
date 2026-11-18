//! Sound: PCM playback straight on /dev/snd/pcmC0D0p with raw ioctls (no alsa-lib), and a few synthesised tones.
//! Kernel side is Intel HDA (QEMU `intel-hda` + `hda-duplex`, VirtualBox "Intel HD Audio"). Whether sound is on and how loud comes from the settings file.
use std::{fs::OpenOptions, io::{self, Write}, os::fd::AsRawFd};

pub const RATE: u32 = 48000;
const DEV: &str = "/dev/snd/pcmC0D0p";

#[repr(C)] #[derive(Clone, Copy)] struct Iv { min: u32, max: u32, flags: u32 }
#[repr(C)] struct HwParams { flags: u32, masks: [[u32; 8]; 3], mres: [[u32; 8]; 5], ivs: [Iv; 12], ires: [Iv; 9], rmask: u32, cmask: u32, info: u32, msbits: u32, rate_num: u32, rate_den: u32, fifo: libc::c_ulong, reserved: [u8; 64] }
// _IOWR('A', 0x11, struct snd_pcm_hw_params), _IO('A', 0x40 prepare), _IO('A', 0x44 drain), _IO('A', 0x43 drop)
const HW_PARAMS: libc::c_ulong = 0xC000_0000 | (std::mem::size_of::<HwParams>() as libc::c_ulong) << 16 | 0x41 << 8 | 0x11;
const PREPARE: libc::c_ulong = 0x4140; const DROP: libc::c_ulong = 0x4143; const DRAIN: libc::c_ulong = 0x4144;

fn bit(m: &mut [u32; 8], n: usize) { *m = [0; 8]; m[n / 32] = 1 << (n % 32); }
/// `ivs` slots: 2 = channels, 3 = rate.
fn params() -> HwParams {
    let mut p: HwParams = unsafe { std::mem::zeroed() };
    for m in &mut p.masks { *m = [u32::MAX; 8]; }
    for i in &mut p.ivs { *i = Iv { min: 0, max: u32::MAX, flags: 0 }; }
    bit(&mut p.masks[0], 3);   // access: RW interleaved
    bit(&mut p.masks[1], 2);   // format: S16_LE
    bit(&mut p.masks[2], 0);   // subformat: standard
    p.ivs[2] = Iv { min: 2, max: 2, flags: 0 }; p.ivs[3] = Iv { min: RATE, max: RATE, flags: 0 };
    p.rmask = u32::MAX; p
}

/// init (root) calls this once when the card is up; users never get the control node.
/// The HDA codec starts with its outputs muted or at zero. Turn every playback switch on and every playback volume up to the top (the volume setting is in the samples).
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
        if !name.contains("Playback") || !(name.ends_with("Switch") || name.ends_with("Volume")) { continue }
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
/// capture (the microphone), the mixer controls and the timer stay root 0600 always.
pub fn node_policy(name: &str, user: Option<u32>) -> (u32, u32) {
    if name.starts_with("pcm") && name.ends_with('p') { (user.unwrap_or(0), 0o600) } else { (0, 0o600) }
}
/// Applies node_policy to every node in /dev/snd. init calls it with None; login (as root, before it drops privileges) with the user, and again with None at a new login prompt.
pub fn grant(user: Option<u32>) {
    let Ok(rd) = std::fs::read_dir("/dev/snd") else { return };
    for e in rd.flatten() {
        let (uid, mode) = node_policy(&e.file_name().to_string_lossy(), user);
        let _ = std::os::unix::fs::chown(e.path(), Some(uid), Some(uid));
        let _ = std::fs::set_permissions(e.path(), std::os::unix::fs::PermissionsExt::from_mode(mode));
    }
}

/// Plays stereo S16 samples (interleaved, 48 kHz) and waits until they are out. Err = no sound device or it refused the format.
pub fn play(pcm: &[i16]) -> io::Result<()> {
    let mut f = OpenOptions::new().write(true).open(DEV)?;
    let fd = f.as_raw_fd();
    let mut p = params();
    let bad = |what: &str| io::Error::new(io::ErrorKind::Other, format!("{what}: {}", io::Error::last_os_error()));
    if unsafe { libc::ioctl(fd, HW_PARAMS as _, &mut p) } < 0 { return Err(bad("hw_params")); }
    if unsafe { libc::ioctl(fd, PREPARE as _) } < 0 { return Err(bad("prepare")); }
    let bytes: Vec<u8> = pcm.iter().flat_map(|s| s.to_le_bytes()).collect();
    let r = f.write_all(&bytes);
    if r.is_ok() { unsafe { libc::ioctl(fd, DRAIN as _); } } else { unsafe { libc::ioctl(fd, DROP as _); } }
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
        for n in ["pcmC0D0c", "controlC0", "timer", "seq", "pcmC0D1c"] { assert_eq!(node_policy(n, Some(1000)), (0, 0o600), "{n}"); }
    }
    #[test] fn struct_matches_the_kernel_abi() { assert_eq!(std::mem::size_of::<HwParams>(), if cfg!(target_pointer_width = "64") { 608 } else { 600 }); assert_eq!(HW_PARAMS & 0xffff, 0x4111); }
    #[test] fn tone_is_stereo_bounded_and_fades() {
        let t = tone(440.0, 100, 1.0);
        assert_eq!(t.len(), (RATE / 10 * 2) as usize);
        assert!(t.iter().any(|&s| s.abs() > 10000)); assert!(t.chunks(2).all(|c| c[0] == c[1]));
        assert!(t[t.len() - 20..].iter().all(|&s| s.abs() < 600));   // decayed
    }
    #[test] fn volume_scales_and_zero_is_silent() {
        let loud = chime(100).iter().map(|s| s.unsigned_abs() as u32).max().unwrap();
        let soft = chime(20).iter().map(|s| s.unsigned_abs() as u32).max().unwrap();
        assert!(loud > 20000 && soft < loud / 3); assert!(chime(0).iter().all(|&s| s == 0));
    }
    #[test] fn config_defaults() { assert_eq!(config(""), (true, 70)); assert_eq!(config("sound=off\nvolume=30\n"), (false, 30)); assert_eq!(config("volume=900"), (true, 100)); }
}
