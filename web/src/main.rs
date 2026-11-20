//! thir-web (`web`): the browser. WPE WebKit (platform.c) draws pages into shared-memory frames; this program copies the damaged parts into the pane's surface (the protocol of
//! gui/surface.rs), draws a one-line strip (address, title, progress) in thir's font, and turns den's keys, pointer and wheel into WPE events.
//! Order matters for safety: arguments are checked, den is connected FIRST, and only then Landlock closes the file system around this process (web::profile); the WebKit helpers
//! start through the wpe-shim, which locks each of them in further.
use std::{collections::HashSet, ffi::{c_char, c_void, CStr, CString}, io::Write, os::unix::fs::DirBuilderExt, path::{Path, PathBuf}, ptr};
use thir::{files::printable, gui::{draw::{fill, glyph, text}, font::Font, layout::Rect, surface::{Client, Down, R16, MOD_CTRL, MOD_SHIFT}, theme::*}, sandbox, web::{self, DKey, DialogKind, Find, Line, Picker, Proc, Tabs}};

#[repr(C)]
struct ThirCb {
    ud: *mut c_void,
    frame: Option<extern "C" fn(*const u8, u32, i32, i32, *const i32, u32, *mut c_void)>,
    title: extern "C" fn(i32, *const c_char, *mut c_void), uri: extern "C" fn(i32, *const c_char, *mut c_void), progress: extern "C" fn(i32, f64, *mut c_void),
    load: extern "C" fn(i32, i32, *mut c_void), failed: extern "C" fn(i32, *const c_char, *mut c_void), download: extern "C" fn(*mut c_void, *const c_char, i64, *mut c_void),
    popup_ok: extern "C" fn(*mut c_void) -> i32, opened: extern "C" fn(i32, *mut c_void), closed: extern "C" fn(i32, *mut c_void),
    dialog: extern "C" fn(i32, i32, *const c_char, *const c_char, *mut c_void, *mut c_void), chooser: extern "C" fn(i32, *mut c_void, *mut c_void), found: extern "C" fn(i32, *mut c_void),
}
extern "C" {
    fn thirweb_init(w: i32, h: i32, scale: f64, data: *const c_char, cache: *const c_char, cb: *const ThirCb) -> i32;
    fn thirweb_tab_new(uri: *const c_char) -> i32; fn thirweb_tab_show(id: i32); fn thirweb_tab_close(id: i32);
    fn thirweb_back(); fn thirweb_forward(); fn thirweb_edit(cmd: i32); fn thirweb_find(text: *const c_char, dir: i32);
    fn thirweb_dialog_answer(dlg: *mut c_void, ok: i32, text: *const c_char); fn thirweb_chooser_answer(req: *mut c_void, path: *const c_char);
    fn thirweb_load(uri: *const c_char); fn thirweb_reload(); fn thirweb_stop(); fn thirweb_resize(w: i32, h: i32, scale: f64); fn thirweb_focus(on: i32);
    fn thirweb_frame_done(); fn thirweb_download_answer(dl: *mut c_void, path: *const c_char);
    fn thirweb_keyinfo(code: u32, value: u32, sym: *mut u32) -> u32; fn thirweb_keysend(code: u32, value: u32);
    fn thirweb_pointer(x: f64, y: f64); fn thirweb_button(code: u32, pressed: i32, x: f64, y: f64); fn thirweb_scroll(dx: f64, dy: f64, x: f64, y: f64);
    fn thirweb_watch_fd(fd: i32, f: extern "C" fn(*mut c_void), d: *mut c_void); fn thirweb_timeout(ms: u32, f: extern "C" fn(*mut c_void), d: *mut c_void);
    fn thirweb_run(); fn thirweb_quit();
}

const K_ESC: u32 = 1; const K_BACKSPACE: u32 = 14; const K_ENTER: u32 = 28; const K_DELETE: u32 = 111; const K_LEFT: u32 = 105; const K_RIGHT: u32 = 106; const K_HOME: u32 = 102; const K_END: u32 = 107;
const K_T: u32 = 20; const K_W: u32 = 17; const K_TAB: u32 = 15; const K_PGUP: u32 = 104; const K_PGDN: u32 = 109; const K_LBRACE: u32 = 26; const K_RBRACE: u32 = 27; const K_F: u32 = 33;
const K_C: u32 = 46; const K_X: u32 = 45; const K_V: u32 = 47; const K_UP: u32 = 103; const K_DOWN: u32 = 108; const BTN_SIDE: u32 = 0x113; const BTN_EXTRA: u32 = 0x114;
const K_A: u32 = 30; const K_E: u32 = 18; const K_U: u32 = 22; const K_L: u32 = 38; const K_R: u32 = 19; const K_Q: u32 = 16; const K_Y: u32 = 21; const K_N: u32 = 49; const K_F5: u32 = 63;

enum Mode {
    Normal, Edit(Line), Ask { dl: *mut c_void, name: String, size: Option<u64> },
    Find(Find), Dialog { dlg: *mut c_void, kind: DialogKind, msg: String, line: Line }, Upload { req: *mut c_void, pick: Picker },
}
/// What the program keeps per tab (WebKit keeps the page).
struct TabState { id: i32, url: String, title: String, progress: Option<u8> }
impl TabState { fn new(id: i32, url: &str) -> TabState { TabState { id, url: url.into(), title: String::new(), progress: None } } }

struct App {
    c: Option<Client>, font: Option<Font>, sc: u32, strip_h: usize, foot_h: usize, headless: bool,
    tabs: Tabs<TabState>, note: Option<String>, mode: Mode, home: PathBuf,
    pending: Vec<R16>, inflight: bool, frame_waiting: bool, frame_sent: bool,   // frame_waiting: a WebKit frame is copied but not yet committed; frame_sent: committed, DONE pending
    attached: (u32, u32), keep_note: bool, consumed: HashSet<u32>, ptr: (i32, i32), btn_down: bool, failed: bool,
}
static mut APP: *mut App = ptr::null_mut();
fn app() -> &'static mut App { unsafe { &mut *APP } }
fn cstr(p: *const c_char) -> String { if p.is_null() { String::new() } else { unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned() } }

/// The pixel heights of the strip on top (tabs, then address) and of the hint row under the page.
fn strip_sizes(f: &Font, sc: u32) -> (usize, usize) { (2 * f.h + 9 * sc as usize, f.h + 8 * sc as usize) }

/// A line editor's characters and cursor at (x, y), at most `room` columns.
fn line_at(px: &mut [u32], w: usize, font: &Font, x: i32, y: i32, l: &Line, room: usize) {
    let start = (l.at + 1).saturating_sub(room);
    for (k, ch) in l.text.iter().skip(start).take(room).enumerate() { glyph(px, w, font, x + (k * font.w) as i32, y, *ch, FG); }
    let cx = x + ((l.at - start) * font.w) as i32;
    fill(px, w, Rect { x: cx, y, w: font.w as i32, h: font.h as i32 }, ACCENT);
    if let Some(ch) = l.text.get(l.at) { glyph(px, w, font, cx, y, *ch, PANEL); }
}

impl App {
    fn page_size(&self) -> (i32, i32) { let c = self.c.as_ref(); let (w, h) = c.map_or((1024, 768), |c| (c.w as usize, (c.h as usize).saturating_sub(self.strip_h + self.foot_h))); ((w as i32 / self.sc as i32).max(1), (h as i32 / self.sc as i32).max(1)) }
    fn commit(&mut self, r: R16) { self.pending.push(r); self.flush(); }
    fn flush(&mut self) {
        if self.inflight || self.pending.is_empty() { return }
        let Some(c) = self.c.as_ref() else { return };
        let rects: Vec<R16> = if self.pending.len() > thir::gui::surface::MAX_RECTS { vec![(0, 0, c.w.min(65535) as u16, c.h.min(65535) as u16)] } else { std::mem::take(&mut self.pending) };
        self.pending.clear();
        if c.commit(&rects).is_err() { unsafe { thirweb_quit() } return }
        self.inflight = true; if self.frame_waiting { (self.frame_waiting, self.frame_sent) = (false, true); }
    }
    fn on_done(&mut self) { self.inflight = false; if self.frame_sent { self.frame_sent = false; unsafe { thirweb_frame_done() } } self.flush(); }
    fn tab(&self) -> &TabState { self.tabs.current() }
    fn draw_strip(&mut self) {
        let labels: Vec<String> = self.tabs.items.iter().map(|t| web::tab_label(&t.url, &t.title)).collect();
        let (url, progress, cur, ntabs) = (self.tab().url.clone(), self.tab().progress, self.tabs.cur, self.tabs.items.len());
        let (Some(c), Some(font)) = (self.c.as_mut(), self.font.as_ref()) else { return };
        let (w, sc, ch) = (c.w as usize, self.sc as i32, c.h as usize);
        let (strip_h, cols) = (self.strip_h as i32, c.w as usize / font.w);
        let (y1, x0) = (3 * sc, font.w as i32);
        let y = y1 + font.h as i32 + 2 * sc;   // line 2: the arrows, the address and whatever is being asked
        let px = c.pixels(); if px.is_empty() { return }
        fill(px, w, Rect { x: 0, y: 0, w: w as i32, h: strip_h }, PANEL);
        fill(px, w, Rect { x: 0, y: strip_h - sc, w: w as i32, h: sc }, BORDER_DIM);
        let pr = progress.map_or(0, |p| p as i32 * w as i32 / 100);
        fill(px, w, Rect { x: 0, y: strip_h - 2 * sc, w: pr, h: 2 * sc }, ACCENT);
        // line 1: the tabs, the active one as `[1 thir.dev]` in peach, the others dim
        { let mut x = x0;
          for (k, (t, on)) in web::strip_tabs(&labels, cur, cols.saturating_sub(6)).iter().enumerate() {
              if k > 0 { x += 2 * font.w as i32; }
              x += if *on { text(px, w, font, x, y1, &format!("[{t}]"), ACCENT) } else { text(px, w, font, x, y1, t, DIM) };
          }
          if ntabs < web::MAX_TABS { text(px, w, font, x + 2 * font.w as i32, y1, "+", DIM); } }
        // a line being edited (address, find, prompt): the label, the characters, a block cursor
        let line = |px: &mut [u32], label: &str, l: &Line, room: usize| { let x = x0 + text(px, w, font, x0, y, label, DIM); line_at(px, w, font, x, y, l, room.saturating_sub(label.chars().count() + 3)); };
        match &self.mode {
            Mode::Ask { name, size, .. } => { text(px, w, font, x0, y, &web::fit(&web::download_prompt(name, *size), cols.saturating_sub(2)), WARN); }
            Mode::Edit(l) => line(px, "go ", l, cols),
            Mode::Find(f) => {
                let st = web::fit(&f.status(), 14); line(px, "find ", &f.line, cols.saturating_sub(st.chars().count() + 1));
                if !st.is_empty() { text(px, w, font, w as i32 - (st.chars().count() as i32 + 1) * font.w as i32, y, &st, if f.matches == Some(0) { ERR } else { ACCENT }); }
            }
            Mode::Dialog { kind, msg, line: l, .. } => {
                let t = web::fit(&web::dialog_text(*kind, msg), cols.saturating_sub(2));
                if *kind == DialogKind::Prompt { let x = x0 + text(px, w, font, x0, y, &t, WARN); line_at(px, w, font, x, y, l, cols.saturating_sub(t.chars().count() + 3)); }
                else { text(px, w, font, x0, y, &t, WARN); }
            }
            Mode::Upload { pick, .. } => { text(px, w, font, x0, y, &web::fit(&pick.text(), cols.saturating_sub(2)), WARN); }
            Mode::Normal => {
                let mut x = x0 + text(px, w, font, x0, y, "\u{2190}  \u{2192}  \u{21bb}  ", DIM);
                let left = cols.saturating_sub(x as usize / font.w + 2);
                let secure = if url.starts_with("https://") { ("  secure", OK) } else if url.starts_with("http://") { ("  not secure", DIM) } else { ("", DIM) };
                let right_cols = progress.map_or(0, |p| format!("{p}%").len());
                let (u, _, r) = web::strip(&url, "", progress, left.saturating_sub(secure.0.len()));
                x += text(px, w, font, x, y, &u, FG);
                if let Some(n) = self.note.clone() {
                    let col = if n.starts_with("download saved") || n.starts_with("downloading") { OK } else { ERR };
                    x += text(px, w, font, x, y, "  ", DIM); text(px, w, font, x, y, &web::fit(&printable(&n), left.saturating_sub(u.chars().count() + right_cols + 4)), col);
                } else if !secure.0.is_empty() { text(px, w, font, x, y, secure.0, secure.1); }
                if !r.is_empty() { text(px, w, font, w as i32 - (r.len() as i32 + 1) * font.w as i32, y, &r, ACCENT); }
            }
        }
        // the hint row under the page
        let (fy, fh) = ((ch - self.foot_h) as i32, self.foot_h as i32);
        fill(px, w, Rect { x: 0, y: fy, w: w as i32, h: fh }, PANEL);
        fill(px, w, Rect { x: 0, y: fy, w: w as i32, h: sc }, BORDER_DIM);
        let hint = [("ctrl+l", "address"), ("ctrl+t", "new tab"), ("ctrl+w", "close tab"), ("ctrl+tab", "next tab"), ("ctrl+f", "find"), ("ctrl+q", "quit")];
        let mut v: Vec<(&str, &str)> = hint[..5].to_vec();
        while !v.is_empty() && (thir::gui::draw::hint_cols(&[v.as_slice(), &hint[5..]].concat()) + 2) * font.w > w { v.pop(); }
        v.push(hint[5]);
        thir::gui::draw::hints(px, w, font, x0, fy + 4 * sc, &v);
        let (sh, fy) = (self.strip_h as u16, fy as u16);
        self.commit((0, 0, w.min(65535) as u16, sh)); self.commit((0, fy, w.min(65535) as u16, self.foot_h as u16));
    }
    /// A WebKit frame: copy the damaged parts under the strip and commit them.
    fn on_frame(&mut self, px: *const u8, stride: u32, fw: i32, fh: i32, rects: &[(i32, i32, i32, i32)]) {
        let (strip_h, foot_h) = (self.strip_h, self.foot_h); let Some(c) = self.c.as_mut() else { return };
        let (w, h) = (c.w as usize, c.h as usize); let dst = c.pixels(); if dst.is_empty() { return }
        let full = [(0, 0, fw, fh)];
        let mut out = vec![];
        for &(x, y, rw, rh) in if rects.is_empty() { &full[..] } else { rects } {
            let (x0, y0) = (x.max(0) as usize, y.max(0) as usize);
            let (x1, y1) = (((x + rw).max(0) as usize).min(fw as usize).min(w), ((y + rh).max(0) as usize).min(fh as usize).min(h.saturating_sub(strip_h + foot_h)));
            if x0 >= x1 || y0 >= y1 { continue }
            for row in y0..y1 {
                // SAFETY: the frame is fh rows of `stride` bytes, and x1 <= fw; the destination row is inside the pane's pixels (row + strip_h < h, x1 <= w)
                unsafe { ptr::copy_nonoverlapping(px.add(row * stride as usize + x0 * 4) as *const u32, dst.as_mut_ptr().add((row + strip_h) * w + x0), x1 - x0); }
            }
            out.push((x0 as u16, (y0 + strip_h) as u16, (x1 - x0) as u16, (y1 - y0) as u16));
        }
        self.pending.extend(out); self.frame_waiting = true; self.flush();
    }
    fn page_xy(&self, x: i32, y: i32) -> Option<(f64, f64)> {
        if y < self.strip_h as i32 || self.c.as_ref().is_some_and(|c| y >= c.h as i32 - self.foot_h as i32) { return None }
        Some((x as f64 / self.sc as f64, (y - self.strip_h as i32) as f64 / self.sc as f64))
    }
    fn load(&mut self, url: &str) { self.note = None; if let Ok(u) = CString::new(url) { unsafe { thirweb_load(u.as_ptr()) } } }
    fn download_path(&self, name: &str) -> PathBuf {
        let dir = web::dirs(&self.home)[2].clone();
        dir.join(web::download_name(name, |n| dir.join(n).symlink_metadata().is_ok()))
    }
    /// A tab became the current one (new, switched to, or its neighbour after a close): the strip, den's bar title and the page are the new tab's.
    fn switched(&mut self) {
        if matches!(self.mode, Mode::Find(_)) { unsafe { thirweb_find(ptr::null(), 3) } }
        self.mode = Mode::Normal; self.note = None; self.pending.clear();
        unsafe { thirweb_tab_show(self.tabs.current().id) }
        
        self.draw_strip();
    }
    fn new_tab(&mut self) {
        if self.tabs.items.len() >= web::MAX_TABS { self.note = Some(format!("no more than {} tabs", web::MAX_TABS)); self.draw_strip(); return }
        let id = unsafe { thirweb_tab_new(ptr::null()) }; self.tabs.open(TabState::new(id, ""));
        self.switched(); self.mode = Mode::Edit(Line::new(""));
        self.draw_strip();
    }
    /// Closes tab `i`; the last tab closing ends the browser.
    fn close_tab(&mut self, i: usize) {
        let Some(t) = self.tabs.close(i) else { unsafe { thirweb_quit() } return };
        unsafe { thirweb_tab_close(t.id) }
        self.switched();
    }
    fn answer_dialog(&mut self, ok: bool) {
        let Mode::Dialog { dlg, kind, line, .. } = std::mem::replace(&mut self.mode, Mode::Normal) else { return };
        let text = CString::new(line.as_string()).unwrap_or_default();
        unsafe { thirweb_dialog_answer(dlg, ok as i32, if kind == DialogKind::Prompt && ok { text.as_ptr() } else { ptr::null() }) }
        self.draw_strip();
    }
    fn key(&mut self, code: u32, value: u32, mods: u32) {
        let (mut sym, ctrl, shift) = (0u32, mods & MOD_CTRL != 0, mods & MOD_SHIFT != 0);
        let ch = unsafe { thirweb_keyinfo(code, value, &mut sym) };
        if value == 0 { if !self.consumed.remove(&code) { unsafe { thirweb_keysend(code, 0) } } return }
        if value == 2 && self.consumed.contains(&code) { return }   // the repeat of a key the strip took
        let mut eat = true;
        // tab keys work while the address, the find line or the page has the keyboard (not while a question is open)
        if ctrl && value == 1 && matches!(self.mode, Mode::Normal | Mode::Edit(_) | Mode::Find(_)) {
            match code {
                K_T => { self.new_tab(); self.consumed.insert(code); return }
                K_W => { let i = self.tabs.cur; self.close_tab(i); self.consumed.insert(code); return }
                K_TAB if !shift => { self.tabs.next(); self.switched(); self.consumed.insert(code); return }
                K_PGDN => { self.tabs.next(); self.switched(); self.consumed.insert(code); return }
                K_PGUP => { self.tabs.prev(); self.switched(); self.consumed.insert(code); return }
                K_TAB => { self.tabs.prev(); self.switched(); self.consumed.insert(code); return }
                _ => {}
            }
        }
        match &mut self.mode {
            Mode::Ask { dl, name, size } => {
                if value == 1 && (code == K_Y || code == K_N || code == K_ESC) {
                    let (dl, name) = (*dl, name.clone()); let _ = size; self.mode = Mode::Normal;
                    if code == K_Y {
                        let p = self.download_path(&name); let shown = p.file_name().map_or(name.clone(), |n| n.to_string_lossy().into_owned());
                        self.note = Some(format!("downloading {}", printable(&shown)));
                        let cp = CString::new(p.into_os_string().into_encoded_bytes()).unwrap_or_default(); unsafe { thirweb_download_answer(dl, cp.as_ptr()) }
                    } else { self.note = Some("download cancelled".into()); unsafe { thirweb_download_answer(dl, ptr::null()) } }
                    self.draw_strip();
                }
            }
            Mode::Dialog { kind, line, .. } => {
                let kind = *kind;
                if kind == DialogKind::Prompt {
                    match code {
                        K_ENTER if value == 1 => self.answer_dialog(true), K_ESC if value == 1 => self.answer_dialog(false),
                        K_BACKSPACE => line.backspace(), K_DELETE => line.delete(), K_LEFT => line.left(), K_RIGHT => line.right(), K_HOME => line.home(), K_END => line.end(),
                        K_A if ctrl => line.home(), K_E if ctrl => line.end(), K_U if ctrl => line.kill(),
                        _ if !ctrl && ch >= 0x20 => if let Some(c) = char::from_u32(ch) { line.insert(c) },
                        _ => {}
                    }
                    if matches!(self.mode, Mode::Dialog { .. }) { self.draw_strip(); }
                } else if value == 1 {
                    let k = match code { K_ENTER => Some(DKey::Enter), K_ESC => Some(DKey::Esc), _ => char::from_u32(ch).filter(|_| !ctrl).map(|c| DKey::Char(c.to_ascii_lowercase())) };
                    if let Some(ok) = k.and_then(|k| web::dialog_key(kind, k)) { self.answer_dialog(ok) }
                }
            }
            Mode::Upload { req, pick } => {
                if value >= 1 {
                    let mut done = None;
                    match code { K_UP => pick.up(), K_DOWN => pick.down(), K_ESC if value == 1 => done = Some(None), K_ENTER if value == 1 => done = Some(pick.chosen().map(String::from)), _ => {} }
                    let req = *req;
                    match done {
                        Some(name) => {
                            self.mode = Mode::Normal;
                            let path = name.map(|n| CString::new(web::dirs(&self.home)[2].join(n).into_os_string().into_encoded_bytes()).unwrap_or_default());
                            unsafe { thirweb_chooser_answer(req, path.as_ref().map_or(ptr::null(), |p| p.as_ptr())) }
                        }
                        None => {}
                    }
                    self.draw_strip();
                }
            }
            Mode::Find(f) => {
                let was = f.line.as_string();
                match code {
                    K_ENTER if value == 1 => { let dir = if shift { 2 } else { 1 }; if !f.line.text.is_empty() { unsafe { thirweb_find(c"x".as_ptr(), dir) } } }
                    K_ESC if value == 1 => { self.mode = Mode::Normal; unsafe { thirweb_find(ptr::null(), 3) } }
                    K_BACKSPACE => f.line.backspace(), K_DELETE => f.line.delete(), K_LEFT => f.line.left(), K_RIGHT => f.line.right(), K_HOME => f.line.home(), K_END => f.line.end(),
                    K_A if ctrl => f.line.home(), K_E if ctrl => f.line.end(), K_U if ctrl => f.line.kill(),
                    K_F if ctrl && value == 1 => {}
                    _ if !ctrl && ch >= 0x20 => if let Some(c) = char::from_u32(ch) { f.line.insert(c) },
                    _ => {}
                }
                if let Mode::Find(f) = &mut self.mode {
                    let now = f.line.as_string();
                    if now != was { f.matches = None; let t = CString::new(now.clone()).unwrap_or_default(); unsafe { thirweb_find(if now.is_empty() { ptr::null() } else { t.as_ptr() }, 0) } }
                }
                self.draw_strip();
            }
            Mode::Edit(l) => {
                match code {
                    K_ENTER if value == 1 => {
                        let typed = l.as_string(); self.mode = Mode::Normal;
                        match web::address(&typed) { Ok(u) => { self.tabs.current_mut().url = u.clone(); self.load(&u) } Err(e) => self.note = Some(e.into()) }
                    }
                    K_ESC if value == 1 => self.mode = Mode::Normal,
                    K_BACKSPACE => l.backspace(), K_DELETE => l.delete(), K_LEFT => l.left(), K_RIGHT => l.right(), K_HOME => l.home(), K_END => l.end(),
                    K_A if ctrl => l.home(), K_E if ctrl => l.end(), K_U if ctrl => l.kill(),
                    _ if !ctrl && ch >= 0x20 => if let Some(c) = char::from_u32(ch) { l.insert(c) },
                    _ => {}
                }
                self.draw_strip();
            }
            Mode::Normal => match code {
                K_L if ctrl && value == 1 => { self.mode = Mode::Edit(Line::new(&self.tab().url)); self.draw_strip(); }
                K_R if ctrl && value == 1 => { self.note = None; unsafe { thirweb_reload() } }
                K_F5 if value == 1 => { self.note = None; unsafe { thirweb_reload() } }
                K_Q if ctrl && value == 1 => unsafe { thirweb_quit() },
                K_LBRACE if ctrl && value == 1 => unsafe { thirweb_back() },
                K_RBRACE if ctrl && value == 1 => unsafe { thirweb_forward() },
                K_F if ctrl && value == 1 => { self.mode = Mode::Find(Find::default()); self.draw_strip(); }
                K_C if ctrl && value == 1 => unsafe { thirweb_edit(0) },
                K_X if ctrl && value == 1 => unsafe { thirweb_edit(1) },
                K_V if ctrl && value == 1 => unsafe { thirweb_edit(2) },
                K_ESC if value == 1 => { unsafe { thirweb_stop() }; eat = false; }
                _ => eat = false,
            },
        }
        if eat { self.consumed.insert(code); } else { unsafe { thirweb_keysend(code, value) } }
    }
    /// The mouse reaches the page (not while a question or the address line is open).
    fn live(&self) -> bool { matches!(self.mode, Mode::Normal | Mode::Find(_)) }
    fn on_den(&mut self) {
        loop {
            let ev = match self.c.as_mut().map(|c| c.poll(0)) { Some(Ok(Some(e))) => e, Some(Ok(None)) => break, _ => { unsafe { thirweb_quit() } return } };
            match ev {
                Down::Done => self.on_done(),
                Down::Configure { w, h, scale, focused } => {
                    unsafe { thirweb_focus(focused as i32) }
                    if (w, h) == self.attached && scale.max(1) == self.sc { continue }
                    if let Some(c) = self.c.as_mut() { if c.attach().is_err() { unsafe { thirweb_quit() } return } }   // den drops the old buffer on a new size
                    self.attached = (w, h); self.sc = scale.max(1); self.pending.clear(); self.inflight = false; self.frame_waiting = false;
                    if self.frame_sent { self.frame_sent = false; unsafe { thirweb_frame_done() } }
                    (self.strip_h, self.foot_h) = self.font.as_ref().map_or((0, 0), |f| strip_sizes(f, self.sc));
                    let (lw, lh) = self.page_size(); unsafe { thirweb_resize(lw, lh, self.sc as f64) }
                    self.draw_strip();
                }
                Down::Key { code, value, mods } => self.key(code, value, mods),
                Down::Pointer { x, y } => { self.ptr = (x, y); if let Some((px, py)) = self.page_xy(x, y) { if self.live() || self.btn_down { unsafe { thirweb_pointer(px, py) } } } }
                Down::Button { code, pressed } if code as u32 == BTN_SIDE || code as u32 == BTN_EXTRA => {   // the mouse's side buttons: back and forward
                    if pressed && matches!(self.mode, Mode::Normal) { if code as u32 == BTN_SIDE { unsafe { thirweb_back() } } else { unsafe { thirweb_forward() } } }
                }
                Down::Button { code, pressed } => {
                    let (x, y) = self.ptr;
                    if pressed { if let (Some((px, py)), true) = (self.page_xy(x, y), self.live()) { self.btn_down = true; unsafe { thirweb_button(code, 1, px, py) } } }
                    else if self.btn_down { self.btn_down = false; let (px, py) = self.page_xy(x, y).unwrap_or((0.0, 0.0)); unsafe { thirweb_button(code, 0, px, py) } }
                }
                Down::Scroll { dx, dy } => if let (Some((px, py)), true) = (self.page_xy(self.ptr.0, self.ptr.1), self.live()) { unsafe { thirweb_scroll(dx as f64, dy as f64, px, py) } },
                Down::Mic { .. } => {}
            }
        }
    }
}

extern "C" fn cb_frame(px: *const u8, stride: u32, w: i32, h: i32, rects: *const i32, n: u32, _: *mut c_void) {
    let r: Vec<(i32, i32, i32, i32)> = (0..n as usize).map(|i| unsafe { (*rects.add(4 * i), *rects.add(4 * i + 1), *rects.add(4 * i + 2), *rects.add(4 * i + 3)) }).collect();
    app().on_frame(px, stride, w, h, &r);
}
fn tab_of(id: i32) -> Option<usize> { app().tabs.items.iter().position(|t| t.id == id) }
extern "C" fn cb_title(id: i32, t: *const c_char, _: *mut c_void) {
    let a = app(); let Some(i) = tab_of(id) else { return }; a.tabs.items[i].title = printable(&cstr(t));
    if !a.headless { a.draw_strip(); }
}
extern "C" fn cb_uri(id: i32, u: *const c_char, _: *mut c_void) { let a = app(); let (u, Some(i)) = (cstr(u), tab_of(id)) else { return }; if !u.is_empty() { a.tabs.items[i].url = u; } a.draw_strip(); }
extern "C" fn cb_progress(id: i32, p: f64, _: *mut c_void) { let a = app(); let Some(i) = tab_of(id) else { return }; a.tabs.items[i].progress = (p < 1.0).then_some((p * 100.0) as u8); a.draw_strip(); }
extern "C" fn cb_load(id: i32, e: i32, _: *mut c_void) {
    let a = app(); let Some(i) = tab_of(id) else { return };
    if e == 0 && i == a.tabs.cur { if a.keep_note { a.keep_note = false } else { a.note = None } }   // the error page's own load keeps the error in the strip
    if e == 3 { a.tabs.items[i].progress = None; if a.headless && !a.failed { unsafe { thirweb_timeout(300, finish_dump, ptr::null_mut()) } } }
    a.draw_strip();
}
extern "C" fn finish_dump(_: *mut c_void) { let a = app(); if !a.failed { println!("{}", a.tab().title); unsafe { thirweb_quit() } } }
extern "C" fn cb_failed(id: i32, m: *const c_char, _: *mut c_void) {
    let a = app(); let m = printable(&cstr(m));
    if a.headless { if !m.starts_with("download") { eprintln!("web: {m}"); a.failed = true; unsafe { thirweb_quit() } } return }
    if id != 0 && tab_of(id) != Some(a.tabs.cur) { return }   // a background tab's error is not the current page's
    a.keep_note = !m.starts_with("download"); a.note = Some(m); a.draw_strip();
}
extern "C" fn cb_download(dl: *mut c_void, name: *const c_char, size: i64, _: *mut c_void) {
    let a = app();
    if !matches!(a.mode, Mode::Normal) || a.headless { unsafe { thirweb_download_answer(dl, ptr::null()) } return }   // one question at a time; headless: no downloads
    a.mode = Mode::Ask { dl, name: cstr(name), size: (size >= 0).then_some(size as u64) }; a.draw_strip();
}
extern "C" fn cb_popup_ok(_: *mut c_void) -> i32 {   // target=_blank, window.open: a tab, unless there are too many already
    let a = app(); if a.headless { return 0 }
    if a.tabs.items.len() >= web::MAX_TABS { a.note = Some(format!("a page wanted a new tab: no more than {} tabs", web::MAX_TABS)); a.draw_strip(); return 0 }
    1
}
extern "C" fn cb_opened(id: i32, _: *mut c_void) {
    let a = app(); a.tabs.open(TabState::new(id, ""));   // shown in a moment: not inside WebKit's `create`
    unsafe { thirweb_timeout(1, show_opened, id as usize as *mut c_void) }
}
extern "C" fn show_opened(id: *mut c_void) { let a = app(); if let Some(i) = tab_of(id as usize as i32) { a.tabs.cur = i; a.switched(); } }
extern "C" fn cb_closed(id: i32, _: *mut c_void) { if let Some(i) = tab_of(id) { app().close_tab(i); } }   // window.close(); the last tab closing ends the browser
extern "C" fn cb_dialog(id: i32, kind: i32, msg: *const c_char, dflt: *const c_char, dlg: *mut c_void, _: *mut c_void) {
    let a = app();
    if a.headless || tab_of(id) != Some(a.tabs.cur) || !matches!(a.mode, Mode::Normal) { unsafe { thirweb_dialog_answer(dlg, 0, ptr::null()) } return }   // never blocks: a dialog nobody can answer now is cancelled
    a.mode = Mode::Dialog { dlg, kind: DialogKind::from_code(kind), msg: cstr(msg), line: Line::new(&printable(&cstr(dflt))) }; a.draw_strip();
}
extern "C" fn cb_chooser(id: i32, req: *mut c_void, _: *mut c_void) {
    let a = app();
    if a.headless || tab_of(id) != Some(a.tabs.cur) || !matches!(a.mode, Mode::Normal) { unsafe { thirweb_chooser_answer(req, ptr::null()) } return }
    a.mode = Mode::Upload { req, pick: Picker { names: web::list_uploads(&web::dirs(&a.home)[2]), at: 0 } }; a.draw_strip();
}
extern "C" fn cb_found(n: i32, _: *mut c_void) { let a = app(); if let Mode::Find(f) = &mut a.mode { f.matches = Some(n.max(0) as u32); a.draw_strip(); } }
extern "C" fn den_ready(_: *mut c_void) { app().on_den(); }
extern "C" fn timeout_quit(_: *mut c_void) { eprintln!("web: the page did not finish loading in time"); app().failed = true; unsafe { thirweb_quit() } }

/// /web/etc/web.env (KEY=VALUE lines of the add-on: software rendering, surfaceless EGL) into this process's environment, which the WebKit helpers inherit. Variables the user set win.
fn load_env(home: &Path) {
    if let Ok(s) = std::fs::read_to_string("/web/etc/web.env") { for l in s.lines() { if let Some((k, v)) = l.split_once('=') { if std::env::var_os(k).is_none() { std::env::set_var(k, v); } } } }
    if std::env::var_os("TZ").is_none() { std::env::set_var("TZ", web::tz_env(thir::clock::home_tz().unwrap_or(0))); }   // JavaScript's dates follow thir's ~/.tz
    std::env::set_var("XDG_CACHE_HOME", home.join(".cache/thir-web")); std::env::set_var("MESA_SHADER_CACHE_DISABLE", "true");
}
/// The browser's folders, made 0700 before the sandbox closes.
fn make_dirs(home: &Path) -> std::io::Result<()> {
    for d in web::dirs(home) { std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&d)?; }
    Ok(())
}
fn lock_in(home: &Path) -> Result<(), String> {
    match sandbox::build(&web::profile(Proc::Ui, home)).enforce() { Ok(true) => Ok(()), Ok(false) => Err("this kernel has no Landlock".into()), Err(e) => Err(e.to_string()) }
}

/// `web --check`: the sandbox self-test. This process is locked in like the browser, then the two helper shims run their own probes.
fn check(home: &Path) -> i32 {
    let _ = std::fs::DirBuilder::new().recursive(true).mode(0o700).create(home.join(".config/thir"));   // so that "denied" cannot be "not there"
    if let Err(e) = make_dirs(home) { eprintln!("web: {e}"); return 1 }
    if let Err(e) = lock_in(home) { println!("FAIL: {e}"); return 1 }
    let mut bad = 0;
    let mut t = |what: &str, ok: bool| { println!("{} {what}", if ok { "ok:  " } else { "FAIL:" }); if !ok { bad += 1 } };
    t("browser: ~/.config/thir is denied", std::fs::read_dir(home.join(".config/thir")).err().and_then(|e| e.raw_os_error()) == Some(libc::EACCES));
    let f = home.join("Downloads/.thir-check"); let w = std::fs::write(&f, "x"); let _ = std::fs::remove_file(&f);
    t("browser: ~/Downloads is writable", w.is_ok());
    for name in ["WPEWebProcess", "WPENetworkProcess"] {
        let p = format!("/usr/libexec/wpe-webkit-2.0/{name}");
        match std::process::Command::new(&p).arg("--thir-check").output() {
            Ok(o) => { print!("{}", String::from_utf8_lossy(&o.stdout)); eprint!("{}", String::from_utf8_lossy(&o.stderr)); if !o.status.success() { bad += 1 } }
            Err(e) => { println!("FAIL: cannot run {p}: {e}"); bad += 1 }
        }
    }
    println!("{}", if bad == 0 { "sandbox ok" } else { "sandbox FAILED" }); (bad != 0) as i32
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let usage = || { eprintln!("usage: web [URL]   (http or https only)\n       web --dump-title URL   web --check"); 2 };
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from).filter(|h| h.is_absolute()) else { eprintln!("web: no home folder"); std::process::exit(1) };
    let (dump, url) = match args.as_slice() {
        [] => (false, None), [a] if a == "--check" => { load_env(&home); std::process::exit(check(&home)) }
        [a, u] if a == "--dump-title" => (true, Some(u.clone())), [u] if !u.starts_with("--") => (false, Some(u.clone())), _ => std::process::exit(usage()),
    };
    if let Some(u) = &url { if !web::url_ok(u) { eprintln!("web: only http and https addresses are opened (not {})", printable(u)); std::process::exit(2) } }
    // den first (before the sandbox hides /run), then the sandbox
    let client = if dump { None } else { match Client::connect() { Ok(c) => Some(c), Err(e) => { eprintln!("web needs the desktop: run it in a den pane ({e})"); std::process::exit(1) } } };
    let font = client.as_ref().and_then(|c| std::fs::read(if c.scale >= 2 { "/share/font32.psf" } else { "/share/font16.psf" }).ok().and_then(|b| Font::parse(&b).ok()));
    if !dump && font.is_none() { eprintln!("web: no font"); std::process::exit(1) }
    load_env(&home);
    if !dump { if let Err(e) = make_dirs(&home) { eprintln!("web: {}: {e}", home.display()); std::process::exit(1) } }
    if let Err(e) = lock_in(&home) { eprintln!("web: the sandbox cannot be set up ({e}); not starting"); std::process::exit(1) }
    let sc = client.as_ref().map_or(1, |c| c.scale.max(1));
    let (strip_h, foot_h) = font.as_ref().map_or((0, 0), |f| strip_sizes(f, sc));
    let mut a = App { c: client, font, sc, strip_h, foot_h, headless: dump, tabs: Tabs::new(TabState::new(0, &url.clone().unwrap_or_default())), note: None, mode: Mode::Normal, home: home.clone(),
        pending: vec![], inflight: false, frame_waiting: false, frame_sent: false, attached: (0, 0), keep_note: false, consumed: HashSet::new(), ptr: (0, 0), btn_down: false, failed: false };
    if let Some(c) = a.c.as_mut() { if c.attach().is_err() { eprintln!("web: no surface"); std::process::exit(1) } a.attached = (c.w, c.h); }
    unsafe { APP = &mut a; }
    let (lw, lh) = a.page_size();
    let [data, cache, _] = web::dirs(&home);
    let (dc, cc) = (CString::new(data.into_os_string().into_encoded_bytes()).unwrap(), CString::new(cache.into_os_string().into_encoded_bytes()).unwrap());
    let cb = ThirCb { ud: ptr::null_mut(), frame: if dump { None } else { Some(cb_frame) }, title: cb_title, uri: cb_uri, progress: cb_progress, load: cb_load, failed: cb_failed, download: cb_download,
        popup_ok: cb_popup_ok, opened: cb_opened, closed: cb_closed, dialog: cb_dialog, chooser: cb_chooser, found: cb_found };
    if unsafe { thirweb_init(if dump { 1024 } else { lw }, if dump { 768 } else { lh }, sc as f64, if dump { ptr::null() } else { dc.as_ptr() }, if dump { ptr::null() } else { cc.as_ptr() }, &cb) } != 0 { std::process::exit(1) }
    if dump { unsafe { thirweb_timeout(90_000, timeout_quit, ptr::null_mut()) } }
    if let Some(c) = a.c.as_ref() { unsafe { thirweb_watch_fd(c.raw_fd(), den_ready, ptr::null_mut()) } }
    let id = unsafe { thirweb_tab_new(ptr::null()) }; a.tabs.current_mut().id = id; unsafe { thirweb_tab_show(id) }
    match &url {
        Some(u) => a.load(u),
        None => { a.mode = Mode::Edit(Line::new("")); }   // no address: type one
    }
    if !a.headless { let mut o = std::io::stdout(); let _ = write!(o, "\x1b]2;browser\x07"); let _ = o.flush(); }   // the pane's name in den
    a.draw_strip();
    unsafe { thirweb_run() }
    let code = if a.failed { 1 } else { 0 };
    drop(a); if !dump { print!("\x1b]2;\x07"); let _ = std::io::stdout().flush(); }
    std::process::exit(code)
}
