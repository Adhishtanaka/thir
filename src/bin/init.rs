use std::{ffi::CStr, io, process::Command, thread::sleep, time::Duration};
use netos::{decide, net, Action};

fn mount(src: &CStr, dir: &CStr, fs: &CStr, data: &CStr) {
    if unsafe { libc::mount(src.as_ptr(), dir.as_ptr(), fs.as_ptr(), 0, data.as_ptr() as *const _) } != 0 {
        eprintln!("netos: mount {}: {}", dir.to_str().unwrap(), io::Error::last_os_error());   // warn, never die
    }
}
fn shutdown(cmd: libc::c_int) -> ! {
    unsafe { libc::sync(); libc::reboot(cmd); }
    eprintln!("netos: reboot(2) failed: {}", io::Error::last_os_error());
    loop { sleep(Duration::from_secs(60)); }        // PID 1 must never return
}

fn main() {
    mount(c"devtmpfs", c"/dev", c"devtmpfs", c"mode=0755");   // first, so /dev/console etc. exist for everything after
    mount(c"proc", c"/proc", c"proc", c"");
    mount(c"sysfs", c"/sys", c"sysfs", c"");
    mount(c"tmpfs", c"/tmp", c"tmpfs", c"mode=1777");
    mount(c"tmpfs", c"/run", c"tmpfs", c"mode=0755");
    println!("netos: mounted /dev /proc /sys /tmp /run");
    if unsafe { libc::sethostname(c"netos".as_ptr(), 5) } != 0 { eprintln!("netos: sethostname: {}", io::Error::last_os_error()); }
    if let Err(e) = net::set_up("lo") { eprintln!("netos: lo up: {e}"); }
    match net::interfaces() { Ok(v) => for i in &v { println!("netos: iface {}", i.line()); }, Err(e) => eprintln!("netos: interfaces: {e}") }
    println!("netos: init ready");                                   // smoke-test marker; do not change without updating test/smoke.sh
    loop {
        let child = match Command::new("/bin/nsh").env_clear().env("PATH", "/bin").env("HOME", "/").spawn() {
            Ok(c) => c,
            Err(e) => { eprintln!("netos: spawn /bin/nsh: {e}"); sleep(Duration::from_secs(1)); continue; }
        };
        let shell = child.id() as libc::pid_t;                       // do NOT call child.wait(): waitpid(-1) below reaps it
        let status = loop {
            let mut st = 0;
            let pid = unsafe { libc::waitpid(-1, &mut st, 0) };
            if pid == shell { break st; }
            if pid < 0 && io::Error::last_os_error().raw_os_error() != Some(libc::EINTR) { break 0; } // ECHILD: shell gone; restart
        };
        match decide(status) {
            Action::Restart => { println!("netos: shell exited, restarting"); sleep(Duration::from_secs(1)); } // guards against a respawn storm
            Action::Poweroff => shutdown(libc::LINUX_REBOOT_CMD_POWER_OFF),
            Action::Reboot => shutdown(libc::LINUX_REBOOT_CMD_RESTART),
        }
    }
}
