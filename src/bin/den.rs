//! den: tiling terminal GUI drawn straight to /dev/fb0. Alt is the window-manager key.
use std::{fs::{self, File, OpenOptions}, io, os::{fd::IntoRawFd, unix::{fs::OpenOptionsExt, process::CommandExt}}, process::{exit, Command, Stdio}, thread::sleep, time::Duration};
use thir::{decide, gui::{fb::Fb, font::Font, keymap::{Action, Key, Keys}, layout::{Layout, Rect}, png, theme::*, vt::{Term, DEF_BG, DEF_FG}}, net, Action as Exit};

fn blend(fg: u32, bg: u32, a: u32) -> u32 {
    let ch = |s: u32| ((fg >> s & 255) * a + (bg >> s & 255) * (255 - a)) / 255 << s;
    ch(16) | ch(8) | ch(0)
}

/// Scales the image to cover w*h (centered crop), bilinear.
fn cover(img: &png::Image, w: usize, h: usize) -> Vec<u32> {
    let s = (w as f32 / img.w as f32).max(h as f32 / img.h as f32);
    let (ox, oy) = ((img.w as f32 * s - w as f32) / 2.0, (img.h as f32 * s - h as f32) / 2.0);
    let px = |x: usize, y: usize, c: usize| img.rgb[(y * img.w + x) * 3 + c] as f32;
    let mut out = Vec::with_capacity(w * h);
    for y in 0..h {
        let sy = ((y as f32 + oy + 0.5) / s - 0.5).clamp(0.0, (img.h - 1) as f32);
        let (y0, fy) = (sy as usize, sy.fract()); let y1 = (y0 + 1).min(img.h - 1);
        for x in 0..w {
            let sx = ((x as f32 + ox + 0.5) / s - 0.5).clamp(0.0, (img.w - 1) as f32);
            let (x0, fx) = (sx as usize, sx.fract()); let x1 = (x0 + 1).min(img.w - 1);
            let mut v = 0u32;
            for c in 0..3 {
                let top = px(x0, y0, c) * (1.0 - fx) + px(x1, y0, c) * fx;
                let bot = px(x0, y1, c) * (1.0 - fx) + px(x1, y1, c) * fx;
                v = v << 8 | (top * (1.0 - fy) + bot * fy + 0.5) as u32;
            }
            out.push(v);
        }
    }
    out
}

struct Pane { id: usize, master: i32, pid: i32, term: Term, rect: Rect, visible: bool, dirty: bool }

struct Den {
    fb: Fb, font: Font, w: usize, h: usize, back: Vec<u32>, wall: Vec<u32>, panel: Vec<u32>,
    panes: Vec<Pane>, layout: Layout, next_id: usize, area: Rect, full_redraw: bool, dirty_y: (usize, usize),
    keys: Keys, inputs: Vec<(String, i32)>, zombies: Vec<i32>,
}

fn fill(back: &mut [u32], w: usize, r: Rect, c: u32) {
    for y in r.y.max(0)..(r.y + r.h).max(0) { for x in r.x.max(0)..(r.x + r.w).min(w as i32) { if let Some(p) = back.get_mut(y as usize * w + x as usize) { *p = c; } } }
}
fn glyph(back: &mut [u32], w: usize, font: &Font, x: i32, y: i32, ch: char, c: u32) {
    let rb = font.row_bytes();
    for (gy, row) in font.glyph(ch).chunks(rb).enumerate() {
        let py = y + gy as i32; if py < 0 { continue }
        for gx in 0..font.w {
            let px = x + gx as i32;
            if px >= 0 && (px as usize) < w && row[gx / 8] & (0x80 >> (gx % 8)) != 0 { if let Some(p) = back.get_mut(py as usize * w + px as usize) { *p = c; } }
        }
    }
}

fn read_all(p: &str) -> Vec<u8> { fs::read(p).unwrap_or_else(|e| { eprintln!("den: {p}: {e}"); exit(1) }) }

fn ip_string() -> String {
    net::interfaces().ok().and_then(|v| v.into_iter().find(|i| i.name == "eth0").and_then(|i| i.ipv4)).map_or("offline".into(), |a| a.to_string())
}

impl Den {
    fn cell_size(&self, r: Rect) -> (usize, usize) {
        let inner = 2 * (BORDER + PAD);
        ((((r.w - inner).max(0) as usize) / self.font.w).max(1), (((r.h - inner).max(0) as usize) / self.font.h).max(1))
    }

    fn new_pane(&mut self) -> usize {
        let id = self.next_id; self.next_id += 1;
        let (master, pid) = spawn_shell(id, 80, 24).unwrap_or_else(|e| { eprintln!("den: cannot start pane shell: {e}"); exit(1) });
        self.panes.push(Pane { id, master, pid, term: Term::new(80, 24), rect: self.area, visible: false, dirty: true });
        id
    }

    fn relayout(&mut self) {
        let rects = self.layout.rects(self.area, GAP);
        for i in 0..self.panes.len() {
            let found = rects.iter().find(|r| r.0 == self.panes[i].id).map(|r| r.1);
            self.panes[i].visible = found.is_some();
            if let Some(r) = found {
                let (c, rw) = self.cell_size(r);
                let p = &mut self.panes[i];
                p.rect = r; p.dirty = true;
                if (p.term.cols, p.term.rows) != (c, rw) { p.term.resize(c, rw); set_winsize(p.master, c, rw); }
            }
        }
        self.full_redraw = true;
    }

    fn remove_pane(&mut self, id: usize, hup: bool) {
        let Some(i) = self.panes.iter().position(|p| p.id == id) else { return };
        let p = self.panes.remove(i);
        if hup { unsafe { libc::kill(p.pid, libc::SIGHUP); } }
        unsafe { libc::close(p.master); }
        self.zombies.push(p.pid);
        if !self.layout.close(id) { let n = self.new_pane(); self.layout = Layout::new(n); }
        self.relayout();
    }

    fn color(&self, c: u8, bold: bool, def: u32) -> u32 {
        match c { DEF_FG | DEF_BG => def, 0..=7 if bold => PALETTE[c as usize + 8], _ => PALETTE[(c & 15) as usize] }
    }

    fn draw_pane(&mut self, i: usize) {
        let focused = self.panes[i].id == self.layout.focus;
        let p = &self.panes[i]; let r = p.rect; let w = self.w;
        for y in r.y..r.y + r.h { let o = y as usize * w + r.x as usize; self.back[o..o + r.w as usize].copy_from_slice(&self.panel[o..o + r.w as usize]); }
        let bc = if focused { ACCENT } else { BORDER_DIM };
        for e in [Rect { h: BORDER, ..r }, Rect { y: r.y + r.h - BORDER, h: BORDER, ..r }, Rect { w: BORDER, ..r }, Rect { x: r.x + r.w - BORDER, w: BORDER, ..r }] { fill(&mut self.back, w, e, bc); }
        let (x0, y0) = (r.x + BORDER + PAD, r.y + BORDER + PAD);
        let (fw, fh) = (self.font.w as i32, self.font.h as i32);
        for cy in 0..p.term.rows { for cx in 0..p.term.cols {
            let c = p.term.cells[cy * p.term.cols + cx];
            let (mut fg, mut bg) = (self.color(c.fg, c.bold, FG), if c.bg == DEF_BG { None } else { Some(self.color(c.bg, false, 0)) });
            if c.rev { let f = fg; fg = bg.unwrap_or(PANEL); bg = Some(f); }
            if focused && p.term.cursor_on && (cx, cy) == (p.term.cx, p.term.cy) { fg = PANEL; bg = Some(ACCENT); }
            let (x, y) = (x0 + cx as i32 * fw, y0 + cy as i32 * fh);
            if let Some(b) = bg { fill(&mut self.back, w, Rect { x, y, w: fw, h: fh }, b); }
            if c.ch != ' ' { glyph(&mut self.back, w, &self.font, x, y, c.ch, fg); }
        } }
        self.dirty_y = (self.dirty_y.0.min(r.y as usize), self.dirty_y.1.max((r.y + r.h) as usize));
    }

    fn text(&mut self, x: i32, s: &str, c: u32) -> i32 {
        let y = (BAR_H - self.font.h as i32) / 2;
        for (k, ch) in s.chars().enumerate() { glyph(&mut self.back, self.w, &self.font, x + (k * self.font.w) as i32, y, ch, c); }
        s.chars().count() as i32 * self.font.w as i32
    }

    fn draw_bar(&mut self) {
        let w = self.w;
        self.back[..BAR_H as usize * w].copy_from_slice(&self.panel[..BAR_H as usize * w]);
        fill(&mut self.back, w, Rect { x: 0, y: BAR_H - 1, w: w as i32, h: 1 }, BORDER_DIM);
        self.text(PAD * 2, "thir", ACCENT);
        let n = self.layout.ids(self.area, GAP).iter().position(|&id| id == self.layout.focus).map_or(1, |p| p + 1);
        let mid = format!("pane {n}/{}", self.panes.len());
        let mw = mid.len() as i32 * self.font.w as i32;
        self.text((w as i32 - mw) / 2, "pane", DIM); self.text((w as i32 - mw) / 2 + 5 * self.font.w as i32, &mid[5..], FG);
        let t = unsafe { libc::time(std::ptr::null_mut()) } % 86400;
        let right = format!("{}   {:02}:{:02} UTC", ip_string(), t / 3600, t / 60 % 60);
        let rw = right.len() as i32 * self.font.w as i32;
        self.text(w as i32 - rw - PAD * 2, &right, FG);
        self.dirty_y = (0, self.dirty_y.1.max(BAR_H as usize));
    }

    fn render(&mut self, bar: bool) {
        if self.full_redraw {
            self.back.copy_from_slice(&self.wall);
            for i in 0..self.panes.len() { if self.panes[i].visible { self.draw_pane(i); } }
            self.draw_bar(); self.dirty_y = (0, self.h); self.full_redraw = false;
        } else {
            for i in 0..self.panes.len() { if self.panes[i].visible && self.panes[i].term.dirty { self.draw_pane(i); } }
            if bar { self.draw_bar(); }
        }
        for p in &mut self.panes { p.term.dirty = false; p.dirty = false; }
        if self.dirty_y.1 > self.dirty_y.0 { self.fb.present(&self.back, self.dirty_y.0, self.dirty_y.1); }
        self.dirty_y = (usize::MAX, 0);
    }

    fn act(&mut self, a: Action) {
        match a {
            Action::Split => { let n = self.new_pane(); self.layout.split(self.area, GAP, n); }
            Action::Close => { let id = self.layout.focus; self.remove_pane(id, true); return }
            Action::Fullscreen => self.layout.toggle_full(),
            Action::Focus(d) => self.layout.move_focus(self.area, GAP, d),
        }
        self.relayout();
    }

    fn scan_inputs(&mut self) {
        let Ok(rd) = fs::read_dir("/dev/input") else { return };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if !name.starts_with("event") || self.inputs.iter().any(|i| i.0 == name) { continue }
            let Ok(f) = OpenOptions::new().read(true).custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC).open(e.path()) else { continue };
            let fd = f.into_raw_fd();
            // EVIOCGBIT(ev, len): keep devices with EV_KEY and KEY_A (a keyboard, not a mouse)
            let (mut types, mut keys) = ([0u8; 4], [0u8; 64]);
            let ok = unsafe { libc::ioctl(fd, (0x8000_0000u32 | 4 << 16 | 0x45 << 8 | 0x20) as _, types.as_mut_ptr()) } >= 0 && types[0] & 2 != 0
                && unsafe { libc::ioctl(fd, (0x8000_0000u32 | 64 << 16 | 0x45 << 8 | 0x21) as _, keys.as_mut_ptr()) } >= 0 && keys[3] & 0x40 != 0;
            if ok { self.inputs.push((name, fd)); } else { unsafe { libc::close(fd); } }
        }
    }

    fn reap_status(&mut self, pid: i32) -> i32 {
        let mut st = 0;
        for _ in 0..100 { if unsafe { libc::waitpid(pid, &mut st, libc::WNOHANG) } != 0 { return st; } sleep(Duration::from_millis(3)); }
        unsafe { libc::kill(pid, libc::SIGKILL); libc::waitpid(pid, &mut st, 0); }
        st
    }
}

fn set_winsize(fd: i32, cols: usize, rows: usize) {
    let ws = libc::winsize { ws_row: rows as u16, ws_col: cols as u16, ws_xpixel: 0, ws_ypixel: 0 };
    unsafe { libc::ioctl(fd, libc::TIOCSWINSZ as _, &ws); }
}

/// The program a pane runs. One place, so a later login step only changes this.
fn pane_command(_pane: usize) -> Vec<String> { vec!["/bin/nsh".into()] }

/// New pty with `pane_command` on the slave side. Returns (master fd, shell pid).
fn spawn_shell(pane: usize, cols: usize, rows: usize) -> io::Result<(i32, i32)> {
    unsafe {
        let m = libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC);
        if m < 0 { return Err(io::Error::last_os_error()); }
        let mut name = [0 as libc::c_char; 64];
        if libc::grantpt(m) < 0 || libc::unlockpt(m) < 0 || libc::ptsname_r(m, name.as_mut_ptr(), 64) != 0 { let e = io::Error::last_os_error(); libc::close(m); return Err(e); }
        let path = std::ffi::CStr::from_ptr(name.as_ptr()).to_str().unwrap_or("").to_string();
        set_winsize(m, cols, rows);
        libc::fcntl(m, libc::F_SETFL, libc::O_NONBLOCK);
        let slave = OpenOptions::new().read(true).write(true).custom_flags(libc::O_NOCTTY | libc::O_CLOEXEC).open(&path)?;
        let dup = |f: &File| f.try_clone().map(Stdio::from);
        let argv = pane_command(pane);
        let mut cmd = Command::new(&argv[0]); cmd.args(&argv[1..]);
        cmd.env_clear().env("PATH", "/bin").env("HOME", "/").env("TERM", "linux").stdin(dup(&slave)?).stdout(dup(&slave)?).stderr(dup(&slave)?);
        cmd.pre_exec(|| { libc::setsid(); libc::ioctl(0, libc::TIOCSCTTY as _, 0); Ok(()) });
        match cmd.spawn() { Ok(c) => Ok((m, c.id() as i32)), Err(e) => { libc::close(m); Err(e) } }
    }
}

fn main() {
    let fb = Fb::open().unwrap_or_else(|e| { eprintln!("den: framebuffer: {e}"); exit(1) });
    println!("den: {}x{} {}bpp", fb.w, fb.h, fb.bpp);
    let font = Font::parse(&read_all("/share/font.psf")).unwrap_or_else(|e| { eprintln!("den: font: {e}"); exit(1) });
    let img = png::decode(&read_all("/share/wallpaper.png")).unwrap_or_else(|e| { eprintln!("den: wallpaper: {e}"); exit(1) });
    let (w, h) = (fb.w, fb.h);
    let wall = cover(&img, w, h); drop(img);
    let mut panel: Vec<u32> = wall.iter().map(|&p| blend(PANEL, p, PANEL_ALPHA)).collect();
    for (d, s) in panel[..BAR_H as usize * w].iter_mut().zip(&wall) { *d = blend(BAR, *s, BAR_ALPHA); }
    let area = Rect { x: GAP, y: BAR_H + GAP, w: w as i32 - 2 * GAP, h: h as i32 - BAR_H - 2 * GAP };
    let mut den = Den { fb, font, w, h, back: wall.clone(), wall, panel, panes: vec![], layout: Layout::new(0), next_id: 0, area, full_redraw: true, dirty_y: (usize::MAX, 0), keys: Keys::default(), inputs: vec![], zombies: vec![] };
    let first = den.new_pane(); den.layout = Layout::new(first);
    den.relayout(); den.scan_inputs(); den.render(true);

    let mut last_tick = std::time::Instant::now();
    loop {
        let mut fds: Vec<libc::pollfd> = den.inputs.iter().map(|i| i.1).chain(den.panes.iter().map(|p| p.master)).map(|fd| libc::pollfd { fd, events: libc::POLLIN, revents: 0 }).collect();
        let ids: Vec<usize> = den.panes.iter().map(|p| p.id).collect();
        let ni = den.inputs.len();
        if unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as _, 1000) } < 0 && io::Error::last_os_error().raw_os_error() != Some(libc::EINTR) { sleep(Duration::from_millis(100)); }

        let mut gone = vec![];
        for (k, f) in fds[..ni].iter().enumerate() {
            if f.revents == 0 { continue }
            let mut buf = [0u8; 24 * 32];
            loop {
                let n = unsafe { libc::read(f.fd, buf.as_mut_ptr() as *mut _, buf.len()) };
                if n <= 0 { if n < 0 && io::Error::last_os_error().raw_os_error() != Some(libc::EAGAIN) { gone.push(k); } break }
                for ev in buf[..n as usize].chunks_exact(24) {
                    if u16::from_le_bytes([ev[16], ev[17]]) != 1 { continue }   // EV_KEY only; never log or print the keys
                    let (code, val) = (u16::from_le_bytes([ev[18], ev[19]]), i32::from_le_bytes([ev[20], ev[21], ev[22], ev[23]]));
                    match den.keys.event(code, val) {
                        Some(Key::Act(a)) => den.act(a),
                        Some(Key::Bytes(b)) => if let Some(p) = den.panes.iter().find(|p| p.id == den.layout.focus) { unsafe { libc::write(p.master, b.as_ptr() as *const _, b.len()); } },
                        None => {}
                    }
                }
            }
        }
        for k in gone.into_iter().rev() { let (_, fd) = den.inputs.remove(k); unsafe { libc::close(fd); } }

        for (f, id) in fds[ni..].iter().zip(&ids) {
            if f.revents == 0 { continue }
            let Some(i) = den.panes.iter().position(|p| p.id == *id) else { continue };
            let mut buf = [0u8; 4096];
            let n = unsafe { libc::read(den.panes[i].master, buf.as_mut_ptr() as *mut _, buf.len()) };
            if n > 0 { den.panes[i].term.feed(&buf[..n as usize]); continue }
            if n < 0 && io::Error::last_os_error().raw_os_error() == Some(libc::EAGAIN) { continue }
            let pid = den.panes[i].pid;   // EOF/EIO: the shell is gone
            let st = den.reap_status(pid);
            den.panes[i].pid = 0;
            match decide(st) {
                Exit::Poweroff => exit(thir::EXIT_POWEROFF),
                Exit::Reboot => exit(thir::EXIT_REBOOT),
                Exit::Restart => den.remove_pane(*id, false),
            }
        }

        let tick = last_tick.elapsed() >= Duration::from_secs(1);
        if tick {
            last_tick = std::time::Instant::now();
            den.scan_inputs();
            den.zombies.retain(|&p| p > 0 && unsafe { libc::waitpid(p, std::ptr::null_mut(), libc::WNOHANG) } == 0);
        }
        den.render(tick);
    }
}
