//! settings: time zone, wallpaper, sound on or off and the volume. The time zone is ~/.tz (den's clock reads it); the rest is ~/.config/thir/settings.
use super::prefs;
use crate::{clock, files, gui::theme::WALLS, line::Key, sound, store, tui::*};

const SWATCH: [&str; 5] = ["\x1b[34m", "\x1b[90m", "\x1b[32m", "\x1b[35m", "\x1b[31m"];   // blue graphite forest plum ember, in the 16 terminal colours

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

pub fn run() -> i32 {
    let Some(mut s) = Screen::start("settings") else { return 1 };
    let (mut sel, mut note) = (Sel::default(), String::new());
    loop {
        let (cols, rows) = crate::term::size();
        let st = store::read_settings();
        let (on, vol) = sound::config(&st);
        let wi = prefs::wall_index(&st);
        let tz = clock::format_offset(clock::home_tz().unwrap_or(0));
        let bar = format!("{}{}", "\u{2588}".repeat(vol as usize / 10), "\u{2591}".repeat(10 - vol as usize / 10));
        let items: Vec<(&str, String, &str)> = vec![
            ("Time zone", format!("{CYAN}UTC{tz}{R}"), "enter to change"), ("Wallpaper", format!("{}\u{2588}\u{2588}{R} {ACC}{}{R}", SWATCH[wi], WALLS[wi].0), "enter or left/right"),
            ("Sound", if on { format!("{GOOD}\u{25cf} on{R}") } else { format!("{DIM}\u{25cb} off{R}") }, "enter to switch"), ("Volume", format!("{CYAN}{bar}{R} {vol}%"), "left/right"), ("Test sound", format!("{DIM}play the alarm chime{R}"), "enter"),
        ];
        let rows_t: Vec<String> = items.iter().map(|(n, v, h)| format!("{ACC}{}{R} {} {DIM}{h}{R}", pad(n, 12), pad(v, 24))).collect();
        let p = Page { title: "SETTINGS", right: "", count: count(items.len(), "setting", "settings"), header: "SETTING      VALUE".into(), rows: rows_t,
            foot: vec![format!("{CYAN}UTC{tz}{R}"), format!("{} wallpaper", WALLS[wi].0), if on { format!("{GOOD}sound on {vol}%{R}") } else { format!("{DIM}sound off{R}") }], note: note.clone(),
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
                (Key::Enter | Key::Left | Key::Right, 1) => put("wall", WALLS[(wi + if dir == 0 { 1 } else { (WALLS.len() as i32 + dir) as usize }) % WALLS.len()].0),
                (Key::Enter | Key::Char(' '), 2) => { put("sound", if on { "off" } else { "on" }); }
                (Key::Left | Key::Right, 3) => { put("volume", &(vol as i32 + dir * 10).clamp(0, 100).to_string()); note = test_sound(); }
                (Key::Enter, 4) => note = test_sound(),
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
    #[test] fn swatches_match_presets() { assert_eq!(SWATCH.len(), WALLS.len()); }
}
