//! settings: time zone, wallpaper (a preset of theme::WALLS: navy is the default and the design system itself), sound on or off and the volume. The time zone is ~/.tz (den's clock reads it); the rest is ~/.config/thir/settings.
use super::prefs;
use crate::{clock, files, gui::{theme::{self, ACCENTS, WALLS}, wallpaper}, line::Key, sound, store, tui::*};
use std::path::Path;

/// Makes `file` the wallpaper: decoded in the sandbox to the screen size, written 0600 with its header, then the settings get a new stamp (den reloads when it changes).
pub fn set_picture(file: &str) -> Result<(), String> {
    let (w, h) = std::fs::read_to_string("/run/den/screen").ok().and_then(|t| wallpaper::parse_size(&t)).ok_or("no screen size (is the desktop running?)")?;
    let home = store::home();
    let path = if let Some(rest) = file.strip_prefix("~/") { format!("{home}/{rest}") } else { file.to_string() };
    let raw = super::view::wallpaper(Path::new(&path), w, h)?;
    store::dir("").map_err(|e| e.to_string())?; let (r, hd) = (format!("{home}/{}", wallpaper::RAW), format!("{home}/{}", wallpaper::HEADER));
    for (p, d) in [(&r, &raw[..]), (&hd, wallpaper::header(w, h).as_bytes())] {
        files::save(Path::new(p), d).map_err(|e| e.to_string())?;
        std::fs::set_permissions(p, std::os::unix::fs::PermissionsExt::from_mode(0o600)).map_err(|e| e.to_string())?;
    }
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_nanos()).to_string();
    put("wallpic", &stamp); Ok(())
}
/// Back to a colour wallpaper (the picture files are deleted).
pub fn clear_picture() {
    let home = store::home();
    for p in [wallpaper::RAW, wallpaper::HEADER] { let _ = std::fs::remove_file(format!("{home}/{p}")); }
    let _ = store::write_settings(&prefs::set(&store::read_settings(), "wallpic", ""));
}
/// An accent typed as a hex code: the colour to save and the message. A colour that fails AA is not saved as typed: the message offers the nearest lighter one and `use_adjusted` says whether the user took it.
pub fn judge_accent(text: &str) -> Result<u32, String> {
    let c = theme::parse_hex(text).ok_or("use a hex colour like #7fb2f0")?;
    let r = theme::accent_ratio(c);
    if r >= theme::AA { return Ok(c) }
    let a = theme::adjust_accent(c);
    Err(format!("{:06x} is too dark to read: {r:.1}:1, text needs {:.1}:1. The nearest readable one is #{a:06x} ({:.1}:1)", c, theme::AA, theme::accent_ratio(a)))
}

/// `settings wallpaper FILE|reset` and `settings accent NAME|#rrggbb` without the screen; the exit code.
fn command(args: &[String]) -> i32 {
    let say = |r: Result<String, String>| match r { Ok(m) => { println!("{m}"); 0 } Err(e) => { eprintln!("settings: {e}"); 1 } };
    match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        ["wallpaper", "reset"] => { clear_picture(); println!("colour wallpaper"); 0 }
        ["wallpaper", f] => say(set_picture(f).map(|_| "wallpaper set".into())),
        ["accent", a] => say(match ACCENTS.iter().find(|p| p.0 == a) { Some(p) => { put("accent", p.0); Ok(format!("accent {}", p.0)) } None => judge_accent(a).map(|c| { put("accent", &format!("#{c:06x}")); format!("accent #{c:06x}") }) }),
        _ => { eprintln!("usage: settings   |   settings wallpaper FILE|reset   |   settings accent NAME|#rrggbb"); 2 }
    }
}

/// A swatch in a colour: 24-bit SGR. The peach would map to den's accent slot (it would show the chosen accent), so it is one blue step off.
fn accent_sgr(c: u32) -> String { let c = if c == crate::gui::theme::ACCENT { c + 1 } else { c }; format!("\x1b[38;2;{};{};{}m", c >> 16 & 255, c >> 8 & 255, c & 255) }
const SWATCH: [&str; 5] = ["\x1b[34m", "\x1b[90m", "\x1b[32m", "\x1b[35m", "\x1b[31m"];   // navy graphite forest plum ember, in the 16 terminal colours

fn set_tz(home: &str, text: &str) -> Result<(), String> {
    let off = clock::parse_offset(text.trim()).ok_or("use +HH:MM or -HH:MM, like +05:30")?;
    files::save(std::path::Path::new(&format!("{home}/.tz")), format!("{}\n", clock::format_offset(off)).as_bytes()).map_err(|e| e.to_string())
}
fn put(key: &str, val: &str) { let _ = store::write_settings(&prefs::set(&store::read_settings(), key, val)); }
/// Plays the chime now; the message to show.
pub fn test_sound() -> String {
    let (on, vol) = sound::config(&store::read_settings());
    if !on { return format!("{WARN}sound is off{R}") }
    match sound::play(&sound::chime(vol)) { Ok(()) => format!("{GOOD}\u{2713}{R} played the chime"), Err(e) if e.kind() == std::io::ErrorKind::NotFound => format!("{DIM}no sound device found. Alarms flash and show in the top bar.{R}"), Err(e) => format!("{DIM}the sound device would not play ({e}). Alarms flash and show in the top bar.{R}") }
}

/// Runs a program on the terminal in normal (cooked) mode, then goes back to the app's raw mode; a message for the screen.
fn run_cooked(prog: &str, args: &[&str]) -> String {
    let mut saved: libc::termios = unsafe { std::mem::zeroed() };
    if unsafe { libc::tcgetattr(0, &mut saved) } != 0 { return String::new() }
    let mut t = saved; t.c_lflag |= libc::ICANON | libc::ECHO | libc::ISIG | libc::IEXTEN; t.c_iflag |= libc::ICRNL; t.c_oflag |= libc::OPOST | libc::ONLCR;
    unsafe { libc::tcsetattr(0, libc::TCSANOW, &t); }
    crate::term::put(b"\x1b[0m\x1b[2J\x1b[H\x1b[?25h");
    let ok = std::process::Command::new(prog).args(args).status().is_ok_and(|s| s.success());
    unsafe { libc::tcsetattr(0, libc::TCSANOW, &saved); }
    if ok { format!("{GOOD}\u{2713}{R} done") } else { format!("{WARN}setup did not finish{R}") }
}

pub fn run(args: &[String]) -> i32 {
    if !args.is_empty() { return command(args) }
    let Some(mut s) = Screen::start("settings") else { return 1 };
    let (mut sel, mut note) = (Sel::default(), String::new());
    loop {
        let (cols, rows) = crate::term::size();
        let st = store::read_settings();
        let (on, vol) = sound::config(&st);
        let (wi, pic, mark) = (prefs::wall_index(&st), prefs::pic_stamp(&st).is_some(), prefs::wordmark(&st));
        let acc = prefs::accent_color(&st); let acc_name = ACCENTS.iter().find(|p| p.1 == acc).map_or(format!("#{acc:06x}"), |p| p.0.to_string());
        let tz = clock::format_offset(clock::home_tz().unwrap_or(0));
        let bar = format!("{ACC}{}{R}{}{}{R}", "\u{25ae}".repeat(vol as usize / 10), BDIM, "\u{2500}".repeat(10 - vol as usize / 10));
        let ai_on = std::fs::read_to_string(format!("{}/.config/thir/ai", store::home())).map_or(true, |t| t.trim() != "off");   // ~/.config/thir/ai = off: the user chose no AI
        let mut items: Vec<(&str, String, &str)> = vec![
            ("time zone", format!("UTC{tz}"), "enter to change"), ("wallpaper", if pic { format!("{DIM}your picture{R}") } else { format!("{}\u{25ae}\u{25ae}{R} {}", SWATCH[wi], WALLS[wi].0) }, "left/right colour, enter picture"), ("accent", format!("{}\u{25ae}\u{25ae}{R} {acc_name}", accent_sgr(acc)), "left/right, enter hex"),
            ("wordmark", if mark { format!("{GOOD}on{R}") } else { format!("{DIM}off{R}") }, "enter to switch"),
            ("sound", if on { format!("{GOOD}on{R}") } else { format!("{DIM}off{R}") }, "enter to switch"), ("volume", format!("{bar} {vol}"), "left/right"), ("test sound", format!("{DIM}play the alarm chime{R}"), "enter"),
        ];
        items.push(("ai", if ai_on { format!("{GOOD}on{R}") } else { format!("{DIM}off{R}") }, if ai_on { "enter to turn off (keys stay)" } else { "enter to turn on" }));
        items.push(("provider", "AI provider and key".to_string(), "enter to change"));
        let rows_t: Vec<String> = items.iter().map(|(n, v, h)| format!("{} {} {DIM}{h}{R}", pad(n, 12), pad(v, 24))).collect();
        let p = Page { title: "settings", right: "", count: count(items.len(), "setting", "settings"), header: "setting      value".into(), rows: rows_t,
            foot: vec![], note: note.clone(),
            hints: vec![("up/down", "choose"), ("enter", "change"), ("left/right", "adjust"), ("q", "quit")], empty: "" };
        draw(&p.lines(&mut sel, cols, rows));
        let Some(keys) = s.keys(-1) else { return 0 };
        for k in keys {
            note.clear();
            if sel.key(&k, items.len()) { continue }
            let dir = match k { Key::Left => -1, Key::Right => 1, _ => 0 };
            match (k, sel.pos) {
                (Key::Char('q') | Key::Ctrl(b'c'), _) => return 0,
                (Key::Enter, 0) => if let Some(t) = s.ask("UTC offset (+HH:MM):", &tz) { note = match set_tz(&store::home(), &t) { Ok(()) => format!("{GOOD}\u{2713}{R} time zone saved"), Err(e) => format!("{BAD}{e}{R}") }; },
                (Key::Left | Key::Right, 1) => { clear_picture(); put("wall", WALLS[(wi + (WALLS.len() as i32 + dir) as usize) % WALLS.len()].0) }
                (Key::Enter, 1) => if let Some(t) = s.ask("picture file (~/Downloads/sky.jpg), empty for the colour wallpaper:", "") {
                    note = if t.trim().is_empty() { clear_picture(); format!("{GOOD}\u{2713}{R} colour wallpaper") } else { match set_picture(t.trim()) { Ok(()) => format!("{GOOD}\u{2713}{R} wallpaper set"), Err(e) => format!("{BAD}{e}{R}") } };
                },
                (Key::Left | Key::Right, 2) => { let i = ACCENTS.iter().position(|p| p.1 == acc).map_or(0, |i| (i + (ACCENTS.len() as i32 + dir) as usize) % ACCENTS.len()); put("accent", ACCENTS[i].0) }
                (Key::Enter, 2) => if let Some(t) = s.ask("accent: a name or #rrggbb:", &acc_name) {
                    let t = t.trim().to_string();
                    note = if let Some(p) = ACCENTS.iter().find(|p| p.0 == t) { put("accent", p.0); format!("{GOOD}\u{2713}{R} accent {}", p.0) } else { match judge_accent(&t) {
                        Ok(c) => { put("accent", &format!("#{c:06x}")); format!("{GOOD}\u{2713}{R} accent #{c:06x}") }
                        Err(m) if theme::parse_hex(&t).is_some() => { let a = theme::adjust_accent(theme::parse_hex(&t).unwrap_or(0)); match s.ask(&format!("{m}. Use #{a:06x}? (y/n)"), "y") { Some(x) if x.trim().eq_ignore_ascii_case("y") => { put("accent", &format!("#{a:06x}")); format!("{GOOD}\u{2713}{R} accent #{a:06x}") } _ => format!("{DIM}accent unchanged{R}") } }
                        Err(m) => format!("{BAD}{m}{R}"),
                    } };
                },
                (Key::Enter | Key::Char(' '), 3) => put("wordmark", if mark { "off" } else { "on" }),
                (Key::Enter | Key::Char(' '), 4) => { put("sound", if on { "off" } else { "on" }); }
                (Key::Left | Key::Right, 5) => { put("volume", &(vol as i32 + dir * 10).clamp(0, 100).to_string()); note = test_sound(); }
                (Key::Enter, 6) => note = test_sound(),
                (Key::Enter, 7) => note = if ai_on { match std::fs::create_dir_all(format!("{}/.config/thir", store::home())).and_then(|_| std::fs::write(format!("{}/.config/thir/ai", store::home()), "off\n")) { Ok(()) => format!("{GOOD}\u{2713}{R} the AI is off. Saved keys stay until you remove them."), Err(e) => format!("{BAD}{e}{R}") } } else { run_cooked("/bin/agent", &["setup"]) },
                (Key::Enter, 8) => note = run_cooked("/bin/agent", &["setup"]),
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn tz_input() {
        let d = std::env::temp_dir().join(format!("thir-settings-{}", std::process::id())); let _ = std::fs::remove_dir_all(&d); std::fs::create_dir_all(&d).unwrap();
        let h = d.to_str().unwrap();
        assert!(set_tz(h, "5").is_err()); assert!(set_tz(h, "+15:00").is_err());
        assert!(set_tz(h, "+05:30").is_ok()); assert_eq!(std::fs::read_to_string(d.join(".tz")).unwrap(), "+05:30\n");
        let _ = std::fs::remove_dir_all(d);
    }
    #[test] fn typed_accent_is_judged() {
        assert_eq!(judge_accent("#7fb2f0"), Ok(0x7fb2f0)); assert_eq!(judge_accent("F0B27F"), Ok(0xf0b27f));
        let e = judge_accent("#1a2a6c").unwrap_err(); assert!(e.contains("too dark") && e.contains("4.5:1") && e.contains('#'), "{e}");
        assert!(judge_accent("blue").is_err() && judge_accent("#12").is_err());
    }
    #[test] fn accent_sgr_is_24_bit() { assert_eq!(accent_sgr(0x7fb2f0), "\x1b[38;2;127;178;240m"); }
    #[test] fn swatches_match_presets() { assert_eq!(SWATCH.len(), WALLS.len()); }
}
