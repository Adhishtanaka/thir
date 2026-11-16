# thir — Step 1 implementation plan (made by the Fable planner)

Host facts: Apple M2, 8 cores, 16 GB. Docker daemon currently has only 2 CPUs (`docker info` → `2 cpus`), which roughly triples kernel build time. `rustc 1.99.0` on host. `linux-6.12.117` (current 6.12 LTS) tarball and `rust:1.99-bookworm` image both exist.

## Decisions

| Decision | Choice | Why |
|---|---|---|
| Guest arch | **aarch64** (`qemu-system-aarch64 -M virt -accel hvf -cpu host`) | HVF = near-native: boot-to-init well under 1 s. Kernel and Rust build run natively in an arm64 container. Switching = `THIR_ARCH=x86_64` + one kernel fragment file. |
| Where Rust is compiled | **Docker**, same image as the kernel | Native arm64 Linux container: `cargo build --target aarch64-unknown-linux-musl` works with stock gcc + Rust's bundled musl crt. `cargo test` must run on Linux anyway. On the Mac only `cargo check --target aarch64-unknown-linux-musl`. |
| Kernel base | **`tinyconfig` + explicit fragment** | ~4–6 MB Image, 3–8 min build. Build script verifies every fragment line landed in `.config`. Fallback: `tinyconfig`→`defconfig` in one line of `scripts/build-in-docker.sh`. |
| Framebuffer | `DRM_VIRTIO_GPU` + `DRM_FBDEV_EMULATION` + `FB_DEVICE` | QEMU `-device virtio-gpu-pci` gives `/dev/dri/card0` and `/dev/fb0`. simpledrm skipped (no simple-framebuffer node on `-M virt` without UEFI). |
| Crates | **`libc` only** (`libc = "0.2"`, locked by committed `Cargo.lock`) | Needed for `SIOCGIFFLAGS/SIOCGIFADDR` ioctls; mount/sethostname/waitpid/reboot are one-liners. |
| poweroff/reboot protocol | **exit-code**, not signals | `nsh poweroff` = `exit(120)`, `reboot` = `exit(121)`; init's `waitpid(-1)` loop sees it and calls `sync(); reboot(2)`. No signal handling in PID 1. Not 100/101: Rust panics exit 101. |
| initramfs tool | kernel's `usr/gen_init_cpio.c`, compiled with `cc` | Emits `newc`; device nodes from a manifest, no root/`mknod`. |
| Kernel objects | Docker named volume `thir-build` | Bind mount is 2–3x slower; volume makes rebuilds incremental and caches tarball + cargo registry. Only `Image`, `init`, `nsh`, `initramfs.cpio.gz` copied to `out/`. |

Pinned: kernel `6.12.117`, image `rust:1.99-bookworm` (fallback `rust:1.98-bookworm`), `libc = "0.2"` locked, edition 2021, QEMU = whatever `brew install qemu` gives (record in README).

## File tree

```
thir/
├── .gitignore                 out/  target/
├── README.md
├── env.sh                     single source of truth: THIR_ARCH + derived vars
├── build.sh                   host: docker build + docker run → out/
├── run.sh                     host: interactive QEMU
├── test/smoke.sh              host: headless boot test, exit 0/1
├── docker/Dockerfile
├── scripts/build-in-docker.sh container: kernel + cargo + initramfs (+ `test` subcommand)
├── kernel/common.config       arch-independent fragment
├── kernel/aarch64.config      arch fragment (console driver)
├── Cargo.toml                 package "thir", two bins + lib
├── Cargo.lock                 committed
└── src/
    ├── lib.rs                 EXIT_POWEROFF/EXIT_REBOOT, Action, decide() + tests
    ├── parse.rs               parse_line() + tests
    ├── net.rs                 Iface, interfaces(), set_up()
    └── bin/
        ├── init.rs            PID 1
        └── nsh.rs             shell
```

## File contents

### `env.sh`
```bash
# Single source of truth for the guest arch. Sourced by build.sh, run.sh, test/smoke.sh, scripts/build-in-docker.sh.
# NOTE: variable is THIR_ARCH, not ARCH, because the kernel Makefile reads $ARCH.
THIR_ARCH=${THIR_ARCH:-aarch64}
KERNEL_VERSION=6.12.117
DOCKER_PLATFORM=linux/arm64        # build container is always native arm64
case "$THIR_ARCH" in
  aarch64)
    KARCH=arm64; KIMAGE=arch/arm64/boot/Image; RUST_TARGET=aarch64-unknown-linux-musl
    CONSOLE=ttyAMA0; QEMU_BIN=qemu-system-aarch64; ACCEL=${ACCEL:-hvf}
    if [ "$ACCEL" = hvf ]; then CPU=host; else CPU=cortex-a72; fi
    QEMU_MACHINE=(-M virt,gic-version=3 -cpu "$CPU" -accel "$ACCEL") ;;
  x86_64)  # shortcut: untested; needs gcc-x86-64-linux-gnu + CROSS_COMPILE in the Dockerfile/script and kernel/x86_64.config (SERIAL_8250 + _CONSOLE). Upgrade when a PC target is wanted.
    KARCH=x86_64; KIMAGE=arch/x86/boot/bzImage; RUST_TARGET=x86_64-unknown-linux-musl
    CONSOLE=ttyS0; QEMU_BIN=qemu-system-x86_64; QEMU_MACHINE=(-M q35 -cpu max -accel tcg) ;;
  *) echo "unknown THIR_ARCH=$THIR_ARCH" >&2; return 1 ;;
esac
QEMU=("$QEMU_BIN" "${QEMU_MACHINE[@]}" -m 256 -smp 1
      -kernel out/Image -initrd out/initramfs.cpio.gz
      -append "console=$CONSOLE panic=1"
      -netdev user,id=n0 -device virtio-net-pci,netdev=n0
      -device virtio-gpu-pci)
```

### `docker/Dockerfile`
```dockerfile
FROM rust:1.99-bookworm
ARG RUST_TARGET=aarch64-unknown-linux-musl
RUN apt-get update && apt-get install -y --no-install-recommends bc bison flex libssl-dev libelf-dev xz-utils \
 && rm -rf /var/lib/apt/lists/*
RUN rustup target add "$RUST_TARGET"
```

### `build.sh` (host)
```bash
#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"; source ./env.sh
docker build --platform "$DOCKER_PLATFORM" --build-arg RUST_TARGET="$RUST_TARGET" -t thir-build docker
mkdir -p out
docker run --rm --platform "$DOCKER_PLATFORM" -e THIR_ARCH="$THIR_ARCH" \
  -v "$PWD":/work -v thir-build:/build -w /work thir-build scripts/build-in-docker.sh "${1:-all}"
```

### `scripts/build-in-docker.sh` (container; `chmod +x`)
```bash
#!/usr/bin/env bash
set -euo pipefail
source /work/env.sh
export CARGO_HOME=/build/cargo-home CARGO_TARGET_DIR=/build/target
if [ "${1:-all}" = test ]; then cd /work && exec cargo test; fi

SRC=/build/linux-$KERNEL_VERSION; OBJ=/build/kobj-$THIR_ARCH
[ -d "$SRC" ] || curl -fL "https://cdn.kernel.org/pub/linux/kernel/v6.x/linux-$KERNEL_VERSION.tar.xz" | tar -xJ -C /build
mkdir -p "$OBJ"
make -C "$SRC" O="$OBJ" ARCH="$KARCH" tinyconfig
cat /work/kernel/common.config "/work/kernel/$THIR_ARCH.config" >> "$OBJ/.config"
make -C "$SRC" O="$OBJ" ARCH="$KARCH" olddefconfig
# every requested line must survive olddefconfig (catches unmet dependencies)
grep -hv '^#' /work/kernel/common.config "/work/kernel/$THIR_ARCH.config" | grep . | while read -r line; do
  grep -qxF "$line" "$OBJ/.config" || { echo "kernel config: '$line' was not applied (missing dependency?)" >&2; exit 1; }
done
make -C "$SRC" O="$OBJ" ARCH="$KARCH" -j"$(nproc)" Image
cp "$OBJ/$KIMAGE" /work/out/Image
cc -O2 -o /build/gen_init_cpio "$SRC/usr/gen_init_cpio.c"

cd /work && cargo build --release --target "$RUST_TARGET"
cp "$CARGO_TARGET_DIR/$RUST_TARGET/release/init" "$CARGO_TARGET_DIR/$RUST_TARGET/release/nsh" /work/out/

cat > /build/initramfs.list <<EOF
dir /dev 0755 0 0
nod /dev/console 0600 0 0 c 5 1
dir /proc 0755 0 0
dir /sys 0755 0 0
dir /tmp 01777 0 0
dir /run 0755 0 0
dir /bin 0755 0 0
file /init /work/out/init 0755 0 0
file /bin/nsh /work/out/nsh 0755 0 0
EOF
/build/gen_init_cpio /build/initramfs.list | gzip -9 > /work/out/initramfs.cpio.gz
ls -l /work/out
```

### `kernel/common.config`
```
CONFIG_EXPERT=y
CONFIG_PRINTK=y
CONFIG_PRINTK_TIME=y
CONFIG_MULTIUSER=y
CONFIG_FUTEX=y
CONFIG_POSIX_TIMERS=y
CONFIG_SHMEM=y
CONFIG_TMPFS=y
CONFIG_PROC_FS=y
CONFIG_PROC_SYSCTL=y
CONFIG_SYSFS=y
CONFIG_DEVTMPFS=y
CONFIG_BLK_DEV_INITRD=y
CONFIG_RD_GZIP=y
CONFIG_BINFMT_ELF=y
CONFIG_TTY=y
CONFIG_UNIX98_PTYS=y
CONFIG_NET=y
CONFIG_UNIX=y
CONFIG_INET=y
CONFIG_NETDEVICES=y
CONFIG_PCI=y
CONFIG_VIRTIO_MENU=y
CONFIG_VIRTIO_PCI=y
CONFIG_VIRTIO_NET=y
CONFIG_DRM=y
CONFIG_DRM_VIRTIO_GPU=y
CONFIG_DRM_FBDEV_EMULATION=y
CONFIG_FB_DEVICE=y
```

### `kernel/aarch64.config`
```
CONFIG_PCI_HOST_GENERIC=y
CONFIG_SERIAL_AMBA_PL011=y
CONFIG_SERIAL_AMBA_PL011_CONSOLE=y
```
If the verification loop reports a line not applied, find the missing dependency in the Kconfig files, add it to the fragment, rebuild. Do not delete the line.

### `Cargo.toml`
```toml
[package]
name = "thir"
version = "0.1.0"
edition = "2021"

[dependencies]
libc = "0.2"

[profile.release]
opt-level = "s"
lto = true
panic = "abort"
strip = true
```

### `src/lib.rs`
```rust
pub mod net;
pub mod parse;

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
```

### `src/parse.rs`
```rust
/// Split a command line into argv. Whitespace separates; "..." and '...' group;
/// backslash escapes the next char outside single quotes. Unterminated quote => Err.
pub fn parse_line(line: &str) -> Result<Vec<String>, &'static str> { /* ~25-line char loop with in_single, in_double, cur: String, have_token: bool so that "" yields an empty arg */ }
```
Tests (5): empty/whitespace-only → `[]`; `  ls  -l /  ` → `["ls","-l","/"]`; `cat "a b" 'c d'` → `["cat","a b","c d"]`; `echo a\ b` → `["echo","a b"]`; `"unterminated` → `Err`.

### `src/net.rs`
```rust
use std::{fs, io, net::Ipv4Addr};

pub struct Iface { pub name: String, pub mac: String, pub up: bool, pub ipv4: Option<Ipv4Addr> }

impl Iface { pub fn line(&self) -> String { format!("{:<8} {:<17} {:<4} {}", self.name, self.mac, if self.up {"up"} else {"down"}, self.ipv4.map_or("-".into(), |a| a.to_string())) } }

fn ifreq(name: &str) -> libc::ifreq {
    let mut r: libc::ifreq = unsafe { std::mem::zeroed() };
    for (d, s) in r.ifr_name.iter_mut().zip(name.bytes().take(libc::IFNAMSIZ - 1)) { *d = s as libc::c_char; }
    r
}
struct Sock(libc::c_int);           // AF_INET/SOCK_DGRAM control socket; Drop closes it
impl Sock { fn new() -> io::Result<Self> { let fd = unsafe { libc::socket(libc::AF_INET, libc::SOCK_DGRAM, 0) }; if fd < 0 { Err(io::Error::last_os_error()) } else { Ok(Sock(fd)) } } }
impl Drop for Sock { fn drop(&mut self) { unsafe { libc::close(self.0); } } }

fn ioctl(s: &Sock, req: libc::c_ulong, r: &mut libc::ifreq) -> io::Result<()> {
    // `as _`: the request is c_ulong on glibc but c_int on musl
    if unsafe { libc::ioctl(s.0, req as _, r) } < 0 { Err(io::Error::last_os_error()) } else { Ok(()) }
}

pub fn set_up(name: &str) -> io::Result<()> {
    let s = Sock::new()?; let mut r = ifreq(name);
    ioctl(&s, libc::SIOCGIFFLAGS, &mut r)?;
    unsafe { r.ifr_ifru.ifru_flags |= libc::IFF_UP as libc::c_short; }
    ioctl(&s, libc::SIOCSIFFLAGS, &mut r)
}

/// Names from /sys/class/net (sorted), MAC from .../address, flags + IPv4 via ioctl.
pub fn interfaces() -> io::Result<Vec<Iface>> {
    let s = Sock::new()?;
    let mut names: Vec<String> = fs::read_dir("/sys/class/net")?.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    Ok(names.into_iter().map(|name| {
        let mac = fs::read_to_string(format!("/sys/class/net/{name}/address")).map(|s| s.trim().to_string()).unwrap_or_default();
        let mut r = ifreq(&name);
        let up = ioctl(&s, libc::SIOCGIFFLAGS, &mut r).is_ok() && unsafe { r.ifr_ifru.ifru_flags } & libc::IFF_UP as libc::c_short != 0;
        let ipv4 = ioctl(&s, libc::SIOCGIFADDR, &mut r).ok().map(|_| {
            let sa = unsafe { &*(&r.ifr_ifru.ifru_addr as *const libc::sockaddr as *const libc::sockaddr_in) };
            Ipv4Addr::from(u32::from_be(sa.sin_addr.s_addr))
        });
        Iface { name, mac, up, ipv4 }
    }).collect())
}
```
Bringing `lo` up is enough: the kernel auto-assigns 127.0.0.1/8. `eth0` will show `down -` (no DHCP in step 1).

### `src/bin/init.rs`
```rust
use std::{ffi::CStr, io, process::Command, thread::sleep, time::Duration};
use thir::{decide, net, Action};

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
    if unsafe { libc::sethostname(c"thir".as_ptr(), 5) } != 0 { eprintln!("thir: sethostname: {}", io::Error::last_os_error()); }
    if let Err(e) = net::set_up("lo") { eprintln!("thir: lo up: {e}"); }
    match net::interfaces() { Ok(v) => for i in &v { println!("thir: iface {}", i.line()); }, Err(e) => eprintln!("thir: interfaces: {e}") }
    println!("thir: init ready");                                   // smoke-test marker; do not change without updating test/smoke.sh
    loop {
        let child = match Command::new("/bin/nsh").env_clear().env("PATH", "/bin").env("HOME", "/").spawn() {
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
```
No HashMap anywhere in init or nsh (pitfall 9).

### `src/bin/nsh.rs`
```
loop:
  print "nsh> ", flush
  stdin.read_line → Ok(0) (EOF) => exit(0); Err => exit(0)
  argv = parse_line(&line); Err(e) => print "nsh: e", continue; empty => continue
  match argv[0]:
    "cd"       => set_current_dir(argv.get(1) or "/"), print error on failure
    "exit"     => exit(argv.get(1).parse().unwrap_or(0))
    "help"     => println!("nsh builtins: cd exit help ls cat ip poweroff reboot")
    "ls"       => read_dir(argv.get(1) or "."), names sorted, "/" suffix for dirs, one per line
    "cat"      => for each arg: io::copy(File::open(arg), stdout), print "nsh: cat: arg: err" on failure
    "ip"       => for i in net::interfaces(): println!(i.line())
    "poweroff" => exit(thir::EXIT_POWEROFF)
    "reboot"   => exit(thir::EXIT_REBOOT)
    other      => Command::new(&argv[0]).args(&argv[1..]).status()
                  Err(e) => "nsh: {cmd}: {e}";  Ok(s) if !s.success() => "[exit {code}]"
```
No echo of input (tty line discipline echoes). No job control, no Ctrl-C handling.

### `run.sh`
```bash
#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"; source ./env.sh
echo "thir: Ctrl-A X quits QEMU" >&2
exec "${QEMU[@]}" -nographic "$@"
```

### `test/smoke.sh`
```bash
#!/usr/bin/env bash
# Headless boot test: wait for init marker, check eth0 was found, run one shell command, check output, power off.
set -euo pipefail
cd "$(dirname "$0")/.."; source ./env.sh
TIMEOUT=${SMOKE_TIMEOUT:-30}
log=$(mktemp -t thir-smoke); fifo=$(mktemp -u -t thir-fifo); mkfifo "$fifo"
cleanup() { kill "$qpid" 2>/dev/null || true; rm -f "$fifo"; }
"${QEMU[@]}" -no-reboot -display none -monitor none -serial stdio < "$fifo" > "$log" 2>&1 &
qpid=$!; trap cleanup EXIT
exec 3>"$fifo"                      # keep the writer open until poweroff

wait_for() {  # $1 = extended regex (CRs stripped), $2 = seconds
  local deadline=$((SECONDS + $2))
  until tr -d '\r' < "$log" | grep -qE "$1"; do
    if ! kill -0 "$qpid" 2>/dev/null || (( SECONDS >= deadline )); then
      echo "FAIL: did not see /$1/ within $2s"; echo "--- serial log ---"; cat "$log"; exit 1
    fi
    sleep 0.2
  done
}
wait_for '^thir: init ready$' "$TIMEOUT"
echo "boot-to-marker: ${SECONDS}s"
wait_for '^thir: iface eth0 ' 1
printf 'cat /proc/sys/kernel/hostname\n' >&3
wait_for '^thir$' 10
printf 'poweroff\n' >&3
for _ in $(seq 1 50); do kill -0 "$qpid" 2>/dev/null || break; sleep 0.2; done
if kill -0 "$qpid" 2>/dev/null; then echo "FAIL: QEMU still running after poweroff"; exit 1; fi
echo PASS
```

### `README.md` outline
Purpose (step 1 of the roadmap) · Prerequisites (`brew install qemu`; Docker Desktop, ≥ 6 CPUs / 8 GB recommended) · Layout · Commands (`./build.sh`, `./build.sh test`, `./run.sh`, `./test/smoke.sh`, `docker volume rm thir-build` for a clean kernel) · How boot works (kernel → `/init` → mounts → `lo` up → marker → `/bin/nsh` loop; exit codes 120/121) · Switching arch (`THIR_ARCH`, x86_64 untested) · Kernel config rationale · Pinned versions (kernel, image, libc from Cargo.lock, real QEMU version) · Known limitations (no Ctrl-C/job control, eth0 has no address yet, `poweroff` only from the top-level shell, Mac-side `cargo test` does not compile: use `./build.sh test`).

### `.gitignore`
```
out/
target/
```

## Known pitfalls

1. **musl static**: `file out/init` must say `ARM aarch64 ... static-pie linked` (or `statically linked`). No musl-gcc/zig needed. No `.cargo/config.toml` linker overrides.
2. **PID 1 signals**: kernel ignores default-action signals for PID 1; init must never return (panic "Attempted to kill init!"). No signal handlers. Ctrl-C does nothing; quit QEMU with Ctrl-A X.
3. **/dev/console in the cpio**: without the `nod /dev/console` line, boot is silent.
4. **Console name**: aarch64 `virt` = `ttyAMA0`; x86 = `ttyS0`. Debug with `earlycon` in `-append`.
5. **cpio format** must be `newc` (`gen_init_cpio` does this).
6. **Docker on Apple Silicon**: always `--platform linux/arm64`. Named volume for the kernel tree.
7. **Kernel build time**: 2 CPUs → 10–20 min first build; 8 CPUs → 3–6 min.
8. **ioctl request type**: `c_ulong` vs musl `c_int` → `req as _`.
9. **getrandom can block early in boot**: no HashMap/HashSet. If a hang after "init ready", add `-device virtio-rng-pci` and `CONFIG_HW_RANDOM_VIRTIO=y`.
10. **QEMU accel**: `-cpu host` only with `-accel hvf`; TCG uses `cortex-a72`.
11. **Poweroff on `virt`** via PSCI (selected by ARM64). `-no-reboot` in smoke test.
12. **Smoke-test FIFO ordering**: QEMU background first, then `exec 3>`; keep fd 3 open until after `poweroff`.
13. **Rust panic exit code is 101**: hence 120/121.
14. **`cargo test` on macOS does not compile**: use `./build.sh test`.
15. **Fragment verification**: add the missing dependency; never delete the line.
16. **`ARCH` name clash**: use `THIR_ARCH`; never `export ARCH`.
17. **Rust `Command` under PID 1**: `env_clear()` + `PATH=/bin`; no `child.wait()`.
18. **`-nographic`** muxes monitor+serial: Ctrl-A X quits. Smoke test uses `-display none -monitor none -serial stdio`.

## Ordered build checklist

1. (User action, optional) Docker Desktop → Resources: CPUs ≥ 6, Memory ≥ 8 GB.
2. `brew install qemu`; confirm `qemu-system-aarch64 --version` and `-accel help | grep hvf`.
3. `git init` in the project folder.
4. Create scripts and kernel fragments (`chmod +x` scripts).
5. Create Cargo.toml and src files.
6. `./build.sh test` → all 9 tests pass.
7. `./build.sh` → no "not applied" message. `out/Image` 4–8 MB, initramfs < 1 MB.
8. `file out/init out/nsh` → aarch64 static-pie.
9. Interactive checks (help, ls /, ls /dev/dri, cat /proc/version, ip, cd /sys; ls, exit, reboot, poweroff).
10. `./test/smoke.sh` → PASS exit 0; `SMOKE_TIMEOUT=0 ./test/smoke.sh` → exit 1.
11. README with real QEMU version, libc version, measured boot time.
12. `git add -A && git commit`.

## Acceptance criteria

- `./build.sh` exits 0 on a clean volume; produces `out/Image`, `out/init`, `out/nsh`, `out/initramfs.cpio.gz`; no config line missing.
- `file` reports aarch64 static/static-pie ELF for init and nsh.
- `./build.sh test` exits 0 with ≥ 9 passing tests.
- `./test/smoke.sh`: `boot-to-marker: Ns` with N ≤ 2, `PASS`, exit 0, ≤ 15 s wall. `SMOKE_TIMEOUT=0` → exit 1.
- Serial log shows `thir: iface lo … up 127.0.0.1` and `thir: iface eth0 … down -`; `/dev/dri/card0` and `/dev/fb0` exist; exit restarts shell; reboot reboots; poweroff ends QEMU.
- `out/Image` + `out/initramfs.cpio.gz` < 30 MB.
- Repo committed; `out/`, `target/` untracked; `Cargo.lock` tracked.
- No crate other than `libc`; no busybox, no shell scripts inside the image.
