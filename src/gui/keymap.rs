//! evdev key events (US layout) -> bytes for the pty, or a window-manager action.
use super::layout::Dir;

#[derive(Debug, PartialEq)]
pub enum Action { Split, Close, Fullscreen, StopMic, Copy, Paste, Focus(Dir) }
#[derive(Debug, PartialEq)]
pub enum Key { Bytes(Vec<u8>), Act(Action) }

#[derive(Default)]
pub struct Keys { shift: bool, ctrl: bool, alt: bool, meta: bool, caps: bool }

/// (unshifted, shifted) for printable keys.
fn printable(code: u16) -> Option<(u8, u8)> {
    const ROWS: [(u16, &[u8], &[u8]); 5] = [
        (2, b"1234567890-=", b"!@#$%^&*()_+"), (16, b"qwertyuiop[]", b"QWERTYUIOP{}"), (30, b"asdfghjkl;'", b"ASDFGHJKL:\""),
        (44, b"zxcvbnm,./", b"ZXCVBNM<>?"), (41, b"`", b"~"),
    ];
    for (start, lo, hi) in ROWS { if code >= start && ((code - start) as usize) < lo.len() { let i = (code - start) as usize; return Some((lo[i], hi[i])); } }
    match code { 43 => Some((b'\\', b'|')), 57 => Some((b' ', b' ')), _ => None }
}

impl Keys {
    /// Alt or Cmd is held: a surface app gets no key presses then (they are window-manager keys).
    pub fn wm(&self) -> bool { self.alt || self.meta }
    /// The held modifiers as surface protocol bits (shift, ctrl, caps lock).
    pub fn mods(&self) -> u32 { self.shift as u32 | (self.ctrl as u32) << 1 | (self.caps as u32) << 3 }
    pub fn is_wm_key(code: u16) -> bool { matches!(code, 56 | 100 | 125 | 126) }
    /// `value`: 0 release, 1 press, 2 repeat. Returns None for modifiers, releases and unknown keys.
    pub fn event(&mut self, code: u16, value: i32) -> Option<Key> {
        let down = value != 0;
        match code {
            42 | 54 => { self.shift = down; return None }
            29 | 97 => { self.ctrl = down; return None }
            56 | 100 => { self.alt = down; return None }
            125 | 126 => { self.meta = down; return None }   // Cmd (Super) works like Alt for the window keys, since some Mac keyboards have no Alt
            58 => { if value == 1 { self.caps = !self.caps } return None }
            _ => {}
        }
        if !down { return None }
        if self.wm() {
            if self.meta && !self.alt { match code { 46 => return Some(Key::Act(Action::Copy)), 47 => return Some(Key::Act(Action::Paste)), _ => {} } }   // Cmd+C, Cmd+V
            return Some(Key::Act(match code {
                28 => Action::Split, 16 => Action::Close, 33 => Action::Fullscreen, 19 => Action::StopMic,
                103 | 37 => Action::Focus(Dir::Up), 108 | 36 => Action::Focus(Dir::Down),
                105 | 35 => Action::Focus(Dir::Left), 106 | 38 => Action::Focus(Dir::Right),
                _ => return None,   // other Alt combinations are swallowed
            }));
        }
        if self.ctrl && self.shift { match code { 46 => return Some(Key::Act(Action::Copy)), 47 => return Some(Key::Act(Action::Paste)), _ => {} } }   // Ctrl+Shift+C, Ctrl+Shift+V
        let fixed: &[u8] = match code {
            1 => b"\x1b", 14 => b"\x7f", 15 => b"\t", 28 => b"\r",
            103 => b"\x1b[A", 108 => b"\x1b[B", 106 => b"\x1b[C", 105 => b"\x1b[D", 111 => b"\x1b[3~", 102 => b"\x1b[H", 107 => b"\x1b[F", 104 => b"\x1b[5~", 109 => b"\x1b[6~",
            _ => b"",
        };
        if !fixed.is_empty() { return Some(Key::Bytes(fixed.to_vec())) }
        let (lo, hi) = printable(code)?;
        let letter = lo.is_ascii_lowercase();
        let c = if self.shift != (self.caps && letter) { hi } else { lo };
        Some(Key::Bytes(vec![if self.ctrl { match lo { b'a'..=b'z' | b'[' | b'\\' | b']' => lo & 0x1f, b' ' => 0, _ => return None } } else { c }]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn b(s: &[u8]) -> Option<Key> { Some(Key::Bytes(s.to_vec())) }
    #[test] fn letters_shift_caps() {
        let mut k = Keys::default();
        assert_eq!(k.event(30, 1), b(b"a")); assert_eq!(k.event(30, 0), None);
        k.event(42, 1); assert_eq!(k.event(30, 1), b(b"A")); assert_eq!(k.event(2, 1), b(b"!")); k.event(42, 0);
        k.event(58, 1); k.event(58, 0); assert_eq!(k.event(30, 1), b(b"A")); assert_eq!(k.event(2, 1), b(b"1"));
        k.event(42, 1); assert_eq!(k.event(30, 1), b(b"a"));   // shift cancels caps on letters
    }
    #[test] fn specials() {
        let mut k = Keys::default();
        assert_eq!(k.event(28, 1), b(b"\r")); assert_eq!(k.event(14, 1), b(b"\x7f")); assert_eq!(k.event(15, 1), b(b"\t")); assert_eq!(k.event(1, 1), b(b"\x1b"));
        assert_eq!(k.event(103, 1), b(b"\x1b[A")); assert_eq!(k.event(108, 2), b(b"\x1b[B")); assert_eq!(k.event(106, 1), b(b"\x1b[C")); assert_eq!(k.event(105, 1), b(b"\x1b[D"));
        assert_eq!(k.event(104, 1), b(b"\x1b[5~")); assert_eq!(k.event(109, 1), b(b"\x1b[6~"));
        assert_eq!(k.event(57, 1), b(b" ")); assert_eq!(k.event(53, 1), b(b"/")); assert_eq!(k.event(43, 1), b(b"\\"));
    }
    #[test] fn meta_acts_like_alt() {
        let mut k = Keys::default(); k.event(125, 1);
        assert_eq!(k.event(28, 1), Some(Key::Act(Action::Split)));
        k.event(125, 0); assert_eq!(k.event(28, 1), b(b"\r"));
        assert_eq!(k.event(102, 1), b(b"\x1b[H")); assert_eq!(k.event(107, 1), b(b"\x1b[F"));
    }
    #[test] fn copy_and_paste_keys() {
        let mut k = Keys::default(); k.event(29, 1); k.event(42, 1);
        assert_eq!(k.event(46, 1), Some(Key::Act(Action::Copy))); assert_eq!(k.event(47, 1), Some(Key::Act(Action::Paste)));
        k.event(42, 0); assert_eq!(k.event(46, 1), b(b"\x03")); assert_eq!(k.event(47, 1), b(b"\x16")); k.event(29, 0);
        k.event(125, 1); assert_eq!(k.event(46, 1), Some(Key::Act(Action::Copy))); assert_eq!(k.event(47, 1), Some(Key::Act(Action::Paste)));
        assert_eq!(k.event(28, 1), Some(Key::Act(Action::Split)), "the Cmd window keys still work"); k.event(125, 0);
        k.event(56, 1); assert_eq!(k.event(46, 1), None, "Alt+C stays swallowed"); k.event(56, 0); assert_eq!(k.event(46, 1), b(b"c"));
    }
    #[test] fn ctrl_letters() {
        let mut k = Keys::default(); k.event(29, 1);
        assert_eq!(k.event(46, 1), b(b"\x03")); assert_eq!(k.event(32, 1), b(b"\x04")); assert_eq!(k.event(2, 1), None);
        k.event(29, 0); assert_eq!(k.event(46, 1), b(b"c"));
    }
    #[test] fn mods_and_wm_keys() {
        let mut k = Keys::default(); assert_eq!(k.mods(), 0);
        k.event(42, 1); k.event(29, 1); assert_eq!(k.mods(), 3); k.event(58, 1); k.event(58, 0); assert_eq!(k.mods(), 11);
        assert!(!k.wm()); k.event(56, 1); assert!(k.wm()); assert!(Keys::is_wm_key(56) && Keys::is_wm_key(125) && !Keys::is_wm_key(30));
    }
    #[test] fn alt_actions() {
        let mut k = Keys::default(); k.event(56, 1);
        assert_eq!(k.event(28, 1), Some(Key::Act(Action::Split))); assert_eq!(k.event(16, 1), Some(Key::Act(Action::Close)));
        assert_eq!(k.event(33, 1), Some(Key::Act(Action::Fullscreen))); assert_eq!(k.event(19, 1), Some(Key::Act(Action::StopMic)));
        k.event(56, 0); assert_eq!(k.event(19, 1), b(b"r")); k.event(56, 1);   // without Alt it is a plain r
        assert_eq!(k.event(105, 1), Some(Key::Act(Action::Focus(Dir::Left)))); assert_eq!(k.event(38, 1), Some(Key::Act(Action::Focus(Dir::Right))));
        assert_eq!(k.event(30, 1), None);
        k.event(56, 0); assert_eq!(k.event(28, 1), b(b"\r"));
    }
}
