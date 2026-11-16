# netos

A tiny research OS for network work: the Linux kernel plus a userspace written only in Rust.
No distro, no systemd, no busybox. This is **step 1**: a Rust PID 1 (`init`) and a small Rust shell (`nsh`).
Next steps: framebuffer tiling-terminal GUI with a wallpaper, then Rust network tools.

## Prerequisites

- `brew install qemu`
- A Docker engine (Docker Desktop, Kiri, ...). More CPUs = faster kernel build.

## Commands

| Command | What it does |
|---|---|
| `./build.sh` | Builds kernel + Rust binaries + initramfs in Docker → `out/` |
| `./build.sh test` | Runs `cargo test` in Docker (does not compile on macOS) |
| `./run.sh` | Boots in QEMU on this terminal. Ctrl-A X quits |
| `./test/smoke.sh` | Headless boot test, exit 0 = PASS |
| `docker volume rm netos-build` | Clean kernel tree and caches |

## Layout

```
env.sh                     NETOS_ARCH + all derived settings (sourced by every script)
build.sh, run.sh           host scripts
test/smoke.sh              boot test
docker/Dockerfile          build image
scripts/build-in-docker.sh kernel config/verify/build, cargo, initramfs
kernel/*.config            kernel fragments applied on top of tinyconfig
src/lib.rs                 exit-code protocol (decide)
src/parse.rs               shell line parser
src/net.rs                 interface list / bring-up (ioctl)
src/bin/init.rs            PID 1
src/bin/nsh.rs             shell
```

## How boot works

kernel → `/init` → mount `/dev /proc /sys /tmp /run` → hostname `netos` → `lo` up → print interfaces →
`netos: init ready` → start `/bin/nsh` and restart it when it exits.
`nsh` asks init to power off with exit code 120 and to reboot with 121. PID 1 has no signal handlers.

## Kernel config

`tinyconfig` + `kernel/common.config` + `kernel/$NETOS_ARCH.config`. The build stops if a requested
line does not survive `olddefconfig` (an unmet dependency). Add the dependency; do not delete the line.

## Switching arch

`NETOS_ARCH=aarch64` (default, uses HVF on Apple Silicon). `x86_64` is in `env.sh` but untested:
it needs a cross gcc in the Dockerfile and `kernel/x86_64.config`.

## Pinned versions

Linux 6.12.117 · `rust:1.99-bookworm` · libc 0.2.191 · QEMU 11.1.3 (Homebrew)

## Measured (Apple M2, Docker with 2 CPUs)

| | |
|---|---|
| `out/Image` | 5.3 MB |
| `out/initramfs.cpio.gz` | 426 KB |
| Boot to `init ready` | ≤ 1 s |
| Smoke test (boot + command + poweroff) | 0.5 s |
| First full build (incl. 142 MB kernel download) | ~11 min |

## Known limitations

- No Ctrl-C or job control (the shell has no controlling tty).
- `eth0` has no address yet (no DHCP client).
- `poweroff`/`reboot` work only from the top-level shell.
- The kernel creates legacy BSD ptys (`/dev/ptyXX`) and an IPv6 `sit0` device; trim these from the config later.
