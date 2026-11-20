//! A small sandbox for helper processes (ffmpeg under `view`/`play`, the WebKit processes through the wpe-shim): no_new_privs, then Landlock (which paths can be read, written, executed;
//! TCP; abstract unix sockets and signals to other domains), then a seccomp deny-list of syscalls no helper has a reason to make.
//! Landlock needs kernel >= 5.13 with it enabled (net: ABI 4, scoping: ABI 6); where a part is missing the rest still applies (`enforce` says whether Landlock is on).
use std::{io, os::{fd::{AsRawFd, FromRawFd, OwnedFd}, unix::fs::OpenOptionsExt}, path::{Path, PathBuf}};

const SYS_CREATE: libc::c_long = 444; const SYS_ADD: libc::c_long = 445; const SYS_RESTRICT: libc::c_long = 446;   // the same number on arm64 and x86_64
const RX_FILE: u64 = 1 << 2;                 // LANDLOCK_ACCESS_FS_READ_FILE
const RX_DIR: u64 = 1 << 0 | 1 << 2 | 1 << 3;   // EXECUTE | READ_FILE | READ_DIR
const RW_FILE: u64 = 1 << 1 | 1 << 2 | 1 << 14 | 1 << 15;   // WRITE_FILE | READ_FILE | TRUNCATE | IOCTL_DEV (bits the ABI lacks are masked off)

#[repr(C, packed)] struct PathBeneath { allowed: u64, parent_fd: i32 }

/// Handled FS rights for a Landlock ABI: all of them, so everything not allowed is denied.
pub fn fs_mask(abi: i64) -> u64 { (1u64 << 13) - 1 | if abi >= 2 { 1 << 13 } else { 0 } | if abi >= 3 { 1 << 14 } else { 0 } | if abi >= 5 { 1 << 15 } else { 0 } }

/// What a process may do. Everything not listed is denied. Missing paths are skipped.
#[derive(Default, Clone, Debug, PartialEq)]
pub struct Spec {
    pub rx: Vec<PathBuf>,         // directories: read and execute
    pub r: Vec<PathBuf>,          // files: read
    pub rw: Vec<PathBuf>,         // directories: everything (create, write, remove)
    pub rw_files: Vec<PathBuf>,   // files or devices: read and write (/dev/null)
    pub deny_tcp: bool,           // no TCP connect or bind (ABI 4)
    pub scope: bool,              // no abstract unix sockets and no signals outside the domain (ABI 6)
    pub seccomp: bool,
}

pub struct Rules { fd: Option<OwnedFd>, seccomp: bool }

fn open_path(p: &Path) -> Option<std::fs::File> { std::fs::File::options().read(true).custom_flags(libc::O_PATH | libc::O_CLOEXEC).open(p).ok() }

/// Opens the rule set (in the parent, before fork).
pub fn build(s: &Spec) -> Rules {
    let abi = unsafe { libc::syscall(SYS_CREATE, 0usize, 0usize, 1u32 as libc::c_long) };   // LANDLOCK_CREATE_RULESET_VERSION
    if abi < 1 { return Rules { fd: None, seccomp: s.seccomp } }
    let attr: [u64; 3] = [fs_mask(abi), if s.deny_tcp { 3 } else { 0 }, if s.scope { 3 } else { 0 }];   // handled fs, net (BIND_TCP | CONNECT_TCP), scoped (ABSTRACT_UNIX_SOCKET | SIGNAL)
    let size = if abi >= 6 { 24 } else if abi >= 4 { 16 } else { 8 };
    let attr = if abi < 4 { [attr[0], 0, 0] } else { attr };
    let fd = unsafe { libc::syscall(SYS_CREATE, attr.as_ptr(), size as libc::c_long, 0 as libc::c_long) };
    if fd < 0 { return Rules { fd: None, seccomp: s.seccomp } }
    let fd = unsafe { OwnedFd::from_raw_fd(fd as i32) };
    let all = fs_mask(abi);
    for (list, allowed) in [(&s.rx, RX_DIR), (&s.r, RX_FILE), (&s.rw, all), (&s.rw_files, RW_FILE & all)] { for p in list {
        let Some(f) = open_path(p) else { continue };
        let r = PathBeneath { allowed, parent_fd: f.as_raw_fd() };
        unsafe { libc::syscall(SYS_ADD, fd.as_raw_fd() as libc::c_long, 1 as libc::c_long, &r as *const PathBeneath, 0 as libc::c_long); }
    } }
    Rules { fd: Some(fd), seccomp: s.seccomp }
}

/// The rules of a helper that reads `dirs` and `files` and does nothing else: no writes, no TCP, no scary syscalls.
pub fn rules(dirs: &[&Path], files: &[&Path]) -> Rules {
    build(&Spec { rx: dirs.iter().map(|p| p.to_path_buf()).collect(), r: files.iter().map(|p| p.to_path_buf()).collect(), deny_tcp: true, scope: true, seccomp: true, ..Spec::default() })
}

impl Rules {
    /// Applies no_new_privs, the Landlock rules and the seccomp filter to this process (call it in the child, between fork and exec). Ok(true) = Landlock is on, Ok(false) = it is not.
    pub fn enforce(&self) -> io::Result<bool> {
        if unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) } != 0 { return Err(io::Error::last_os_error()) }
        let on = match &self.fd {
            Some(fd) => { if unsafe { libc::syscall(SYS_RESTRICT, fd.as_raw_fd() as libc::c_long, 0 as libc::c_long) } != 0 { return Err(io::Error::last_os_error()) } true }
            None => false,
        };
        if self.seccomp {
            let f = seccomp_filter(ARCH, DENY);
            let prog = libc::sock_fprog { len: f.len() as u16, filter: f.as_ptr() as *mut libc::sock_filter };
            if unsafe { libc::prctl(libc::PR_SET_SECCOMP, libc::SECCOMP_MODE_FILTER as libc::c_ulong, &prog as *const libc::sock_fprog, 0, 0) } != 0 { return Err(io::Error::last_os_error()) }
        }
        Ok(on)
    }
}

#[cfg(target_arch = "aarch64")] const ARCH: u32 = 0xC000_00B7;   // AUDIT_ARCH_AARCH64
#[cfg(target_arch = "x86_64")] const ARCH: u32 = 0xC000_003E;    // AUDIT_ARCH_X86_64

#[cfg(target_arch = "aarch64")] const SYS_USERFAULTFD: libc::c_long = 282;
#[cfg(target_arch = "x86_64")] const SYS_USERFAULTFD: libc::c_long = 323;
#[cfg(target_arch = "aarch64")] const SYS_KEXEC_FILE_LOAD: libc::c_long = 294;
#[cfg(target_arch = "x86_64")] const SYS_KEXEC_FILE_LOAD: libc::c_long = 320;
/// Syscalls no helper needs: debugging other processes, kernel and device control, mounting, key rings, io_uring (which bypasses seccomp), page-fault tricks.
pub const DENY: &[libc::c_long] = &[
    libc::SYS_ptrace, libc::SYS_process_vm_readv, libc::SYS_process_vm_writev, libc::SYS_perf_event_open, libc::SYS_bpf, SYS_USERFAULTFD,
    libc::SYS_kexec_load, SYS_KEXEC_FILE_LOAD, libc::SYS_mount, libc::SYS_umount2, libc::SYS_pivot_root, libc::SYS_chroot,
    428, 429, 430, 431, 432, 433, 442,   // open_tree move_mount fsopen fsconfig fsmount fspick mount_setattr (the same on arm64 and x86_64; not in every libc)
    libc::SYS_keyctl, libc::SYS_add_key, libc::SYS_request_key, 425, 426, 427,   // io_uring_setup enter register
    libc::SYS_personality, libc::SYS_reboot, libc::SYS_swapon, libc::SYS_swapoff, libc::SYS_init_module, libc::SYS_finit_module, libc::SYS_delete_module,
    libc::SYS_acct, libc::SYS_settimeofday, libc::SYS_clock_settime, libc::SYS_clock_adjtime, libc::SYS_adjtimex, libc::SYS_setns, libc::SYS_unshare,
];

// classic BPF: seccomp_data is { nr: u32, arch: u32, ip: u64, args }
const LD_ABS: u16 = 0x20; const JEQ_K: u16 = 0x15; const JGE_K: u16 = 0x35; const RET_K: u16 = 0x06;
const RET_ERRNO_PERM: u32 = 0x0005_0000 | libc::EPERM as u32; const RET_ALLOW: u32 = 0x7fff_0000; const RET_KILL: u32 = 0;

/// The filter: wrong architecture -> kill; x32 numbers (x86_64) -> kill; a number in `deny` -> EPERM; the rest is allowed.
pub fn seccomp_filter(arch: u32, deny: &[libc::c_long]) -> Vec<libc::sock_filter> {
    let ins = |code, jt, jf, k| libc::sock_filter { code, jt, jf, k };
    let mut v = vec![ins(LD_ABS, 0, 0, 4), ins(JEQ_K, 1, 0, arch), ins(RET_K, 0, 0, RET_KILL), ins(LD_ABS, 0, 0, 0)];
    if arch == 0xC000_003E { v.push(ins(JGE_K, 0, 1, 0x4000_0000)); v.push(ins(RET_K, 0, 0, RET_KILL)); }
    for &n in deny { v.push(ins(JEQ_K, 0, 1, n as u32)); v.push(ins(RET_K, 0, 0, RET_ERRNO_PERM)); }
    v.push(ins(RET_K, 0, 0, RET_ALLOW));
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    /// Runs the filter on a (arch, nr) pair the way the kernel would.
    fn run(f: &[libc::sock_filter], arch: u32, nr: u32) -> u32 {
        let (mut a, mut pc) = (0u32, 0usize);
        loop {
            let i = &f[pc]; pc += 1;
            match i.code {
                LD_ABS => a = if i.k == 0 { nr } else { arch },
                JEQ_K => pc += if a == i.k { i.jt } else { i.jf } as usize,
                JGE_K => pc += if a >= i.k { i.jt } else { i.jf } as usize,
                RET_K => return i.k,
                c => panic!("opcode {c:#x}"),
            }
        }
    }
    #[test] fn masks_grow_with_the_abi() { assert_eq!(fs_mask(1), 0x1fff); assert_eq!(fs_mask(3), 0x7fff); assert_eq!(fs_mask(6), 0xffff); }
    #[test] fn filter_denies_listed_calls_only() {
        let f = seccomp_filter(ARCH, DENY);
        for &n in DENY { assert_eq!(run(&f, ARCH, n as u32), RET_ERRNO_PERM, "{n}"); }
        for n in [libc::SYS_read, libc::SYS_write, libc::SYS_mmap, libc::SYS_futex, libc::SYS_socket, libc::SYS_openat, libc::SYS_clone] { assert_eq!(run(&f, ARCH, n as u32), RET_ALLOW, "{n}"); }
        assert_eq!(run(&f, ARCH ^ 1, libc::SYS_read as u32), RET_KILL);            // another ABI could name the denied calls differently
        let x = seccomp_filter(0xC000_003E, &[101]); assert_eq!(run(&x, 0xC000_003E, 101 | 0x4000_0000), RET_KILL); assert_eq!(run(&x, 0xC000_003E, 0), RET_ALLOW);
    }
    #[test] fn deny_list_has_no_duplicates_and_fits_the_jump_range() {
        let mut d = DENY.to_vec(); d.sort(); d.dedup(); assert_eq!(d.len(), DENY.len()); assert!(seccomp_filter(ARCH, DENY).len() < 255);
    }
    #[test] fn rules_wrapper_denies_everything_but_reads() {
        let s = Spec { rx: vec!["/web".into()], r: vec!["/x".into()], deny_tcp: true, scope: true, seccomp: true, ..Spec::default() };
        assert_eq!(s, Spec { rx: vec![PathBuf::from("/web")], r: vec![PathBuf::from("/x")], rw: vec![], rw_files: vec![], deny_tcp: true, scope: true, seccomp: true });
    }
}
