//! Linux framebuffer: /dev/fb0 mapped into memory. Back buffers hold 0xRRGGBB pixels.
use std::{fs::OpenOptions, io, os::fd::AsRawFd, os::unix::fs::OpenOptionsExt};

const FBIOGET_VSCREENINFO: libc::c_ulong = 0x4600;
const FBIOPUT_VSCREENINFO: libc::c_ulong = 0x4601;
const FBIOGET_FSCREENINFO: libc::c_ulong = 0x4602;

pub struct Fb { pub w: usize, pub h: usize, pub bpp: u32, stride: usize, shifts: [u32; 3], map: *mut u8, len: usize }

fn err(m: &str) -> io::Error { io::Error::new(io::ErrorKind::Other, m.to_string()) }

impl Fb {
    pub fn open() -> io::Result<Fb> {
        let f = OpenOptions::new().read(true).write(true).custom_flags(libc::O_CLOEXEC).open("/dev/fb0")?;
        // Plain u32 arrays mirror struct fb_var_screeninfo (40 words) and fb_fix_screeninfo (80 bytes).
        let (mut v, mut x) = ([0u32; 40], [0u32; 20]);
        if unsafe { libc::ioctl(f.as_raw_fd(), FBIOGET_VSCREENINFO as _, v.as_mut_ptr()) } < 0 { return Err(io::Error::last_os_error()); }
        if unsafe { libc::ioctl(f.as_raw_fd(), FBIOGET_FSCREENINFO as _, x.as_mut_ptr()) } < 0 { return Err(io::Error::last_os_error()); }
        // Without fbcon nothing sets the mode, so the virtio-gpu scanout stays off until userspace does.
        v[21] |= 0x80;   // FB_ACTIVATE_FORCE: apply even though the mode is unchanged
        unsafe { libc::ioctl(f.as_raw_fd(), FBIOPUT_VSCREENINFO as _, v.as_mut_ptr()); }
        let (w, h, bpp, shifts) = (v[0] as usize, v[1] as usize, v[6], [v[8], v[11], v[14]]);
        let (len, stride) = (x[6] as usize, x[12] as usize);
        if bpp != 32 || shifts.iter().any(|s| s % 8 != 0 || *s > 24) { return Err(err(&format!("unsupported framebuffer: {bpp} bpp, rgb offsets {shifts:?} (need 32 bpp XRGB8888-style)"))); }
        if w == 0 || h == 0 || stride < w * 4 || len < stride * h { return Err(err("bad framebuffer geometry")); }
        let map = unsafe { libc::mmap(std::ptr::null_mut(), len, libc::PROT_READ | libc::PROT_WRITE, libc::MAP_SHARED, f.as_raw_fd(), 0) };
        if map == libc::MAP_FAILED { return Err(io::Error::last_os_error()); }
        Ok(Fb { w, h, bpp, stride, shifts, map: map as *mut u8, len })
    }

    /// Copies rows y0..y1 of `back` (w*h pixels) to the screen.
    pub fn present(&mut self, back: &[u32], y0: usize, y1: usize) {
        let [r, g, b] = self.shifts;
        let y1 = y1.min(self.h);
        for y in y0..y1 {
            // SAFETY: open() checked stride*h <= len; y < h.
            let dst = unsafe { std::slice::from_raw_parts_mut(self.map.add(y * self.stride) as *mut u32, self.w) };
            let src = &back[y * self.w..(y + 1) * self.w];
            if (r, g, b) == (16, 8, 0) { dst.copy_from_slice(src) } else {
                for (d, s) in dst.iter_mut().zip(src) { *d = (s >> 16 & 255) << r | (s >> 8 & 255) << g | (s & 255) << b; }
            }
        }
    }
}

impl Drop for Fb { fn drop(&mut self) { unsafe { libc::munmap(self.map as *mut _, self.len); } } }
