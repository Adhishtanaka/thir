//! Settings: ~/.config/thir/settings, `key=value` lines (sound, volume, wall). The time zone stays in ~/.tz, which den reads.
use crate::gui::theme::WALLS;

pub fn get(text: &str, key: &str) -> String {
    text.lines().filter_map(|l| l.split_once('=')).find(|(k, _)| k.trim() == key).map(|(_, v)| crate::files::printable(v.trim())).unwrap_or_default()
}
pub fn set(text: &str, key: &str, val: &str) -> String {
    let mut o: String = text.lines().filter(|l| l.split_once('=').map_or(false, |(k, _)| k.trim() != key)).map(|l| format!("{l}\n")).collect();
    o += &format!("{key}={}\n", val.replace('\n', " ")); o
}
/// The wallpaper preset named in the settings, as an index into WALLS (0 = navy, the default, when missing or unknown; the old name "blue" also maps to it).
pub fn wall_index(text: &str) -> usize { let w = get(text, "wall"); WALLS.iter().position(|p| p.0 == w).unwrap_or(0) }

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn get_and_set() {
        let t = set(&set("", "sound", "off"), "volume", "40");
        assert_eq!(get(&t, "sound"), "off"); assert_eq!(get(&t, "volume"), "40"); assert_eq!(get(&t, "x"), "");
        let t = set(&t, "sound", "on"); assert_eq!(get(&t, "sound"), "on"); assert_eq!(t.lines().count(), 2);
        assert_eq!(get("sound = off\n", "sound"), "off"); assert_eq!(get("junk\n=\n", "junk"), "");
    }
    #[test] fn wallpaper_index() { assert_eq!(wall_index(""), 0); assert_eq!(wall_index("wall=plum\n"), 3); assert_eq!(wall_index("wall=nope"), 0); assert_eq!(WALLS[0].0, "navy"); assert_eq!(wall_index("wall=blue"), 0); }
}
