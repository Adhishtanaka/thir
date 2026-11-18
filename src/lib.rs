pub mod clock;
pub mod dhcp;
pub mod disk;
pub mod edit;
pub mod fetch;
pub mod files;
pub mod glob;
pub mod gui;
pub mod http;
pub mod line;
pub mod net;
pub mod parse;
pub mod procinfo;
pub mod term;
pub mod user;

/// nsh exit codes that init turns into actions. Not 101: that is Rust's panic exit code.
pub const EXIT_POWEROFF: i32 = 120;
pub const EXIT_REBOOT: i32 = 121;

#[derive(Debug, PartialEq)]
pub enum Action { Restart, Poweroff, Reboot }

/// `status` is the raw waitpid status.
pub fn decide(status: i32) -> Action {
    if libc::WIFEXITED(status) {
        match libc::WEXITSTATUS(status) { EXIT_POWEROFF => Action::Poweroff, EXIT_REBOOT => Action::Reboot, _ => Action::Restart }
    } else { Action::Restart }
}

#[cfg(test)]
mod tests {   // raw status: exit code c => c << 8; killed by signal n => n
    use super::*;
    #[test] fn normal_exit_restarts() { assert_eq!(decide(0 << 8), Action::Restart); assert_eq!(decide(1 << 8), Action::Restart); }
    #[test] fn poweroff_code() { assert_eq!(decide(EXIT_POWEROFF << 8), Action::Poweroff); }
    #[test] fn reboot_code() { assert_eq!(decide(EXIT_REBOOT << 8), Action::Reboot); }
    #[test] fn killed_by_signal_restarts() { assert_eq!(decide(9), Action::Restart); }
}
