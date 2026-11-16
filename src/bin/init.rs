use std::{ffi::CStr, fs::{self, File}, io, os::unix::process::CommandExt, process::Command, thread::sleep, time::Duration};
use thir::{decide, net, Action};

/// The real tty behind /dev/console. /dev/console cannot be a controlling terminal, so Ctrl-C needs this.
fn console_tty() -> Option<File> {
    let active = fs::read_to_string("/sys/class/tty/console/active").ok()?;
    let name = active.split_whitespace().last()?;                 // the last one listed is /dev/console
    let name = if name == "tty0" { "tty1" } else { name };        // tty0 is an alias for "current VT"
    File::options().read(true).write(true).open(format!("/dev/{name}")).ok()
}

fn shell() -> Command {
    let mut cmd = Command::new("/bin/nsh");
    cmd.env_clear().env("PATH", "/bin").env("HOME", "/");
    if let Some(tty) = console_tty() {
        if let (Ok(i), Ok(o)) = (tty.try_clone(), tty.try_clone()) {
            cmd.stdin(i).stdout(o).stderr(tty);
            // new session, then make the tty (now fd 0) its controlling terminal
            unsafe { cmd.pre_exec(|| { libc::setsid(); libc::ioctl(0, libc::TIOCSCTTY as _, 0); Ok(()) }); }
        }
    }
    cmd
}

fn mount(src: &CStr, dir: &CStr, fs: &CStr, data: &CStr) {
    if unsafe { libc::mount(src.as_ptr(), dir.as_ptr(), fs.as_ptr(), 0, data.as_ptr() as *const _) } != 0 {
        eprintln!("thir: mount {}: {}", dir.to_str().unwrap(), io::Error::last_os_error());   // warn, never die
    }
}
fn shutdown(cmd: libc::c_int) -> ! {
    unsafe { libc::sync(); libc::reboot(cmd); }
    eprintln!("thir: reboot(2) failed: {}", io::Error::last_os_error());
    loop { sleep(Duration::from_secs(60)); }        // PID 1 must never return
}

fn main() {
    mount(c"devtmpfs", c"/dev", c"devtmpfs", c"mode=0755");   // first, so /dev/console etc. exist for everything after
    mount(c"proc", c"/proc", c"proc", c"");
    mount(c"sysfs", c"/sys", c"sysfs", c"");
    mount(c"tmpfs", c"/tmp", c"tmpfs", c"mode=1777");
    mount(c"tmpfs", c"/run", c"tmpfs", c"mode=0755");
    println!("thir: mounted /dev /proc /sys /tmp /run");
    if unsafe { libc::sethostname(c"thir".as_ptr(), 4) } != 0 { eprintln!("thir: sethostname: {}", io::Error::last_os_error()); }
    if let Err(e) = net::set_up("lo") { eprintln!("thir: lo up: {e}"); }
    match net::interfaces() { Ok(v) => for i in &v { println!("thir: iface {}", i.line()); }, Err(e) => eprintln!("thir: interfaces: {e}") }
    if let Err(e) = Command::new("/bin/dhcp").env_clear().spawn() { eprintln!("thir: spawn /bin/dhcp: {e}"); }   // runs beside the shell; reaped below
    println!("thir: init ready");                                   // smoke-test marker; do not change without updating test/smoke.sh
    loop {
        let child = match shell().spawn() {
            Ok(c) => c,
            Err(e) => { eprintln!("thir: spawn /bin/nsh: {e}"); sleep(Duration::from_secs(1)); continue; }
        };
        let shell = child.id() as libc::pid_t;                       // do NOT call child.wait(): waitpid(-1) below reaps it
        let status = loop {
            let mut st = 0;
            let pid = unsafe { libc::waitpid(-1, &mut st, 0) };
            if pid == shell { break st; }
            if pid < 0 && io::Error::last_os_error().raw_os_error() != Some(libc::EINTR) { break 0; } // ECHILD: shell gone; restart
        };
        match decide(status) {
            Action::Restart => { println!("thir: shell exited, restarting"); sleep(Duration::from_secs(1)); } // guards against a respawn storm
            Action::Poweroff => shutdown(libc::LINUX_REBOOT_CMD_POWER_OFF),
            Action::Reboot => shutdown(libc::LINUX_REBOOT_CMD_RESTART),
        }
    }
}
