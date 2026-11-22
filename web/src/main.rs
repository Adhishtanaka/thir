//! thir-web (`web`): the browser. WPE WebKit (platform.c) draws pages into shared-memory frames; this program copies the damaged parts into the pane's surface (the protocol of
//! gui/surface.rs), draws a one-line strip (address, title, progress) in thir's font, and turns den's keys, pointer and wheel into WPE events.
//! Order matters for safety: arguments are checked, den is connected FIRST, and only then Landlock closes the file system around this process (web::profile); the WebKit helpers
//! start through the wpe-shim, which locks each of them in further.
use std::{collections::HashSet, ffi::{c_char, c_void, CStr, CString}, io::Write, os::unix::fs::DirBuilderExt, path::{Path, PathBuf}, ptr};
use std::{os::unix::fs::OpenOptionsExt, time::Instant};
use thir::{files::printable, gui::{draw::{fill, glyph, text}, font::Font, layout::Rect, surface::{Client, Down, Ev, Up, R16, MOD_CTRL, MOD_SHIFT}, theme::*}, sandbox, web::{self, DKey, Dl, DlState, DialogKind, Find, History, Line, Picker, Proc, Tabs}};

#[repr(C)]
struct ThirCb {
    ud: *mut c_void,
    frame: Option<extern "C" fn(*const u8, u32, i32, i32, *const i32, u32, *mut c_void)>,
    title: extern "C" fn(i32, *const c_char, *mut c_void), uri: extern "C" fn(i32, *const c_char, *mut c_void), progress: extern "C" fn(i32, f64, *mut c_void),
    load: extern "C" fn(i32, i32, *mut c_void), failed: extern "C" fn(i32, *const c_char, *mut c_void), download: extern "C" fn(*mut c_void, *const c_char, i64, *const c_char, i32, *mut c_void),
    popup_ok: extern "C" fn(*mut c_void) -> i32, opened: extern "C" fn(i32, *mut c_void), closed: extern "C" fn(i32, *mut c_void),
    dialog: extern "C" fn(i32, i32, *const c_char, *const c_char, *mut c_void, *mut c_void), chooser: extern "C" fn(i32, *mut c_void, *mut c_void), found: extern "C" fn(i32, *mut c_void),
    option_menu: extern "C" fn(i32, *mut c_void, i32, i32, i32, i32, *mut c_void), dl_event: extern "C" fn(i32, u32, i64, *const c_char, *mut c_void),
}
extern "C" {
    fn thirweb_init(w: i32, h: i32, scale: f64, data: *const c_char, cache: *const c_char, cb: *const ThirCb) -> i32;
    fn thirweb_tab_new(uri: *const c_char) -> i32; fn thirweb_tab_show(id: i32); fn thirweb_tab_close(id: i32);
    fn thirweb_back(); fn thirweb_forward(); fn thirweb_edit(cmd: i32); fn thirweb_find(text: *const c_char, dir: i32);
    fn thirweb_dialog_answer(dlg: *mut c_void, ok: i32, text: *const c_char); fn thirweb_chooser_answer(req: *mut c_void, path: *const c_char);
    fn thirweb_load(uri: *const c_char); fn thirweb_reload(); fn thirweb_stop(); fn thirweb_resize(w: i32, h: i32, scale: f64); fn thirweb_focus(on: i32);
    fn thirweb_frame_done(); fn thirweb_download_answer(dl: *mut c_void, path: *const c_char, id: u32); fn thirweb_download_cancel(id: u32); fn thirweb_download_resume(id: u32, url: *const c_char, from: i64, path: *const c_char);
    fn thirweb_menu_count(m: *mut c_void) -> i32; fn thirweb_menu_item(m: *mut c_void, i: i32, label: *mut *const c_char, flags: *mut i32); fn thirweb_menu_answer(m: *mut c_void, index: i32); fn thirweb_repaint();
    fn thirweb_clip_set(text: *const c_char); fn thirweb_clip_get() -> *mut c_char; fn g_free(p: *mut c_void);
    fn thirweb_keyinfo(code: u32, value: u32, sym: *mut u32) -> u32; fn thirweb_keysend(code: u32, value: u32);
    fn thirweb_pointer(x: f64, y: f64); fn thirweb_button(code: u32, pressed: i32, x: f64, y: f64); fn thirweb_scroll(dx: f64, dy: f64, x: f64, y: f64);
    fn thirweb_watch_fd(fd: i32, f: extern "C" fn(*mut c_void), d: *mut c_void); fn thirweb_timeout(ms: u32, f: extern "C" fn(*mut c_void), d: *mut c_void);
    fn thirweb_run(); fn thirweb_quit();
}

const K_ESC: u32 = 1; const K_ENTER: u32 = 28; const K_DELETE: u32 = 111; const K_RIGHT: u32 = 106; const K_HOME: u32 = 102; const K_END: u32 = 107;
const K_T: u32 = 20; const K_W: u32 = 17; const K_TAB: u32 = 15; const K_PGUP: u32 = 104; const K_PGDN: u32 = 109; const K_LBRACE: u32 = 26; const K_RBRACE: u32 = 27; const K_F: u32 = 33;
const K_C: u32 = 46; const K_X: u32 = 45; const K_V: u32 = 47; const K_UP: u32 = 103; const K_DOWN: u32 = 108; const BTN_SIDE: u32 = 0x113; const BTN_EXTRA: u32 = 0x114;
const K_D: u32 = 32; const K_J: u32 = 36; const K_H: u32 = 35; const K_P: u32 = 25; const K_K: u32 = 37; const K_A: u32 = 30; const K_L: u32 = 38; const K_R: u32 = 19; const K_Q: u32 = 16; const K_Y: u32 = 21; const K_N: u32 = 49; const K_F5: u32 = 63;

enum Mode {
    Normal, Edit(Line), Ask { dl: *mut c_void, name: String, size: Option<u64>, url: String, ranges: bool },
    Menu { menu: *mut c_void, m: web::Menu, anchor: (i32, i32, i32, i32) }, Downloads { at: usize }, History { q: Line, at: usize, confirm: bool },
    Find(Find), Dialog { dlg: *mut c_void, kind: DialogKind, msg: String, line: Line }, Upload { req: *mut c_void, pick: Picker },
}
/// What the program keeps per tab (WebKit keeps the page).
struct TabState { id: i32, url: String, title: String, progress: Option<u8> }
impl TabState { fn new(id: i32, url: &str) -> TabState { TabState { id, url: url.into(), title: String::new(), progress: None } } }

/// Where a download's bytes are: `part` (name.part, what the user sees growing), `seg` (the piece WebKit writes now; the same file as `part` for the first piece), `fin` (the final name).
struct DlFiles { part: PathBuf, seg: PathBuf, fin: PathBuf }
struct App {
    hist: History, hist_path: PathBuf, dls: Vec<(Dl, DlFiles)>, next_dl: u32, t0: Instant, off: i32,
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

/// A line editor's characters, selection (accent fill, panel text) and cursor at (x, y), at most `room` columns; `sugg` is the grey completion after the text.
fn line_at(px: &mut [u32], w: usize, font: &Font, x: i32, y: i32, l: &Line, room: usize, sugg: &str) {
    let start = (l.at + 1).saturating_sub(room); let sel = l.selection();
    for (k, ch) in l.text.iter().enumerate().skip(start).take(room) {
        let cx = x + ((k - start) * font.w) as i32;
        if sel.is_some_and(|(a, b)| k >= a && k < b) { fill(px, w, Rect { x: cx, y, w: font.w as i32, h: font.h as i32 }, accent()); glyph(px, w, font, cx, y, *ch, panel()); } else { glyph(px, w, font, cx, y, *ch, FG); }
    }
    let cx = x + ((l.at - start) * font.w) as i32;
    if l.at >= l.text.len() { for (k, ch) in sugg.chars().take(room.saturating_sub(l.at - start + 1)).enumerate() { glyph(px, w, font, cx + ((k + 1) * font.w) as i32, y, ch, DIM); } }
    if sel.is_none() {
        fill(px, w, Rect { x: cx, y, w: font.w as i32, h: font.h as i32 }, accent());
        if let Some(ch) = l.text.get(l.at) { glyph(px, w, font, cx, y, *ch, panel()); }
    }
}
/// The clipboard shared by the address line, the find line, the prompts and the page (WPE's, in this process).
fn clip_get() -> String { let p = unsafe { thirweb_clip_get() }; if p.is_null() { return String::new() } let s = cstr(p); unsafe { g_free(p as *mut c_void) } s }
/// WPE's clipboard only (what den just gave us); `clip_set` also tells den, so that a copy here can be pasted in a terminal pane.
fn clip_local(t: &str) { if let Ok(c) = CString::new(t) { unsafe { thirweb_clip_set(c.as_ptr()) } } }
fn clip_set(t: &str) { clip_local(t); clip_to_den(t) }
fn clip_to_den(t: &str) { if let Some(c) = app().c.as_ref() { let _ = c.send(&Up::ClipSet(thir::gui::clip::cap(t).to_string())); } }
/// A page copy ends in WPE's clipboard some moments after the key: look, and tell den.
extern "C" fn push_clip(_: *mut c_void) { let t = clip_get(); if !t.is_empty() { clip_to_den(&t) } }
/// A key in a line editor (address, find, prompt, history search). Returns true when it was a line key and the text changed.
fn edit_line(l: &mut Line, code: u32, ctrl: bool, shift: bool, ch: u32) -> bool {
    if let Some(k) = web::line_key(code, ctrl) {
        let mut clip = if k == web::LineKey::Paste { clip_get() } else { String::new() };
        let changed = web::line_apply(l, k, ctrl, shift, &mut clip);
        if matches!(k, web::LineKey::Copy | web::LineKey::Cut) && !clip.is_empty() { clip_set(&clip) }
        return changed
    }
    if !ctrl && ch >= 0x20 { if let Some(c) = char::from_u32(ch) { l.insert(c); return true } }
    false
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
        let sug = match &self.mode { Mode::Edit(l) if l.at == l.text.len() && l.sel.is_none() => self.hist.suggest(&l.as_string()).unwrap_or_default(), _ => String::new() };
        let ndl = self.dls.iter().filter(|d| d.0.active()).count();
        let (Some(c), Some(font)) = (self.c.as_mut(), self.font.as_ref()) else { return };
        let (w, sc, ch) = (c.w as usize, self.sc as i32, c.h as usize);
        let (strip_h, cols) = (self.strip_h as i32, c.w as usize / font.w);
        let (y1, x0) = (3 * sc, font.w as i32);
        let y = y1 + font.h as i32 + 2 * sc;   // line 2: the arrows, the address and whatever is being asked
        let px = c.pixels(); if px.is_empty() { return }
        fill(px, w, Rect { x: 0, y: 0, w: w as i32, h: strip_h }, panel());
        fill(px, w, Rect { x: 0, y: strip_h - sc, w: w as i32, h: sc }, BORDER_DIM);
        let pr = progress.map_or(0, |p| p as i32 * w as i32 / 100);
        fill(px, w, Rect { x: 0, y: strip_h - 2 * sc, w: pr, h: 2 * sc }, accent());
        // line 1: the tabs, the active one as `[1 thir.dev]` in peach, the others dim
        { let mut x = x0;
          for (k, (t, on)) in web::strip_tabs(&labels, cur, cols.saturating_sub(6)).iter().enumerate() {
              if k > 0 { x += 2 * font.w as i32; }
              x += if *on { text(px, w, font, x, y1, &format!("[{t}]"), accent()) } else { text(px, w, font, x, y1, t, DIM) };
          }
          if ntabs < web::MAX_TABS { text(px, w, font, x + 2 * font.w as i32, y1, "+", DIM); } }
        // a line being edited (address, find, prompt): the label, the characters, a block cursor
        let line = |px: &mut [u32], label: &str, l: &Line, room: usize, sugg: &str| { let x = x0 + text(px, w, font, x0, y, label, DIM); line_at(px, w, font, x, y, l, room.saturating_sub(label.chars().count() + 3), sugg); };
        match &self.mode {
            Mode::Ask { name, size, .. } => { text(px, w, font, x0, y, &web::fit(&web::download_prompt(name, *size), cols.saturating_sub(2)), WARN); }
            Mode::Edit(l) => line(px, "go ", l, cols, &sug),
            Mode::Find(f) => {
                let st = web::fit(&f.status(), 14); line(px, "find ", &f.line, cols.saturating_sub(st.chars().count() + 1), "");
                if !st.is_empty() { text(px, w, font, w as i32 - (st.chars().count() as i32 + 1) * font.w as i32, y, &st, if f.matches == Some(0) { ERR } else { accent() }); }
            }
            Mode::Dialog { kind, msg, line: l, .. } => {
                let t = web::fit(&web::dialog_text(*kind, msg), cols.saturating_sub(2));
                if *kind == DialogKind::Prompt { let x = x0 + text(px, w, font, x0, y, &t, WARN); line_at(px, w, font, x, y, l, cols.saturating_sub(t.chars().count() + 3), ""); }
                else { text(px, w, font, x0, y, &t, WARN); }
            }
            Mode::Menu { m, .. } => { text(px, w, font, x0, y, &web::fit(&format!("choose: {}  ({}/{})", m.items.get(m.at).map_or("", |i| i.0.as_str()), m.at + 1, m.items.len()), cols.saturating_sub(2)), WARN); }
            Mode::Downloads { .. } => { text(px, w, font, x0, y, &format!("downloads  {}", if ndl > 0 { format!("{ndl} running") } else { "none running".into() }), accent()); }
            Mode::History { q, confirm, .. } => {
                if *confirm { text(px, w, font, x0, y, "clear all history? y/n", WARN); } else { let x = x0 + text(px, w, font, x0, y, "history ", accent()); line_at(px, w, font, x, y, q, cols.saturating_sub(12), ""); }
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
                if !r.is_empty() { text(px, w, font, w as i32 - (r.len() as i32 + 1) * font.w as i32, y, &r, accent()); }
                else if ndl > 0 { let t = format!("{ndl} downloading"); text(px, w, font, w as i32 - (t.len() as i32 + 1) * font.w as i32, y, &t, accent()); }
            }
        }
        // the hint row under the page
        let (fy, fh) = ((ch - self.foot_h) as i32, self.foot_h as i32);
        fill(px, w, Rect { x: 0, y: fy, w: w as i32, h: fh }, panel());
        fill(px, w, Rect { x: 0, y: fy, w: w as i32, h: sc }, BORDER_DIM);
        let hint: Vec<(&str, &str)> = match &self.mode {
            Mode::Downloads { .. } => vec![("up/down", "choose"), ("p", "pause or resume"), ("c", "cancel"), ("x", "remove"), ("esc", "back")],
            Mode::History { confirm: true, .. } => vec![("y", "clear it"), ("n", "keep it")],
            Mode::History { .. } => vec![("type", "search"), ("enter", "open"), ("delete", "remove one"), ("ctrl+k", "clear all"), ("esc", "back")],
            Mode::Menu { .. } => vec![("up/down", "choose"), ("enter", "select"), ("esc", "close")],
            _ => vec![("ctrl+l", "address"), ("ctrl+t", "new tab"), ("ctrl+w", "close tab"), ("ctrl+f", "find"), ("ctrl+j", "downloads"), ("ctrl+h", "history"), ("ctrl+q", "quit")],
        };
        let last = hint[hint.len() - 1]; let mut v: Vec<(&str, &str)> = hint[..hint.len() - 1].to_vec();
        while !v.is_empty() && (thir::gui::draw::hint_cols(&[v.as_slice(), &[last]].concat()) + 2) * font.w > w { v.pop(); }
        v.push(last);
        thir::gui::draw::hints(px, w, font, x0, fy + 4 * sc, &v);
        let (sh, fy) = (self.strip_h as u16, fy as u16);
        self.commit((0, 0, w.min(65535) as u16, sh)); self.commit((0, fy, w.min(65535) as u16, self.foot_h as u16));
        self.draw_body();
    }

    // ---- views over the page: downloads, history; and the <select> popup ----
    fn covered(&self) -> bool { matches!(self.mode, Mode::Downloads { .. } | Mode::History { .. }) }
    /// The pixel area between the strip and the hint row: (y, height).
    fn body(&self) -> (i32, i32) { let h = self.c.as_ref().map_or(0, |c| c.h as i32); (self.strip_h as i32, (h - self.strip_h as i32 - self.foot_h as i32).max(0)) }
    fn rh(&self) -> i32 { self.font.as_ref().map_or(16, |f| f.h as i32) + 4 * self.sc as i32 }
    fn draw_body(&mut self) {
        match &self.mode { Mode::Downloads { .. } => self.draw_downloads(), Mode::History { .. } => self.draw_history(), Mode::Menu { .. } => self.draw_menu(), _ => {} }
    }
    /// Fills the body with the panel colour and returns what a view needs to draw rows.
    fn view_start(&mut self) -> Option<(usize, Font)> {
        let (by, bh) = self.body(); self.font.as_ref()?; let c = self.c.as_mut()?;
        let w = c.w as usize; let px = c.pixels(); if px.is_empty() { return None }
        fill(px, w, Rect { x: 0, y: by, w: w as i32, h: bh }, panel());
        Some((w, self.font.take()?))   // the caller gives the font back
    }
    fn draw_downloads(&mut self) {
        let Mode::Downloads { at } = self.mode else { return };
        let rows: Vec<(String, String, String, Option<u8>, DlState)> = self.dls.iter().rev().map(|(d, _)| (d.name.clone(), d.status_text(), format!("{} of {}", web::size_text(Some(d.got())), web::size_text(d.total)), d.percent(), d.state.clone())).collect();
        let path = rows.get(at).map(|_| format!("saved in ~/Downloads/{}", self.dls[self.dls.len() - 1 - at].0.name));
        let (by, bh) = self.body(); let (rh, sc) = (self.rh(), self.sc as i32);
        let Some((w, font)) = self.view_start() else { return };
        let entry = 2 * rh + 6 * sc; let x0 = font.w as i32; let cols = w / font.w;
        let vis = ((bh - 2 * rh) / entry).max(1) as usize; let first = (at + 1).saturating_sub(vis).min(rows.len().saturating_sub(vis));
        let Some(c) = self.c.as_mut() else { return }; let px = c.pixels();   // (view_start saw it)
        if rows.is_empty() {
            text(px, w, &font, x0, by + rh / 2, "no downloads yet", DIM); text(px, w, &font, x0, by + rh / 2 + rh, "a file a page offers is saved in ~/Downloads after you answer y", DIM);
        }
        for (k, (name, status, sizes, pct, st)) in rows.iter().enumerate().skip(first).take(vis) {
            let y = by + rh / 2 + ((k - first) as i32) * entry;
            if k == at { fill(px, w, Rect { x: 0, y, w: w as i32, h: entry - sc }, SEL); fill(px, w, Rect { x: 0, y, w: 3 * sc, h: entry - sc }, accent()); }
            let col = match st { DlState::Done => OK, DlState::Failed(_) | DlState::Cancelled => ERR, DlState::Paused => WARN, _ => DIM };
            let sw = status.chars().count().min(cols / 2); let st_ = web::fit(status, sw);
            text(px, w, &font, x0, y + 2 * sc, &web::fit(&printable(name), cols.saturating_sub(sw + 4)), FG);
            text(px, w, &font, w as i32 - (sw as i32 + 1) * font.w as i32, y + 2 * sc, &st_, col);
            let y2 = y + 2 * sc + rh; let right = format!("{sizes}  {}", pct.map_or(String::new(), |p| format!("{p}%")));
            text(px, w, &font, w as i32 - (right.chars().count() as i32 + 1) * font.w as i32, y2, &right, DIM);
            let bw = (w as i32 - 2 * x0 - (right.chars().count() as i32 + 2) * font.w as i32).max(font.w as i32);
            let cy = y2 + font.h as i32 / 2; fill(px, w, Rect { x: x0, y: cy, w: bw, h: sc }, BORDER_DIM);
            let filled = bw * web::bar_cells(*pct, 100) as i32 / 100; fill(px, w, Rect { x: x0, y: cy - sc, w: filled, h: 3 * sc }, if matches!(st, DlState::Done) { OK } else { accent() });
        }
        if let Some(p) = path { text(px, w, &font, x0, by + bh - rh, &web::fit(&p, cols.saturating_sub(2)), DIM); }
        self.font = Some(font); self.commit((0, by as u16, w.min(65535) as u16, bh as u16));
    }
    fn history_rows(&self) -> Vec<(String, String, String)> {
        let Mode::History { q, .. } = &self.mode else { return vec![] }; let now = thir::clock::now();
        self.hist.search(&q.as_string()).iter().map(|v| (web::visit_time(v.t, now, self.off), v.title.clone(), v.url.clone())).collect()
    }
    fn draw_history(&mut self) {
        let Mode::History { at, .. } = self.mode else { return };
        let rows = self.history_rows(); let total = self.hist.items.len();
        let (by, bh) = self.body(); let (rh, sc) = (self.rh(), self.sc as i32);
        let Some((w, font)) = self.view_start() else { return };
        let x0 = font.w as i32; let cols = w / font.w; let tcol = 14usize; let tw = (cols.saturating_sub(tcol + 4) * 2 / 5).max(8);
        let Some(c) = self.c.as_mut() else { return }; let px = c.pixels();
        let y0 = by + rh / 2;
        text(px, w, &font, x0, y0, "when", DIM); text(px, w, &font, x0 + (tcol * font.w) as i32, y0, "title", DIM); text(px, w, &font, x0 + ((tcol + tw + 2) * font.w) as i32, y0, "address", DIM);
        let cnt = format!("{} of {} pages", rows.len(), total); text(px, w, &font, w as i32 - (cnt.chars().count() as i32 + 1) * font.w as i32, y0, &cnt, DIM);
        fill(px, w, Rect { x: 0, y: y0 + rh, w: w as i32, h: sc }, BORDER_DIM);
        if rows.is_empty() { text(px, w, &font, x0, y0 + 2 * rh, if total == 0 { "no pages yet: the pages you open are listed here" } else { "no page matches" }, DIM); }
        let vis = ((bh - 3 * rh) / rh).max(1) as usize; let first = (at + 1).saturating_sub(vis).min(rows.len().saturating_sub(vis));
        for (k, (t, title, url)) in rows.iter().enumerate().skip(first).take(vis) {
            let y = y0 + 2 * rh + ((k - first) as i32) * rh;
            if k == at { fill(px, w, Rect { x: 0, y: y - 2 * sc, w: w as i32, h: rh }, SEL); fill(px, w, Rect { x: 0, y: y - 2 * sc, w: 3 * sc, h: rh }, accent()); }
            text(px, w, &font, x0, y, t, DIM);
            let tt = if title.is_empty() { "(no title)".to_string() } else { title.clone() };
            text(px, w, &font, x0 + (tcol * font.w) as i32, y, &web::fit(&printable(&tt), tw), FG);
            text(px, w, &font, x0 + ((tcol + tw + 2) * font.w) as i32, y, &web::fit(&printable(url), cols.saturating_sub(tcol + tw + 4)), DIM);
        }
        self.font = Some(font); self.commit((0, by as u16, w.min(65535) as u16, bh as u16));
    }
    /// The popup's box in pixels: (x, y, w, h, first row, rows).
    fn menu_geom(&self) -> Option<(i32, i32, i32, i32, usize, usize)> {
        let Mode::Menu { m, anchor, .. } = &self.mode else { return None }; let (c, font) = (self.c.as_ref()?, self.font.as_ref()?);
        let (by, bh) = self.body(); let (sc, rh) = (self.sc as i32, self.rh()); let w = c.w as i32;
        let (first, n) = m.window((((bh - 4 * sc) / rh).max(1) as usize).min(12));
        let widest = m.items.iter().map(|i| printable(&i.0).chars().count()).max().unwrap_or(4);
        let bw = (((widest + 4) * font.w) as i32).max(anchor.2 * sc).max(12 * font.w as i32).min(w - 2 * sc); let bhh = n as i32 * rh + 2 * sc;
        let x = (anchor.0 * sc).clamp(0, (w - bw).max(0)); let below = by + (anchor.1 + anchor.3) * sc;
        let y = if below + bhh <= by + bh { below } else { (by + anchor.1 * sc - bhh).max(by) };
        Some((x, y, bw, bhh, first, n))
    }
    fn draw_menu(&mut self) {
        let Some((x, y, bw, bh, first, n)) = self.menu_geom() else { return };
        let Mode::Menu { m, .. } = &self.mode else { return }; let items: Vec<(String, bool, bool)> = m.items[first..first + n].to_vec(); let at = m.at;
        let (Some(c), Some(font)) = (self.c.as_mut(), self.font.as_ref()) else { return };
        let (w, sc, rh) = (c.w as usize, self.sc as i32, self.font.as_ref().map_or(16, |f| f.h as i32) + 4 * self.sc as i32); let px = c.pixels(); if px.is_empty() { return }
        fill(px, w, Rect { x, y, w: bw, h: bh }, accent()); fill(px, w, Rect { x: x + sc, y: y + sc, w: bw - 2 * sc, h: bh - 2 * sc }, panel());
        for (k, (label, enabled, group)) in items.iter().enumerate() {
            let ry = y + sc + k as i32 * rh;
            if first + k == at { fill(px, w, Rect { x: x + sc, y: ry, w: bw - 2 * sc, h: rh }, SEL); fill(px, w, Rect { x: x + sc, y: ry, w: 3 * sc, h: rh }, accent()); }
            text(px, w, font, x + 2 * font.w as i32 / 1 + if *group { 0 } else { font.w as i32 / 2 }, ry + 2 * sc, &web::fit(&printable(label), ((bw / font.w as i32) as usize).saturating_sub(3)), if !enabled || *group { DIM } else { FG });
        }
        self.commit((x as u16, y as u16, bw.min(65535) as u16, bh as u16));
    }
    fn menu_row_at(&self, px: i32, py: i32) -> Option<usize> {
        let (x, y, bw, bh, first, n) = self.menu_geom()?; let (sc, rh) = (self.sc as i32, self.rh());
        if px < x || px >= x + bw || py < y + sc || py >= y + bh - sc { return None }
        let k = ((py - y - sc) / rh) as usize; (k < n).then_some(first + k)
    }
    fn close_menu(&mut self, pick: Option<usize>) {
        let Mode::Menu { menu, .. } = std::mem::replace(&mut self.mode, Mode::Normal) else { return };
        unsafe { thirweb_menu_answer(menu, pick.map_or(-1, |i| i as i32)); thirweb_repaint() }
        self.draw_strip();
    }
    fn leave_view(&mut self) { self.mode = Mode::Normal; unsafe { thirweb_repaint() } self.draw_strip(); }

    // ---- downloads ----
    fn dl_index(&self, id: u32) -> Option<usize> { self.dls.iter().position(|d| d.0.id == id) }
    fn ms(&self) -> u64 { self.t0.elapsed().as_millis() as u64 }
    fn dl_start(&mut self, dl: *mut c_void, name: &str, size: Option<u64>, url: &str, ranges: bool) {
        let dir = web::dirs(&self.home)[2].clone();
        let taken = |n: &str| dir.join(n).symlink_metadata().is_ok() || dir.join(format!("{n}.part")).symlink_metadata().is_ok() || self.dls.iter().any(|d| d.0.active() && d.0.name == n);
        let fin_name = web::download_name(name, taken);
        let fin = dir.join(&fin_name); let part = dir.join(format!("{fin_name}.part")); let id = self.next_dl; self.next_dl += 1;
        let mut d = Dl::new(id, &fin_name, url, size); d.ranges = ranges;
        self.dls.push((d, DlFiles { seg: part.clone(), part: part.clone(), fin }));
        self.note = Some(format!("downloading {}", printable(&fin_name)));
        let cp = CString::new(part.into_os_string().into_encoded_bytes()).unwrap_or_default(); unsafe { thirweb_download_answer(dl, cp.as_ptr(), id) }
    }
    /// Appends `seg` to `part` (the first piece is the part file itself: nothing to do) and returns the part's size.
    fn dl_join(f: &DlFiles) -> u64 {
        if f.seg != f.part {
            if let Ok(mut src) = std::fs::File::open(&f.seg) {
                if let Ok(mut dst) = std::fs::OpenOptions::new().create(true).append(true).mode(0o600).open(&f.part) { let _ = std::io::copy(&mut src, &mut dst); }
            }
            let _ = std::fs::remove_file(&f.seg);
        }
        std::fs::metadata(&f.part).map_or(0, |m| m.len())
    }
    fn on_dl_event(&mut self, kind: i32, id: u32, a: i64, s: &str) {
        let Some(i) = self.dl_index(id) else { return }; let ms = self.ms();
        match kind {
            1 => self.dls[i].0.progress(a.max(0) as u64, ms),
            2 => {
                let (d, f) = &mut self.dls[i]; d.seg = a.max(0) as u64; let n = Self::dl_join(f);
                if n > 0 || d.total == Some(0) {
                    // never replace a file: link to the final name (fails if it exists, even one made since the start), else to the next free name
                    let dir = f.fin.parent().map(PathBuf::from).unwrap_or_default();
                    let mut fin = f.fin.clone();
                    let linked = loop {
                        match std::fs::hard_link(&f.part, &fin) {
                            Ok(()) => break true,
                            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => { let name = web::download_name(&d.name, |n| dir.join(n).symlink_metadata().is_ok()); fin = dir.join(&name); d.name = name; }
                            Err(_) => break false,
                        }
                    };
                    if linked { let _ = std::fs::remove_file(&f.part); f.fin = fin; }
                    d.finish(); d.done = n; self.note = Some(format!("download saved: {}", printable(&d.name))); }
                else { d.fail("nothing was received") }
            }
            3 => {
                let (d, f) = &mut self.dls[i];
                match d.state {
                    DlState::Pausing => { let before = std::fs::metadata(&f.part).map_or(0, |m| m.len()); let n = Self::dl_join(f); let got = if f.seg == f.part { n } else { n - before.min(n) }; d.stopped(got); }
                    DlState::Cancelled => { let _ = std::fs::remove_file(&f.part); let _ = std::fs::remove_file(&f.seg); }
                    _ => { if a != 1 { d.fail(s); self.note = Some(format!("download failed: {}", printable(s))); } let _ = std::fs::remove_file(&f.seg).ok().filter(|_| f.seg != f.part); }
                }
            }
            4 => { let (d, f) = &mut self.dls[i]; if let Err(e) = d.answered(a as u32, Some(s).filter(|x| !x.is_empty())) { self.note = Some(e.into()); let _ = std::fs::remove_file(&f.seg); } }
            _ => {}
        }
        if matches!(self.mode, Mode::Downloads { .. }) || kind != 1 || self.dls.iter().any(|d| d.0.active()) { self.refresh(); }
    }
    /// The strip and the view change (not the page).
    fn refresh(&mut self) { if self.headless { return } if matches!(self.mode, Mode::Downloads { .. }) { self.draw_strip() } else if matches!(self.mode, Mode::Normal) { self.draw_strip() } }
    fn dl_pause_resume(&mut self, i: usize) {
        let ms = self.ms(); let _ = ms;
        let (d, f) = &mut self.dls[i];
        match d.state {
            DlState::Running => match d.pause() { Ok(()) => unsafe { thirweb_download_cancel(d.id) }, Err(e) => self.note = Some(e.into()) },
            DlState::Paused => {
                let name = d.name.clone(); let dir = web::dirs(&self.home)[2].clone(); f.seg = dir.join(format!(".{name}.seg")); let _ = std::fs::remove_file(&f.seg);
                if let Some(_) = d.resume() {
                    let (u, p) = (CString::new(d.url.clone()).unwrap_or_default(), CString::new(f.seg.clone().into_os_string().into_encoded_bytes()).unwrap_or_default());
                    unsafe { thirweb_download_resume(d.id, u.as_ptr(), d.done as i64, p.as_ptr()) }
                }
            }
            _ => {}
        }
    }
    fn dl_cancel(&mut self, i: usize) {
        let (d, f) = &mut self.dls[i];
        match d.state {
            DlState::Running | DlState::Resuming | DlState::Pausing => { d.cancel(); unsafe { thirweb_download_cancel(d.id) } }
            DlState::Paused => { d.cancel(); let _ = std::fs::remove_file(&f.part); let _ = std::fs::remove_file(&f.seg); }
            _ => {}
        }
    }
    fn hist_save(&self) {
        let tmp = self.hist_path.with_extension("tmp");
        if let Ok(mut f) = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp) { if f.write_all(self.hist.serialize().as_bytes()).is_ok() { let _ = std::fs::rename(&tmp, &self.hist_path); } }
    }
    /// A WebKit frame: copy the damaged parts under the strip and commit them.
    fn on_frame(&mut self, px: *const u8, stride: u32, fw: i32, fh: i32, rects: &[(i32, i32, i32, i32)]) {
        if self.covered() { unsafe { thirweb_frame_done() } return }   // a list is shown over the page: the frame is not wanted
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
        if matches!(self.mode, Mode::Menu { .. }) { self.draw_menu(); }   // the popup stays over the page
    }
    fn page_xy(&self, x: i32, y: i32) -> Option<(f64, f64)> {
        if y < self.strip_h as i32 || self.c.as_ref().is_some_and(|c| y >= c.h as i32 - self.foot_h as i32) { return None }
        Some((x as f64 / self.sc as f64, (y - self.strip_h as i32) as f64 / self.sc as f64))
    }
    fn load(&mut self, url: &str) { self.note = None; if let Ok(u) = CString::new(url) { unsafe { thirweb_load(u.as_ptr()) } } }
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
        let view = matches!(self.mode, Mode::Downloads { .. } | Mode::History { confirm: false, .. });
        // tab keys work while the address, the find line or the page has the keyboard (not while a question is open)
        if ctrl && value == 1 && (view || matches!(self.mode, Mode::Normal | Mode::Edit(_) | Mode::Find(_))) {
            match code {
                K_J => { self.mode = Mode::Downloads { at: 0 }; self.consumed.insert(code); self.draw_strip(); return }
                K_H => { self.mode = Mode::History { q: Line::default(), at: 0, confirm: false }; self.consumed.insert(code); self.draw_strip(); return }
                _ if view => {}
                K_T => { self.new_tab(); self.consumed.insert(code); return }
                K_W => { let i = self.tabs.cur; self.close_tab(i); self.consumed.insert(code); return }
                K_L => { self.mode = Mode::Edit(Line::new(&self.tab().url)); self.consumed.insert(code); self.draw_strip(); return }   // from the address or find line too
                K_F => { self.mode = Mode::Find(Find::default()); self.consumed.insert(code); self.draw_strip(); return }
                K_R => { self.note = None; unsafe { thirweb_reload() } self.consumed.insert(code); return }
                K_Q => unsafe { thirweb_quit() },
                K_TAB if !shift => { self.tabs.next(); self.switched(); self.consumed.insert(code); return }
                K_PGDN => { self.tabs.next(); self.switched(); self.consumed.insert(code); return }
                K_PGUP => { self.tabs.prev(); self.switched(); self.consumed.insert(code); return }
                K_TAB => { self.tabs.prev(); self.switched(); self.consumed.insert(code); return }
                _ => {}
            }
        }
        if ctrl && code == K_Q && value == 1 && view { unsafe { thirweb_quit() } return }
        match &mut self.mode {
            Mode::Ask { dl, name, size, url, ranges } => {
                if value == 1 && (code == K_Y || code == K_N || code == K_ESC) {
                    let (dl, name, size, url, ranges) = (*dl, name.clone(), *size, url.clone(), *ranges); self.mode = Mode::Normal;
                    if code == K_Y { self.dl_start(dl, &name, size, &url, ranges); }
                    else { self.note = Some("download cancelled".into()); unsafe { thirweb_download_answer(dl, ptr::null(), 0) } }
                    self.draw_strip();
                }
            }
            Mode::Menu { m, .. } => {
                if value >= 1 {
                    match code {
                        K_UP => m.up(), K_DOWN => m.down(), K_HOME => m.first(), K_END => m.last(), K_PGUP => { for _ in 0..8 { m.up() } } K_PGDN => { for _ in 0..8 { m.down() } }
                        K_ESC if value == 1 => { self.close_menu(None); return }
                        K_ENTER if value == 1 => { let c = m.chosen(); self.close_menu(c); return }
                        K_TAB if value == 1 => { let c = m.chosen(); self.close_menu(c); return }
                        _ if !ctrl && ch > 0x20 => if let Some(c) = char::from_u32(ch) { m.jump(c) },
                        _ => {}
                    }
                    self.draw_strip();
                }
            }
            Mode::Downloads { at } => {
                let n = self.dls.len(); let idx = |at: usize| n.checked_sub(1 + at);
                if value >= 1 {
                    match code {
                        K_UP => *at = at.saturating_sub(1), K_DOWN => *at = (*at + 1).min(n.saturating_sub(1)),
                        K_ESC if value == 1 => { self.leave_view(); return }
                        K_P if value == 1 => { if let Some(i) = idx(*at) { self.dl_pause_resume(i) } }
                        K_C if value == 1 => { if let Some(i) = idx(*at) { self.dl_cancel(i) } }
                        K_X | K_DELETE if value == 1 => { if let Some(i) = idx(*at) { if !self.dls[i].0.active() { self.dls.remove(i); } } if let Mode::Downloads { at } = &mut self.mode { *at = (*at).min(self.dls.len().saturating_sub(1)) } }
                        _ => {}
                    }
                    self.draw_strip();
                }
            }
            Mode::History { q, at, confirm } => {
                if *confirm {
                    if value == 1 && (code == K_Y || code == K_N || code == K_ESC) { if code == K_Y { self.hist.clear(); self.hist_save(); } if let Mode::History { confirm, at, .. } = &mut self.mode { *confirm = false; *at = 0 } self.draw_strip(); }
                } else if value >= 1 {
                    let was = q.as_string(); let rows = self.hist.search(&was).len();
                    match code {
                        K_UP => *at = at.saturating_sub(1), K_DOWN => *at = (*at + 1).min(rows.saturating_sub(1)),
                        K_PGUP => *at = at.saturating_sub(8), K_PGDN => *at = (*at + 8).min(rows.saturating_sub(1)),
                        K_ESC if value == 1 => { self.leave_view(); return }
                        K_ENTER if value == 1 => {
                            let url = self.hist.search(&was).get(*at).map(|v| v.url.clone());
                            if let Some(u) = url { self.mode = Mode::Normal; self.tabs.current_mut().url = u.clone(); unsafe { thirweb_repaint() } self.load(&u); self.draw_strip(); }
                            return
                        }
                        K_DELETE if value == 1 => { let url = self.hist.search(&was).get(*at).map(|v| v.url.clone()); if let Some(u) = url { self.hist.remove(&u); self.hist_save(); } }
                        K_D if ctrl && value == 1 => { let url = self.hist.search(&was).get(*at).map(|v| v.url.clone()); if let Some(u) = url { self.hist.remove(&u); self.hist_save(); } }
                        K_K if ctrl && value == 1 => { *confirm = true; }
                        _ => { edit_line(q, code, ctrl, shift, ch); }
                    }
                    if let Mode::History { q, at, .. } = &mut self.mode { if q.as_string() != was { *at = 0 } let n = self.hist.search(&q.as_string()).len(); *at = (*at).min(n.saturating_sub(1)); }
                    self.draw_strip();
                }
            }
            Mode::Dialog { kind, line, .. } => {
                let kind = *kind;
                if kind == DialogKind::Prompt {
                    match code {
                        K_ENTER if value == 1 => self.answer_dialog(true), K_ESC if value == 1 => self.answer_dialog(false),
                        _ => { edit_line(line, code, ctrl, shift, ch); }
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
                    K_F if ctrl && value == 1 => {}
                    _ => { edit_line(&mut f.line, code, ctrl, shift, ch); }
                }
                if let Mode::Find(f) = &mut self.mode {
                    let now = f.line.as_string();
                    if now != was { f.matches = None; let t = CString::new(now.clone()).unwrap_or_default(); unsafe { thirweb_find(if now.is_empty() { ptr::null() } else { t.as_ptr() }, 0) } }
                }
                self.draw_strip();
            }
            Mode::Edit(l) => {
                let at_end = l.at == l.text.len() && l.sel.is_none();
                match code {
                    K_ENTER if value == 1 => {
                        let typed = l.as_string(); self.mode = Mode::Normal;
                        match web::address(&typed) { Ok(u) => { self.tabs.current_mut().url = u.clone(); self.load(&u) } Err(e) => self.note = Some(e.into()) }
                    }
                    K_ESC if value == 1 => self.mode = Mode::Normal,
                    K_TAB | K_RIGHT | K_END if at_end && !shift && !ctrl && value >= 1 && self.hist.suggest(&l.as_string()).is_some() => {   // the grey completion becomes text (like the shell)
                        let rest = self.hist.suggest(&l.as_string()).unwrap_or_default(); for c in rest.chars() { l.insert(c) }
                    }
                    _ => { edit_line(l, code, ctrl, shift, ch); }
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
                K_C if ctrl && value == 1 => unsafe { thirweb_edit(0); thirweb_timeout(300, push_clip, ptr::null_mut()) },
                K_X if ctrl && value == 1 => unsafe { thirweb_edit(1); thirweb_timeout(300, push_clip, ptr::null_mut()) },
                K_V if ctrl && value == 1 => unsafe { thirweb_edit(2) },
                K_A if ctrl && value == 1 => unsafe { thirweb_edit(3) },
                K_ESC if value == 1 => { unsafe { thirweb_stop() }; eat = false; }
                _ => eat = false,
            },
        }
        if eat { self.consumed.insert(code); } else { unsafe { thirweb_keysend(code, value) } }
    }
    /// The mouse reaches the page (not while a question, the address line, a list or the popup is open).
    fn live(&self) -> bool { matches!(self.mode, Mode::Normal | Mode::Find(_)) }
    /// A click in the strip: the arrows, reload, and the downloads note on the right.
    fn strip_click(&mut self, x: i32, y: i32) {
        let Some(font) = self.font.as_ref() else { return }; let (fw, sc) = (font.w as i32, self.sc as i32);
        if !matches!(self.mode, Mode::Normal) || y < 3 * sc + font.h as i32 + 2 * sc { return }   // line 2 only
        let col = x / fw;
        if (1..=1).contains(&col) { unsafe { thirweb_back() } } else if col == 4 { unsafe { thirweb_forward() } } else if col == 7 { self.note = None; unsafe { thirweb_reload() } }
        else if self.dls.iter().any(|d| d.0.active()) && x > self.c.as_ref().map_or(0, |c| c.w as i32) - 18 * fw { self.mode = Mode::Downloads { at: 0 }; self.draw_strip(); }
    }
    /// Pointer and buttons while a popup or a list is open. Returns true when it took the event.
    fn view_pointer(&mut self, x: i32, y: i32, click: bool) -> bool {
        match &self.mode {
            Mode::Menu { .. } => {
                let row = self.menu_row_at(x, y);
                if click { match row { Some(i) => { let ok = matches!(&self.mode, Mode::Menu { m, .. } if m.items[i].1 && !m.items[i].2); if ok { self.close_menu(Some(i)) } } None => self.close_menu(None) } }
                else if let (Some(i), Mode::Menu { m, .. }) = (row, &mut self.mode) { if m.items[i].1 && !m.items[i].2 && m.at != i { m.at = i; self.draw_menu(); } }
                true
            }
            Mode::Downloads { .. } | Mode::History { .. } => {
                if click {
                    let (by, bh) = self.body(); let rh = self.rh(); let sc = self.sc as i32;
                    if y >= by && y < by + bh {
                        let hist = matches!(self.mode, Mode::History { .. }); let (entry, top) = if hist { (rh, rh / 2 + 2 * rh) } else { (2 * rh + 6 * sc, rh / 2) };
                        let k = ((y - by - top) / entry) as i32; let n = if hist { self.history_rows().len() } else { self.dls.len() };
                        let vis = if hist { ((bh - 3 * rh) / rh).max(1) as usize } else { ((bh - 2 * rh) / entry).max(1) as usize };
                        if k >= 0 && y - by >= top {
                            let cur = match &self.mode { Mode::Downloads { at } | Mode::History { at, .. } => *at, _ => 0 }; let first = (cur + 1).saturating_sub(vis).min(n.saturating_sub(vis));
                            let i = first + k as usize;
                            if i < n { if let Mode::Downloads { at } | Mode::History { at, .. } = &mut self.mode { *at = i } self.draw_strip(); }
                        }
                    }
                }
                true
            }
            _ => false,
        }
    }
    fn on_den(&mut self) {
        loop {
            let ev = match self.c.as_mut().map(|c| c.poll_ev(0)) { Some(Ok(Some(Ev::Down(e)))) => e, Some(Ok(Some(Ev::Clip(t)))) => { clip_local(&t); continue } Some(Ok(Some(Ev::Pcm(_)))) => continue, Some(Ok(None)) => break, _ => { unsafe { thirweb_quit() } return } };
            match ev {
                Down::Done => self.on_done(),
                Down::Configure { w, h, scale, focused, .. } => {
                    unsafe { thirweb_focus(focused as i32) }
                    if (w, h) == self.attached && scale.max(1) == self.sc { self.draw_strip(); continue }   // same size: maybe a new theme
                    if let Some(c) = self.c.as_mut() { if c.attach().is_err() { unsafe { thirweb_quit() } return } }   // den drops the old buffer on a new size
                    self.attached = (w, h); self.sc = scale.max(1); self.pending.clear(); self.inflight = false; self.frame_waiting = false;
                    if self.frame_sent { self.frame_sent = false; unsafe { thirweb_frame_done() } }
                    (self.strip_h, self.foot_h) = self.font.as_ref().map_or((0, 0), |f| strip_sizes(f, self.sc));
                    let (lw, lh) = self.page_size(); unsafe { thirweb_resize(lw, lh, self.sc as f64) }
                    self.draw_strip();
                }
                Down::Key { code, value, mods } => self.key(code, value, mods),
                Down::Pointer { x, y } => { self.ptr = (x, y); if self.view_pointer(x, y, false) { continue } if let Some((px, py)) = self.page_xy(x, y) { if self.live() || self.btn_down { unsafe { thirweb_pointer(px, py) } } } }
                Down::Button { code, pressed } if code as u32 == BTN_SIDE || code as u32 == BTN_EXTRA => {   // the mouse's side buttons: back and forward
                    if pressed && matches!(self.mode, Mode::Normal) { if code as u32 == BTN_SIDE { unsafe { thirweb_back() } } else { unsafe { thirweb_forward() } } }
                }
                Down::Button { code, pressed } if !self.live() && !self.btn_down => { if pressed && code as u32 == 0x110 { let (x, y) = self.ptr; if !self.view_pointer(x, y, true) && y < self.strip_h as i32 { self.strip_click(x, y) } } }
                Down::Button { code, pressed } => {
                    let (x, y) = self.ptr;
                    if pressed && code as u32 == 0x110 && y < self.strip_h as i32 { self.strip_click(x, y); continue }
                    if pressed { if let (Some((px, py)), true) = (self.page_xy(x, y), self.live()) { self.btn_down = true; unsafe { thirweb_button(code, 1, px, py) } } }
                    else if self.btn_down { self.btn_down = false; let (px, py) = self.page_xy(x, y).unwrap_or((0.0, 0.0)); unsafe { thirweb_button(code, 0, px, py) } }
                }
                Down::Scroll { dy, .. } if matches!(self.mode, Mode::Menu { .. } | Mode::Downloads { .. } | Mode::History { .. }) => {
                    for _ in 0..dy.unsigned_abs().min(5) {
                        match &mut self.mode { Mode::Menu { m, .. } => if dy > 0 { m.up() } else { m.down() }, Mode::Downloads { at } => if dy > 0 { *at = at.saturating_sub(1) } else { *at = (*at + 1).min(self.dls.len().saturating_sub(1)) },
                            Mode::History { at, .. } => if dy > 0 { *at = at.saturating_sub(1) } else { *at += 1 }, _ => {} }
                    }
                    if let Mode::History { q, at, .. } = &mut self.mode { let n = self.hist.search(&q.as_string()).len(); *at = (*at).min(n.saturating_sub(1)); }
                    self.draw_strip();
                }
                Down::Scroll { dx, dy } => if let (Some((px, py)), true) = (self.page_xy(self.ptr.0, self.ptr.1), self.live()) { unsafe { thirweb_scroll(dx as f64, dy as f64, px, py) } },
                Down::Mic { .. } | Down::Cam { .. } | Down::CamFrame { .. } => {}
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
    let a = app(); let Some(i) = tab_of(id) else { return }; a.tabs.items[i].title = printable(&cstr(t)); if !a.headless && !a.tabs.items[i].url.is_empty() { let (u, t) = (a.tabs.items[i].url.clone(), a.tabs.items[i].title.clone()); a.hist.set_title(&u, &t); }
    if !a.headless { a.draw_strip(); }
}
extern "C" fn cb_uri(id: i32, u: *const c_char, _: *mut c_void) { let a = app(); let (u, Some(i)) = (cstr(u), tab_of(id)) else { return }; if !u.is_empty() { a.tabs.items[i].url = u; } a.draw_strip(); }
extern "C" fn cb_progress(id: i32, p: f64, _: *mut c_void) { let a = app(); let Some(i) = tab_of(id) else { return }; a.tabs.items[i].progress = (p < 1.0).then_some((p * 100.0) as u8); a.draw_strip(); }
extern "C" fn cb_load(id: i32, e: i32, _: *mut c_void) {
    let a = app(); let Some(i) = tab_of(id) else { return };
    if e == 0 && i == a.tabs.cur { if a.keep_note { a.keep_note = false } else { a.note = None } }   // the error page's own load keeps the error in the strip
    if e == 3 && !a.headless && a.note.is_none() { let (u, t) = (a.tabs.items[i].url.clone(), a.tabs.items[i].title.clone()); a.hist.visit(&u, &t, thir::clock::now()); a.hist_save(); }
    if e == 3 { a.tabs.items[i].progress = None; if a.headless && !a.failed { unsafe { thirweb_timeout(dump_wait(&a.tabs.items[i].url), finish_dump, ptr::null_mut()) } } }
    a.draw_strip();
}
/// How long `--dump-title` waits after the load: 300 ms, or `#wait=MS` (at most a minute) at the end of the address for a page that sets its title later (a video that has started).
fn dump_wait(url: &str) -> u32 { url.rsplit_once("#wait=").and_then(|(_, n)| n.parse::<u32>().ok()).map_or(300, |n| n.min(60_000)) }
extern "C" fn finish_dump(_: *mut c_void) { let a = app(); if !a.failed { println!("{}", a.tab().title); unsafe { thirweb_quit() } } }
extern "C" fn cb_failed(id: i32, m: *const c_char, _: *mut c_void) {
    let a = app(); let m = printable(&cstr(m));
    if a.headless { if !m.starts_with("download") { eprintln!("web: {m}"); a.failed = true; unsafe { thirweb_quit() } } return }
    if id != 0 && tab_of(id) != Some(a.tabs.cur) { return }   // a background tab's error is not the current page's
    a.keep_note = !m.starts_with("download"); a.note = Some(m); a.draw_strip();
}
extern "C" fn cb_download(dl: *mut c_void, name: *const c_char, size: i64, url: *const c_char, ranges: i32, _: *mut c_void) {
    let a = app();
    if !matches!(a.mode, Mode::Normal) || a.headless { unsafe { thirweb_download_answer(dl, ptr::null(), 0) } return }   // one question at a time; headless: no downloads
    a.mode = Mode::Ask { dl, name: cstr(name), size: (size >= 0).then_some(size as u64), url: cstr(url), ranges: ranges != 0 }; a.draw_strip();
}
extern "C" fn cb_option_menu(id: i32, menu: *mut c_void, x: i32, y: i32, w: i32, h: i32, _: *mut c_void) {
    let a = app();
    if a.headless || tab_of(id) != Some(a.tabs.cur) || !matches!(a.mode, Mode::Normal) { unsafe { thirweb_menu_answer(menu, -1) } return }
    let n = unsafe { thirweb_menu_count(menu) }.clamp(0, 5000); let mut items = vec![]; let mut sel = None;
    for i in 0..n {
        let (mut l, mut f): (*const c_char, i32) = (ptr::null(), 0); unsafe { thirweb_menu_item(menu, i, &mut l, &mut f) }
        if f & 4 != 0 { sel = Some(i as usize) } items.push((printable(&cstr(l)), f & 1 != 0, f & 2 != 0));
    }
    if items.is_empty() { unsafe { thirweb_menu_answer(menu, -1) } return }
    a.mode = Mode::Menu { menu, m: web::Menu::new(items, sel), anchor: (x, y, w, h) }; a.draw_strip();
}
extern "C" fn cb_dl_event(kind: i32, id: u32, v: i64, s: *const c_char, _: *mut c_void) { app().on_dl_event(kind, id, v, &cstr(s)); }
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
    // quiet by default: no glib debug output, Mesa and EGL say nothing, fontconfig no debug (their stderr goes to ~/.cache/thir-web/log anyway)
    std::env::remove_var("G_MESSAGES_DEBUG"); std::env::remove_var("FC_DEBUG");
    // GStreamer (audio and video in pages): the add-on's plugins only, the registry in the cache folder (the web process may write nothing else), no scanner process, no debug output; ALSA reads the add-on's config
    for (k, v) in [("GST_PLUGIN_SYSTEM_PATH_1_0", "/web/usr/lib/gstreamer-1.0"), ("GST_PLUGIN_PATH_1_0", ""), ("GST_REGISTRY_FORK", "no"), ("GST_DEBUG_NO_COLOR", "1"), ("ALSA_CONFIG_PATH", "/web/usr/share/alsa/alsa.conf")] { std::env::set_var(k, v); }
    std::env::set_var("GST_REGISTRY_1_0", home.join(".cache/thir-web/gstreamer/registry.bin"));
    for (k, v) in [("MESA_DEBUG", "silent"), ("LIBGL_DEBUG", "quiet"), ("EGL_LOG_LEVEL", "fatal"), ("MESA_LOG_LEVEL", "fatal")] { if std::env::var_os(k).is_none() { std::env::set_var(k, v); } }
}
/// The browser's folders, made 0700 before the sandbox closes.
fn make_dirs(home: &Path) -> std::io::Result<()> {
    for d in web::dirs(home) { std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&d)?; }
    for d in ["fontconfig", "gstreamer"] { std::fs::DirBuilder::new().recursive(true).mode(0o700).create(home.join(".cache/thir-web").join(d))?; }   // fontconfig's per-user cache (XDG_CACHE_HOME/fontconfig)
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
    let usage = || { eprintln!("usage: web [ADDRESS]   (example.com, or http and https URLs)\n       web --dump-title URL   web --check"); 2 };
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from).filter(|h| h.is_absolute()) else { eprintln!("web: no home folder"); std::process::exit(1) };
    let (dump, url) = match args.as_slice() {
        [] => (false, None), [a] if a == "--check" => { load_env(&home); std::process::exit(check(&home)) }
        [a, u] if a == "--dump-title" => (true, Some(u.clone())), [u] if !u.starts_with("--") => (false, Some(u.clone())), _ => std::process::exit(usage()),
    };
    // the same rule as the address line: `web example.com` becomes https://example.com, a few words become a search
    let url = match url.map(|u| web::address(&u).map_err(|_| u)).transpose() { Ok(u) => u, Err(u) => { eprintln!("web: only http and https addresses are opened (not {})", printable(&u)); std::process::exit(2) } };
    // den first (before the sandbox hides /run), then the sandbox
    let client = if dump { None } else { match Client::connect() { Ok(c) => Some(c), Err(e) => { eprintln!("web needs the desktop: run it in a den pane ({e})"); std::process::exit(1) } } };
    let font = client.as_ref().and_then(|c| std::fs::read(if c.scale >= 2 { "/share/font32.psf" } else { "/share/font16.psf" }).ok().and_then(|b| Font::parse(&b).ok()));
    if !dump && font.is_none() { eprintln!("web: no font"); std::process::exit(1) }
    load_env(&home);
    if std::env::var_os("GST_DEBUG").is_none() { std::env::set_var("GST_DEBUG", if dump { "2" } else { "0" }); }   // quiet in the browser (words go to ~/.cache/thir-web/log); `--dump-title` is for tests and bug reports: GStreamer warnings show
    if true { if let Err(e) = make_dirs(&home) { eprintln!("web: {}: {e}", home.display()); std::process::exit(1) } }
    if let Err(e) = lock_in(&home) { eprintln!("web: the sandbox cannot be set up ({e}); not starting"); std::process::exit(1) }
    let sc = client.as_ref().map_or(1, |c| c.scale.max(1));
    let (strip_h, foot_h) = font.as_ref().map_or((0, 0), |f| strip_sizes(f, sc));
    let hist_path = web::dirs(&home)[0].join("history");
    let mut a = App { hist: History::parse(&std::fs::read_to_string(&hist_path).unwrap_or_default()), hist_path, dls: vec![], next_dl: 1, t0: Instant::now(), off: thir::clock::home_tz().unwrap_or(0), c: client, font, sc, strip_h, foot_h, headless: dump, tabs: Tabs::new(TabState::new(0, &url.clone().unwrap_or_default())), note: None, mode: Mode::Normal, home: home.clone(),
        pending: vec![], inflight: false, frame_waiting: false, frame_sent: false, attached: (0, 0), keep_note: false, consumed: HashSet::new(), ptr: (0, 0), btn_down: false, failed: false };
    if let Some(c) = a.c.as_mut() { if c.attach().is_err() { eprintln!("web: no surface"); std::process::exit(1) } a.attached = (c.w, c.h); }
    unsafe { APP = &mut a; }
    if !dump { thir::logfile::redirect_stderr(&home); }   // WebKit, Mesa, glib and fontconfig words go to ~/.cache/thir-web/log; the strip shows what the user must know
    let (lw, lh) = a.page_size();
    let [data, cache, _] = web::dirs(&home);
    let (dc, cc) = (CString::new(data.into_os_string().into_encoded_bytes()).unwrap(), CString::new(cache.into_os_string().into_encoded_bytes()).unwrap());
    let cb = ThirCb { ud: ptr::null_mut(), frame: if dump { None } else { Some(cb_frame) }, title: cb_title, uri: cb_uri, progress: cb_progress, load: cb_load, failed: cb_failed, download: cb_download,
        popup_ok: cb_popup_ok, opened: cb_opened, closed: cb_closed, dialog: cb_dialog, chooser: cb_chooser, found: cb_found, option_menu: cb_option_menu, dl_event: cb_dl_event };
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
