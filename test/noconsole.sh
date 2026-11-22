#!/usr/bin/env bash
# A kernel with no console device (VirtualBox ARM without a serial port: console=ttyAMA0 matches nothing) must still boot to den, and stay up.
# Regression: init died at once (no /dev/null in the initramfs for Rust's std to fill fds 0-2), a panic loop that showed only the VirtualBox logo; then a login on /dev/null restarted every second and took the sound card back from the user.
# Usage: test/noconsole.sh   (aarch64; build first)
set -euo pipefail
cd "$(dirname "$0")/.."; source ./env.sh
tmp=$(mktemp -d -t thir-nocons); trap 'kill "${qpid:-}" 2>/dev/null || true; rm -rf "$tmp"' EXIT
"${QEMU[@]}" -append "console=ttyS9 quiet loglevel=3 panic=0" -display none -serial file:$tmp/serial.log -no-reboot -monitor unix:$tmp/m.sock,server,nowait > "$tmp/qemu.out" 2>&1 & qpid=$!
sleep 30
kill -0 "$qpid" 2>/dev/null || { echo "FAIL: QEMU exited"; exit 1; }
printf 'screendump %s\n' "$tmp/s.ppm" | ( python3 -c "
import socket,sys,time
s=socket.socket(socket.AF_UNIX); s.connect(sys.argv[1]); time.sleep(.5); s.recv(4096); s.send(sys.stdin.read().encode()); time.sleep(2)" "$tmp/m.sock" )
c=$(magick "$tmp/s.ppm" -format %k info:); [ "$c" -gt 4 ] || { echo "FAIL: blank screen ($c colours): den did not start"; exit 1; }
echo "PASS: no console device: booted to den ($c colours)"
