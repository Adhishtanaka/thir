use std::{ffi::CStr, fs::{self, File}, io, os::unix::process::CommandExt, path::Path, process::Command, thread::sleep, time::{Duration, Instant}};
use thir::{decide, net, Action};

/// The real tty behind /dev/console. /dev/console cannot be a controlling terminal, so Ctrl-C needs this.
fn console_tty() -> Option<File> {
    let active = fs::read_to_string("/sys/class/tty/console/active").ok()?;
    let name = active.split_whitespace().last()?;                 // the last one listed is /dev/console
    let name = if name == "tty0" { "tty1" } else { name };        // tty0 is an alias for "current VT"
    File::options().read(true).write(true).open(format!("/dev/{name}")).ok()
}

/// The serial console session: /bin/login on the real console tty.
fn session() -> Command {
    let mut cmd = Command::new("/bin/login");
    cmd.env_clear().env("PATH", "/bin");
    if let Some(tty) = console_tty() {
        if let (Ok(i), Ok(o)) = (tty.try_clone(), tty.try_clone()) {
            cmd.stdin(i).stdout(o).stderr(tty);
            // new session, then make the tty (now fd 0) its controlling terminal
            unsafe { cmd.pre_exec(|| { libc::setsid(); libc::ioctl(0, libc::TIOCSCTTY as _, 0); Ok(()) }); }
        }
    }
    cmd
}

fn mount(src: &CStr, dir: &CStr, fs: &CStr, flags: libc::c_ulong, data: &CStr) -> io::Result<()> {
    if unsafe { libc::mount(src.as_ptr(), dir.as_ptr(), fs.as_ptr(), flags, data.as_ptr() as *const _) } != 0 { return Err(io::Error::last_os_error()); }
    Ok(())
}
fn mount_warn(src: &CStr, dir: &CStr, fs: &CStr, flags: libc::c_ulong, data: &CStr) {
    if let Err(e) = mount(src, dir, fs, flags, data) { eprintln!("thir: mount {}: {e}", dir.to_str().unwrap()); }   // warn, never die
}
fn wait_for(path: &str, ms: u64) -> bool {
    for _ in 0..ms / 50 { if Path::new(path).exists() { return true; } sleep(Duration::from_millis(50)); }
    Path::new(path).exists()
}
fn mount_data() {
    let r = if wait_for("/dev/vda", 3000) {
        mount(c"/dev/vda", c"/data", c"ext4", libc::MS_NOSUID | libc::MS_NODEV | libc::MS_NOEXEC, c"")
    } else { Err(io::Error::new(io::ErrorKind::NotFound, "no /dev/vda")) };
    match r {
        Ok(()) => unsafe {
            libc::mkdir(c"/data/etc".as_ptr(), 0o700); libc::mkdir(c"/data/home".as_ptr(), 0o755);   // EEXIST is fine
            println!("thir: mounted /data");
        },
        Err(e) => eprintln!("thir: /data unavailable: {e}"),
    }
}
fn shutdown(cmd: libc::c_int) -> ! {
    unsafe { libc::sync(); libc::umount2(c"/data".as_ptr(), 0); libc::sync(); libc::reboot(cmd); }
    eprintln!("thir: reboot(2) failed: {}", io::Error::last_os_error());
    loop { sleep(Duration::from_secs(60)); }        // PID 1 must never return
}

fn den() -> Command { let mut c = Command::new("/bin/den"); c.env_clear().env("PATH", "/bin"); c }

fn main() {
    let nosuid = libc::MS_NOSUID | libc::MS_NODEV;
    mount_warn(c"devtmpfs", c"/dev", c"devtmpfs", 0, c"mode=0755");   // first, so /dev/console etc. exist for everything after
    unsafe { libc::mkdir(c"/dev/pts".as_ptr(), 0o755); }
    mount_warn(c"devpts", c"/dev/pts", c"devpts", 0, c"mode=0620");   // den's pane ptys
    mount_warn(c"proc", c"/proc", c"proc", 0, c"");
    mount_warn(c"sysfs", c"/sys", c"sysfs", 0, c"");
    mount_warn(c"tmpfs", c"/tmp", c"tmpfs", nosuid, c"mode=1777");
    mount_warn(c"tmpfs", c"/run", c"tmpfs", 0, c"mode=0755");
    println!("thir: mounted /dev /proc /sys /tmp /run");
    mount_data();
    if unsafe { libc::sethostname(c"thir".as_ptr(), 4) } != 0 { eprintln!("thir: sethostname: {}", io::Error::last_os_error()); }
    if let Err(e) = net::set_up("lo") { eprintln!("thir: lo up: {e}"); }
    match net::interfaces() { Ok(v) => for i in &v { println!("thir: iface {}", i.line()); }, Err(e) => eprintln!("thir: interfaces: {e}") }
    if let Err(e) = Command::new("/bin/dhcp").env_clear().spawn() { eprintln!("thir: spawn /bin/dhcp: {e}"); }   // runs beside the session; reaped below
    println!("thir: init ready");                                   // smoke-test marker; do not change without updating test/smoke.sh
    // Two supervised services: the serial console session and (when /dev/fb0 exists) den. A service that exits is restarted
    // at once, but never sooner than 1 s after its previous start (guards against a respawn storm).
    let mut svc = [Svc { pid: 0, started: None }, Svc { pid: 0, started: None }];
    let (mut have_fb, mut fb_checked) = (false, false);
    loop {
        for i in 0..2 {
            if svc[i].pid != 0 || (i == 1 && !have_fb) { continue }
            if let Some(t) = svc[i].started { sleep(Duration::from_secs(1).saturating_sub(t.elapsed())); }
            svc[i].started = Some(Instant::now());
            let r = if i == 0 { session().spawn() } else { den().spawn() };
            match r { Ok(c) => svc[i].pid = c.id() as libc::pid_t, Err(e) => eprintln!("thir: spawn {}: {e}", ["/bin/login", "/bin/den"][i]) }
            if i == 0 && !fb_checked { have_fb = wait_for("/dev/fb0", 5000); fb_checked = true; }   // den needs the DRM fb, which can appear after init starts
        }
        let mut st = 0;
        let pid = unsafe { libc::waitpid(-1, &mut st, 0) };   // blocks; also reaps dhcp and re-parented orphans
        if pid < 0 { sleep(Duration::from_secs(1)); continue }   // ECHILD: nothing running (spawns failed); retry
        let Some(i) = svc.iter().position(|s| s.pid == pid) else { continue };
        svc[i].pid = 0;
        match decide(st) {
            Action::Restart => if i == 0 { println!("thir: console session ended, restarting") } else { println!("thir: den exited, restarting") },
            Action::Poweroff => shutdown(libc::LINUX_REBOOT_CMD_POWER_OFF),
            Action::Reboot => shutdown(libc::LINUX_REBOOT_CMD_RESTART),
        }
    }
}

struct Svc { pid: libc::pid_t, started: Option<Instant> }
