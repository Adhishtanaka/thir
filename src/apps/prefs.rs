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

/// The accent named in the settings, as an index into ACCENTS (0 = peach when missing or unknown).
pub fn accent_index(text: &str) -> usize { let a = get(text, "accent"); crate::gui::theme::ACCENTS.iter().position(|p| p.0 == a).unwrap_or(0) }

/// The accent colour in the settings: a preset name, or `#rrggbb`. A custom colour that fails AA on any wallpaper is ignored (peach): Settings never saves one, but the file is the user's to edit.
pub fn accent_color(text: &str) -> u32 {
    use crate::gui::theme::*;
    let a = get(text, "accent");
    ACCENTS.iter().find(|p| p.0 == a).map(|p| p.1).or_else(|| parse_hex(&a).filter(|&c| accent_ratio(c) >= AA)).unwrap_or(ACCENT)
}
/// The picture-wallpaper stamp (Settings writes a new one whenever it makes a new picture; den reloads when it changes). None = the colour wallpaper.
pub fn pic_stamp(text: &str) -> Option<String> { Some(get(text, "wallpic")).filter(|s| !s.is_empty()) }
/// Is the wordmark shown on the wallpaper? Explicit `wordmark=on|off`; otherwise on for the colour wallpaper, off for a picture.
pub fn wordmark(text: &str) -> bool { match get(text, "wordmark").as_str() { "on" => true, "off" => false, _ => pic_stamp(text).is_none() } }

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn get_and_set() {
        let t = set(&set("", "sound", "off"), "volume", "40");
        assert_eq!(get(&t, "sound"), "off"); assert_eq!(get(&t, "volume"), "40"); assert_eq!(get(&t, "x"), "");
        let t = set(&t, "sound", "on"); assert_eq!(get(&t, "sound"), "on"); assert_eq!(t.lines().count(), 2);
        assert_eq!(get("sound = off\n", "sound"), "off"); assert_eq!(get("junk\n=\n", "junk"), "");
    }
    #[test] fn custom_accent_and_picture_keys() {
        assert_eq!(accent_color("accent=#7fb2f0"), 0x7fb2f0); assert_eq!(accent_color("accent=lilac"), 0xc3a3f0); assert_eq!(accent_color("accent=#101010"), crate::gui::theme::ACCENT); assert_eq!(accent_color("accent=zzz"), crate::gui::theme::ACCENT);
        assert_eq!(pic_stamp(""), None); assert_eq!(pic_stamp("wallpic=17\n"), Some("17".into()));
        assert!(wordmark("")); assert!(!wordmark("wallpic=1")); assert!(wordmark("wallpic=1\nwordmark=on")); assert!(!wordmark("wordmark=off"));
    }
    #[test] fn accent_names() { assert_eq!(accent_index(""), 0); assert_eq!(accent_index("accent=blue\n"), 1); assert_eq!(accent_index("accent=nope"), 0); }
    #[test] fn wallpaper_index() { assert_eq!(wall_index(""), 0); assert_eq!(wall_index("wall=plum\n"), 3); assert_eq!(wall_index("wall=nope"), 0); assert_eq!(WALLS[0].0, "navy"); assert_eq!(wall_index("wall=blue"), 0); }
}
