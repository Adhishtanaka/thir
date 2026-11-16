# thir

A tiny research OS for network work: the Linux kernel plus a userspace written only in Rust.
No distro, no systemd, no busybox. Step 1: a Rust PID 1 (`init`), a small Rust shell (`nsh`) and a Rust DHCP client (`dhcp`).
Step 2: `den`, a tiling terminal GUI on the framebuffer. Phase 3: a persistent disk, accounts, login and file commands. Next: Rust network tools.

## Prerequisites

- `brew install qemu`
- A Docker engine (Docker Desktop, Kiri, ...). More CPUs = faster kernel build.

## Commands

| Command | What it does |
|---|---|
| `./build.sh` | Builds kernel + Rust binaries + initramfs in Docker → `out/` |
| `./build.sh data` | Recreates `out/<arch>/data.img` (128 MiB, ext4). **Erases all user data and accounts** |
| `./build.sh test` | Runs `cargo test` in Docker (does not compile on macOS) |
| `./run.sh` | Boots in QEMU on this terminal. Ctrl-A X quits |
| `./run.sh gui` | Boots with a QEMU window running den; serial shell stays on this terminal |
| `./test/smoke.sh` | Headless two-boot test on a fresh disk (first-boot setup, files, passwd, persistence), exit 0 = PASS |
| `./test/gui.sh [out.png]` | Headless GUI test: logs in and types via the QEMU monitor on its own fresh disk, screendumps, checks the picture (aarch64) |
| `docker volume rm thir-build` | Clean kernel tree and caches |

## Layout

```
env.sh                     THIR_ARCH + all derived settings (sourced by every script)
build.sh, run.sh           host scripts
test/smoke.sh              boot test
docker/Dockerfile          build image
scripts/build-in-docker.sh kernel config/verify/build, cargo, initramfs
kernel/*.config            kernel fragments applied on top of tinyconfig
src/lib.rs                 exit-code protocol (decide)
src/parse.rs               shell line parser (quotes, > and >>)
src/user.rs                accounts: Argon2id hashes, /data/etc/users, privilege drop
src/files.rs               rm safety check
src/net.rs                 interfaces, address, route (ioctl)
src/dhcp.rs                DHCP packet build/parse
src/bin/dhcp.rs            DHCP client
src/bin/init.rs            PID 1
src/bin/nsh.rs             shell
src/bin/login.rs           login (serial and den panes), first-boot setup
src/bin/passwd.rs          change your own password (setuid root)
out/<arch>/data.img        raw ext4 data disk, created when missing, never reformatted by a build
src/bin/den.rs             GUI event loop
src/gui/                   fb, png, font (PSF), vt (terminal), layout (split tree), keymap, theme (all colors)
assets/                    wallpaper.png, fonts/ (Terminus PSF + OFL license)
test/gui.sh                GUI test
```

## How boot works

kernel → `/init` → mount `/dev /proc /sys /tmp /run` → hostname `thir` → `lo` up → print interfaces →
mount `/data` (ext4 on `/dev/vda`) → `thir: init ready` → start `/bin/login` on the serial console and restart it when it exits.
init blocks in `waitpid(-1)` and dispatches on the pid (console session, den, or any other child that is just reaped).
`nsh` asks init to power off with exit code 120 and to reboot with 121. PID 1 has no signal handlers.

## Kernel config

`tinyconfig` + `kernel/common.config` + `kernel/$THIR_ARCH.config`. The build stops if a requested
line does not survive `olddefconfig` (an unmet dependency). Add the dependency; do not delete the line.

## Switching arch

`THIR_ARCH=aarch64` (default, HVF on Apple Silicon) or `THIR_ARCH=x86_64` (cross-built, QEMU emulated).
Output goes to `out/$THIR_ARCH/`. Example: `THIR_ARCH=x86_64 ./build.sh && THIR_ARCH=x86_64 ./test/smoke.sh`.

## Pinned versions

Linux 6.12.117 · `rust:1.99-bookworm` · libc 0.2.191 · QEMU 11.1.3 (Homebrew)

## Measured (Apple M2, Docker with 2 CPUs)

| | aarch64 | x86_64 |
|---|---|---|
| Kernel image | 5.9 MB | 1.9 MB (compressed bzImage) |
| initramfs | 3.4 MB (with wallpaper) | 3.5 MB |
| Boot to `thir: init ready` | < 1 s (HVF) | ~1–2 s (TCG emulation) |

## GUI (den)

`init` starts `/bin/den` when `/dev/fb0` exists, beside the serial console, and restarts it (at most once a second).
den draws the wallpaper, a status bar (pane number, eth0 address, UTC time) and one login session per tiled pane (see Accounts and sessions).
Alt is the modifier (the macOS Cmd key does not reach QEMU). US keyboard layout.

| Key | Action |
|---|---|
| `Alt+Enter` | split the focused pane along its longer side |
| `Alt+Arrows` or `Alt+h/j/k/l` | move focus |
| `Alt+q` | close the pane (the last pane is replaced by a new one) |
| `Alt+f` | fullscreen toggle |

`poweroff`/`reboot` in a pane make den exit with 120/121 and init acts on it. Colors and sizes: `src/gui/theme.rs`.
den needs a 32 bpp framebuffer. den works at any size; set `xres=`/`yres=` on `virtio-gpu-pci` in `env.sh` to try one.

## Storage

`/dev/vda` (a virtio disk, `out/<arch>/data.img`) is mounted at `/data` with `nosuid,nodev,noexec`. It is ext4 with a journal,
so a power-off without unmounting is replayed on the next mount. The image is formatted in the Docker build container
(macOS has no mkfs) when it is missing; `./build.sh data` recreates it. `/data/home/<user>` is the home (0700, owned by the user),
`/data/etc/users` holds the accounts (0600 root, in a 0700 directory). Everything else is RAM and gone after a reboot.
The guest clock comes from the RTC (PL031 on aarch64, CMOS on x86_64), so the bar shows the real UTC time.

## Accounts and sessions

There is no root login. On the first boot `login` asks you to create the first user (name `a-z 0-9 _ -`, password 8 to 128 bytes, uid 1000).
After that every console asks `thir login:`. Passwords are hashed with Argon2id (m=19456 KiB, t=2, p=1, 16 byte random salt),
stored with their parameters, and compared in constant time. After a login `login` makes the home, sets umask 077,
drops all privileges (`setgroups`, `setgid`, `setuid`, all checked) and execs `nsh`. Files you create are 0600, directories 0700.
`passwd` (setuid root) changes your own password after asking the old one. `exit` logs out.

The den contract: pane 1 runs `/bin/login` and shows only that pane, centered, with `login` in the bar (Alt+Enter is ignored).
den sees the login when `Uid:` in `/proc/<pane 1 pid>/status` is no longer 0, then switches to the tiled layout and remembers the uid.
Later panes run `/bin/login --as <uid>` (no second password). When the last pane closes, den forgets the uid and shows the login screen.
den never echoes or logs keys and executes only `/bin/login`; the pty masters and input fds are close-on-exec. Echo is switched off by `login`.

File commands in `nsh`: `pwd echo whoami mkdir [-p] rmdir touch rm [-r] mv cp write`, and `>` / `>>` redirection.
`write file` reads lines until a line with only `.`. `rm` refuses `/`, `.` and `..`.

## Security

- Root runs `init`, `dhcp`, `den`, `login` (until it execs `nsh`) and `passwd` (setuid, short-lived). Everything a user starts runs as uid 1000 or higher.
- Protects against: someone at the serial console or GUI who lacks the password; a logged-in user reading the account file or raw devices, or escalating through den, login, passwd or dhcp; a power-off corrupting data; passwords in logs.
- Does not protect against: anyone with access to the Mac (the disk image is plaintext, the QEMU monitor, host RAM); kernel bugs; a malicious root binary; offline guessing on a copied image (Argon2 only slows it).
- Crate exception: the userspace is libc only, except `argon2` (RustCrypto, Argon2id) and `subtle` (constant-time compare), plus `miniz_oxide` for PNG. Crypto is not written here. Versions are pinned by `Cargo.lock`.
- No recovery shell. A lost password means deleting `out/<arch>/data.img` on the Mac (all data is lost).

## Networking

`init` starts `/bin/dhcp` beside the shell. It configures every ethernet interface (address, mask,
default route, `/etc/resolv.conf`). Run `dhcp eth0` to do it again by hand.

## Known limitations

- No job control: Ctrl-C and Ctrl-\ stop the running program; Ctrl-Z is ignored.
- DHCP does not renew the lease. No IPv6 address setup.
- `poweroff`/`reboot` work from the serial shell and from a den pane, not from a nested shell.
- den: no scrollback, no mouse, no wide (CJK) characters, no 256-color/truecolor SGR, no VT cursor save/restore or alternate screen.
- The data disk is not encrypted: anyone with `out/<arch>/data.img` reads everything.
- One user (no `adduser`). `write` replaces a whole file. `mv` across filesystems fails. `/data` is `noexec`.
- Two consoles creating the first user at the same moment can overwrite each other (no lock).
- No Ctrl-C test in the smoke test any more: its only long-running command (`dhcp`) needs root.
