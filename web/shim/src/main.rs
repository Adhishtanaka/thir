//! The sandbox shim for the WebKit helper processes. WebKit starts /usr/libexec/wpe-webkit-2.0/WPEWebProcess or WPENetworkProcess with its own arguments and a few open sockets.
//! In the add-on those two names are this program. It picks the rules from its own name (thir::web::profile), locks itself in (no_new_privs, Landlock, seccomp) and then runs the
//! real program (`<name>.real`) with the same arguments and open files. If the lock cannot be set the real program is never started.
//! `NAME --thir-check` is the self-test behind `web --check`: it locks itself in the same way and then tries things the sandbox must stop.
use std::{os::unix::{fs::OpenOptionsExt, process::CommandExt}, path::{Path, PathBuf}, process::{exit, Command}};
use thir::{sandbox, web::{self, Proc}};

fn errno_of<T>(r: std::io::Result<T>) -> Option<i32> { r.err().and_then(|e| e.raw_os_error()) }

/// What the locked-in process can do. Each line: (what, true if the result is the wanted one).
fn probe(kind: Proc, home: &Path) -> Vec<(String, bool)> {
    let mut v = vec![];
    let tcp = unsafe {
        let s = libc::socket(libc::AF_INET, libc::SOCK_STREAM | libc::SOCK_CLOEXEC, 0);
        let a = libc::sockaddr_in { sin_family: libc::AF_INET as _, sin_port: 9u16.to_be(), sin_addr: libc::in_addr { s_addr: u32::from_be_bytes([127, 0, 0, 1]).to_be() }, sin_zero: [0; 8] };
        let r = libc::connect(s, &a as *const _ as *const _, std::mem::size_of::<libc::sockaddr_in>() as _);
        let e = if r == 0 { 0 } else { std::io::Error::last_os_error().raw_os_error().unwrap_or(-1) }; libc::close(s); e
    };
    match kind {
        Proc::Web => v.push((format!("web process: TCP connect denied (errno {tcp})"), tcp == libc::EACCES)),
        _ => v.push((format!("network process: TCP connect allowed (errno {tcp}, nothing listens)"), tcp != libc::EACCES)),
    }
    v.push(("~/.config/thir cannot be read".into(), errno_of(std::fs::read_dir(home.join(".config/thir"))) == Some(libc::EACCES)));
    v.push(("~/.local/share/thir-web and the rest of the home folder: a new file in the home folder is refused".into(), errno_of(std::fs::File::create(home.join("thir-probe"))) == Some(libc::EACCES)));
    v.push(("/tmp cannot be written".into(), errno_of(std::fs::File::create("/tmp/thir-probe")) == Some(libc::EACCES)));
    let dl = home.join("Downloads").join(format!("thir-probe-{}", std::process::id()));
    let w = std::fs::File::create(&dl); let made = w.is_ok(); drop(w); let _ = std::fs::remove_file(&dl);
    match kind { Proc::Web => v.push(("~/Downloads cannot be written".into(), !made)), _ => v.push(("~/Downloads can be written".into(), made)) }
    let bpf = unsafe { libc::syscall(libc::SYS_bpf, 0, 0, 0) }; let e = std::io::Error::last_os_error().raw_os_error();
    v.push(("seccomp stops bpf()".into(), bpf == -1 && e == Some(libc::EPERM)));
    v.push(("/web can be read".into(), std::fs::read_dir("/web").is_ok()));
    if kind == Proc::Web && std::path::Path::new("/dev/snd/pcmC0D0p").exists() {   // the page's sound: the playback node opens, the mixer and the microphone do not
        let open = |p: &str| std::fs::OpenOptions::new().read(true).write(true).custom_flags(libc::O_NONBLOCK).open(p);
        let e = errno_of(open("/dev/snd/pcmC0D0p")); v.push((format!("web process: the playback device opens (errno {e:?})"), e.is_none() || e == Some(libc::EBUSY)));
        if let Ok(f) = open("/dev/snd/pcmC0D0p") { use std::os::fd::AsRawFd; let mut ver: libc::c_int = 0; let r = unsafe { libc::ioctl(f.as_raw_fd(), 0x8004_4100u32 as _, &mut ver) };   // SNDRV_PCM_IOCTL_PVERSION
            v.push((format!("web process: ioctl on the playback device (returns {r}, version {ver:#x}, errno {})", std::io::Error::last_os_error().raw_os_error().unwrap_or(0)), r == 0)); }
        v.push((format!("web process: the sound control opens (errno {:?})", errno_of(open("/dev/snd/controlC0"))), errno_of(open("/dev/snd/controlC0")).is_none()));
        v.push(("web process: the microphone device is refused".into(), errno_of(open("/dev/snd/pcmC0D0c")).is_some()));
    }
    v
}

fn main() {
    let args: Vec<std::ffi::OsString> = std::env::args_os().collect();
    let argv0 = args[0].to_string_lossy().into_owned();
    let Some(kind) = web::proc_of(&argv0) else { eprintln!("wpe-shim: run me as WPEWebProcess or WPENetworkProcess"); exit(2) };
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    let real = if argv0.starts_with('/') { PathBuf::from(format!("{argv0}.real")) } else { match std::env::current_exe() { Ok(p) => PathBuf::from(format!("{}.real", p.display())), Err(e) => { eprintln!("wpe-shim: {e}"); exit(1) } } };
    match sandbox::build(&web::profile(kind, &home)).enforce() {
        Ok(true) => {}
        Ok(false) => { eprintln!("wpe-shim: this kernel has no Landlock; only seccomp and no_new_privs are on"); }
        Err(e) => { eprintln!("wpe-shim: cannot lock the process in ({e}); not starting"); exit(1) }
    }
    unsafe { libc::umask(0o077); }
    if args.get(1).is_some_and(|a| a == "--thir-check") {
        let r = probe(kind, &home); let bad = r.iter().filter(|(_, ok)| !ok).count();
        for (what, ok) in &r { println!("{} {what}", if *ok { "ok:  " } else { "FAIL:" }); }
        exit(if bad == 0 { 0 } else { 1 });
    }
    let e = Command::new(&real).arg0(&args[0]).args(&args[1..]).exec();
    eprintln!("wpe-shim: cannot start {}: {e}", real.display()); exit(1)
}
