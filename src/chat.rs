//! The AI conversation of one shell (`! words`, then the replies at the `↳` prompt). nsh keeps it in memory only (nothing is written to disk) and hands it to /bin/agent
//! in the environment (THIR_CHAT); agent adds the system snapshot to the newest message only, so the model always sees the current state.
//! Size: the last KEEP messages stay whole (each cut to MAX_MSG), older ones shrink to OLD characters, and the oldest go first when the whole is over MAX_TOTAL.
//! shortcut: older turns are cut, not summarised by the model; make agent write a summary when conversations need to run longer.

pub const IDLE: i64 = 600;      // seconds after the last exchange in which a new `!` line continues the conversation
const KEEP: usize = 6;          // messages kept whole (3 exchanges)
const MAX_MSG: usize = 600;
const OLD: usize = 60;
const MAX_TOTAL: usize = 3000;
const SEP: char = '\u{1e}';

#[derive(Default, Debug, PartialEq, Clone)]
pub struct Chat { pub msgs: Vec<(bool, String)>, pub last: i64 }   // (true = the user, text)

fn cut(s: &str, n: usize) -> String {
    let s: String = s.chars().filter(|&c| c == '\n' || c == '\t' || !c.is_control()).collect();
    let s = s.trim();
    if s.chars().count() <= n { s.to_string() } else { format!("{}\u{2026}", s.chars().take(n).collect::<String>().trim_end()) }
}

impl Chat {
    /// A conversation that is not empty and had its last exchange less than IDLE seconds before `now`.
    pub fn live(&self, now: i64) -> bool { !self.msgs.is_empty() && now - self.last < IDLE }
    pub fn clear(&mut self) { self.msgs.clear(); }
    /// One exchange: what the user said and what the AI answered. Then the size rules.
    pub fn add(&mut self, user: &str, ai: &str, now: i64) {
        self.msgs.push((true, cut(user, MAX_MSG))); self.msgs.push((false, cut(ai, MAX_MSG)));
        self.last = now;
        let n = self.msgs.len();
        for (i, m) in self.msgs.iter_mut().enumerate() { if i + KEEP < n { m.1 = cut(&m.1, OLD); } }
        while self.msgs.len() > 2 && self.msgs.iter().map(|m| m.1.len()).sum::<usize>() > MAX_TOTAL { self.msgs.drain(..2); }
    }
    /// For the environment of /bin/agent: records `u` or `a` and the text, separated by U+001E. "" for an empty conversation.
    pub fn encode(&self) -> String { self.msgs.iter().map(|(u, t)| format!("{}{}", if *u { 'u' } else { 'a' }, t.replace(SEP, " "))).collect::<Vec<_>>().join(&SEP.to_string()) }
    /// The other way. The input is not trusted: messages must alternate, user first (anything else is dropped from the front), and each is cut again.
    pub fn decode(s: &str) -> Vec<(bool, String)> {
        let mut v: Vec<(bool, String)> = s.split(SEP).filter_map(|r| { let (k, t) = r.split_at(r.chars().next()?.len_utf8()); let t = cut(t, MAX_MSG); (!t.is_empty() && (k == "u" || k == "a")).then(|| (k == "u", t)) }).collect();
        let mut keep: Vec<(bool, String)> = vec![];
        for m in v.drain(..) { if m.0 == (keep.len() % 2 == 0) { keep.push(m); } }   // even places are the user's
        if keep.len() % 2 == 1 { keep.pop(); }                                              // the last one must be the AI's: the new line is the next user message
        let n = keep.len().saturating_sub(2 * KEEP + 4); keep.drain(..n);
        keep
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn exchange_roundtrip() {
        let mut c = Chat::default(); assert!(!c.live(0)); assert_eq!(c.encode(), "");
        c.add("remind me to call mum", "ANSWER\nWhat time?", 100);
        assert!(c.live(100 + IDLE - 1) && !c.live(100 + IDLE));
        assert_eq!(Chat::decode(&c.encode()), c.msgs);
        assert_eq!(c.msgs, vec![(true, "remind me to call mum".to_string()), (false, "ANSWER\nWhat time?".to_string())]);
        c.clear(); assert!(!c.live(100));
    }
    #[test] fn old_turns_shrink_and_the_total_is_capped() {
        let mut c = Chat::default();
        for i in 0..12 { c.add(&format!("question {i} {}", "x".repeat(300)), &format!("answer {i} {}", "y".repeat(300)), i); }
        let n = c.msgs.len();
        assert!(c.msgs.iter().map(|m| m.1.len()).sum::<usize>() <= MAX_TOTAL + 8, "{n}");
        assert!(c.msgs[n - 1].1.len() > 300 && c.msgs[n - KEEP].1.len() > 300, "the last {KEEP} stay whole");
        assert!(c.msgs[0].1.chars().count() <= OLD + 1 || n <= KEEP, "older ones are short");
        assert!(c.msgs[n - 1].1.starts_with("answer 11") && c.msgs[0].0 && n % 2 == 0);
        assert!(c.msgs.iter().take(n - KEEP).all(|m| m.1.chars().count() <= OLD + 1));
    }
    #[test] fn decode_does_not_trust_its_input() {
        assert!(Chat::decode("").is_empty());
        assert_eq!(Chat::decode("aanswer\u{1e}uhi\u{1e}ahello\u{1e}ufoo").len(), 2);          // starts with the AI: dropped to u,a and the open user message goes
        assert_eq!(Chat::decode("uone\u{1e}utwo\u{1e}athree"), vec![(true, "one".to_string()), (false, "three".to_string())]);
        assert!(Chat::decode("xbad\u{1e}u\u{1e}a").is_empty());
        assert_eq!(Chat::decode(&format!("u{}\u{1e}a{}", "q".repeat(5000), "a\x1b[2J")).iter().map(|m| m.1.chars().count()).collect::<Vec<_>>(), [MAX_MSG + 1, 4]);
    }
    #[test] fn control_characters_and_separators_never_get_in() {
        let mut c = Chat::default(); c.add("a\x1b[2Jb\u{1e}c", "d\0e", 1);
        assert!(!c.encode().contains('\x1b') && !c.encode().contains('\0') && c.encode().matches('\u{1e}').count() == 1, "{:?}", c.encode());
    }
}
