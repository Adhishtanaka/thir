# thir

A tiny research OS for network work: the Linux kernel plus a userspace written only in Rust.
No distro, no systemd, no busybox. Step 1: a Rust PID 1 (`init`), a small Rust shell (`nsh`) and a Rust DHCP client (`dhcp`).
Step 2: `den`, a tiling terminal GUI on the framebuffer. Next: Rust network tools.

## Prerequisites

- `brew install qemu`
- A Docker engine (Docker Desktop, Kiri, ...). More CPUs = faster kernel build.

## Commands

| Command | What it does |
|---|---|
| `./build.sh` | Builds kernel + Rust binaries + initramfs in Docker → `out/` |
| `./build.sh test` | Runs `cargo test` in Docker (does not compile on macOS) |
| `./run.sh` | Boots in QEMU on this terminal. Ctrl-A X quits |
| `./run.sh gui` | Boots with a QEMU window running den; serial shell stays on this terminal |
| `./test/smoke.sh` | Headless boot test, exit 0 = PASS |
| `./test/gui.sh [out.png]` | Headless GUI test: types via the QEMU monitor, screendumps, checks the picture (aarch64) |
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
src/parse.rs               shell line parser
src/net.rs                 interfaces, address, route (ioctl)
src/dhcp.rs                DHCP packet build/parse
src/bin/dhcp.rs            DHCP client
src/bin/init.rs            PID 1
src/bin/nsh.rs             shell
src/bin/den.rs             GUI event loop
src/gui/                   fb, png, font (PSF), vt (terminal), layout (split tree), keymap, theme (all colors)
assets/                    wallpaper.png, fonts/ (Terminus PSF + OFL license)
test/gui.sh                GUI test
```

## How boot works

kernel → `/init` → mount `/dev /proc /sys /tmp /run` → hostname `thir` → `lo` up → print interfaces →
`thir: init ready` → start `/bin/nsh` and restart it when it exits.
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
| Kernel image | 5.3 MB | 1.9 MB (compressed bzImage) |
| initramfs | 3.0 MB (with wallpaper) | 3.0 MB |
| Boot to `thir: init ready` | < 1 s (HVF) | ~1–2 s (TCG emulation) |

## GUI (den)

`init` starts `/bin/den` when `/dev/fb0` exists, beside the serial shell, and restarts it after 1 s.
den draws the wallpaper, a status bar (pane number, eth0 address, UTC time) and one `nsh` per tiled pane.
Alt is the modifier (the macOS Cmd key does not reach QEMU). US keyboard layout.

| Key | Action |
|---|---|
| `Alt+Enter` | split the focused pane along its longer side |
| `Alt+Arrows` or `Alt+h/j/k/l` | move focus |
| `Alt+q` | close the pane (the last pane is replaced by a new one) |
| `Alt+f` | fullscreen toggle |

`poweroff`/`reboot` in a pane make den exit with 120/121 and init acts on it. Colors and sizes: `src/gui/theme.rs`.
den needs a 32 bpp framebuffer. den works at any size; set `xres=`/`yres=` on `virtio-gpu-pci` in `env.sh` to try one.

## Networking

`init` starts `/bin/dhcp` beside the shell. It configures every ethernet interface (address, mask,
default route, `/etc/resolv.conf`). Run `dhcp eth0` to do it again by hand.

## Known limitations

- No job control: Ctrl-C and Ctrl-\ stop the running program; Ctrl-Z is ignored.
- DHCP does not renew the lease. No IPv6 address setup.
- `poweroff`/`reboot` work from the serial shell and from a den pane, not from a nested shell.
- den: no scrollback, no mouse, no wide (CJK) characters, no 256-color/truecolor SGR, no VT cursor save/restore or alternate screen.
- Everything runs as root; nothing persists across reboot.
