//! The microphone broker (den's side). Only /bin/rec may ask; den asks the user on its OWN bar and takes the answer from the physical keyboard (evdev), then opens the capture node itself
//! (it is root 0600 for everyone else) and relays the samples. The state machine is pure: time and keys come in as arguments, what den must do comes out as `Do`.
use std::{os::unix::fs::MetadataExt, path::Path};

/// `Down::Mic` states.
pub const ASKED: u32 = 0; pub const GRANTED: u32 = 1; pub const DENIED: u32 = 2; pub const STOPPED: u32 = 3;
/// No answer in this long = no.
pub const ASK_MS: u64 = 30_000;
const KEY_Y: u16 = 21;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum State { Idle, Asking { pane: usize, at: u64 }, Live { pane: usize, at: u64 } }
/// What den must do: tell a pane's app (pane, state, rate), open the capture device (then call `opened`), close it.
#[derive(Debug, Clone, PartialEq)]
pub enum Do { Tell(usize, u32, u32), Open, Close }

pub struct Broker { pub state: State }

/// Keys that are only modifiers: pressing one is not an answer.
fn modifier(code: u16) -> bool { matches!(code, 29 | 42 | 54 | 56 | 58 | 97 | 100 | 125 | 126) }

impl Broker {
    pub fn new() -> Broker { Broker { state: State::Idle } }
    /// MIC_START from the app of `pane`. `exe_ok` = its program is /bin/rec: anything else gets no prompt at all (den drops it). One request at a time: a second one is denied.
    pub fn start(&mut self, pane: usize, exe_ok: bool, now: u64) -> Vec<Do> {
        if !exe_ok { return vec![] }
        match self.state {
            State::Idle => { self.state = State::Asking { pane, at: now }; vec![Do::Tell(pane, ASKED, 0)] }
            _ => vec![Do::Tell(pane, DENIED, 0)],
        }
    }
    /// A physical key press (value 1) while the question is up: `y` = yes (den opens the device), any other key that is not a modifier = no.
    pub fn key(&mut self, code: u16, _now: u64) -> Vec<Do> {
        let State::Asking { pane, .. } = self.state else { return vec![] };
        if modifier(code) { return vec![] }
        if code == KEY_Y { return vec![Do::Open] }
        self.state = State::Idle; vec![Do::Tell(pane, DENIED, 0)]
    }
    /// den tried to open the device after a yes: `rate` = it worked.
    pub fn opened(&mut self, rate: Option<u32>, now: u64) -> Vec<Do> {
        let State::Asking { pane, .. } = self.state else { return vec![Do::Close] };
        match rate {
            Some(r) => { self.state = State::Live { pane, at: now }; vec![Do::Tell(pane, GRANTED, r)] }
            None => { self.state = State::Idle; vec![Do::Tell(pane, DENIED, 0)] }
        }
    }
    /// Called often: an unanswered question times out.
    pub fn tick(&mut self, now: u64) -> Vec<Do> {
        match self.state { State::Asking { pane, at } if now.saturating_sub(at) >= ASK_MS => { self.state = State::Idle; vec![Do::Tell(pane, DENIED, 0)] } _ => vec![] }
    }
    /// Alt+R, MIC_STOP of `pane` (pass None for Alt+R: whoever it is) or logout: an open question is a no, a live recording ends. Close comes before the message.
    pub fn stop(&mut self, pane: Option<usize>) -> Vec<Do> {
        match self.state {
            State::Asking { pane: p, .. } if pane.map_or(true, |q| q == p) => { self.state = State::Idle; vec![Do::Tell(p, DENIED, 0)] }
            State::Live { pane: p, .. } if pane.map_or(true, |q| q == p) => { self.state = State::Idle; vec![Do::Close, Do::Tell(p, STOPPED, 0)] }
            _ => vec![],
        }
    }
    /// The app of `pane` disconnected, or its pane closed: nobody to tell.
    pub fn gone(&mut self, pane: usize) -> Vec<Do> {
        match self.state {
            State::Asking { pane: p, .. } if p == pane => { self.state = State::Idle; vec![] }
            State::Live { pane: p, .. } if p == pane => { self.state = State::Idle; vec![Do::Close] }
            _ => vec![],
        }
    }
    /// Seconds recorded so far (for the bar).
    pub fn seconds(&self, now: u64) -> Option<u64> { if let State::Live { at, .. } = self.state { Some(now.saturating_sub(at) / 1000) } else { None } }
    pub fn asking(&self) -> Option<usize> { if let State::Asking { pane, .. } = self.state { Some(pane) } else { None } }
}

/// Is `exe` (what /proc/PID/exe says) the recorder? /bin/rec is its own program, not a link to /bin/tools, so only that one file qualifies; both sides are resolved anyway. The program must belong to `owner` (root) and nobody else can write it.
pub fn exe_ok(exe: &Path, rec: &Path, owner: u32) -> bool {
    let (Ok(a), Ok(b)) = (exe.canonicalize(), rec.canonicalize()) else { return false };
    a == b && std::fs::metadata(&a).is_ok_and(|m| m.is_file() && m.uid() == owner && m.mode() & 0o022 == 0)
}

/// 72 -> "1:12".
pub fn clock(s: u64) -> String { format!("{}:{:02}", s / 60, s % 60) }

#[cfg(test)]
mod tests {
    use super::*;
    const OTHER: u16 = 49;   // n
    fn asking() -> Broker { let mut b = Broker::new(); assert_eq!(b.start(2, true, 1000), vec![Do::Tell(2, ASKED, 0)]); b }
    fn live() -> Broker { let mut b = asking(); assert_eq!(b.key(KEY_Y, 1500), vec![Do::Open]); assert_eq!(b.opened(Some(48000), 2000), vec![Do::Tell(2, GRANTED, 48000)]); b }

    #[test] fn only_rec_can_raise_the_prompt() {
        let mut b = Broker::new();
        assert_eq!(b.start(1, false, 0), vec![]); assert_eq!(b.state, State::Idle); assert_eq!(b.asking(), None);
        assert_eq!(b.key(KEY_Y, 1), vec![]);   // a y with no question opens nothing
    }
    #[test] fn yes_opens_and_grants() {
        let b = live(); assert_eq!(b.state, State::Live { pane: 2, at: 2000 }); assert_eq!(b.seconds(14_500), Some(12)); assert_eq!(b.asking(), None);
    }
    #[test] fn any_other_key_is_no() {
        for code in [OTHER, 1, 28, 57, 30, 16] { let mut b = asking(); assert_eq!(b.key(code, 1500), vec![Do::Tell(2, DENIED, 0)], "{code}"); assert_eq!(b.state, State::Idle); }
    }
    #[test] fn modifiers_are_not_an_answer() {
        let mut b = asking();
        for code in [42, 54, 29, 97, 56, 100, 125, 126, 58] { assert_eq!(b.key(code, 1500), vec![], "{code}"); }
        assert_eq!(b.asking(), Some(2)); assert_eq!(b.key(KEY_Y, 1600), vec![Do::Open]);
    }
    #[test] fn no_answer_in_30_seconds_is_no() {
        let mut b = asking();
        assert_eq!(b.tick(1000 + ASK_MS - 1), vec![]); assert_eq!(b.asking(), Some(2));
        assert_eq!(b.tick(1000 + ASK_MS), vec![Do::Tell(2, DENIED, 0)]); assert_eq!(b.state, State::Idle); assert_eq!(b.tick(99_999_999), vec![]);
        let mut l = live(); assert_eq!(l.tick(999_999_999), vec![]); assert!(matches!(l.state, State::Live { .. }));   // a live recording has no timeout
    }
    #[test] fn failing_to_open_is_no() {
        let mut b = asking(); b.key(KEY_Y, 1500);
        assert_eq!(b.opened(None, 1600), vec![Do::Tell(2, DENIED, 0)]); assert_eq!(b.state, State::Idle);
        assert_eq!(Broker::new().opened(Some(48000), 0), vec![Do::Close]);   // opened without a question: close it again
    }
    #[test] fn a_second_start_is_denied_and_changes_nothing() {
        let mut b = asking(); assert_eq!(b.start(3, true, 1200), vec![Do::Tell(3, DENIED, 0)]); assert_eq!(b.state, State::Asking { pane: 2, at: 1000 });
        let mut l = live(); assert_eq!(l.start(3, true, 3000), vec![Do::Tell(3, DENIED, 0)]); assert_eq!(l.start(2, true, 3000), vec![Do::Tell(2, DENIED, 0)]); assert_eq!(l.state, State::Live { pane: 2, at: 2000 });
        assert_eq!(l.start(3, false, 3000), vec![]);   // and a wrong program still gets nothing
    }
    #[test] fn alt_r_ends_a_recording_and_closes_before_telling() {
        let mut b = live(); assert_eq!(b.stop(None), vec![Do::Close, Do::Tell(2, STOPPED, 0)]); assert_eq!(b.state, State::Idle); assert_eq!(b.stop(None), vec![]);
    }
    #[test] fn mic_stop_from_the_owner_only() {
        let mut b = live(); assert_eq!(b.stop(Some(3)), vec![]); assert!(matches!(b.state, State::Live { .. }));
        assert_eq!(b.stop(Some(2)), vec![Do::Close, Do::Tell(2, STOPPED, 0)]);
        let mut a = asking(); assert_eq!(a.stop(Some(2)), vec![Do::Tell(2, DENIED, 0)]); assert_eq!(a.state, State::Idle);
    }
    #[test] fn disconnect_or_pane_close_closes_the_device() {
        let mut b = live(); assert_eq!(b.gone(3), vec![]); assert!(matches!(b.state, State::Live { .. }));
        assert_eq!(b.gone(2), vec![Do::Close]); assert_eq!(b.state, State::Idle);
        let mut a = asking(); assert_eq!(a.gone(2), vec![]); assert_eq!(a.state, State::Idle);
    }
    #[test] fn after_an_end_the_next_request_is_asked_again() {
        let mut b = live(); b.stop(None);
        assert_eq!(b.start(4, true, 9000), vec![Do::Tell(4, ASKED, 0)]);   // never "remembered": every recording asks
    }
    #[test] fn exe_must_be_the_root_owned_recorder() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let d = std::env::temp_dir().join(format!("thir-mic-{}", std::process::id())); std::fs::create_dir_all(&d).unwrap();
        let (tools, rec, copy) = (d.join("tools"), d.join("rec"), d.join("copy"));
        std::fs::write(&tools, "x").unwrap(); std::fs::write(&copy, "x").unwrap(); symlink(&tools, &rec).unwrap();
        let me = unsafe { libc::getuid() };
        assert!(exe_ok(&tools, &rec, me));                        // /proc/PID/exe shows the link's target
        assert!(exe_ok(&rec, &rec, me));
        assert!(!exe_ok(&copy, &rec, me));                         // the same bytes at another path
        assert!(!exe_ok(&d.join("nothing"), &rec, me)); assert!(!exe_ok(&tools, &d.join("nothing"), me));
        assert!(!exe_ok(&tools, &rec, me.wrapping_add(1)));       // not owned by the expected (root) owner
        std::fs::set_permissions(&tools, std::fs::Permissions::from_mode(0o666)).unwrap(); assert!(!exe_ok(&tools, &rec, me));   // anyone could write it
        let real = d.join("realrec"); std::fs::write(&real, "y").unwrap();   // the real layout: /bin/rec is a file of its own
        assert!(exe_ok(&real, &real, me)); assert!(!exe_ok(&tools, &real, me)); assert!(!exe_ok(&copy, &real, me));   // /bin/tools (and a copy of it named rec) no longer qualify
        std::fs::remove_dir_all(&d).unwrap();
    }
    #[test] fn clock_format() { assert_eq!(clock(72), "1:12"); assert_eq!(clock(0), "0:00"); assert_eq!(clock(3725), "62:05"); }
}
