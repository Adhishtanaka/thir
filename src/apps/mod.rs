//! The built-in apps. Each module holds the data format and logic (tested) and the full-screen `run` of one app; src/bin/tools is the one binary that starts them by name.
pub mod alarm;
pub mod bell;
pub mod cal;
pub mod calc;
pub mod contacts;
pub mod help;
pub mod notes;
pub mod prefs;
pub mod settings;

/// The apps `tools` starts by its own name (symlinks in /bin).
pub const NAMES: &[&str] = &["clock", "calendar", "calc", "contacts", "notes", "settings", "help", "beep", "bell"];

/// Starts the app called `name`; the exit code.
pub fn run(name: &str, args: &[String]) -> i32 {
    match name {
        "clock" => alarm::run(), "calendar" => cal::run(), "calc" => calc::run(), "contacts" => contacts::run(), "notes" => notes::run(),
        "settings" => settings::run(), "help" => help::run(), "bell" => bell::run(), "beep" => bell::beep(args),
        _ => { eprintln!("thir: unknown app {name}"); 2 }
    }
}
