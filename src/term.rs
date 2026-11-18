//! Shared full-screen terminal bits for edit and top. Raw mode is restored on drop, on panic and on SIGTERM/SIGHUP.
use crate::line::{Act, Decoder, Editor, Key};
use std::io::{self, BufRead, Write};
use std::sync::OnceLock;

static SAVED: OnceLock<libc::termios> = OnceLock::new();
pub const REV: &str = "\x1b[7m";
pub const RST: &str = "\x1b[0m";

/// Raw mode only while needed, so children always see a cooked tty. None if fd 0 is not a tty.
pub fn raw_on() -> Option<libc::termios> {
    let mut old: libc::termios = unsafe { std::mem::zeroed() };
    if unsafe { libc::tcgetattr(0, &mut old) } != 0 { return None; }
    let mut t = old;
    t.c_lflag &= !(libc::ICANON | libc::ECHO | libc::ISIG | libc::IEXTEN);
    t.c_iflag &= !(libc::ICRNL | libc::IXON);
    t.c_cc[libc::VMIN] = 1; t.c_cc[libc::VTIME] = 0;
    unsafe { libc::tcsetattr(0, libc::TCSANOW, &t); }
    Some(old)
}
pub fn raw_off(t: &libc::termios) { unsafe { libc::tcsetattr(0, libc::TCSADRAIN, t); } }

pub fn put(b: &[u8]) { let mut o = 0; while o < b.len() { let n = unsafe { libc::write(1, b[o..].as_ptr() as *const _, b.len() - o) }; if n <= 0 { break } o += n as usize; } }

pub struct Raw(libc::termios);
impl Raw {
    pub fn enter() -> Option<Raw> {
        let old = raw_on()?;
        let _ = SAVED.set(old);
        unsafe {
            let mut sa: libc::sigaction = std::mem::zeroed();
            sa.sa_sigaction = restore_and_raise as extern "C" fn(i32) as usize;
            sa.sa_flags = 0;
            for s in [libc::SIGTERM, libc::SIGHUP, libc::SIGALRM, libc::SIGABRT] { libc::sigaction(s, &sa, std::ptr::null_mut()); }
        }
        let prev = std::panic::take_hook();   // panic = abort: the hook is the last code that runs
        std::panic::set_hook(Box::new(move |i| { if let Some(t) = SAVED.get() { unsafe { libc::tcsetattr(0, libc::TCSANOW, t); } } put(b"\x1b[?25h\x1b[0m\r\n"); prev(i); }));
        Some(Raw(old))
    }
}
impl Drop for Raw { fn drop(&mut self) { put(b"\x1b[?25h\x1b[0m"); raw_off(&self.0); } }

extern "C" fn restore_and_raise(sig: i32) {   // put the terminal back when a signal ends the program
    if let Some(t) = SAVED.get() { unsafe { libc::tcsetattr(0, libc::TCSANOW, t); } }
    unsafe { const B: &[u8] = b"\x1b[0m\x1b]2;\x07\x1b[?25h"; libc::write(1, B.as_ptr() as *const _, B.len()); }   // colours, den title and cursor back (write is async-signal-safe)
    unsafe { libc::signal(sig, libc::SIG_DFL); libc::raise(sig); }
}

static CURSOR_SAFE: std::sync::Once = std::sync::Once::new();
extern "C" fn show_and_raise(sig: i32) {
    unsafe { const B: &[u8] = b"\x1b[0m\x1b[?25h"; libc::write(1, B.as_ptr() as *const _, B.len()); libc::signal(sig, libc::SIG_DFL); libc::raise(sig); }
}
/// Hides the cursor for a progress display. It comes back on `cursor_show`, on a panic, and on SIGINT, SIGTERM and SIGHUP.
pub fn cursor_hide() {
    CURSOR_SAFE.call_once(|| {
        unsafe { for s in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] { libc::signal(s, show_and_raise as extern "C" fn(i32) as usize); } }
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |i| { put(b"\x1b[?25h\x1b[0m"); prev(i); }));
    });
    put(b"\x1b[?25l");
}
pub fn cursor_show() { put(b"\x1b[?25h"); }

/// (cols, rows) of the terminal; unknown (0 or error, as on a serial line) is 80x24.
pub fn size() -> (usize, usize) {
    let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
    if unsafe { libc::ioctl(1, libc::TIOCGWINSZ as _, &mut ws) } == 0 && ws.ws_col > 0 && ws.ws_row > 0 { (ws.ws_col as usize, ws.ws_row as usize) } else { (80, 24) }
}

/// Waits up to `timeout_ms` (-1 = forever) for input. None = end of input; Some(empty) = timeout or only half a key.
pub fn read_keys(dec: &mut Decoder, timeout_ms: i32) -> Option<Vec<Key>> {
    let mut p = libc::pollfd { fd: 0, events: libc::POLLIN, revents: 0 };
    loop {
        let r = unsafe { libc::poll(&mut p, 1, timeout_ms) };
        if r < 0 { if std::io::Error::last_os_error().raw_os_error() == Some(libc::EINTR) { continue } return None }
        if r == 0 { return Some(vec![]) }
        let mut buf = [0u8; 256];
        let n = unsafe { libc::read(0, buf.as_mut_ptr() as *mut _, buf.len()) };
        if n < 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::EINTR) { continue }
        return if n <= 0 { None } else { Some(dec.decode(&buf[..n as usize])) };
    }
}

/// One-line input on screen row `row` (1-based). Enter returns the text; Ctrl-C, or Ctrl-D on an empty line, cancels (None).
pub fn prompt(dec: &mut Decoder, row: usize, label: &str) -> Option<String> { prompt_with(dec, row, label, "") }
/// `prompt` that starts with `initial` already typed (to edit a value).
pub fn prompt_with(dec: &mut Decoder, row: usize, label: &str, initial: &str) -> Option<String> {
    let mut s = initial.to_string();
    loop {
        let room = size().0.saturating_sub(crate::tui::vis(label) + 2).max(1);
        let shown: String = { let v: Vec<char> = s.chars().collect(); v[v.len().saturating_sub(room)..].iter().collect() };
        put(format!("\x1b[{row};1H{label}{}\x1b[K\x1b[?25h", crate::files::printable(&shown)).as_bytes());
        for k in read_keys(dec, 500)? {
            match k {
                Key::Enter => return Some(s),
                Key::Char(c) if s.len() < 256 => s.push(c),
                Key::Backspace => { s.pop(); }
                Key::Ctrl(b'c') => return None,
                Key::Ctrl(b'd') if s.is_empty() => return None,
                _ => {}
            }
        }
    }
}

/// One line from the user with editing and Tab completion. None = end of input. Raw mode is always off again on return.
pub fn read_line(ed: &mut Editor, prompt: &str, complete: &dyn Fn(bool, &str) -> Vec<String>) -> Option<String> {
    let p = prompt;
    let Some(old) = raw_on() else {
        print!("{p}"); let _ = io::stdout().flush();
        let mut l = String::new();
        return match io::stdin().lock().read_line(&mut l) { Ok(0) | Err(_) => None, Ok(_) => Some(l) };
    };
    ed.cols = size().0; put(&ed.redraw(p));
    let r = 'read: loop {
        let mut buf = [0u8; 256];
        let n = unsafe { libc::read(0, buf.as_mut_ptr() as *mut _, buf.len()) };
        if n < 0 && io::Error::last_os_error().raw_os_error() == Some(libc::EINTR) { continue }
        if n <= 0 { break None }
        ed.cols = size().0;
        for k in ed.decode(&buf[..n as usize]) {   // shortcut: keys after Enter in the same read are dropped
            if k == Key::Tab { let (first, w) = ed.word(); let o = ed.complete(&complete(first, &w), p); put(&o); continue }
            let (a, o) = ed.key(k, p); put(&o);
            match a { Act::Submit(l) => break 'read Some(l), Act::Eof => break 'read None, Act::None => {} }
        }
    };
    raw_off(&old);
    r
}
