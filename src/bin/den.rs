//! den: tiling terminal GUI drawn straight to /dev/fb0. Alt is the window-manager key.
use std::{fs::{self, File, OpenOptions}, io, os::{fd::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd}, unix::{fs::{MetadataExt, OpenOptionsExt}, process::CommandExt}}, process::{exit, Command, Stdio}, thread::sleep, time::Duration};
use thir::{apps::prefs, sound as snd_cfg, clock, decide, sound, files::{printable, read_small}, notify, store, user, gui::{draw::{fill, glyph, text}, fb::Fb, font::Font, keymap::{Action, Key, Keys}, layout::{Layout, Rect}, pointer::{Pointer, Ptr}, mic::{self, Broker, Do}, surface::{self, recv_msg, Down, Got, Surface, Up}, theme::*, wall, vt::{Term, DEF_BG, DEF_FG}}, net, Action as Exit};

const LOGIN_COLS: i32 = 44;
const LOGIN_ROWS: i32 = 6;

struct Pane { id: usize, master: i32, pid: i32, term: Term, rect: Rect, visible: bool, dirty: bool, tty: u64, surface: Option<Surface> }   // tty: rdev of the pane's pty slave (who may connect a surface)
struct PtrDev { name: String, fd: i32, abs: Option<[(i32, i32); 2]> }   // a mouse (abs None) or a tablet with its ABS_X / ABS_Y ranges

/// The mouse arrow: X = CUR_FILL (black), o = CUR_EDGE (white) outline, one cell per `sc` pixels; the white edge shows on the dark panes and the black fill on the blue wallpaper.
const CUR_FILL: u32 = 0x000000; const CUR_EDGE: u32 = 0xffffff;
const ARROW: [&str; 19] = ["o...........", "oo..........", "oXo.........", "oXXo........", "oXXXo.......", "oXXXXo......", "oXXXXXo.....", "oXXXXXXo....", "oXXXXXXXo...", "oXXXXXXXXo..", "oXXXXXXXXXo.",
    "oXXXXXXoooo.", "oXXXoXXo....", "oXXo.oXXo...", "oXo..oXXo...", "oo....oXXo..", "o.....oXXo..", "......oXXo..", ".......oo..."];

struct Den {
    fb: Fb, font: Font, m: Metrics, w: usize, h: usize, back: Vec<u32>, wall_login: Vec<u32>, wall_big: Vec<u32>, tint: Vec<u32>, look: (u32, u32, u32),   // tint: the big-mark wall with the pane colour over it at PANEL_ALPHA, what a plain shell pane shows. look: wall, pane fill, selected row of the preset on screen
    vol: Option<u32>, ai: Ai,   // the bar's right side: the volume setting (None = sound off) and the AI state

    panes: Vec<Pane>, layout: Layout, next_id: usize, area: Rect, full_redraw: bool, dirty_y: (usize, usize),
    keys: Keys, inputs: Vec<(String, i32)>, zombies: Vec<i32>,
    ptrs: Vec<PtrDev>, ptr: Pointer, cursor_on: bool, cur_screen: Option<Rect>,   // pointer devices, the cursor position, shown after the first move, where it is on the screen now
    listener: i32,   // /run/den/sock
    uid: Option<u32>,   // None = nobody is logged in: one centered login pane
    home: Option<String>,   // the logged-in user's home: ~/.tz for the bar clock, ~/.notify for notices, ~/.config/thir/settings for the wallpaper
    mic: Broker, mic_dev: Option<File>, t0: std::time::Instant, bar_dirty: bool,   // the microphone: who may record, the open capture device (only den ever has it), the clock of the broker
    wall_idx: usize, sc: i32,   // the wallpaper preset on screen (index into WALLS) and the pixel scale
}

#[derive(Clone, Copy, PartialEq)]
enum Ai { On, Working, Off }

fn read_all(p: &str) -> Vec<u8> { fs::read(p).unwrap_or_else(|e| { eprintln!("den: {p}: {e}"); exit(1) }) }

fn ip_string() -> String {
    net::interfaces().ok().and_then(|v| v.into_iter().find(|i| i.name == "eth0").and_then(|i| i.ipv4)).map_or("offline".into(), |a| a.to_string())
}

impl Den {
    fn cell_size(&self, r: Rect) -> (usize, usize) {
        let (iw, ih) = (2 * (self.m.border + self.m.padx), 2 * (self.m.border + self.m.pady) + self.title_inset());
        ((((r.w - iw).max(0) as usize) / self.font.w).max(1), (((r.h - ih).max(0) as usize) / self.font.h).max(1))
    }

    fn new_pane(&mut self) -> usize {
        let id = self.next_id; self.next_id += 1;
        let (master, pid, tty) = spawn_shell(self.uid, 80, 24).unwrap_or_else(|e| { eprintln!("den: cannot start pane shell: {e}"); exit(1) });
        self.panes.push(Pane { id, master, pid, term: Term::new(80, 24), rect: self.area, visible: false, dirty: true, tty, surface: None });
        id
    }

    /// The login screen: one pane of LOGIN_COLS x LOGIN_ROWS cells in the middle of the screen.
    fn login_rect(&self) -> Rect {
        let (rw, rh) = (LOGIN_COLS * self.font.w as i32 + 2 * (self.m.border + self.m.padx), LOGIN_ROWS * self.font.h as i32 + 2 * (self.m.border + self.m.pady) + self.title_inset());
        Rect { x: (self.w as i32 - rw) / 2, y: self.h as i32 / 2 + 3 * self.sc * 5, w: rw, h: rh }
    }

    /// First time the login pane's process runs as a non-root uid: show the tiled layout.
    fn check_login(&mut self) {
        let Some(p) = self.panes.first() else { return };
        let uid = fs::read_to_string(format!("/proc/{}/status", p.pid)).ok()
            .and_then(|s| s.lines().find_map(|l| l.strip_prefix("Uid:")?.split_whitespace().next()?.parse::<u32>().ok()));
        if let Some(u) = uid.filter(|&u| u != 0) {
            self.home = user::load().ok().and_then(|v| v.into_iter().find(|x| x.uid == u)).map(|x| x.home());
            self.uid = Some(u); self.relayout();
        }
    }

    fn relayout(&mut self) {
        let rects = if self.uid.is_none() { self.panes.first().map(|p| (p.id, self.login_rect())).into_iter().collect() } else { self.layout.rects(self.area, self.m.gap) };
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
        self.push_config();
    }

    /// The pane's inside, where an app's pixels go: the pane minus its border.
    fn inner(&self, r: Rect) -> Rect { let t = self.title_inset(); Rect { x: r.x + self.m.border, y: r.y + self.m.border + t, w: (r.w - 2 * self.m.border).max(1), h: (r.h - 2 * self.m.border - t).max(1) } }
    /// A plain shell shows the wallpaper through it; a full-screen app (alternate screen, a title it set, or a pixel surface) is opaque. The login box stays opaque.
    fn translucent(&self, i: usize) -> bool { let p = &self.panes[i]; self.uid.is_some() && p.surface.is_none() && !p.term.alt && p.term.title.is_empty() }
    /// The pane title straddles the top border: its lower half needs this much room inside.
    fn title_inset(&self) -> i32 { self.font.h as i32 / 2 }

    /// Tells every connected app its size and focus when they changed. A new size makes its old buffer useless: the text grid shows until the app sends a new one.
    fn push_config(&mut self) {
        let (focus, scale) = (self.layout.focus, self.sc as u32);
        for i in 0..self.panes.len() {
            let inner = self.inner(self.panes[i].rect);
            let p = &mut self.panes[i];
            let Some(s) = p.surface.as_mut().filter(|s| s.ready()) else { continue };
            let cfg = (inner.w as u32, inner.h as u32, p.id == focus && p.visible);
            if s.sent == Some(cfg) { continue }
            if s.sent.map_or(true, |o| (o.0, o.1) != (cfg.0, cfg.1)) { s.buf = None; p.term.dirty = true; }
            s.sent = Some(cfg);
            s.send(Down::Configure { w: cfg.0, h: cfg.1, scale, focused: cfg.2 });
        }
    }

    fn remove_pane(&mut self, id: usize, hup: bool) {
        let Some(i) = self.panes.iter().position(|p| p.id == id) else { return };
        let a = self.mic.gone(id); self.mic_run(a);   // the pane's recording ends with it
        let p = self.panes.remove(i);
        if hup { unsafe { libc::kill(p.pid, libc::SIGHUP); } }
        unsafe { libc::close(p.master); }
        self.zombies.push(p.pid);
        if !self.layout.close(id) { let a = self.mic.stop(None); self.mic_run(a); self.uid = None; self.home = None; let n = self.new_pane(); self.layout = Layout::new(n); }   // last pane closed: logout
        self.relayout();
    }

    fn color(&self, c: u8, bold: bool, def: u32) -> u32 {
        match c { DEF_FG | DEF_BG => def, 0..=7 if bold => PALETTE[c as usize + 8], 0..=15 => PALETTE[c as usize], SLOT_ACCENT => ACCENT, SLOT_SEL => self.look.2, SLOT_WALL => self.look.0, SLOT_BORDER => BORDER_DIM, SLOT_PANEL => self.look.1, SLOT_SOFT => SOFT, _ => def }
    }

    fn draw_pane(&mut self, i: usize) {
        let focused = self.panes[i].id == self.layout.focus;
        let m = self.m; let p = &self.panes[i]; let r = p.rect; let w = self.w;
        let (fw, fh) = (self.font.w as i32, self.font.h as i32);
        {   // wall again under the strip the title patch of an earlier draw may have left above the pane
            let wall = (if self.uid.is_none() { &self.wall_login } else { &self.wall_big });
            for y in (r.y - fh / 2).max(0)..r.y { let o = y as usize * w + r.x.max(0) as usize; let e = (o + r.w as usize).min(self.back.len()); self.back[o..e].copy_from_slice(&wall[o..e]); }
        }
        if self.translucent(i) {
            for y in r.y.max(0)..(r.y + r.h).min(self.h as i32) { let (o, e) = (y as usize * w + r.x.max(0) as usize, y as usize * w + (r.x + r.w).clamp(0, w as i32) as usize); self.back[o..e].copy_from_slice(&self.tint[o..e]); }
        } else { fill(&mut self.back, w, r, self.look.1); }
        let bc = if focused { ACCENT } else { BORDER_DIM };
        for e in [Rect { h: m.border, ..r }, Rect { y: r.y + r.h - m.border, h: m.border, ..r }, Rect { w: m.border, ..r }, Rect { x: r.x + r.w - m.border, w: m.border, ..r }] { fill(&mut self.back, w, e, bc); }
        let inner = self.inner(r);
        let mut shown = false;
        if let Some(b) = self.panes[i].surface.as_ref().and_then(|s| s.buf.as_ref()) {   // an app's pixels fill the inside of the border
            b.blit(&mut self.back, w, inner.x as usize, inner.y as usize, (0, 0, b.w, b.h));
            shown = true;
        }
        let p = &self.panes[i];
        if !shown {
            let (x0, y0) = (r.x + m.border + m.padx, inner.y + m.pady);
            for cy in 0..p.term.rows { for cx in 0..p.term.cols {
                let c = p.term.cells[cy * p.term.cols + cx];
                let (mut fg, mut bg) = (self.color(c.fg, c.bold, FG), if c.bg == DEF_BG { None } else { Some(self.color(c.bg, false, 0)) });
                if c.rev { let f = fg; fg = bg.unwrap_or(self.look.1); bg = Some(f); }
                if focused && p.term.cursor_on && (cx, cy) == (p.term.cx, p.term.cy) { fg = self.look.0; bg = Some(ACCENT); }
                let (x, y) = (x0 + cx as i32 * fw, y0 + cy as i32 * fh);
                if let Some(b) = bg { fill(&mut self.back, w, Rect { x, y, w: fw, h: fh }, b); }
                if c.ch != ' ' { glyph(&mut self.back, w, &self.font, x, y, c.ch, fg); }
            } }
            // the shell's own key hint on its last row, only where the row is empty (an app draws its own)
            if p.term.title.is_empty() && self.uid.is_some() && p.term.rows > 2 && (0..p.term.cols).all(|cx| p.term.cells[(p.term.rows - 1) * p.term.cols + cx].ch == ' ') && p.term.cy + 1 < p.term.rows {
                let y = y0 + (p.term.rows as i32 - 1) * fh;
                let mut x = x0;
                for (k, (key, what)) in [("alt+enter", "split"), ("alt+arrows", "focus"), ("alt+f", "full"), ("alt+q", "close")].into_iter().enumerate() {
                    let need = (key.len() + what.len() + 1 + if k > 0 { 3 } else { 0 }) as i32 * fw;
                    if x + need > r.x + r.w - m.border - m.padx { break }
                    if k > 0 { x += 3 * fw; }
                    x += text(&mut self.back, w, &self.font, x, y, key, FG) + fw;
                    x += text(&mut self.back, w, &self.font, x, y, what, DIM);
                }
            }
        }
        // the pane name sits in the top border, 18 px in, on a 8 px wide patch of wall
        let p = &self.panes[i];
        let name = if !p.term.title.is_empty() { p.term.title.as_str() } else if self.uid.is_none() { "login" } else { "shell" };
        let (tx, ty) = (r.x + 18 * self.sc, r.y + m.border / 2 - fh / 2);
        let tw = name.chars().count() as i32 * fw;
        for y in ty.max(0)..(ty + fh).min(self.h as i32) { for x in (tx - 8 * self.sc).max(0)..(tx + tw + 8 * self.sc).min(w as i32) { let o = y as usize * w + x as usize; self.back[o] = wall::blend(self.look.0, (if self.uid.is_none() { &self.wall_login } else { &self.wall_big })[o], PANEL_ALPHA); } }   // the label's patch is wall colour over the wallpaper, so the mark does not show through the letters
        text(&mut self.back, w, &self.font, tx, ty, name, if focused { ACCENT } else { DIM });
        self.dirty_y = (self.dirty_y.0.min((r.y - fh / 2).max(0) as usize), self.dirty_y.1.max((r.y + r.h) as usize));
    }

    fn text(&mut self, x: i32, s: &str, c: u32) -> i32 {
        let y = (self.m.bar - self.font.h as i32) / 2;
        for (k, ch) in s.chars().enumerate() { glyph(&mut self.back, self.w, &self.font, x + (k * self.font.w) as i32, y, ch, c); }
        s.chars().count() as i32 * self.font.w as i32
    }

    /// `parts` in a row at x on the bar; the width drawn.
    fn segs(&mut self, x: i32, parts: &[(&str, u32)]) -> i32 {
        let mut x2 = x;
        for (t, c) in parts { x2 += self.text(x2, t, *c); }
        x2 - x
    }

    /// Whether an `agent` process of the user is running (not its background helper): the bar says "ai working".
    fn agent_busy(uid: u32) -> bool {
        let Ok(rd) = fs::read_dir("/proc") else { return false };
        rd.flatten().any(|e| {
            let n = e.file_name(); let Some(pid) = n.to_str().filter(|n| n.bytes().all(|b| b.is_ascii_digit())) else { return false };
            fs::read_to_string(format!("/proc/{pid}/comm")).is_ok_and(|c| c.trim() == "agent")
                && fs::metadata(format!("/proc/{pid}")).is_ok_and(|m| m.uid() == uid)
                && fs::read(format!("/proc/{pid}/cmdline")).is_ok_and(|c| !c.split(|&b| b == 0).skip(1).any(|a| a == b"daemon" || a == b"setup"))
        })
    }

    fn draw_bar(&mut self) {
        let (w, m) = (self.w, self.m);
        fill(&mut self.back, w, Rect { x: 0, y: 0, w: w as i32, h: m.bar }, self.look.0);
        let fw = self.font.w as i32;
        let (l, r) = (m.gap, w as i32 - m.gap);
        // left: pane numbers (the focused one in peach), then the focused pane's name
        let ids = self.layout.ids(self.area, m.gap);
        let mut x = l;
        if self.uid.is_some() { for (k, &id) in ids.iter().enumerate() { x += self.segs(x, &[(&(k + 1).to_string(), if id == self.layout.focus { ACCENT } else { DIM }), (" ", DIM)]); } }
        let title = self.panes.iter().find(|p| p.id == self.layout.focus).map_or("", |p| p.term.title.as_str());   // set by the app in the pane (OSC 0/2)
        let name = if self.uid.is_none() { "login" } else if title.is_empty() { "shell" } else { title }.to_string();
        self.segs(x + if self.uid.is_some() { fw } else { 0 }, &[(&name, DIM)]);
        // right: ip, vol, ai
        let (ai_t, ai_c) = match self.ai { Ai::On => ("ai on", OK), Ai::Working => ("ai working", ACCENT), Ai::Off => ("ai off", DIM) };
        let vol = match self.vol { Some(v) => format!("vol {v}"), None => "vol off".to_string() };
        let ip = ip_string();
        let rw = (ip.chars().count() + vol.chars().count() + ai_t.len() + 6) as i32 * fw;
        self.segs(r - rw, &[(&ip, DIM), ("   ", DIM), (&vol, DIM), ("   ", DIM), (ai_t, ai_c)]);
        // centre: the day and time, or what needs attention: den's own question, the open microphone, a fresh notice
        let now = clock::now();
        let off = self.home.as_deref().zip(self.uid).and_then(|(h, u)| clock::read_tz(&format!("{h}/.tz"), u)).or_else(clock::default_offset);
        let notice = self.home.as_deref().zip(self.uid).and_then(|(h, u)| notify::latest_at(&format!("{h}/.notify"), u, now));
        let n_of = |id: usize| ids.iter().position(|&x| x == id).map_or(0, |p| p + 1);
        let now_ms = self.now_ms();
        let room = ((w as i32 - 2 * (rw.max(x - l) + 2 * m.gap)) / fw).max(10) as usize;
        let centred = |den: &mut Den, parts: &[(&str, u32)]| -> (i32, i32) { let tw: i32 = parts.iter().map(|p| p.0.chars().count() as i32).sum::<i32>() * fw; let x = (w as i32 - tw) / 2; den.segs(x, parts); (x, tw) };
        if let Some(pane) = self.mic.asking() {   // the question is den's own: a peach outlined box, nothing in a pane can draw here
            let q = format!("rec (pane {}) wants the microphone", n_of(pane));
            let tw = (q.chars().count() + 3 + 9 + 3 + 18) as i32 * fw;
            let b = Rect { x: (w as i32 - tw) / 2 - fw, y: 2 * self.sc, w: tw + 2 * fw, h: m.bar - 4 * self.sc };
            fill(&mut self.back, w, b, ACCENT); fill(&mut self.back, w, Rect { x: b.x + m.border, y: b.y + m.border, w: b.w - 2 * m.border, h: b.h - 2 * m.border }, self.look.1);
            centred(self, &[(&q, FG), ("   ", DIM), ("[y]", OK), (" allow", FG), ("   ", DIM), ("[n] deny (default)", DIM)]);
        } else if let Some(sec) = self.mic.seconds(now_ms) {   // steady red while the microphone is open
            let (x, _) = centred(self, &[("  ", ERR), (&format!("rec {}   ", mic::clock(sec)), FG), ("alt+r", FG), (" stop", DIM)]);
            fill(&mut self.back, w, Rect { x: x + 4 * self.sc, y: (m.bar - 8 * self.sc) / 2, w: 8 * self.sc, h: 8 * self.sc }, ERR);   // the red recording light: a solid square, easy to see
        } else if let Some((ts, n)) = notice {   // a notice from the helper replaces the clock while it is fresh; in its first 20 seconds it flashes (the alert when there is no sound)
            let t: String = format!("\u{2192} {}", printable(&n)).chars().take(room).collect();
            let flash = now - ts < 20 && now % 2 == 0;
            if flash { let tw = t.chars().count() as i32 * fw; fill(&mut self.back, w, Rect { x: (w as i32 - tw) / 2 - fw / 2, y: 3 * self.sc, w: tw + fw, h: m.bar - 6 * self.sc }, ACCENT); }
            centred(self, &[(&t, if flash { self.look.0 } else { WARN })]);
        } else {
            centred(self, &[(&clock::day_time(now, off), FG)]);
        }
        self.dirty_y = (0, self.dirty_y.1.max(m.bar as usize));
    }

    fn render(&mut self, bar: bool) {
        if self.full_redraw {
            self.back.copy_from_slice((if self.uid.is_none() { &self.wall_login } else { &self.wall_big }));
            for i in 0..self.panes.len() { if self.panes[i].visible { self.draw_pane(i); } }
            self.draw_bar(); self.dirty_y = (0, self.h); self.full_redraw = false;
        } else {
            for i in 0..self.panes.len() { if self.panes[i].visible && self.panes[i].term.dirty { self.draw_pane(i); } }
            if bar { self.draw_bar(); }
        }
        for p in &mut self.panes { p.term.dirty = false; p.dirty = false; }
        // the cursor is drawn last: into `back` for the present, then taken out again, so `back` never holds it. A move re-presents the rows of the old and the new place.
        let cur = self.cursor_on.then(|| Rect { x: self.ptr.x, y: self.ptr.y, w: 12 * self.sc, h: 19 * self.sc });
        if let Some(c) = cur.filter(|c| self.cur_screen != Some(*c)) {
            for r in self.cur_screen.into_iter().chain([c]) { self.dirty_y = (self.dirty_y.0.min(r.y as usize), self.dirty_y.1.max(((r.y + r.h) as usize).min(self.h))); }
        }
        if self.dirty_y.1 > self.dirty_y.0 {
            let under = cur.filter(|c| (c.y as usize) < self.dirty_y.1 && ((c.y + c.h) as usize) > self.dirty_y.0).map(|c| self.draw_cursor(c));
            self.fb.present(&self.back, self.dirty_y.0, self.dirty_y.1);
            if let (Some(c), Some(u)) = (cur, under) { self.restore_cursor(c, &u); }
            self.cur_screen = cur;
        }
        self.dirty_y = (usize::MAX, 0);
    }

    /// The part of `c` on the screen as (x0, y0, x1, y1).
    fn clip(&self, c: Rect) -> (usize, usize, usize, usize) { (c.x as usize, c.y as usize, ((c.x + c.w) as usize).min(self.w), ((c.y + c.h) as usize).min(self.h)) }
    /// Paints the arrow into `back`; returns the pixels it covered.
    fn draw_cursor(&mut self, c: Rect) -> Vec<u32> {
        let (x0, y0, x1, y1) = self.clip(c);
        let under = (y0..y1).flat_map(|y| self.back[y * self.w + x0..y * self.w + x1].to_vec()).collect();
        for y in y0..y1 { for x in x0..x1 {
            let col = match ARROW[(y - y0) / self.sc as usize].as_bytes()[(x - x0) / self.sc as usize] { b'X' => CUR_FILL, b'o' => CUR_EDGE, _ => continue };
            self.back[y * self.w + x] = col;
        } }
        under
    }
    fn restore_cursor(&mut self, c: Rect, under: &[u32]) {
        let (x0, y0, x1, y1) = self.clip(c);
        for (k, y) in (y0..y1).enumerate() { self.back[y * self.w + x0..y * self.w + x1].copy_from_slice(&under[k * (x1 - x0)..(k + 1) * (x1 - x0)]); }
    }

    /// The focused pane's app, if one is connected.
    fn focus_app(&self) -> Option<(&Pane, &Surface)> {
        self.panes.iter().find(|p| p.id == self.layout.focus).and_then(|p| p.surface.as_ref().filter(|s| s.ready() && !s.mic).map(|s| (p, s)))
    }
    /// A key for the focused app (releases too). Alt and Cmd are never sent, and no press while they are held. True when an app took the key: the shell does not get it.
    fn app_key(&self, code: u16, val: i32) -> bool {
        let Some((_, s)) = self.focus_app() else { return false };
        if !Keys::is_wm_key(code) && (val == 0 || !self.keys.wm()) { s.send(Down::Key { code: code as u32, value: val as u32, mods: self.keys.mods() }); }
        true
    }
    /// The cursor in the focused app's pixels (negative or too big while it is outside).
    fn app_pointer(&self) -> Option<(&Surface, i32, i32)> {
        let (p, s) = self.focus_app()?; let i = self.inner(p.rect);
        Some((s, self.ptr.x - i.x, self.ptr.y - i.y))
    }
    fn pointer_event(&mut self, dev: usize, ty: u16, code: u16, val: i32) {
        match self.ptr.event(self.ptrs[dev].abs, ty, code, val) {
            Some(Ptr::Moved) => { self.cursor_on = true; if let Some((s, x, y)) = self.app_pointer() { s.send(Down::Pointer { x, y }); } }
            Some(Ptr::Button(c, down)) => {
                if down && self.uid.is_some() {   // a click focuses the pane under the cursor, and that app gets the click and the position
                    let hit = self.panes.iter().find(|p| p.visible && (p.rect.x..p.rect.x + p.rect.w).contains(&self.ptr.x) && (p.rect.y..p.rect.y + p.rect.h).contains(&self.ptr.y)).map(|p| p.id);
                    if let Some(id) = hit.filter(|&id| id != self.layout.focus) {
                        self.layout.focus = id; self.relayout();
                        if let Some((s, x, y)) = self.app_pointer() { s.send(Down::Pointer { x, y }); }
                    }
                }
                if let Some((s, _, _)) = self.app_pointer() { s.send(Down::Button { code: c as u32, pressed: down }); }
            }
            Some(Ptr::Scroll(dx, dy)) => { if let Some((s, _, _)) = self.app_pointer() { s.send(Down::Scroll { dx, dy }); } }
            None => {}
        }
    }

    /// New connections on /run/den/sock. Trust checks 1 and 2: the peer is the logged-in user and runs on the pty of one pane (the app draws only into the pane it was started in).
    fn accept_clients(&mut self) {
        loop {
            let fd = unsafe { libc::accept4(self.listener, std::ptr::null_mut(), std::ptr::null_mut(), libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC) };
            if fd < 0 { break }
            let fd = unsafe { OwnedFd::from_raw_fd(fd) };
            let tty = match self.uid { Some(u) => surface::peer_tty(fd.as_raw_fd(), u), None => Err("nobody is logged in") };
            match tty.map(|t| self.panes.iter_mut().find(|p| p.tty == t && t != 0 && p.surface.is_none())) {
                Ok(Some(p)) => p.surface = Some(Surface::new(fd)),
                Ok(None) => eprintln!("den: surface refused: not a pane of this desktop (or the pane has an app already)"),
                Err(e) => eprintln!("den: surface refused: {e}"),
            }
        }
    }
    fn drop_surface(&mut self, i: usize) { let a = self.mic.gone(self.panes[i].id); self.mic_run(a); self.panes[i].surface = None; self.panes[i].term.dirty = true; }

    /// Messages from the app of pane `id` (trust checks 3 and 4 are in Surface::handle). Anything wrong drops the app and the pane shows its text again.
    fn surface_read(&mut self, id: usize) {
        let Some(i) = self.panes.iter().position(|p| p.id == id) else { return };
        for _ in 0..64 {
            let Some(s) = self.panes[i].surface.as_mut() else { return };
            let mut buf = [0u8; 512];
            let (n, fds, bad) = match recv_msg(s.raw(), &mut buf, true) {
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return,
                Ok((n, f, b)) if n > 0 => (n, f, b),
                _ => return self.drop_surface(i),   // closed or broken
            };
            let msg = if bad { None } else { Up::parse(&buf[..n]) };
            let Some(m) = msg else { eprintln!("den: surface dropped: bad message"); return self.drop_surface(i) };
            let commit = matches!(m, Up::Commit(_));
            match s.handle(m, fds, s.sent.map_or((0, 0), |c| (c.0, c.1))) {
                Err(e) => { eprintln!("den: surface dropped: {e}"); return self.drop_surface(i) }
                Ok(Got::Hello) => self.push_config(),
                Ok(Got::Attached) => self.panes[i].term.dirty = true,
                Ok(Got::Damage(rects)) => self.blit(i, &rects),
                Ok(Got::MicStart) => {
                    // only the genuine, root-owned /bin/rec may even raise the question; read the program again now, not only at connect
                    let ok = surface::peer_exe(s.raw()).is_ok_and(|e| mic::exe_ok(&e, std::path::Path::new("/bin/rec"), 0));
                    if !ok { eprintln!("den: microphone request refused: not /bin/rec"); return self.drop_surface(i) }
                    s.mic = true;   // rec is a text app: it keeps the keyboard
                    let (id, now) = (self.panes[i].id, self.now_ms());
                    let a = self.mic.start(id, true, now); self.mic_run(a);
                }
                Ok(Got::MicStop) => { let a = self.mic.stop(Some(self.panes[i].id)); self.mic_run(a); }
                Ok(Got::Nothing) => {}
            }
            if commit { if let Some(s) = &self.panes[i].surface { s.send(Down::Done); } }
        }
    }
    /// An app committed: copy just the damaged rects onto the screen buffer.
    fn blit(&mut self, i: usize, rects: &[(usize, usize, usize, usize)]) {
        let p = &self.panes[i];
        let Some(b) = p.surface.as_ref().and_then(|s| s.buf.as_ref()).filter(|_| p.visible) else { return };
        let i2 = self.inner(p.rect); let (ox, oy) = (i2.x as usize, i2.y as usize);
        for r in rects { b.blit(&mut self.back, self.w, ox, oy, *r); self.dirty_y = (self.dirty_y.0.min(oy + r.1), self.dirty_y.1.max(oy + r.1 + r.3)); }
    }

    fn now_ms(&self) -> u64 { self.t0.elapsed().as_millis() as u64 }
    /// Does what the broker says: tell an app, open the capture device (as root, here), or close it. Closing the fd is what really revokes: no app ever holds it.
    fn mic_run(&mut self, acts: Vec<Do>) {
        if acts.is_empty() { return }
        self.bar_dirty = true;
        for a in acts { match a {
            Do::Tell(pane, state, rate) => { if let Some(s) = self.panes.iter().find(|p| p.id == pane).and_then(|p| p.surface.as_ref()) { s.send(Down::Mic { state, rate }); } }
            Do::Close => { if self.mic_dev.take().is_some() { eprintln!("den: microphone off"); } }
            Do::Open => {
                let r = sound::open_capture();
                if let Err(e) = &r { eprintln!("den: microphone: {e}"); }
                let (rate, now) = (r.as_ref().ok().map(|x| x.1), self.now_ms());
                self.mic_dev = r.ok().map(|x| x.0);
                if self.mic_dev.is_some() { eprintln!("den: microphone on"); }
                let more = self.mic.opened(rate, now); self.mic_run(more);
            }
        } }
    }
    /// While the question is up every key press is den's: y allows, anything else denies, nothing reaches a pane. True = consumed.
    fn mic_consumes(&mut self, code: u16, val: i32) -> bool {
        if self.mic.asking().is_none() { return false }
        if val == 1 { let now = self.now_ms(); let a = self.mic.key(code, now); self.mic_run(a); }
        true
    }
    /// Reads what the capture device has and sends it to rec. An error that is not "nothing yet" ends the recording.
    fn mic_pump(&mut self) {
        let (Some(f), mic::State::Live { pane, .. }) = (self.mic_dev.as_ref(), self.mic.state) else { return };
        let mut buf = [0u8; surface::PCM_MAX];
        for _ in 0..16 {
            let n = unsafe { libc::read(f.as_raw_fd(), buf.as_mut_ptr() as *mut _, buf.len()) };
            if n > 0 {
                let m = surface::pcm_msg(&buf[..n as usize & !3]);
                if let Some(s) = self.panes.iter().find(|p| p.id == pane).and_then(|p| p.surface.as_ref()) { let _ = surface::send_msg(s.raw(), &m, None, true); }
                continue
            }
            match io::Error::last_os_error().raw_os_error() {
                Some(libc::EAGAIN) if n < 0 => {}
                Some(libc::EPIPE) if n < 0 => { sound::capture_restart(f); continue }   // overrun: start again
                _ => { eprintln!("den: microphone stopped: {}", io::Error::last_os_error()); let a = self.mic.stop(None); self.mic_run(a); }
            }
            break
        }
    }

    fn act(&mut self, a: Action) {
        if self.uid.is_none() { return }   // no panes or focus games before login
        match a {
            Action::Split => { let n = self.new_pane(); self.layout.split(self.area, self.m.gap, n); }
            Action::Close => { let id = self.layout.focus; self.remove_pane(id, true); return }
            Action::Fullscreen => self.layout.toggle_full(),
            Action::StopMic => { let a = self.mic.stop(None); self.mic_run(a); return }
            Action::Focus(d) => self.layout.move_focus(self.area, self.m.gap, d),
        }
        self.relayout();
    }

    /// Once a second: the wallpaper the logged-in user chose in settings (navy before login and when unset; redrawn only when it changes), the volume and the AI state for the bar.
    fn sync_user(&mut self) {
        let st = self.home.as_deref().zip(self.uid).and_then(|(h, u)| read_small(&store::settings_path(h), u, 4096));
        let idx = st.as_deref().map_or(0, prefs::wall_index);
        let (on, vol) = snd_cfg::config(st.as_deref().unwrap_or(""));
        let vol = on.then_some(vol);
        let off = self.home.as_deref().zip(self.uid).and_then(|(h, u)| read_small(&format!("{h}/.config/thir/ai"), u, 64)).is_some_and(|t| t.trim() == "off");
        let ai = match self.uid { Some(u) if !off && Self::agent_busy(u) => Ai::Working, _ if off => Ai::Off, _ => Ai::On };
        if (vol, ai) != (self.vol, self.ai) { self.vol = vol; self.ai = ai; self.bar_dirty = true; }
        if idx == self.wall_idx { return }
        self.wall_idx = idx;
        self.look = (WALLS[idx].1, WALLS[idx].2, WALLS[idx].3);
        let (login, big, tint) = scene(self.w, self.h, self.look, self.login_rect());
        self.wall_login = login; self.wall_big = big; self.tint = tint; self.full_redraw = true;
    }

    fn scan_inputs(&mut self) {
        let Ok(rd) = fs::read_dir("/dev/input") else { return };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if !name.starts_with("event") || self.inputs.iter().any(|i| i.0 == name) || self.ptrs.iter().any(|i| i.name == name) { continue }
            let Ok(f) = OpenOptions::new().read(true).custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC).open(e.path()) else { continue };
            let fd = f.into_raw_fd();
            // EVIOCGBIT(ev, len): the event types, then the codes of one type
            let bits = |ev: u32, b: &mut [u8]| unsafe { libc::ioctl(fd, (0x8000_0000u32 | (b.len() as u32) << 16 | 0x45 << 8 | (0x20 + ev)) as _, b.as_mut_ptr()) } >= 0;
            let (mut types, mut keys, mut abs, mut rel) = ([0u8; 4], [0u8; 64], [0u8; 8], [0u8; 4]);
            let t = bits(0, &mut types);
            if t && types[0] & 2 != 0 && bits(1, &mut keys) && keys[3] & 0x40 != 0 { self.inputs.push((name, fd)); continue }   // EV_KEY and KEY_A: a keyboard
            let range = |a: u32| { let mut v = [0i32; 6]; (unsafe { libc::ioctl(fd, (0x8000_0000u32 | 24 << 16 | 0x45 << 8 | (0x40 + a)) as _, v.as_mut_ptr()) } >= 0).then_some((v[1], v[2])) };   // EVIOCGABS: value min max ...
            if t && types[0] & 8 != 0 && bits(3, &mut abs) && abs[0] & 3 == 3 {   // EV_ABS with ABS_X, ABS_Y: a tablet
                if let (Some(x), Some(y)) = (range(0), range(1)) { self.ptrs.push(PtrDev { name, fd, abs: Some([x, y]) }); continue }
            } else if t && types[0] & 4 != 0 && bits(2, &mut rel) && rel[0] & 3 == 3 { self.ptrs.push(PtrDev { name, fd, abs: None }); continue }   // EV_REL with REL_X, REL_Y: a mouse
            unsafe { libc::close(fd); }
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

/// The program a pane runs: always /bin/login. Panes after the first one log in as the uid already logged in.
fn pane_command(uid: Option<u32>) -> Vec<String> {
    match uid { None => vec!["/bin/login".into()], Some(u) => vec!["/bin/login".into(), "--as".into(), u.to_string()] }
}

/// New pty with `pane_command` on the slave side. Returns (master fd, shell pid, the slave's device number: what /proc/PID/stat reports as tty_nr for processes in the pane).
fn spawn_shell(uid: Option<u32>, cols: usize, rows: usize) -> io::Result<(i32, i32, u64)> {
    unsafe {
        let m = libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC);
        if m < 0 { return Err(io::Error::last_os_error()); }
        let mut name = [0 as libc::c_char; 64];
        if libc::grantpt(m) < 0 || libc::unlockpt(m) < 0 || libc::ptsname_r(m, name.as_mut_ptr(), 64) != 0 { let e = io::Error::last_os_error(); libc::close(m); return Err(e); }
        let path = std::ffi::CStr::from_ptr(name.as_ptr()).to_str().unwrap_or("").to_string();
        set_winsize(m, cols, rows);
        libc::fcntl(m, libc::F_SETFL, libc::O_NONBLOCK);
        let slave = OpenOptions::new().read(true).write(true).custom_flags(libc::O_NOCTTY | libc::O_CLOEXEC).open(&path)?;
        let tty = slave.metadata()?.rdev();
        let dup = |f: &File| f.try_clone().map(Stdio::from);
        let argv = pane_command(uid);
        let mut cmd = Command::new(&argv[0]); cmd.args(&argv[1..]);
        cmd.env_clear().env("PATH", "/bin").env("HOME", "/").env("TERM", "linux").stdin(dup(&slave)?).stdout(dup(&slave)?).stderr(dup(&slave)?);
        cmd.pre_exec(|| { libc::setsid(); libc::ioctl(0, libc::TIOCSCTTY as _, 0); Ok(()) });
        match cmd.spawn() { Ok(c) => Ok((m, c.id() as i32, tty)), Err(e) => { libc::close(m); Err(e) } }
    }
}

/// The wallpapers of one preset: the small wordmark above the login box (`login` is its rectangle), the big wordmark, muted so the gaps between panes look calm (a desktop), and the big one under the pane colour (what a plain shell pane shows).
fn scene(w: usize, h: usize, look: (u32, u32, u32), login: Rect) -> (Vec<u32>, Vec<u32>, Vec<u32>) {
    let flat = vec![look.0; w * h];
    let cell = wall::cell_for(w);
    let mut big = flat.clone();
    wall::wordmark(&mut big, w, (w as i32 - wall::MARK_COLS * cell) / 2, h as i32 / 2 + 10 * (cell / 20).max(1) - wall::MARK_ROWS * cell / 2, cell, wall::blend(MARK, look.0, MARK_ALPHA));
    let mut lg = flat;
    let c2 = (cell / 2).max(4);
    wall::wordmark(&mut lg, w, (w as i32 - wall::MARK_COLS * c2) / 2, login.y - 6 * c2 - wall::MARK_ROWS * c2, c2, MARK);
    let tint = big.iter().map(|&p| wall::blend(look.1, p, PANEL_ALPHA)).collect();
    (lg, big, tint)
}

/// The surface socket /run/den/sock, or -1 (apps then cannot draw; den works as before). Any local user may connect; `accept_clients` decides who is trusted.
fn listen() -> i32 {
    let r = (|| -> io::Result<i32> {
        fs::create_dir_all("/run/den")?; let _ = fs::remove_file(surface::SOCK);
        let s = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_SEQPACKET | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC, 0) };
        if s < 0 { return Err(io::Error::last_os_error()) }
        let mut a: libc::sockaddr_un = unsafe { std::mem::zeroed() };
        a.sun_family = libc::AF_UNIX as _;
        for (d, b) in a.sun_path.iter_mut().zip(surface::SOCK.bytes()) { *d = b as _; }
        if unsafe { libc::bind(s, &a as *const _ as *const _, std::mem::size_of::<libc::sockaddr_un>() as _) } != 0 || unsafe { libc::listen(s, 8) } != 0 { let e = io::Error::last_os_error(); unsafe { libc::close(s); } return Err(e) }
        fs::set_permissions(surface::SOCK, std::os::unix::fs::PermissionsExt::from_mode(0o666))?;
        Ok(s)
    })();
    r.unwrap_or_else(|e| { eprintln!("den: {}: {e}", surface::SOCK); -1 })
}

fn main() {
    let fb = Fb::open().unwrap_or_else(|e| { eprintln!("den: framebuffer: {e}"); exit(1) });
    println!("den: {}x{} {}bpp", fb.w, fb.h, fb.bpp);
    let sc = if fb.w >= 2000 { 2 } else { 1 };   // integer scale keeps the bitmap font crisp
    let m = Metrics::at(sc);
    let font = Font::parse(&read_all(if sc == 2 { "/share/font32.psf" } else { "/share/font16.psf" })).unwrap_or_else(|e| { eprintln!("den: font: {e}"); exit(1) });
    let (w, h) = (fb.w, fb.h);
    let area = Rect { x: m.gap, y: m.bar + m.gap, w: w as i32 - 2 * m.gap, h: h as i32 - m.bar - 2 * m.gap };
    let mut den = Den { fb, font, m, w, h, back: vec![0; w * h], wall_login: vec![], wall_big: vec![], tint: vec![], look: (WALLS[0].1, WALLS[0].2, WALLS[0].3), vol: None, ai: Ai::On, panes: vec![], layout: Layout::new(0), next_id: 0, area, full_redraw: true, dirty_y: (usize::MAX, 0), keys: Keys::default(), inputs: vec![], zombies: vec![], ptrs: vec![], ptr: Pointer::new(w, h), cursor_on: false, cur_screen: None, listener: listen(), mic: Broker::new(), mic_dev: None, t0: std::time::Instant::now(), bar_dirty: false, uid: None, home: None, wall_idx: 0, sc };
    let (a, b, c) = scene(w, h, den.look, den.login_rect()); den.wall_login = a; den.wall_big = b; den.tint = c;
    let first = den.new_pane(); den.layout = Layout::new(first);
    den.relayout(); den.scan_inputs(); den.render(true);

    let mut last_tick = std::time::Instant::now();
    loop {
        let sids: Vec<usize> = den.panes.iter().filter(|p| p.surface.is_some()).map(|p| p.id).collect();
        let mut fds: Vec<libc::pollfd> = den.inputs.iter().map(|i| i.1).chain(den.ptrs.iter().map(|p| p.fd)).chain([den.listener])
            .chain(den.panes.iter().filter_map(|p| p.surface.as_ref().map(|s| s.raw()))).chain(den.panes.iter().map(|p| p.master)).chain(den.mic_dev.iter().map(|f| f.as_raw_fd()))   // the capture fd is last: only the zips below stop short of it
            .map(|fd| libc::pollfd { fd, events: libc::POLLIN, revents: 0 }).collect();
        let ids: Vec<usize> = den.panes.iter().map(|p| p.id).collect();
        let (ni, np, ns) = (den.inputs.len(), den.ptrs.len(), sids.len());
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
                    let key = den.keys.event(code, val);
                    if den.mic_consumes(code, val) { continue }
                    if let Some(Key::Act(a)) = key { den.act(a); continue }
                    if den.app_key(code, val) { continue }   // an app in the pane takes keys and releases itself
                    match key {
                        Some(Key::Bytes(b)) => if let Some(p) = den.panes.iter().find(|p| p.id == den.layout.focus) { unsafe { libc::write(p.master, b.as_ptr() as *const _, b.len()); } },
                        _ => {}
                    }
                }
            }
        }
        for k in gone.into_iter().rev() { let (_, fd) = den.inputs.remove(k); unsafe { libc::close(fd); } }

        let mut gone = vec![];
        for (k, f) in fds[ni..ni + np].iter().enumerate() {
            if f.revents == 0 { continue }
            let mut buf = [0u8; 24 * 32];
            loop {
                let n = unsafe { libc::read(f.fd, buf.as_mut_ptr() as *mut _, buf.len()) };
                if n <= 0 { if n < 0 && io::Error::last_os_error().raw_os_error() != Some(libc::EAGAIN) { gone.push(k); } break }
                for ev in buf[..n as usize].chunks_exact(24) { den.pointer_event(k, u16::from_le_bytes([ev[16], ev[17]]), u16::from_le_bytes([ev[18], ev[19]]), i32::from_le_bytes([ev[20], ev[21], ev[22], ev[23]])); }
            }
        }
        for k in gone.into_iter().rev() { let d = den.ptrs.remove(k); unsafe { libc::close(d.fd); } }

        if fds[ni + np].revents != 0 { den.accept_clients(); }
        for (f, id) in fds[ni + np + 1..ni + np + 1 + ns].iter().zip(&sids) { if f.revents != 0 { den.surface_read(*id); } }

        for (f, id) in fds[ni + np + 1 + ns..].iter().zip(&ids) {
            if f.revents == 0 { continue }
            let Some(i) = den.panes.iter().position(|p| p.id == *id) else { continue };
            let mut buf = [0u8; 4096];
            let n = unsafe { libc::read(den.panes[i].master, buf.as_mut_ptr() as *mut _, buf.len()) };
            if n > 0 { den.panes[i].term.feed(&buf[..n as usize]); if den.uid.is_none() { den.check_login(); } continue }
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

        den.mic_pump();
        let a = den.mic.tick(den.now_ms()); den.mic_run(a);
        let tick = last_tick.elapsed() >= Duration::from_secs(1);
        if tick {
            last_tick = std::time::Instant::now();
            den.scan_inputs();
            if den.uid.is_none() { den.check_login(); }
            den.sync_user();
            if den.uid.is_none() { let a = den.mic.stop(None); den.mic_run(a); }   // nobody logged in: the microphone is never open
            den.zombies.retain(|&p| p > 0 && unsafe { libc::waitpid(p, std::ptr::null_mut(), libc::WNOHANG) } == 0);
        }
        let bar = tick | std::mem::take(&mut den.bar_dirty) | den.panes.iter_mut().fold(false, |a, p| std::mem::take(&mut p.term.title_changed) | a);
        den.render(bar);
    }
}
