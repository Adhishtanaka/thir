//! evdev pointers: tablets (absolute axes) and mice (relative axes) drive one cursor in screen pixels; buttons and the wheel pass through.
pub const EV_SYN: u16 = 0; pub const EV_KEY: u16 = 1; pub const EV_REL: u16 = 2; pub const EV_ABS: u16 = 3;
pub const BTN_LEFT: u16 = 0x110;   // BTN_LEFT..=BTN_EXTRA (0x114) are the five mouse buttons

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum Ptr { Moved, Button(u16, bool), Scroll(i32, i32) }   // Scroll(dx, dy): dy > 0 = wheel up (away from the user), dx > 0 = right

pub struct Pointer { pub x: i32, pub y: i32, w: i32, h: i32, moved: bool }

impl Pointer {
    pub fn new(w: usize, h: usize) -> Pointer { Pointer { x: w as i32 / 2, y: h as i32 / 2, w: w as i32, h: h as i32, moved: false } }

    /// One input event. `abs` = the device's (min, max) for ABS_X and ABS_Y (None for a mouse). A move is reported at the next EV_SYN, so ABS_X and ABS_Y arrive together.
    pub fn event(&mut self, abs: Option<[(i32, i32); 2]>, ty: u16, code: u16, val: i32) -> Option<Ptr> {
        match (ty, code) {
            (EV_SYN, 0) => return std::mem::take(&mut self.moved).then_some(Ptr::Moved),
            (EV_REL, 0) => { self.x = (self.x as i64 + val as i64).clamp(0, self.w as i64 - 1) as i32; self.moved = true; }
            (EV_REL, 1) => { self.y = (self.y as i64 + val as i64).clamp(0, self.h as i64 - 1) as i32; self.moved = true; }
            (EV_REL, 8) => return Some(Ptr::Scroll(0, val)),
            (EV_REL, 6) => return Some(Ptr::Scroll(val, 0)),
            (EV_ABS, 0 | 1) => {
                let (min, max) = abs?[code as usize];
                if max <= min { return None }
                let size = if code == 0 { self.w } else { self.h } as i64;
                let v = ((val as i64 - min as i64) * (size - 1) / (max as i64 - min as i64)).clamp(0, size - 1) as i32;
                if code == 0 { self.x = v } else { self.y = v }
                self.moved = true;
            }
            (EV_KEY, BTN_LEFT..=0x114) if val < 2 => return Some(Ptr::Button(code, val == 1)),
            _ => {}
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const TAB: Option<[(i32, i32); 2]> = Some([(0, 32767), (0, 32767)]);
    #[test] fn abs_scales_to_the_screen() {
        let mut p = Pointer::new(1280, 800);
        assert_eq!(p.event(TAB, EV_ABS, 0, 0), None); assert_eq!(p.event(TAB, EV_ABS, 1, 32767), None);
        assert_eq!(p.event(TAB, EV_SYN, 0, 0), Some(Ptr::Moved)); assert_eq!((p.x, p.y), (0, 799));
        p.event(TAB, EV_ABS, 0, 32767); p.event(TAB, EV_ABS, 1, 0); p.event(TAB, EV_SYN, 0, 0); assert_eq!((p.x, p.y), (1279, 0));
        p.event(TAB, EV_ABS, 0, 16384); p.event(TAB, EV_SYN, 0, 0); assert_eq!(p.x, 639);   // 16384 * 1279 / 32767, rounded down
        p.event(TAB, EV_ABS, 0, 99999); assert_eq!(p.x, 1279);   // out of range: clamped
        p.event(TAB, EV_ABS, 0, -5); assert_eq!(p.x, 0);
        let mut q = Pointer::new(100, 100); q.event(Some([(100, 200), (0, 0)]), EV_ABS, 0, 150); assert_eq!(q.x, 49);
        assert_eq!(q.event(Some([(5, 5), (0, 0)]), EV_ABS, 0, 5), None);   // empty range: ignored
        assert_eq!(q.event(None, EV_ABS, 0, 5), None);                     // abs event from a device without a range
    }
    #[test] fn rel_moves_and_clamps() {
        let mut p = Pointer::new(100, 50); assert_eq!((p.x, p.y), (50, 25));
        p.event(None, EV_REL, 0, 10); p.event(None, EV_REL, 1, -5); assert_eq!(p.event(None, EV_SYN, 0, 0), Some(Ptr::Moved)); assert_eq!((p.x, p.y), (60, 20));
        assert_eq!(p.event(None, EV_SYN, 0, 0), None);   // nothing moved since
        p.event(None, EV_REL, 0, 1000); p.event(None, EV_REL, 1, -1000); assert_eq!((p.x, p.y), (99, 0));
        p.event(None, EV_REL, 0, i32::MIN); assert_eq!(p.x, 0);
    }
    #[test] fn buttons_and_wheel() {
        let mut p = Pointer::new(10, 10);
        assert_eq!(p.event(None, EV_KEY, 0x110, 1), Some(Ptr::Button(0x110, true))); assert_eq!(p.event(None, EV_KEY, 0x110, 0), Some(Ptr::Button(0x110, false)));
        assert_eq!(p.event(None, EV_KEY, 0x111, 1), Some(Ptr::Button(0x111, true))); assert_eq!(p.event(None, EV_KEY, 0x114, 1), Some(Ptr::Button(0x114, true)));
        assert_eq!(p.event(None, EV_KEY, 0x110, 2), None);   // autorepeat
        assert_eq!(p.event(None, EV_KEY, 30, 1), None); assert_eq!(p.event(None, EV_KEY, 0x115, 1), None);   // a key, a non-mouse button
        assert_eq!(p.event(None, EV_REL, 8, 1), Some(Ptr::Scroll(0, 1))); assert_eq!(p.event(None, EV_REL, 8, -2), Some(Ptr::Scroll(0, -2)));
        assert_eq!(p.event(None, EV_REL, 6, 3), Some(Ptr::Scroll(3, 0)));
        assert_eq!(p.event(None, EV_REL, 11, 120), None);   // hi-res wheel duplicates REL_WHEEL: ignored
    }
}
